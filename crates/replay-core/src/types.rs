use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CallType {
    Http,
    Grpc,
    Postgres,
    Redis,
    Function,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CallStatus {
    Completed,
    Error,
    Cancelled,
    Timeout,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ReplayMode {
    Record,
    Replay,
    /// Shadow mode — fired by the replay harness when it wants per-interaction diffs.
    ///
    /// External interceptors (HTTP, DB, Redis, gRPC) mock from the original recording
    /// exactly like `Replay`.  `#[record_io]` functions execute their real code and
    /// write results to a separate *capture* record_id so the harness can diff them
    /// against the original without side-effecting external systems.
    Shadow,
    Passthrough,
    Off,
}
