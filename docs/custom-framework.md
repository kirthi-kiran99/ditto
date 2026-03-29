# Integrating with an Unsupported Framework

The replay system is built around a three-step contract that any HTTP framework
can satisfy in about 10 lines of code.

---

## The Contract

Every framework integration must do three things at the boundary of each
incoming request:

```rust
// 1. At request start — create a MockContext with a fresh or replayed record_id
let mode      = ReplayMode::from_env(); // reads REPLAY_MODE env var
let record_id = match mode {
    ReplayMode::Replay => read_record_id_from_header(&request),
    _                  => Uuid::new_v4(),
};
let ctx = MockContext::with_id(record_id, mode);

// 2. Scope the handler inside the context
let response = MOCK_CTX.scope(ctx, async {
    your_handler(request).await
}).await;

// 3. Done — all replay_compat clients inside the handler see the context
//    automatically via MOCK_CTX.try_with(|c| c.clone())
response
```

That's it.  All built-in interceptors (`replay_compat::http::Client`,
`replay_compat::redis::open`, `replay_compat::sql::connect`) read
`MOCK_CTX` via `try_with` and handle the rest.

---

## Reading the Record ID in Replay Mode

The replay harness sets `x-replay-record-id` on every replayed request so
the service knows which recording to serve from:

```rust
fn read_record_id_from_header(request: &YourRequest) -> Uuid {
    request
        .header("x-replay-record-id")
        .and_then(|v| Uuid::parse_str(v).ok())
        .unwrap_or_else(Uuid::new_v4)
}
```

---

## Using the Built-in Tower Layer

If your framework supports [Tower](https://docs.rs/tower) middleware, use
`RecordingLayer` directly — no custom integration code needed:

```rust
use replay_interceptors::RecordingLayer;
use replay_core::ReplayMode;
use tower::ServiceBuilder;

let service = ServiceBuilder::new()
    .layer(RecordingLayer::new(ReplayMode::from_env()))
    .service(my_handler);
```

For entry-point recording (sequence 0 owned by the inbound HTTP call):

```rust
let service = ServiceBuilder::new()
    .layer(RecordingLayer::with_store(store.clone(), ReplayMode::from_env()))
    .service(my_handler);
```

### Frameworks known to be compatible with RecordingLayer

- **Axum** — can also use `recording_middleware` / `recording_middleware_with_store`
  from `replay_interceptors::http_server` for Axum-idiomatic wiring.
- **hyper** — wrap the `Service<Request>` directly.
- **tower-http** — chain alongside `TraceLayer`, `CorsLayer`, etc.
- **warp** — via `warp::service()` adapter.

---

## Using the Actix-web Middleware

Enable the `actix-middleware` feature and use `ReplayActixMiddleware`:

```toml
# Cargo.toml
replay-interceptors = { path = "...", features = ["actix-middleware"] }
```

```rust
use replay_interceptors::middleware::actix::ReplayActixMiddleware;
use replay_core::ReplayMode;

HttpServer::new(move || {
    App::new()
        .wrap(ReplayActixMiddleware::new(ReplayMode::from_env()))
        .service(payment_handler)
})
.bind("0.0.0.0:8080")?
.run()
.await
```

---

## Global Store Setup

All built-in interceptors fall back to the global store when no store is
present in the `MockContext`.  Register it once at startup:

```rust
// In main() — before serving any requests
replay_core::set_global_store(Arc::new(PostgresStore::new(&db_url).await?));

// Or, if using replay-compat:
replay_compat::install(
    Arc::new(PostgresStore::new(&db_url).await?),
    ReplayMode::from_env(),
);
```

For tests, use `InMemoryStore` and pass it directly into `MockContext::with_store`
so each test is fully isolated (no global state needed):

```rust
let store     = Arc::new(InMemoryStore::new());
let record_id = Uuid::new_v4();
let ctx       = MockContext::with_store(record_id, ReplayMode::Record, store.clone());
MOCK_CTX.scope(ctx, async { /* your test */ }).await;
let stored = store.get_by_record_id(record_id).await.unwrap();
```

---

## Context Propagation through `tokio::spawn`

If your handler spawns sub-tasks, use `replay_core::spawn_with_ctx` instead of
`tokio::spawn`.  This captures the current `MockContext` and re-injects it into
the spawned task:

```rust
// Before
tokio::spawn(async { background_work().await });

// After — context propagates automatically
replay_core::spawn_with_ctx(async { background_work().await });
```

Or annotate the function with `#[instrument_spawns]` from `replay_macro` to
rewrite all `tokio::spawn` calls automatically:

```rust
use replay_macro::instrument_spawns;

#[instrument_spawns]
async fn my_handler(req: Request) -> Response {
    tokio::spawn(async { background_work().await }); // rewritten automatically
    build_response().await
}
```
