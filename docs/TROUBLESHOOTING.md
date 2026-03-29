# Troubleshooting

Common failure modes, their symptoms, and how to fix them.

---

## Table of Contents

- [Interactions not being recorded](#interactions-not-being-recorded)
- [Replay fails: no matching interaction found](#replay-fails-no-matching-interaction-found)
- [tokio::spawn loses context](#tokiospawn-loses-context)
- [Non-deterministic fingerprints](#non-deterministic-fingerprints)
- [Store write failures silently dropped](#store-write-failures-silently-dropped)
- [Replay returns stale data](#replay-returns-stale-data)
- [Sequence drift across builds](#sequence-drift-across-builds)
- [Context visible after request ends](#context-visible-after-request-ends)
- [Actix-web middleware not intercepting requests](#actix-web-middleware-not-intercepting-requests)
- [Postgres schema not found](#postgres-schema-not-found)
- [Replay in unit tests: install() panics](#replay-in-unit-tests-install-panics)
- [cargo replay-audit not found](#cargo-replay-audit-not-found)

---

## Interactions not being recorded

**Symptom:** The store remains empty even though the service is handling requests.

**Diagnosis checklist:**

1. **Check `REPLAY_MODE`** — the service must see `REPLAY_MODE=record`:
   ```bash
   echo $REPLAY_MODE  # must be "record"
   # or inside the app:
   println!("{}", std::env::var("REPLAY_MODE").unwrap_or_default());
   ```

2. **Check `replay_compat::install()` is called** — it must be called once,
   before the first request is served. A common mistake is calling it inside
   a per-request handler.

3. **Check middleware registration** — the `RecordingLayer` (or
   `recording_middleware`) must be in the middleware stack that wraps your
   routes. Middleware added after `.layer(...)` in Axum is applied in reverse
   order; if the recording layer is added last, it may not see all routes.

4. **Check `REPLAY_SAMPLE_RATE`** — if set to `100`, only 1-in-100 requests are
   recorded. For development, set `REPLAY_SAMPLE_RATE=1` (record every request).

5. **Check the store's `len()`** in `InMemoryStore` tests — `InMemoryStore` is
   not shared across processes. If the service and the test checker run in
   separate processes, the in-memory store will appear empty to the checker.
   Use `PostgresStore` for multi-process scenarios.

---

## Replay fails: no matching interaction found

**Symptom:** `runner.run()` returns `Err(ReplayError::NotFound { fingerprint, sequence })`,
or the replay harness reports missing interactions.

**Cause 1 — Fingerprint mismatch**

The call site changed its URL structure, SQL, or key pattern between the
recording build and the replay build.  Compare the fingerprint from the error
with what is stored:

```sql
-- What was recorded?
SELECT fingerprint, sequence, call_type
FROM interactions
WHERE record_id = '<your-record-id>'
ORDER BY sequence;
```

Common mistakes:
- A UUID that was not in the path during recording is now included (or vice versa).
- SQL changed whitespace or parameter ordering.
- A Kafka topic name includes an environment suffix that changed.

**Fix:** Adjust `fingerprint()` in your interceptor so the same logical call
produces the same fingerprint across builds.

---

**Cause 2 — Sequence mismatch**

A new outbound call was inserted *before* an existing call in the code path.
This shifts all subsequent sequence numbers.

The replay runtime uses `find_nearest` as a fallback when the exact sequence
does not match.  If the drift is larger than one or two positions, `find_nearest`
may pick the wrong interaction.

**Fix options:**

- Increase `MatchConfig::max_sequence_drift` (default: 2):
  ```rust
  MatchConfig { max_sequence_drift: 5, ..Default::default() }
  ```

- Re-record the affected flows after the structural change.

---

**Cause 3 — Context not propagated across `tokio::spawn`**

If part of the call stack crosses a `tokio::spawn` boundary, the `MockContext`
is lost and the interceptor claims a slot from a *different* context (or none
at all).  See [tokio::spawn loses context](#tokiospawn-loses-context).

---

## tokio::spawn loses context

**Symptom:** Interactions inside spawned tasks are not recorded; or a task
inside a handler always sees `current_ctx() == None` even though the parent
has a context.

**Root cause:** `MockContext` lives in a `task_local` — it is scoped to the
current async task.  Crossing a `tokio::spawn` boundary spawns a new task with
an empty task-local.

**Fix option A — Module alias (zero per-site changes):**

```rust
// In lib.rs or main.rs — once per crate
use replay_compat as tokio;

// Now tokio::spawn(...) automatically propagates context
tokio::spawn(async { background_work().await });
```

**Fix option B — Annotate functions that call spawn:**

```rust
use replay_macro::instrument_spawns;

#[instrument_spawns]
async fn handle_payment(req: PaymentRequest) -> Result<()> {
    // All tokio::spawn calls in this function body are rewritten automatically.
    tokio::spawn(async { send_notification().await });
    Ok(())
}
```

**Fix option C — Manual, per call-site:**

```rust
replay_core::spawn_with_ctx(async { background_work().await });
```

**Detection:** Run `cargo replay-audit ./crates` to find all unpatched
`tokio::spawn` call sites.

---

## Non-deterministic fingerprints

**Symptom:** Replay fails even though no code changed between the recording and
replay runs.

**Cause:** The fingerprint includes a value that changes per-request — a
timestamp, random nonce, session token, or a UUID embedded in a URL that is
not normalised.

**Diagnosis:** Log the fingerprint in your interceptor:

```rust
fn fingerprint(&self, req: &MyRequest) -> String {
    let fp = format!("{} {}", req.method, req.path);
    println!("[DEBUG] fingerprint: {fp}");
    fp
}
```

Make two requests to the same logical endpoint and compare the fingerprints.
If they differ, the non-constant part is the culprit.

**Fix:** In `fingerprint()`, ensure all volatile path segments are replaced
with a placeholder.  `FingerprintBuilder::http(method, path)` does this
automatically for UUID-shaped segments and numeric IDs.  For custom ID formats,
extend the normalisation:

```rust
fn fingerprint(&self, req: &MyRequest) -> String {
    // Strip date-prefixed IDs: "ord_20240115_abc" → "ord_{id}"
    let path = regex::Regex::new(r"\d{8}_[a-z0-9]+")
        .unwrap()
        .replace_all(&req.path, "{id}");
    FingerprintBuilder::http(&req.method, &path)
}
```

---

## Store write failures silently dropped

**Design decision:** A store write failure must never fail the real request.
All `store.write()` calls log the error and continue.

**Symptom:** Requests succeed but interactions are not stored; logs contain
`WARN replay_store: write failed: ...`.

**Common causes:**

1. **`REPLAY_DB_URL` not set** — the store is never initialized:
   ```
   Error: replay_compat::install() has not been called
   ```

2. **Postgres connection pool exhausted** — under high load, writes are
   dropped when the pool is full.  Increase `max_connections`:
   ```rust
   PostgresStore::with_pool_size(&db_url, 20).await?
   ```

3. **Schema migrations not run:**
   ```bash
   sqlx migrate run --database-url $REPLAY_DB_URL \
     --source crates/replay-store/migrations/
   ```

4. **Disk space / network partition** — check your Postgres logs.

---

## Replay returns stale data

**Symptom:** The replayed response is missing fields that were added after the
recording was made, or contains fields that were removed.

**Cause:** The recording predates a schema change.  The stored JSON was
normalised at the time of recording and does not include the new field.

**Fix options:**

1. **Re-record** the affected flows against the old build, or with the new
   schema in place.

2. **Add a migration script** to backfill the new field in stored interactions:
   ```sql
   UPDATE interactions
   SET response = jsonb_set(response, '{new_field}', '"default_value"')
   WHERE fingerprint LIKE 'GET /payments%'
     AND (response->>'new_field') IS NULL;
   ```

3. **Mark the field as a known difference** in your diff config so it does not
   cause CI failures while you re-record:
   ```toml
   [noise_filters]
   response_fields = ["$.new_field"]
   ```

---

## Sequence drift across builds

**Symptom:** Replay works for the first few calls but starts failing mid-handler
with sequence mismatches.

**Cause:** The new build makes a different number of outbound calls before
reaching the failing call site — for example, a new feature added a Redis
lookup that was not in the recording.

**Diagnosis:** Compare the sequence of calls in Record mode vs Replay mode:

```sql
-- What was recorded
SELECT sequence, call_type, fingerprint
FROM interactions
WHERE record_id = '<record_id>'
ORDER BY sequence;
```

vs the log output from the new build during replay.

**Fix options:**

1. **Increase max sequence drift** in your `MatchConfig`.

2. **Re-record** after the structural change stabilises.

3. **Use `find_nearest`** fallback — it is enabled by default, so drifts of 1
   position are bridged automatically.

---

## Context visible after request ends

**Symptom:** `current_ctx()` returns `Some(...)` outside of any request handler;
test assertions fail because the context from a previous test leaks into the
next one.

**Cause:** The `MOCK_CTX.scope(ctx, ...)` future completed but a `tokio::spawn`
task that held the context is still running.

**Fix in tests:** Await all spawned tasks before the test ends:

```rust
let handle = tokio::spawn(async { background_work().await });
// ... assertions ...
handle.await.unwrap();  // ensure the task — and its context scope — finishes
```

**Fix in production:** This should not happen because `MOCK_CTX.scope()` is
tied to the request lifetime.  If it does, check for long-running background
tasks that escape the request scope.

---

## Actix-web middleware not intercepting requests

**Symptom:** Requests go through but no interactions are recorded; `current_ctx()`
returns `None` inside handlers.

**Cause 1 — Feature flag not enabled:**

```toml
# Cargo.toml — must include the feature
replay-interceptors = { version = "0.1", features = ["actix-middleware"] }
```

**Cause 2 — Middleware registered after routes:**

In Actix-web, `.wrap()` is applied in reverse registration order.  The replay
middleware must be registered *last* so it is the *outermost* layer:

```rust
App::new()
    .service(payment_scope())  // routes first
    .wrap(ReplayActixMiddleware::new(mode))  // middleware last = outermost
```

**Cause 3 — Per-scope vs app-level registration:**

If `.wrap()` is applied only to a sub-scope, routes outside that scope are not
covered.  Register at the `App` level for full coverage.

---

## Postgres schema not found

**Symptom:** `replay_store::PostgresStore::new()` returns an error like:
```
error: relation "interactions" does not exist
```

**Fix:** Run the bundled migrations:

```bash
sqlx migrate run \
  --database-url "$REPLAY_DB_URL" \
  --source crates/replay-store/migrations/
```

Or use `PostgresStore::migrate_and_new()` which runs migrations automatically
at startup (safe to call on every boot — idempotent):

```rust
let store = PostgresStore::migrate_and_new(&db_url).await?;
```

---

## Replay in unit tests: install() panics

**Symptom:**
```
thread 'main' panicked at 'replay_compat::install() has not been called'
```
or the second call to `install()` in the same test binary overwrites the first.

**Cause:** `replay_compat::install()` sets a process-global store.  Unit tests
run in the same process and share the global.

**Fix:** In tests, bypass the global entirely by passing the store directly to
`MockContext`:

```rust
use replay_core::{MockContext, ReplayMode, MOCK_CTX};
use replay_store::InMemoryStore;

let store     = Arc::new(InMemoryStore::new());
let record_id = Uuid::new_v4();

// Pass store directly — no global required
let ctx = MockContext::with_store(record_id, ReplayMode::Record, store.clone());

MOCK_CTX.scope(ctx, async {
    // your_interceptor.run(request).await
}).await;
```

Each test gets its own `InMemoryStore`, so tests are fully isolated even when
run in parallel.

---

## cargo replay-audit not found

**Symptom:**
```
error: no such subcommand: `replay-audit`
```

**Cause:** The alias is defined in `.cargo/config.toml` but the binary is
feature-gated.

**Fix:** Ensure the `audit` feature is included when running the alias:

```bash
# Explicit invocation (always works)
cargo run -p replay-interceptors --features audit --bin replay-audit ./crates

# Or rebuild with the feature enabled so the alias picks it up
cargo build -p replay-interceptors --features audit
```

The alias in `.cargo/config.toml`:
```toml
[alias]
replay-audit = "run -p replay-interceptors --features audit --bin replay-audit --"
```
