# ditto

Record real traffic against your Rust service — HTTP, gRPC, Postgres, Redis, and arbitrary async functions — then replay it against any future build to catch regressions automatically. Includes a visual UI for inspecting recordings and triggering replays.

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
- [Visual UI (ditto-server)](#visual-ui-ditto-server)
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
  Request → [RecordingMiddleware] → handler → reqwest / sqlx / redis / #[record_io]
                                                    ↓ writes to store
                                              interactions table (Postgres)

Shadow replay phase (every PR):
  Stored request → [ReplayMiddleware] → handler → reqwest / sqlx / redis (all mocked)
                                                    ↓ #[record_io] runs real code
                                              capture new results → diff report
```

The **Shadow** replay mode is the key insight: external dependencies (HTTP, DB, Redis, gRPC) are mocked from the recording so the service runs deterministically, while `#[record_io]`-annotated functions execute their *actual* code and write fresh results to a temporary capture record. The harness then diffs the capture against the original recording to find regressions — even in pure Rust business logic.

---

## Architecture

```
ditto/
├── crates/
│   ├── replay-core/          # Central types: Interaction, MockContext, CallType
│   │                         # ReplayInterceptor trait, InterceptorRunner
│   │                         # ReplayMode (Record | Replay | Shadow | Passthrough | Off)
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
│   ├── replay-interceptors/  # Built-in interceptors: HTTP client, gRPC, Postgres, Redis
│   │                         # Entry-point middleware: Axum, Actix, Tower
│   │                         # ReplayHarness — fires recorded requests at service under test
│   │
│   ├── replay-compat/        # Drop-in re-exports:
│   │                         #   replay_compat::http    (≈ reqwest)
│   │                         #   replay_compat::sql     (≈ sqlx)
│   │                         #   replay_compat::redis   (≈ redis-rs)
│   │                         #   replay_compat::tokio   (≈ tokio, context-aware)
│   │
│   └── replay-diff/          # Field-level diffing, noise filters, ChangeManifest, report gen
│
├── server/                   # ditto-server binary: Axum REST API + serves React UI
│                             # Exposes /api/recordings and /api/recordings/:id/replay
│
├── ui/                       # React + Vite + TypeScript + Tailwind
│                             # Split-panel UI: sidebar recording list + replay detail pane
│
└── examples/
    ├── minimal-axum/         # Minimal integration in ~150 lines
    ├── full-flow/            # Full stack: HTTP + gRPC + Postgres + Redis + #[record_io]
    ├── full-flow-faulty/     # Same as full-flow with deliberate bugs — for regression demos
    └── custom-interceptor/   # Elasticsearch integration example
```

### Data flow for a single recorded request

```
Incoming HTTP request
  └─► recording_middleware_with_store creates MockContext { record_id, mode, build_hash }
      ├─► claims sequence=0 for the HTTP entry-point interaction
      └─► MOCK_CTX.scope(ctx, handler).await
          └─► handler runs normally
              ├─► replay_compat::http::Client::new().get(url)
              │     Record:  make real call, write Interaction(seq=N) to store
              │     Replay/Shadow: find stored Interaction by record_id+seq, return it
              │
              ├─► sqlx::query!(...).fetch_one(&replay_pool)  — same pattern
              │
              ├─► redis_conn.get("session:abc")              — same pattern
              │
              └─► #[record_io] async fn compute_tax(amount: f64) -> f64
                    Record:  run function body, write result to store
                    Replay:  return stored result without running body
                    Shadow:  run function body, write result to capture_id (NOT record_id)
```

### Shadow mode — how regression detection works

```
┌─ Harness ─────────────────────────────────────────────────────────────────┐
│                                                                             │
│  1. Load original recording (record_id)                                    │
│  2. Generate fresh capture_id                                               │
│  3. Fire request with headers:                                              │
│       X-Replay-Record-Id: <record_id>                                       │
│       X-Ditto-Capture-Id: <capture_id>                                      │
│                                                                             │
│  Service runs in Shadow mode:                                               │
│    - External interceptors (HTTP/gRPC/DB/Redis) → mock from record_id      │
│    - #[record_io] functions → execute real code → write to capture_id      │
│                                                                             │
│  4. Read capture_id interactions (actual new results)                       │
│  5. Merge into recorded interactions by sequence number                     │
│  6. Diff: recorded vs merged → field-level regression report                │
│  7. Delete capture_id (cleanup)                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## Quick Start

### Prerequisites

- Rust 1.75+
- Postgres 14+ (for the interaction store)
- Node.js 18+ (for the UI, optional)

### Step 1 — Add dependencies

```toml
# Cargo.toml
[dependencies]
replay-compat        = "0.1"
replay-interceptors  = { version = "0.1", features = ["axum-middleware"] }
replay-store         = "0.1"
replay-core          = "0.1"
replay-macro         = "0.1"   # only if you use #[record_io]
```

### Step 2 — Set environment variables

```bash
# .env
REPLAY_MODE=record
REPLAY_DB_URL=postgres://user:pass@localhost:5432/replay
REPLAY_BUILD_HASH=local-dev
```

### Step 3 — Install at startup

```rust
// src/main.rs
use replay_store::PostgresStore;
use replay_core::{ReplayMode, InteractionStore, MacroStore};
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let db_url = std::env::var("REPLAY_DB_URL")?;
    let mode   = ReplayMode::from_env();

    // Capture both typed Arcs before upcasting — required for #[record_io]
    let store = Arc::new(PostgresStore::new(&db_url).await?);
    let interaction_store = store.clone() as Arc<dyn InteractionStore>;
    let macro_store       = store        as Arc<dyn MacroStore>;

    replay_compat::install(interaction_store, mode.clone());
    replay_core::set_global_store(macro_store);  // enables #[record_io] recording

    // ... rest of your app startup
    start_server().await
}
```

> **Important:** Both `replay_compat::install` and `replay_core::set_global_store` must be called. They write to separate globals — omitting either will silently drop one category of recordings.

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
// Axum — store-aware variant records the HTTP entry-point interaction
use replay_interceptors::http_server::recording_middleware_with_store;
use axum::middleware;

let app = Router::new()
    .route("/payments", post(create_payment))
    .layer(middleware::from_fn_with_state(
        store.clone(),
        recording_middleware_with_store,
    ));
```

