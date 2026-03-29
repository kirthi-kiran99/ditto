use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use replay_interceptors::HarnessStatus;
use uuid::Uuid;

use crate::{
    AppState,
    types::{
        DiffNodeJson, InteractionJson, ListParams, MismatchJson, RecordingDetailJson,
        RecordingsPage, RecordingSummaryJson, ReplayResultJson, RunAllResultJson,
        TagListParams, TagParams, TagSummaryJson, TagsPage,
    },
};

fn to_recording_summary_json(s: replay_core::RecordingSummary) -> RecordingSummaryJson {
    RecordingSummaryJson {
        record_id:         s.record_id,
        method:            s.method,
        path:              s.path,
        status_code:       s.status_code,
        recorded_at:       s.recorded_at,
        build_hash:        s.build_hash,
        service_name:      s.service_name,
        tag:               s.tag,
        interaction_count: s.interaction_count,
    }
}

pub async fn list_recordings(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> impl IntoResponse {
    match state.store.list_recordings(params.limit, params.offset).await {
        Ok(summaries) => {
            let items: Vec<RecordingSummaryJson> = summaries
                .into_iter()
                .map(to_recording_summary_json)
                .collect();
            let total = if items.len() == params.limit {
                // May have more pages; report at least one more item than current page end
                params.offset + items.len() + 1
            } else {
                params.offset + items.len()
            };
            (
                StatusCode::OK,
                Json(RecordingsPage { items, total, limit: params.limit, offset: params.offset }),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

pub async fn get_recording(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match state.store.get_by_record_id(id).await {
        Ok(interactions) => {
            let items = interactions
                .into_iter()
                .map(|i| InteractionJson {
                    id:          i.id,
                    sequence:    i.sequence,
                    call_type:   format!("{:?}", i.call_type).to_lowercase(),
                    fingerprint: i.fingerprint,
                    request:     i.request,
                    response:    i.response,
                    duration_ms: i.duration_ms,
                    status:      format!("{:?}", i.status).to_lowercase(),
                    error:       i.error,
                })
                .collect();
            (StatusCode::OK, Json(RecordingDetailJson { record_id: id, interactions: items }))
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

// ── Tag-based handlers ────────────────────────────────────────────────────────

pub async fn list_tags(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> impl IntoResponse {
    let total = match state.store.count_tags().await {
        Ok(n) => n,
        Err(e) => return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ).into_response(),
    };
    match state.store.list_tags(params.limit, params.offset).await {
        Ok(tags) => {
            let items: Vec<TagSummaryJson> = tags
                .into_iter()
                .map(|t| TagSummaryJson {
                    service_name:      t.service_name,
                    tag:               t.tag,
                    recording_count:   t.recording_count,
                    interaction_count: t.interaction_count,
                    last_recorded_at:  t.last_recorded_at,
                })
                .collect();
            (StatusCode::OK, Json(TagsPage { items, total, limit: params.limit, offset: params.offset }))
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ).into_response(),
    }
}

pub async fn list_recordings_by_tag(
    State(state): State<AppState>,
    Query(params): Query<TagListParams>,
) -> impl IntoResponse {
    let service_name = &params.service_name;
    let tag          = &params.tag;
    let total = match state.store.count_recordings_by_tag(service_name, tag).await {
        Ok(n) => n,
        Err(e) => return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ).into_response(),
    };
    match state.store.list_recordings_by_tag(service_name, tag, params.limit, params.offset).await {
        Ok(summaries) => {
            let items: Vec<RecordingSummaryJson> = summaries
                .into_iter()
                .map(to_recording_summary_json)
                .collect();
            (StatusCode::OK, Json(RecordingsPage { items, total, limit: params.limit, offset: params.offset }))
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ).into_response(),
    }
}

pub async fn replay_all_by_tag(
    State(state): State<AppState>,
    Query(params): Query<TagParams>,
) -> impl IntoResponse {
    let service_name = &params.service_name;
    let tag          = &params.tag;
    let record_ids = match state.store.get_record_ids_by_tag(service_name, tag).await {
        Ok(ids) => ids,
        Err(e) => return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ).into_response(),
    };

    let total = record_ids.len();
    let start = std::time::Instant::now();

    let mut results_json = Vec::with_capacity(total);
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut errors = 0usize;

    for id in record_ids {
        let result = state.harness.run_one(id).await;

        let (status_str, error_msg) = match &result.status {
            HarnessStatus::Passed     => { passed += 1; ("passed", None) }
            HarnessStatus::Failed     => { failed += 1; ("failed", None) }
            HarnessStatus::Error(msg) => { errors += 1; ("error", Some(msg.clone())) }
        };

        let mismatches = result.report.mismatches.iter().map(|m| MismatchJson {
            sequence:      m.sequence,
            fingerprint:   m.fingerprint.clone(),
            is_regression: m.diff.is_regression,
            nodes:         m.diff.nodes.iter().map(|n| DiffNodeJson {
                path:      n.path.clone(),
                old_value: n.old_value.clone(),
                new_value: n.new_value.clone(),
                category:  format!("{:?}", n.category).to_lowercase(),
            }).collect(),
        }).collect();

        results_json.push(ReplayResultJson {
            record_id:       id,
            status:          status_str.to_string(),
            error:           error_msg,
            is_regression:   result.report.is_regression,
            duration_ms:     result.duration_ms,
            matched:         result.report.matched,
            missing_replays: result.report.missing_replays.clone(),
            extra_replays:   result.report.extra_replays.clone(),
            mismatches,
        });
    }

    let body = RunAllResultJson {
        service_name: service_name.clone(),
        tag:          tag.clone(),
        total,
        passed,
        failed,
        errors,
        duration_ms: start.elapsed().as_millis() as u64,
        results:     results_json,
    };

    (StatusCode::OK, Json(body)).into_response()
}
