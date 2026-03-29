use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use uuid::Uuid;

use crate::{
    AppState,
    types::{InteractionJson, ListParams, RecordingDetailJson, RecordingsPage, RecordingSummaryJson},
};

pub async fn list_recordings(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> impl IntoResponse {
    match state.store.list_recordings(params.limit, params.offset).await {
        Ok(summaries) => {
            let items: Vec<RecordingSummaryJson> = summaries
                .into_iter()
                .map(|s| RecordingSummaryJson {
                    record_id:         s.record_id,
                    method:            s.method,
                    path:              s.path,
                    status_code:       s.status_code,
                    recorded_at:       s.recorded_at,
                    build_hash:        s.build_hash,
                    service_name:      s.service_name,
                    interaction_count: s.interaction_count,
                })
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
