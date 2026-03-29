# Writing a Custom Interceptor

This guide explains how to add record/replay support to a client library not
covered by the built-in interceptors (`replay-compat::http`, `replay-compat::sql`,
`replay-compat::redis`).

---

## When do you need a custom interceptor?

Any time your handler makes an outbound call through a client library that is
not one of the three above.  Common examples:

- Elasticsearch / OpenSearch
- AWS SDK (SQS, S3, DynamoDB)
- Kafka producer / consumer
- A bespoke internal HTTP SDK that wraps `reqwest`
- gRPC clients not managed through `tonic` + `ReplayGrpcLayer`
- Payment processors with their own SDK (Stripe, Adyen)

---

## The three-step process

1. **Implement `ReplayInterceptor`** — teach the runtime how to fingerprint,
   normalise, and execute calls for this client.
2. **Wrap with `InterceptorRunner`** — pass the interceptor + store; get back
   an object whose `.run(request)` method handles all mode switching.
3. **Call `runner.run(request)`** instead of the raw client.

---

## Step 1 — Implement `ReplayInterceptor`

```rust
use replay_core::{CallType, FingerprintBuilder, ReplayError, ReplayInterceptor};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── your request/response types ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MyRequest {
    pub method: String,
    pub path:   String,
    pub body:   Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MyResponse {
    pub status: u16,
    pub body:   Value,
}

// ── error type ────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum MyError {
    #[error(transparent)]
    Replay(#[from] ReplayError),      // required — runner surfaces store misses here
    #[error("client error: {0}")]
    Client(String),
}

// ── interceptor ───────────────────────────────────────────────────────────────

pub struct MyInterceptor {
    client: MyClient,
}

#[async_trait]
impl ReplayInterceptor for MyInterceptor {
    type Request  = MyRequest;
    type Response = MyResponse;
    type Error    = MyError;

    fn call_type(&self) -> CallType {
        CallType::Http  // or CallType::Postgres, ::Redis, ::Function, ::Grpc
    }

    fn fingerprint(&self, req: &MyRequest) -> String {
        // Must produce the SAME string for semantically equivalent calls.
        // Use FingerprintBuilder to normalize IDs in paths.
        FingerprintBuilder::http(&req.method, &req.path)
        // → strips /users/abc123 → /users/{id}
        //   strips /payments/019f… (UUID) → /payments/{id}
    }

    fn normalize_request(&self, req: &MyRequest) -> Value {
        // Strip fields that change per-request but do not affect the response.
        let mut body = req.body.clone();
        if let Some(obj) = body.as_object_mut() {
            obj.remove("idempotency_key");
            obj.remove("request_id");
        }
        serde_json::json!({
            "method": req.method,
            "path":   req.path,
            "body":   body,
        })
    }

    fn normalize_response(&self, res: &MyResponse) -> Value {
        // Strip timing, shard info, random nonces — anything that changes
        // per-execution even if the logical answer is identical.
        let mut body = res.body.clone();
        if let Some(obj) = body.as_object_mut() {
            obj.remove("trace_id");
            obj.remove("timestamp");
        }
        serde_json::json!({ "status": res.status, "body": body })
    }

    async fn execute(&self, req: MyRequest) -> Result<MyResponse, MyError> {
        self.client
            .call(&req.method, &req.path, req.body)
            .await
            .map_err(|e| MyError::Client(e.to_string()))
    }
}
```

---

## Step 2 — Wrap with `InterceptorRunner`

```rust
use replay_core::{InterceptorRunner, InteractionStore};
use std::sync::Arc;

fn build_runner(
    client: MyClient,
    store:  Arc<dyn InteractionStore>,
) -> InterceptorRunner<MyInterceptor> {
    InterceptorRunner::new(MyInterceptor { client }, store)
}
```

Call `build_runner` once at startup (or once per request if the client is
per-request) and keep the runner alongside your other state.

---

## Step 3 — Use `runner.run(request)` instead of the raw client

```rust
// before
let resp = my_client.call("POST", "/payments", body).await?;

// after
let resp = runner.run(MyRequest {
    method: "POST".into(),
    path:   "/payments".into(),
    body,
}).await?;
```

The runner uses the ambient `MockContext` (set by the entry-point middleware)
to decide what to do:

