use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use replay_interceptors::HarnessStatus;
use uuid::Uuid;

use crate::{AppState, types::{DiffNodeJson, MismatchJson, ReplayResultJson}};

pub async fn trigger_replay(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let result = state.harness.run_one(id).await;

    let (status_str, error_msg) = match &result.status {
        HarnessStatus::Passed      => ("passed", None),
        HarnessStatus::Failed      => ("failed", None),
        HarnessStatus::Error(msg)  => ("error",  Some(msg.clone())),
    };

    let mismatches = result
        .report
        .mismatches
        .iter()
        .map(|m| MismatchJson {
            sequence:      m.sequence,
            fingerprint:   m.fingerprint.clone(),
            is_regression: m.diff.is_regression,
            nodes:         m.diff.nodes.iter().map(|n| DiffNodeJson {
                path:      n.path.clone(),
                old_value: n.old_value.clone(),
                new_value: n.new_value.clone(),
                category:  format!("{:?}", n.category).to_lowercase(),
            }).collect(),
        })
        .collect();

    let body = ReplayResultJson {
        record_id:       id,
        status:          status_str.to_string(),
        error:           error_msg,
        is_regression:   result.report.is_regression,
        duration_ms:     result.duration_ms,
        matched:         result.report.matched,
        missing_replays: result.report.missing_replays.clone(),
        extra_replays:   result.report.extra_replays.clone(),
        mismatches,
    };

    (StatusCode::OK, Json(body)).into_response()
}