That's it. Run your service with `REPLAY_MODE=record` and every request's full interaction chain — HTTP call in, all outbound calls, all `#[record_io]` return values — is written to Postgres. Switch to `REPLAY_MODE=replay` to serve mocked responses.

---

## Workspace structure

After integrating, your crate's dependency graph looks like this:

```
your-service
  ├── replay-compat          (reqwest, sqlx, redis, tokio re-exports)
  ├── replay-macro           (#[record_io], #[instrument], #[instrument_spawns])
  ├── replay-interceptors    (middleware, built-in interceptors)
  └── replay-store           (PostgresStore / InMemoryStore)
        └── replay-core      (types, traits — no heavy deps)
```

`replay-core` has no network or database dependencies. It can be used in unit tests without any infrastructure.

`ditto-server` and `ui/` are standalone — they run as a separate process alongside your service.

---

## Integration guide

### 1. Install at startup

Both globals must be called **once, before serving any requests**:

```rust
// Sets the store for replay_compat interceptors (HTTP, gRPC, DB, Redis)
replay_compat::install(interaction_store, mode);

// Sets the store for #[record_io] macro runtime
replay_core::set_global_store(macro_store);
```

The two functions write to different static variables. Missing either one will silently drop half your recordings.

If you need per-request mode overrides, construct `MockContext` explicitly in your middleware.

---

### 2. HTTP calls

Replace `reqwest::Client` with `replay_compat::http::Client`. The API is identical.

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

**What gets recorded:** method, path (with IDs normalized to `{id}`), response status, response body. Auth headers are stripped.

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

**What gets recorded:** normalized SQL (parameters replaced with `?`), result rows. Credentials are never stored.