| Mode          | What happens                                           |
|---------------|--------------------------------------------------------|
| `Record`      | `execute()` is called; result written to store        |
| `Replay`      | `execute()` is **not** called; stored result returned |
| `Passthrough` | `execute()` is called; nothing written                |
| `Off`         | `execute()` is called; nothing written                |
| No context    | `execute()` is called (passthrough semantics)         |

---

## Fingerprinting rules

A fingerprint must be **stable** and **discriminating**:

| Rule | Rationale |
|------|-----------|
| Strip all per-request IDs from URL paths | `payments/abc` and `payments/def` are the same logical call |
| Strip auth tokens, timestamps, nonces | These change every request and add no information |
| Keep HTTP method, path structure, SQL verb, command name | These distinguish semantically different calls |
| Two calls that should return the same replay must produce the same fingerprint | The runner uses fingerprint + sequence to look up stored interactions |

`FingerprintBuilder::http(method, path)` handles the common case for HTTP by
normalizing UUID-shaped and numeric ID segments automatically.

For SQL, use `FingerprintBuilder::sql(query)` — it strips parameter values and
normalizes whitespace.

For anything else, build the string yourself:

```rust
fn fingerprint(&self, req: &KafkaMessage) -> String {
    format!("kafka:{}", req.topic)  // strip the key and value; keep the topic
}
```

---

## Normalization rules

Normalize means "make deterministic while keeping everything semantically
relevant". Think of it as writing a snapshot that should match the next
recording even if unimportant metadata changed.

**Normalize request:**
- Remove idempotency keys, request IDs, session tokens
- Remove timestamps, tracing headers
- Keep the logical payload that determines what the server does

**Normalize response:**
- Remove timing fields (`took`, `duration_ms`, `latency`)
- Remove shard/partition info, server version strings
- Remove trace IDs, correlation IDs
- Keep the logical data that the handler reads

---

## Using `CallType`

`CallType` is a first-class discriminator in the store — two interactions with
the same fingerprint but different call types are different records.  Use the
closest match:

| Client type        | `CallType`        |
|--------------------|-------------------|
| HTTP               | `CallType::Http`  |
| Postgres / MySQL   | `CallType::Postgres` |
| Redis              | `CallType::Redis` |
| gRPC               | `CallType::Grpc`  |
| Arbitrary function | `CallType::Function` |

---

## Testing your interceptor

Use `InMemoryStore` and `MockContext::with_id` for fully isolated tests — no
global state, no Postgres, no network.

```rust
#[tokio::test]
async fn record_then_replay() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();
    let runner    = build_runner(MyClient::new(), store.clone() as Arc<dyn InteractionStore>);

    // Record
    let ctx = MockContext::with_id(record_id, ReplayMode::Record);
    MOCK_CTX.scope(ctx, async {
        runner.run(my_request()).await.unwrap();
    }).await;

    // Verify it was stored
    let interactions = store.get_by_record_id(record_id).await.unwrap();
    assert_eq!(interactions.len(), 1);

    // Replay
    let ctx = MockContext::with_id(record_id, ReplayMode::Replay);
    let replayed = MOCK_CTX.scope(ctx, async {
        runner.run(my_request()).await.unwrap()
    }).await;

    assert_eq!(replayed.status, 200);
}
```

See `examples/custom-interceptor/` for a complete, runnable version of this
pattern.

---

## Adding the interceptor to `replay-interceptors`

If your interceptor is general-purpose (useful to other teams), consider
contributing it to the `replay-interceptors` crate:

1. Create `crates/replay-interceptors/src/interceptors/my_library.rs`
2. Gate it behind a feature flag in `Cargo.toml`:
   ```toml
   my-library = ["dep:my-library-crate"]
   ```
3. Re-export from `crates/replay-interceptors/src/lib.rs`:
   ```rust
   #[cfg(feature = "my-library")]
   pub mod my_library_interceptor;
   ```
4. Add integration tests using `InMemoryStore`
5. Update `docs/INTERCEPTORS.md` with a section for the new backend

---

## Complete example

`examples/custom-interceptor/` contains a full, runnable Elasticsearch-style
example covering:

- Request/response type definitions
- `ReplayInterceptor` implementation
- Fingerprinting with index-name normalisation
- Response normalisation (strip `took`, `_shards`)
- Tests for Record, Replay, and Off modes
