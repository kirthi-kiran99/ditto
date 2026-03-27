use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::types::{CallStatus, CallType};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Interaction {
    pub id:           Uuid,
    pub record_id:    Uuid,
    pub parent_id:    Option<Uuid>,
    pub sequence:     u32,
    pub call_type:    CallType,
    pub fingerprint:  String,
    pub request:      Value,
    pub response:     Value,
    pub duration_ms:  u64,
    pub status:       CallStatus,
    pub error:        Option<String>,
    pub recorded_at:  DateTime<Utc>,
    pub build_hash:   String,
    pub service_name: String,
}

impl Interaction {
    pub fn new(
        record_id:   Uuid,
        sequence:    u32,
        call_type:   CallType,
        fingerprint: String,
        request:     Value,
        response:    Value,
        duration_ms: u64,
    ) -> Self {
        Self {
            id:           Uuid::new_v4(),
            record_id,
            parent_id:    None,
            sequence,
            call_type,
            fingerprint,
            request,
            response,
            duration_ms,
            status:       CallStatus::Completed,
            error:        None,
            recorded_at:  Utc::now(),
            build_hash:   String::new(),
            service_name: String::new(),
        }
    }
}