---

### 4. Redis commands

Replace `redis::Client` with `replay_compat::redis::Client`. Connections from `get_async_connection()` are instrumented automatically.

```rust
use replay_compat::redis::Client;

async fn get_session(client: &Client, session_id: &str) -> redis::RedisResult<String> {
    let mut conn = client.get_async_connection().await?;
    conn.get(format!("session:{session_id}")).await
}
```

---

### 5. gRPC clients

Wrap your tonic channel with `ReplayGrpcLayer`:

```rust
use replay_interceptors::grpc::ReplayGrpcLayer;
use tonic::transport::Channel;

let channel = Channel::from_static("http://payment-service:50051")
    .connect()
    .await?;

let channel = tower::ServiceBuilder::new()
    .layer(ReplayGrpcLayer::new(store.clone()))
    .service(channel);

let client = PaymentServiceClient::new(channel);
```

**What gets recorded:** gRPC path (`/package.Service/Method`), request and response as base64-encoded proto bytes — no schema information needed.

---

### 6. Arbitrary async functions

Use `#[record_io]` on any async function whose return value you want to record and replay:

```rust
use replay_macro::record_io;

#[record_io]
async fn compute_tax(amount: f64) -> f64 {
    amount * 0.08
}
```

In **Record** mode: the function body runs and the result is stored.
In **Replay** mode: the stored result is returned without running the body.
In **Shadow** mode: the function body runs with the *current* code and the new result is written to `capture_id` for diffing.

For impl blocks, annotate the whole block:

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

`MockContext` lives in a `task_local` — crossing a `tokio::spawn` boundary loses the context. Three ways to handle this:

**Option A — alias the module (recommended, zero per-site changes):**

```rust
use replay_compat as tokio;

tokio::spawn(async { process_webhook().await; });
```

**Option B — annotate specific functions:**

```rust
use replay_macro::instrument_spawns;

#[instrument_spawns]
async fn handle_payment(req: PaymentRequest) -> Result<()> {
    tokio::spawn(async { send_notification().await; });
    tokio::spawn(async { update_analytics().await;  });
    Ok(())
}
```

**Option C — manual, call-by-call:**

```rust
use replay_core::context::spawn_with_ctx;

spawn_with_ctx(async { process_webhook().await; });
```

---

## Framework middleware

### Axum

Use `recording_middleware_with_store` — it records the HTTP entry-point at sequence=0 so all downstream interceptors receive sequences 1, 2, 3 …, and supports Shadow mode via the `X-Ditto-Capture-Id` header.

```rust
use replay_interceptors::http_server::recording_middleware_with_store;
use axum::middleware;

let app = Router::new()
    .route("/payments", post(create_payment))
    .layer(middleware::from_fn_with_state(
        store.clone() as Arc<dyn InteractionStore>,
        recording_middleware_with_store,
    ));
```

`REPLAY_MODE` is read from the environment at startup.

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
// 1. Create a context at the start of each request
let ctx = MockContext::new(ReplayMode::from_env());

// 2. Scope your handler inside it
let response = MOCK_CTX.scope(ctx, async {
    your_handler(request).await
}).await;
```

---

## Replay modes

Set via the `REPLAY_MODE` environment variable:

| Mode | Behaviour |
|---|---|
| `record` | Makes real calls, writes every interaction (including `#[record_io]` results) to the store |
| `replay` | Returns stored responses for all calls. `#[record_io]` functions return stored values without executing the body |
| `shadow` | External calls (HTTP/gRPC/DB/Redis) are mocked from the recording. `#[record_io]` functions run their **actual current code** and write results to a temporary `capture_id`. The harness diffs capture vs recording to detect regressions in business logic |
| `passthrough` | Intercepts calls but does not read or write to store — for observability |
| `off` | Completely transparent — zero overhead, all interceptors are no-ops |

Shadow mode is set automatically by `ditto-server` when it sends `X-Ditto-Capture-Id` alongside `X-Replay-Record-Id`. You do not set `REPLAY_MODE=shadow` directly; run your service with `REPLAY_MODE=replay` and the middleware upgrades to Shadow per-request when the header is present.

