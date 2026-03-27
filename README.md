# ditto

Record real traffic against your Rust service — HTTP, gRPC, Postgres, Redis, and arbitrary async functions — then replay it against any future build to catch regressions automatically.

Works with **any Rust codebase** using `reqwest`, `sqlx`, `tonic`, `redis-rs`, and `tokio`. Axum and Actix-web entry-point middleware included. Custom client libraries supported via a one-trait extension point.

---

## Table of Contents

- [What it does](#what-it-does)
- [Architecture](#architecture)
- [Quick Start](#quick-start)
- [Workspace structure](#workspace-structure)
- [Integration guide](#integration-guide)
  - [1. Install at startup](#1-install-at-startup)
  - [2. HTTP calls](#2-http-calls)
  - [3. Postgres queries](#3-postgres-queries)
  - [4. Redis commands](#4-redis-commands)
  - [5. gRPC clients](#5-grpc-clients)
  - [6. Arbitrary async functions](#6-arbitrary-async-functions)
  - [7. tokio::spawn propagation](#7-tokiospawn-propagation)
- [Framework middleware](#framework-middleware)
  - [Axum](#axum)
  - [Actix-web](#actix-web)
  - [Any Tower-compatible framework](#any-tower-compatible-framework)
- [Replay modes](#replay-modes)
- [Configuration reference](#configuration-reference)
- [Running a replay](#running-a-replay)
- [Understanding diff reports](#understanding-diff-reports)
- [Adding a custom interceptor](#adding-a-custom-interceptor)
- [CI integration](#ci-integration)
- [Automated audit tool](#automated-audit-tool)
- [Troubleshooting](#troubleshooting)
- [Contributing](#contributing)

---

## What it does

Every time your service handles a request, it makes outbound calls — to payment APIs, your own database, cache, downstream services. When you ship a new build, you want to know: *did any of those calls change?* Not just "does it return 200" but **did the actual data flowing in and out of every external dependency change, and if so, was that intentional?**

This system records those calls during normal operation and replays them deterministically against new builds — without a live network, without a live database, without test data setup.

```
Record phase (staging / 10% prod sample):
  Request → [RecordingMiddleware] → handler → reqwest / sqlx / redis
                                                    ↓ writes to store
                                              interactions table (Postgres)

Replay phase (every PR in CI):
  Stored request → [ReplayMiddleware] → handler → reqwest / sqlx / redis
                                                    ↓ reads from store
                                              compare response → diff report
```

---

## Architecture

```
replay-system/
├── crates/
│   ├── replay-core/          # Central types: Interaction, MockContext, CallType
│   │                         # ReplayInterceptor trait, InterceptorRunner
│   │
│   ├── replay-store/         # InteractionStore trait
│   │                         # PostgresStore (production)
│   │                         # InMemoryStore (tests)
│   │
│   ├── replay-macro/         # Proc macros:
│   │                         #   #[record_io]          — per-function
│   │                         #   #[replay::instrument] — per-impl-block
│   │                         #   #[instrument_spawns]  — rewrites tokio::spawn
│   │
│   ├── replay-interceptors/  # Built-in interceptors: HTTP, gRPC, Postgres, Redis
│   │                         # Entry-point middleware: Axum, Actix, Tower
│   │                         # replay-audit binary
│   │
│   ├── replay-compat/        # Drop-in re-exports:
│   │                         #   replay_compat::http    (≈ reqwest)
│   │                         #   replay_compat::sql     (≈ sqlx)
│   │                         #   replay_compat::redis   (≈ redis-rs)
│   │                         #   replay_compat::tokio   (≈ tokio, context-aware)
│   │
│   └── replay-diff/          # Field-level diffing, noise filters, report gen
│
└── examples/
    ├── minimal-axum/         # Full stack in ~150 lines
    └── custom-interceptor/   # Elasticsearch integration example
```

### Data flow for a single recorded request

```
Incoming HTTP request
  └─► RecordingMiddleware creates MockContext { record_id, mode, build_hash }
      └─► MOCK_CTX.scope(ctx, handler).await
          └─► handler runs normally
              ├─► replay_compat::http::Client::new().get(url)
              │     └─► ReplayMiddleware reads MOCK_CTX
              │           Record: make real call, write Interaction to store
              │           Replay: find stored Interaction, return it
              │
              ├─► sqlx::query!(...).fetch_one(&replay_pool)
              │     └─► same pattern
              │
              └─► redis_conn.get("session:abc")
                    └─► same pattern
```

---

## Quick Start

### Prerequisites

- Rust 1.75+
- Postgres 14+ (for the interaction store)
- The target codebase uses `reqwest`, `sqlx`, and/or `redis-rs`

### Step 1 — Add dependencies

```toml
# Cargo.toml
[dependencies]
replay-compat        = "0.1"
replay-interceptors  = { version = "0.1", features = ["axum-middleware"] }
replay-store         = "0.1"
replay-core          = "0.1"
```

### Step 2 — Set environment variables

```bash
# .env
REPLAY_MODE=record
REPLAY_DB_URL=postgres://user:pass@localhost:5432/replay
REPLAY_BUILD_HASH=local-dev
REPLAY_SAMPLE_RATE=100   # record every request during development
```

### Step 3 — Install at startup

```rust
// src/main.rs
use replay_store::PostgresStore;
use replay_core::ReplayMode;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let db_url = std::env::var("REPLAY_DB_URL")?;
    let store  = Arc::new(PostgresStore::new(&db_url).await?);
    let mode   = ReplayMode::from_env(); // reads REPLAY_MODE

    replay_compat::install(store, mode);

    // ... rest of your app startup
    start_server().await
}
```

### Step 4 — Replace client imports

```rust
// before
use reqwest::Client;
use sqlx::PgPool;
use redis;

// after — one import change per file, API is identical
use replay_compat::http::Client;
use replay_compat::sql::Pool as PgPool;
use replay_compat::redis;
```

### Step 5 — Add middleware to your router

```rust
// Axum
let app = Router::new()
    .route("/payments", post(create_payment))
    .layer(ReplayLayer::from_env()); // reads REPLAY_MODE + REPLAY_DB_URL
```

That's it. Run your app with `REPLAY_MODE=record` and interactions are written to Postgres. Switch to `REPLAY_MODE=replay` to play them back.

---

## Workspace structure

After integrating, your crate's dependency graph looks like this:

```
your-service
  ├── replay-compat          (reqwest, sqlx, redis, tokio re-exports)
  ├── replay-interceptors    (middleware, built-in interceptors)
  └── replay-store           (PostgresStore / InMemoryStore)
        └── replay-core      (types, traits — no heavy deps)
```

`replay-core` has no network or database dependencies. It can be used in unit tests without any infrastructure.

---

## Integration guide

### 1. Install at startup

`replay_compat::install()` must be called **once, before serving any requests**. It sets the global store and mode that all interceptors read from.

```rust
replay_compat::install(
    Arc::new(PostgresStore::new(&db_url).await?),
    ReplayMode::from_env(),
);
```

If you need per-request mode overrides (e.g., some routes always passthrough), you can override the mode when constructing `MockContext` in your middleware.

---

### 2. HTTP calls

Replace `reqwest::Client` with `replay_compat::http::Client`. The API is identical — same methods, same builder pattern, same response types.

```rust
use replay_compat::http::Client;

async fn fetch_user(id: &str) -> reqwest::Result<User> {
    Client::new()
        .get(format!("https://api.example.com/users/{id}"))
        .bearer_auth(&api_key)
        .send()
        .await?
        .json::<User>()
        .await
}
```

**What gets recorded:** method, path (with IDs normalized to `{id}`), request body, response status, response body. Auth headers are stripped from the recording.

**What gets replayed:** the stored response is returned without making a network call.

---

### 3. Postgres queries

Replace `sqlx::PgPool` with `replay_compat::sql::Pool`. Works with `query!`, `query_as!`, `.execute()`, `.fetch_one()`, `.fetch_all()`, and `.fetch_optional()`.

```rust
use replay_compat::sql::Pool;

async fn get_payment(pool: &Pool, id: Uuid) -> sqlx::Result<Payment> {
    sqlx::query_as!(Payment, "SELECT * FROM payments WHERE id = $1", id)
        .fetch_one(pool)
        .await
}
```

**What gets recorded:** normalized SQL (parameters replaced with `?`), result rows. Connection strings and credentials are never stored.

---

### 4. Redis commands

Replace `redis::Client` with `replay_compat::redis::Client`. Connections returned from `get_async_connection()` are instrumented automatically.

```rust
use replay_compat::redis::Client;

async fn get_session(client: &Client, session_id: &str) -> redis::RedisResult<String> {
    let mut conn = client.get_async_connection().await?;
    conn.get(format!("session:{session_id}")).await
}
```

**What gets recorded:** command name, key pattern (IDs normalized), return value.

---

### 5. gRPC clients

Wrap your tonic channel with `ReplayInterceptorLayer`:

```rust
use replay_interceptors::grpc::ReplayInterceptorLayer;
use tonic::transport::Channel;

let channel = Channel::from_static("http://payment-service:50051")
    .connect()
    .await?;

let channel = tower::ServiceBuilder::new()
    .layer(ReplayInterceptorLayer::from_global())
    .service(channel);

let client = PaymentServiceClient::new(channel);
```

**What gets recorded:** service name, method name, request proto (serialized to JSON), response proto.

---

### 6. Arbitrary async functions

Use `#[record_io]` on any async function whose return value you want to record and replay:

```rust
use replay_macro::record_io;

#[record_io]
async fn call_fraud_model(features: FraudFeatures) -> FraudScore {
    // expensive ML model call or internal service
    fraud_client.score(features).await
}
```

For impl blocks, annotate the whole block to avoid per-method annotations:

```rust
use replay_macro::instrument;

#[instrument]
impl PaymentConnector for Stripe {
    async fn charge(&self, req: ChargeRequest) -> Result<ChargeResponse> { ... }
    async fn refund(&self, req: RefundRequest) -> Result<RefundResponse> { ... }
    async fn void(&self,   req: VoidRequest)   -> Result<VoidResponse>   { ... }
    // all three are automatically instrumented
}
```

---

### 7. tokio::spawn propagation

`MockContext` lives in a `task_local` — it is scoped to the current async task. Crossing a `tokio::spawn` boundary loses the context. There are two ways to handle this:

**Option A — alias the module (recommended, zero per-site changes):**

```rust
// at the top of any crate that uses tokio::spawn
use replay_compat as tokio;

// now tokio::spawn(...) automatically propagates context
tokio::spawn(async { process_webhook().await; });
```

**Option B — annotate specific functions:**

```rust
use replay_macro::instrument_spawns;

#[instrument_spawns]
async fn handle_payment(req: PaymentRequest) -> Result<()> {
    // all tokio::spawn calls inside this function are automatically rewritten
    tokio::spawn(async { send_notification().await; });
    tokio::spawn(async { update_analytics().await;  });
    Ok(())
}
```

**Option C — manual, call-by-call:**

```rust
use replay_compat::tokio::task::spawn_with_ctx;

spawn_with_ctx(async { process_webhook().await; });
```

---

## Framework middleware

### Axum

```rust
use replay_interceptors::middleware::axum::ReplayLayer;

let app = Router::new()
    .route("/payments", post(create_payment))
    .layer(ReplayLayer::from_env());
```

`ReplayLayer::from_env()` reads `REPLAY_MODE` and `REPLAY_DB_URL`. Use `ReplayLayer::new(store, mode)` for explicit control.

### Actix-web

```rust
use replay_interceptors::middleware::actix::ReplayMiddleware;

App::new()
    .wrap(ReplayMiddleware::from_env())
    .service(payment_scope())
```

Enable the `actix-middleware` feature:

```toml
replay-interceptors = { version = "0.1", features = ["actix-middleware"] }
```

### Any Tower-compatible framework

```rust
use replay_interceptors::middleware::tower::RecordingLayer;

let service = tower::ServiceBuilder::new()
    .layer(RecordingLayer::new(store, mode))
    .service(your_service);
```

### Custom framework (three-line contract)

If your framework is not listed, the integration contract is:

```rust
// 1. Create a context at the start of each request
let ctx = MockContext::new(ReplayMode::from_env());

// 2. Scope your handler inside it
let response = MOCK_CTX.scope(ctx, async {
    your_handler(request).await
}).await;

// 3. Done — all replay_compat clients inside the handler find the context automatically
```

---

## Replay modes

Set via the `REPLAY_MODE` environment variable:

| Mode | Behaviour |
|---|---|
| `record` | Makes real calls, writes every interaction to the store |
| `replay` | Reads from store, makes no real network/DB calls |
| `passthrough` | Intercepts calls but does not read or write to store — for observability only |
| `off` | Completely transparent — zero overhead, interceptors are no-ops |

Switch modes without recompiling. In production, use `record` with `REPLAY_SAMPLE_RATE=10` (records 1-in-10 requests). In CI, use `replay`.

---

## Configuration reference

| Variable | Default | Description |
|---|---|---|
| `REPLAY_MODE` | `off` | `record`, `replay`, `passthrough`, or `off` |
| `REPLAY_DB_URL` | — | Postgres connection string for the interaction store |
| `REPLAY_BUILD_HASH` | `""` | Git SHA or build ID injected by CI. Used to correlate recordings with builds. |
| `REPLAY_SAMPLE_RATE` | `1` | Record 1-in-N requests in production. `1` = every request. `10` = 10%. |
| `REPLAY_LOG_LEVEL` | `info` | Log level for replay system internals. |
| `REPLAY_TARGET_URL` | `http://localhost:8080` | Service URL during replay runs (the new build under test). |

---

## Running a replay

### Against a local build

```bash
# 1. Start your service
cargo run

# 2. Run replay against it
REPLAY_MODE=replay \
REPLAY_DB_URL=postgres://... \
REPLAY_TARGET_URL=http://localhost:8080 \
cargo replay-test --sample 50   # replay 50 recorded requests
```

### Against a specific build hash

```bash
# replay interactions recorded from commit abc123 against the current build
cargo replay-test \
  --source-build abc123 \
  --target http://localhost:8080 \
  --output replay-report.json
```

### Reading the output

```
replay run: 50 interactions
  ✓ passed:  47
  ✗ failed:   3
  ─ skipped:  0

FAILURE  [2] POST /api/v1/payments
  interaction: 019f2c1a-...
  field: response.body.status
  recorded: "requires_action"
  replayed: "succeeded"

FAILURE  [7] GET /api/v1/payments/{id}
  field: response.body.amount
  recorded: 1000
  replayed: 1050
```

Diffs are classified as `breaking` (status codes, required fields) or `cosmetic` (log fields, timing, ordering). Only breaking diffs fail CI by default.

---

## Understanding diff reports

The diff engine compares recorded and replayed responses field-by-field. Fields are classified automatically:

| Classification | Examples | Fails CI? |
|---|---|---|
| `breaking` | status codes, amounts, IDs, required fields | Yes |
| `cosmetic` | log messages, timing fields, metadata | No |
| `noise` | timestamps, trace IDs, random nonces | No (filtered before diff) |

To mark a field as noise (always ignored in diffs), add it to your config:

```toml
# replay.toml
[noise_filters]
response_fields = [
    "$.created_at",
    "$.updated_at",
    "$.request_id",
    "$.metadata.trace_id",
]
```

---

## Adding a custom interceptor

If your codebase uses a client library not covered by `replay-compat` (Elasticsearch, SQS, Kafka, a bespoke internal SDK), implement `ReplayInterceptor`:

```rust
use replay_core::{ReplayInterceptor, CallType};
use async_trait::async_trait;

pub struct ElasticsearchInterceptor {
    client: elasticsearch::Elasticsearch,
}

#[async_trait]
impl ReplayInterceptor for ElasticsearchInterceptor {
    type Request  = EsSearchRequest;
    type Response = EsSearchResponse;
    type Error    = elasticsearch::Error;

    fn call_type(&self) -> CallType { CallType::Http }

    // must be identical for semantically equivalent calls across builds
    fn fingerprint(&self, req: &EsSearchRequest) -> String {
        format!("{} /{}", req.method, normalize_index(&req.index))
        // e.g. "GET /payments-*/_search"
    }

    // strip volatile fields — what remains must be deterministic
    fn normalize_request(&self, req: &EsSearchRequest) -> serde_json::Value {
        let mut v = serde_json::to_value(req).unwrap();
        remove_keys(&mut v, &["scroll_id", "preference", "routing"]);
        v
    }

    fn normalize_response(&self, res: &EsSearchResponse) -> serde_json::Value {
        let mut v = serde_json::to_value(res).unwrap();
        remove_keys(&mut v, &["took", "_shards"]);  // timing is noise
        v
    }

    async fn execute(&self, req: EsSearchRequest) -> Result<EsSearchResponse, Self::Error> {
        self.client
            .search(SearchParts::Index(&[&req.index]))
            .body(req.body)
            .send()
            .await
    }
}
```

Then wrap it with `InterceptorRunner` wherever you make the call:

```rust
use replay_core::runner::InterceptorRunner;

let runner = InterceptorRunner::new(
    ElasticsearchInterceptor { client: es_client.clone() },
    global_store(),
);

let response = runner.run(search_request).await?;
```

See `examples/custom-interceptor/` for a complete runnable example.

**The three fingerprinting rules:**
1. Strip all parameter values that change per-request (IDs, amounts, session tokens)
2. Keep all structural identifiers (method names, paths, table names, command names)
3. Two calls that should return the same response in replay must produce the same fingerprint

---

## CI integration

### Record on merge to main (staging deploy)

```yaml
# .github/workflows/record.yml
on:
  push:
    branches: [main]

jobs:
  record:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Deploy to staging with recording enabled
        env:
          REPLAY_MODE:        record
          REPLAY_SAMPLE_RATE: 10
          REPLAY_BUILD_HASH:  ${{ github.sha }}
          REPLAY_DB_URL:      ${{ secrets.REPLAY_DB_URL }}
        run: ./deploy.sh staging
```

### Replay on every PR

```yaml
# .github/workflows/replay.yml
on:
  pull_request:

jobs:
  replay:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Build PR binary
        run: cargo build --release

      - name: Start service
        run: ./target/release/your-service &
        env:
          REPLAY_MODE: off   # service itself runs in passthrough during replay test

      - name: Run replay harness
        env:
          REPLAY_MODE:        replay
          REPLAY_DB_URL:      ${{ secrets.REPLAY_DB_URL }}
          REPLAY_TARGET_URL:  http://localhost:8080
          REPLAY_BUILD_HASH:  ${{ github.sha }}
        run: cargo replay-test --sample 100 --output replay-report.json

      - name: Post diff report as PR comment
        if: failure()
        uses: actions/github-script@v7
        with:
          script: |
            const report = require('./replay-report.json');
            github.rest.issues.createComment({
              ...context.repo,
              issue_number: context.issue.number,
              body: formatReport(report),
            });
```

### Local replay

```bash
# alias in .cargo/config.toml
[alias]
replay-test = "run -p replay-interceptors --bin replay_test --"

# usage
cargo replay-test --sample 50 --target http://localhost:8080
```

---

## Automated audit tool

Before manual integration, run the audit tool to find all call sites that need changing:

```bash
# scan and report (read-only)
cargo replay-audit --path ./crates

# output:
# [HTTP]   crates/router/src/client.rs:42   reqwest::Client::new()
# [HTTP]   crates/connector/stripe.rs:18    ClientBuilder::new()
# [SPAWN]  crates/payments/src/handler.rs:91 tokio::spawn(...)
# [SQL]    crates/db/src/queries.rs:34       .execute(&pool)
#
# 12 sites found. 10 auto-fixable. Run with --fix to apply patches.

# auto-patch all fixable sites
cargo replay-audit --path ./crates --fix

# machine-readable output for CI
cargo replay-audit --path ./crates --json > audit-report.json
```

The `--fix` flag applies these rewrites automatically:
- `reqwest::Client::new()` → `replay_compat::http::Client::new()`
- `redis::Client::open(url)` → `replay_compat::redis::Client::open(url)`
- `tokio::spawn(fut)` → `replay_compat::tokio::task::spawn(fut)`
- Adds required `use replay_compat::...` imports to each patched file

---

## Troubleshooting

### Interactions not being recorded

**Symptom:** The store remains empty even though the service is handling requests.

**Diagnosis:**
1. Check `REPLAY_MODE=record` is set and the service sees it: `println!("{}", std::env::var("REPLAY_MODE").unwrap_or_default())`
2. Check `replay_compat::install()` is called before the first request
3. Check your middleware is registered: the `RecordingLayer` must wrap the routes that handle requests
4. Check `REPLAY_SAMPLE_RATE` — if set to 100, only 1-in-100 requests are recorded

### Replay fails with "no matching interaction found"

**Symptom:** `replay_test` reports missing interactions for calls that were recorded.

**Causes and fixes:**
- **Fingerprint mismatch:** The call site changed its URL structure, SQL, or key pattern between the recording build and the replay build. Check the `fingerprint` column in the `interactions` table against what the new build generates.
- **Sequence mismatch:** A new call was inserted before an existing call. The replay engine uses sequence numbers — adding a new external call before the one that fails will shift all subsequent sequence numbers. Use `find_nearest` fuzzy matching as a fallback.
- **Context not propagated:** A `tokio::spawn` boundary was crossed without context propagation. Check for unpatched `tokio::spawn` calls in the call stack.

### tokio::spawn losing context

**Symptom:** Interactions inside spawned tasks are not recorded.

**Fix:** Use one of the three propagation options from the [tokio::spawn propagation](#7-tokiospawn-propagation) section. The quickest fix is `use replay_compat as tokio` at the top of the affected crate.

### Store write failures silently dropped

By design, a store write failure does not fail the request — the real call still completes. Check your logs for `WARN replay_store: write failed: ...`. Common causes: Postgres connection pool exhausted, `REPLAY_DB_URL` not set, schema migrations not run.

Run migrations manually:

```bash
sqlx migrate run --database-url $REPLAY_DB_URL --source crates/replay-store/migrations/
```

### Non-deterministic fingerprints

**Symptom:** Replay fails even when no code changed.

**Cause:** The fingerprint contains a value that changes per-request (timestamp, random nonce, UUID in a URL that isn't normalized).

**Fix:** In your interceptor's `fingerprint()` method, ensure all volatile segments are stripped. For HTTP paths, IDs matching `[0-9a-f\-]{8,}` are replaced with `{id}` by default — if your IDs use a different format, extend `FingerprintBuilder::http()`.

---

## Contributing

### Adding support for a new client library

1. Implement `ReplayInterceptor` for the new client (see [Adding a custom interceptor](#adding-a-custom-interceptor))
2. Add it to `replay-interceptors` under `src/interceptors/your_library.rs`
3. Gate it behind a feature flag in `Cargo.toml`
4. Add integration tests using `InMemoryStore`
5. Add an example to `examples/` or update `docs/INTERCEPTORS.md`

### Adding support for a new web framework

1. Implement `Tower::Layer` + `Tower::Service` using `RecordingLayer` as a reference
2. Add it to `replay-interceptors/src/middleware/your_framework.rs`
3. Gate behind a feature flag
4. The entry-point contract is three lines — see [Custom framework](#any-tower-compatible-framework)

### Running tests

```bash
# unit tests (no infrastructure required)
cargo test --workspace

# integration tests (requires Postgres + Redis)
TEST_DB_URL=postgres://... TEST_REDIS_URL=redis://... \
cargo test --workspace --features postgres-tests

# macro expansion tests
cargo test -p replay-macro
```

### Project conventions

- Every interceptor must pass the `InterceptorRunner` four-mode test suite (record, replay, passthrough, off)
- Fingerprints must be tested against a corpus of real-world values — add cases to `replay-core/src/fingerprint_tests.rs`
- Do not use `unwrap()` in non-test code — use `?` and define errors with `thiserror`
- Store writes must never fail a request — log the error and continue

---

## License

MIT
