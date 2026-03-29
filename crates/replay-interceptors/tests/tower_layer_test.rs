/// Integration tests for the framework-agnostic [`RecordingLayer`].
///
/// Uses a `tower::service_fn` as the inner "handler" and verifies:
/// 1. `MockContext` is injected and readable inside the handler.
/// 2. The `x-replay-record-id` header is honoured in Replay mode.
/// 3. Entry-point interaction is recorded at sequence 0 in Record mode.
/// 4. Off mode passes through with no context and no store writes.

use std::sync::Arc;

use http::{Request, Response};
use replay_core::{current_ctx, InteractionStore, ReplayMode};
use replay_interceptors::RecordingLayer;
use replay_store::InMemoryStore;
use tower::{Layer, Service, ServiceExt};
use uuid::Uuid;

// ── helpers ───────────────────────────────────────────────────────────────────

fn empty_request(method: &str, path: &str) -> Request<()> {
    Request::builder()
        .method(method)
        .uri(path)
        .body(())
        .unwrap()
}

fn empty_request_with_record_id(method: &str, path: &str, id: Uuid) -> Request<()> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("x-replay-record-id", id.to_string())
        .body(())
        .unwrap()
}

/// A handler that reports whether a MockContext is available.
async fn ctx_reporting_handler(
    req: Request<()>,
) -> Result<Response<String>, std::convert::Infallible> {
    let _ = req;
    let body = match current_ctx() {
        Some(ctx) => format!("ctx:{}", ctx.record_id),
        None      => "no-ctx".to_string(),
    };
    Ok(Response::builder()
        .status(200)
        .body(body)
        .unwrap())
}

// ── context injection ─────────────────────────────────────────────────────────

#[tokio::test]
async fn record_mode_injects_context_into_handler() {
    let svc = tower::service_fn(ctx_reporting_handler);
    let mut layered = RecordingLayer::new(ReplayMode::Record).layer(svc);

    let resp = layered
        .ready()
        .await
        .unwrap()
        .call(empty_request("GET", "/foo"))
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.into_body();
    assert!(body.starts_with("ctx:"), "handler must see a MockContext, got: {body}");
}

#[tokio::test]
async fn off_mode_bypasses_context_injection() {
    let svc = tower::service_fn(ctx_reporting_handler);
    let mut layered = RecordingLayer::new(ReplayMode::Off).layer(svc);

    let resp = layered
        .ready()
        .await
        .unwrap()
        .call(empty_request("GET", "/foo"))
        .await
        .unwrap();

    assert_eq!(resp.into_body(), "no-ctx", "Off mode must not inject a context");
}

#[tokio::test]
async fn replay_mode_reads_record_id_from_header() {
    let expected_id = Uuid::new_v4();
    let svc = tower::service_fn(ctx_reporting_handler);
    let mut layered = RecordingLayer::new(ReplayMode::Replay).layer(svc);

    let req = empty_request_with_record_id("GET", "/bar", expected_id);
    let resp = layered.ready().await.unwrap().call(req).await.unwrap();

    let body = resp.into_body();
    assert!(
        body.contains(&expected_id.to_string()),
        "handler must see the record_id from the header; got: {body}"
    );
}

// ── entry-point recording ─────────────────────────────────────────────────────

#[tokio::test]
async fn with_store_records_entry_point_at_sequence_zero() {
    let store = Arc::new(InMemoryStore::new());

    let svc = tower::service_fn(|_req: Request<()>| async {
        Ok::<_, std::convert::Infallible>(
            Response::builder().status(200).body(String::new()).unwrap(),
        )
    });
    let mut layered =
        RecordingLayer::with_store(store.clone(), ReplayMode::Record).layer(svc);

    layered
        .ready()
        .await
        .unwrap()
        .call(empty_request("GET", "/payments/42"))
        .await
        .unwrap();

    // Grab all interactions from the store — we don't know the record_id because
    // the layer generates it internally, so we use get_recent_record_ids.
    let ids = store.get_recent_record_ids(10).await.unwrap();
    assert_eq!(ids.len(), 1, "exactly one recording must exist");

    let interactions = store.get_by_record_id(ids[0]).await.unwrap();
    assert_eq!(interactions.len(), 1);
    assert_eq!(interactions[0].sequence, 0, "entry-point must claim sequence 0");

    let req_json = &interactions[0].request;
    assert_eq!(req_json["method"], "GET");
    assert_eq!(req_json["path"], "/payments/42");
}

#[tokio::test]
async fn with_store_does_not_write_in_replay_mode() {
    let store = Arc::new(InMemoryStore::new());

    let svc = tower::service_fn(|_req: Request<()>| async {
        Ok::<_, std::convert::Infallible>(
            Response::builder().status(200).body(String::new()).unwrap(),
        )
    });
    let mut layered =
        RecordingLayer::with_store(store.clone(), ReplayMode::Replay).layer(svc);

    layered
        .ready()
        .await
        .unwrap()
        .call(empty_request("GET", "/foo"))
        .await
        .unwrap();

    let ids = store.get_recent_record_ids(10).await.unwrap();
    assert!(ids.is_empty(), "Replay mode must not write to the store");
}

// ── context not leaked between requests ───────────────────────────────────────

#[tokio::test]
async fn each_request_gets_a_fresh_record_id() {
    // Two independent layers, each gets a separate service_fn.
    let mut svc1 = RecordingLayer::new(ReplayMode::Record)
        .layer(tower::service_fn(ctx_reporting_handler));
    let mut svc2 = RecordingLayer::new(ReplayMode::Record)
        .layer(tower::service_fn(ctx_reporting_handler));

    let r1: String = svc1
        .ready().await.unwrap()
        .call(empty_request("GET", "/a"))
        .await.unwrap()
        .into_body();

    let r2: String = svc2
        .ready().await.unwrap()
        .call(empty_request("GET", "/b"))
        .await.unwrap()
        .into_body();

    assert_ne!(r1, r2, "each request must get a unique record_id");

    // Neither context should leak outside its request
    assert!(current_ctx().is_none(), "context must not leak outside the service call");
}

// ── context visible from a spawned task ───────────────────────────────────────

#[tokio::test]
async fn context_propagates_to_spawn_with_ctx() {
    let store = Arc::new(InMemoryStore::new());

    let svc = tower::service_fn(|_req: Request<()>| async {
        // Spawn a child task — context must be visible there too
        let handle = replay_core::spawn_with_ctx(async {
            current_ctx().is_some()
        });
        let ctx_in_child = handle.await.unwrap();
        let body = ctx_in_child.to_string();
        Ok::<_, std::convert::Infallible>(Response::builder().status(200).body(body).unwrap())
    });

    let mut layered = RecordingLayer::with_store(
        store as Arc<dyn replay_core::InteractionStore>,
        ReplayMode::Record,
    )
    .layer(svc);

    let resp = layered
        .ready().await.unwrap()
        .call(empty_request("POST", "/spawn-test"))
        .await.unwrap();

    assert_eq!(resp.into_body(), "true", "spawned task must see the MockContext");
}