---

## Configuration reference

### Environment variables (service under test)

| Variable | Default | Description |
|---|---|---|
| `REPLAY_MODE` | `off` | `record`, `replay`, `passthrough`, or `off` |
| `REPLAY_DB_URL` | — | Postgres connection string for the interaction store |
| `REPLAY_BUILD_HASH` | `""` | Git SHA or build ID. Used to correlate recordings with builds |
| `REPLAY_SAMPLE_RATE` | `1` | Record 1-in-N requests in production. `1` = every request |

### Environment variables (ditto-server)

| Variable | Default | Description |
|---|---|---|
| `REPLAY_DB_URL` | — | Same Postgres store as the service |
| `REPLAY_TARGET_URL` | `http://localhost:3000` | Base URL of the service under test |
| `REPLAY_SERVER_PORT` | `4000` | Port ditto-server listens on |
| `REPLAY_UI_DIR` | `./ui/dist` | Path to the built React assets |

### ditto.toml (optional file config)

Create `ditto.toml` in the directory where you run `ditto-server`. Environment variables take precedence.

```toml
# ditto.toml
target_url = "http://localhost:3001"   # service under test
port       = 4000
ui_dir     = "./ui/dist"
```

Override the config file path with `DITTO_CONFIG=/path/to/ditto.toml`.

---

## Visual UI (ditto-server)

`ditto-server` is a standalone binary that exposes the recording store and replay harness over HTTP, and serves a React UI for visual inspection.

### Starting ditto-server

```bash
REPLAY_DB_URL=postgres://... \
REPLAY_TARGET_URL=http://localhost:3000 \
cargo run -p ditto-server
# → http://localhost:4000
```

Or with a `ditto.toml` in the current directory:

```bash
REPLAY_DB_URL=postgres://... cargo run -p ditto-server
```

### UI layout

```
┌─ nav ──────────────────────────────────────────────────────────────┐
├─ sidebar (recordings) ──┬─ replay detail pane ────────────────────┤
│  POST /checkout  200    │  [record ID + Replay button]             │
│  GET  /analyze   200 ◀  │  [summary: matched · missing · diffs]    │
│  POST /pipeline  200    │  [step list — per-interaction rows]      │
│  ...                    │    seq type  fingerprint  ms  status     │
│  (scrollable)           │    ▶ click a row → diff or raw JSON      │
│  [pagination]           │                                          │
└─────────────────────────┴──────────────────────────────────────────┘
```

- Selecting a row loads the interaction trace for that recording
- Clicking **Replay** fires a Shadow-mode replay and shows field-level diffs inline
- The sidebar and detail pane scroll independently — no page scroll needed

### REST API

```
GET  /api/recordings?limit=20&offset=0   → paginated list of RecordingSummary
GET  /api/recordings/:id                 → full interaction trace
POST /api/recordings/:id/replay          → trigger replay, returns ReplayResult
```

### Building the UI

```bash
cd ui && npm ci && npm run build   # → ui/dist/
```

For development with hot-reload:

```bash
cd ui && npm run dev   # → http://localhost:5173 (proxies /api to :4000)
```

---

## Running a replay

### Full dev workflow (3 terminals)

```bash
# Terminal 1 — record real traffic
REPLAY_MODE=record REPLAY_DB_URL=postgres://... cargo run -p full-flow

# Terminal 2 — ditto-server (UI + replay engine)
REPLAY_DB_URL=postgres://... REPLAY_TARGET_URL=http://localhost:3000 \
cargo run -p ditto-server

# Terminal 3 — UI hot-reload (optional)
cd ui && npm run dev
# Open http://localhost:5173
```

Generate a recording by hitting your service:

```bash
curl -X POST http://localhost:3000/checkout \
  -H "Content-Type: application/json" \
  -d '{"product_id":"widget","quantity":2}'
```

Then open the UI, select the recording, and click **Replay**. The harness will fire the request at the service (which must be running in `REPLAY_MODE=replay`), collect the Shadow-mode results, and display a field-level diff.

### Testing regression detection

Run the deliberately faulty version of the service:

```bash
# Start the faulty service on port 3001
REPLAY_MODE=replay cargo run -p full-flow-faulty

# Point ditto-server at it
REPLAY_DB_URL=postgres://... REPLAY_TARGET_URL=http://localhost:3001 \
cargo run -p ditto-server
```

Replay a recording captured from `full-flow`. The diff will surface the deliberate bugs (wrong tax rate, wrong discount, wrong gRPC response, etc.) as regressions.

---

## Understanding diff reports

The diff engine compares recorded and replayed responses field-by-field:

| Category | Examples | Regresses? |
|---|---|---|
| `regression` | status codes, amounts, IDs, business logic values | Yes |
| `added` | new field present in replayed but not recorded | No |
| `removed` | field present in recording but absent in replay | No |
| `intentional` | fields listed in `.ditto-manifest.toml` | No |
| `noise` | timestamps, trace IDs, UUIDs | No (filtered before diff) |

### Marking intentional changes

Create `.ditto-manifest.toml` at the repo root to suppress known changes:

```toml
# .ditto-manifest.toml
[[changes]]
path    = "body.discount_pct"
reason  = "Updated discount tiers in v2.3.0"

[[changes]]
path    = "body.pricing_model"
reason  = "New pricing model launched"
```

Fields listed here are reclassified from `regression` to `intentional` and do not fail CI.

---

## Adding a custom interceptor

Implement `ReplayInterceptor` for any client library not covered by `replay-compat`:

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

    fn fingerprint(&self, req: &EsSearchRequest) -> String {
        format!("{} /{}", req.method, normalize_index(&req.index))
    }

    fn normalize_request(&self, req: &EsSearchRequest) -> serde_json::Value {
        let mut v = serde_json::to_value(req).unwrap();
        remove_keys(&mut v, &["scroll_id", "preference"]);
        v
    }

    fn normalize_response(&self, res: &EsSearchResponse) -> serde_json::Value {
        let mut v = serde_json::to_value(res).unwrap();
        remove_keys(&mut v, &["took", "_shards"]);
        v
    }

    async fn execute(&self, req: EsSearchRequest) -> Result<EsSearchResponse, Self::Error> {
        self.client.search(SearchParts::Index(&[&req.index])).body(req.body).send().await
    }
}
```

Wrap with `InterceptorRunner` at the call site:

```rust
use replay_core::InterceptorRunner;

let runner = InterceptorRunner::new(
    ElasticsearchInterceptor { client: es_client.clone() },
    store.clone() as Arc<dyn InteractionStore>,
);

let response = runner.run(search_request).await?;
```

See `examples/custom-interceptor/` for a complete runnable example.

**The three fingerprinting rules:**
1. Strip all values that change per-request (IDs, amounts, session tokens)
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
          REPLAY_MODE:       record
          REPLAY_BUILD_HASH: ${{ github.sha }}
          REPLAY_DB_URL:     ${{ secrets.REPLAY_DB_URL }}
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

      - name: Start service in replay mode
        run: ./target/release/your-service &
        env:
          REPLAY_MODE:  replay
          REPLAY_DB_URL: ${{ secrets.REPLAY_DB_URL }}

      - name: Run ditto-server replay
        env:
          REPLAY_DB_URL:     ${{ secrets.REPLAY_DB_URL }}
          REPLAY_TARGET_URL: http://localhost:8080
        run: |
          # Trigger replays for the last 50 recordings and fail if any regress
          cargo run -p ditto-server --bin replay-ci -- --sample 50 --fail-on-regression
```

---

## Automated audit tool

Before integration, run the audit tool to find all call sites that need changing:

```bash
# scan and report (read-only)
cargo replay-audit ./crates

# auto-patch all fixable sites
cargo replay-audit ./crates --fix
```

The `--fix` flag rewrites:
- `reqwest::Client::new()` → `replay_compat::http::Client::new()`
- `redis::Client::open(url)` → `replay_compat::redis::Client::open(url)`
- `tokio::spawn(fut)` → `replay_compat::tokio::task::spawn(fut)`

---

## Troubleshooting

### `#[record_io]` functions not appearing in the trace

**Cause:** `replay_core::set_global_store` was not called — only `replay_compat::install`.

**Fix:** Call both at startup:

```rust
replay_compat::install(interaction_store, mode);
replay_core::set_global_store(macro_store);
```

Both must be given the same underlying store. Capture the concrete `Arc<PostgresStore>` before upcasting:

```rust
let store = Arc::new(PostgresStore::new(&db_url).await?);
replay_compat::install(store.clone() as Arc<dyn InteractionStore>, mode);
replay_core::set_global_store(store as Arc<dyn MacroStore>);
```

### Replay returns all sequences matched (no diffs) even with faulty code

**Cause:** Running with `REPLAY_MODE=replay` — `#[record_io]` functions return stored values without executing the function body, so faulty code never runs.

**Fix:** Use Shadow mode. Run the service with `REPLAY_MODE=replay` and send both `X-Replay-Record-Id` and `X-Ditto-Capture-Id` headers (the harness does this automatically). In Shadow mode the function body executes with the current code and the new result is captured for diffing.

### Sequences misaligned in Shadow replay (wrong diff matches)

**Symptom:** Diff shows a gRPC response where a `#[record_io]` result should be, or values are shifted by one.

**Cause:** In Record mode the HTTP middleware claims sequence=0; in a prior version of Shadow mode it did not, causing all downstream sequences to be off by one.

**Fix:** This is fixed in `recording_middleware_with_store` — it now claims sequence=0 in both Record and Shadow mode (but only writes the interaction in Record mode).

### Replay fails with "HTTP request failed: Connection refused"

**Cause:** The service under test is not running, or `REPLAY_TARGET_URL` points to the wrong port.

**Fix:** Start the service first, then trigger the replay. Check `ditto.toml` or `REPLAY_TARGET_URL` to confirm the port matches.

### Replay fails with "no entry-point (sequence=0) found"

**Cause:** The recording was made without `recording_middleware_with_store` — only `#[record_io]` interactions were stored, not the HTTP entry-point.

**Fix:** Use `recording_middleware_with_store` (not the simpler `recording_middleware`) so the HTTP request and response are stored at sequence=0.

### Replay fails with "no recorded interactions"

**Cause:** The `record_id` exists in the UI but the store the server is reading from is empty (e.g., using an in-memory store in one process and Postgres in another).

**Fix:** Ensure `REPLAY_DB_URL` is the same Postgres database for both the service and `ditto-server`.

### `tokio::spawn` losing context

**Symptom:** Interactions inside spawned tasks are not recorded.

**Fix:** Use `spawn_with_ctx`, `#[instrument_spawns]`, or `use replay_compat as tokio` — see [tokio::spawn propagation](#7-tokiospawn-propagation).

### Store write failures silently dropped

By design, a store write failure does not fail the request. Check logs for `WARN replay_store: write failed: ...`. Run migrations if you haven't:

```bash
sqlx migrate run --database-url $REPLAY_DB_URL \
  --source crates/replay-store/migrations/
```

---

## Contributing

### Running tests

```bash
# all unit and integration tests (no infrastructure required)
cargo test --workspace

# with a live Postgres + Redis
TEST_DB_URL=postgres://... TEST_REDIS_URL=redis://... \
cargo test --workspace --features postgres-tests

# macro expansion tests only
cargo test -p replay-macro
```

### Project conventions

- Every interceptor must pass the `InterceptorRunner` five-mode test suite (record, replay, shadow, passthrough, off)
- Fingerprints must be tested against a corpus of real-world values
- Do not use `unwrap()` in non-test code — use `?` and `thiserror`
- Store writes must never fail a request — log the error and continue
- `MockContext` must be propagated across all `tokio::spawn` boundaries

---

## License

MIT
