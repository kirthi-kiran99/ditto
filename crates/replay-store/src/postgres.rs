use async_trait::async_trait;
use replay_core::{CallStatus, CallType, Interaction, MacroStore};
use serde_json::Value;
use sqlx::{postgres::PgPoolOptions, PgPool, Row};
use uuid::Uuid;

use crate::store::{InteractionStore, Result, StoreError};

#[derive(Debug)]
pub struct PostgresStore {
    pool: PgPool,
}

impl PostgresStore {
    pub async fn new(url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(url)
            .await
            .map_err(|e| StoreError::Database(e.to_string()))?;

        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(|e| StoreError::Database(e.to_string()))?;

        Ok(Self { pool })
    }

    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn call_type_str(ct: &CallType) -> &'static str {
    match ct {
        CallType::Http     => "http",
        CallType::Grpc     => "grpc",
        CallType::Postgres => "postgres",
        CallType::Redis    => "redis",
        CallType::Function => "function",
    }
}

fn call_status_str(cs: &CallStatus) -> &'static str {
    match cs {
        CallStatus::Completed => "completed",
        CallStatus::Error     => "error",
        CallStatus::Cancelled => "cancelled",
        CallStatus::Timeout   => "timeout",
    }
}

fn parse_call_type(s: &str) -> CallType {
    match s {
        "http"     => CallType::Http,
        "grpc"     => CallType::Grpc,
        "postgres" => CallType::Postgres,
        "redis"    => CallType::Redis,
        _          => CallType::Function,
    }
}

fn parse_call_status(s: &str) -> CallStatus {
    match s {
        "error"     => CallStatus::Error,
        "cancelled" => CallStatus::Cancelled,
        "timeout"   => CallStatus::Timeout,
        _           => CallStatus::Completed,
    }
}

fn row_to_interaction(row: &sqlx::postgres::PgRow) -> Interaction {
    Interaction {
        id:           row.get("id"),
        record_id:    row.get("record_id"),
        parent_id:    row.get("parent_id"),
        sequence:     row.get::<i32, _>("sequence") as u32,
        call_type:    parse_call_type(row.get("call_type")),
        fingerprint:  row.get("fingerprint"),
        request:      row.get("request"),
        response:     row.get("response"),
        duration_ms:  row.get::<i64, _>("duration_ms") as u64,
        status:       parse_call_status(row.get("status")),
        error:        row.get("error"),
        recorded_at:  row.get("recorded_at"),
        build_hash:   row.get("build_hash"),
        service_name: row.get("service_name"),
    }
}

const SELECT_COLS: &str = r#"
    id, record_id, parent_id, sequence,
    call_type::text as call_type, fingerprint,
    request, response, duration_ms,
    status::text as status, error,
    recorded_at, build_hash, service_name
"#;

// ── InteractionStore impl ─────────────────────────────────────────────────────

#[async_trait]
impl InteractionStore for PostgresStore {
    async fn write(&self, i: &Interaction) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO interactions
                (id, record_id, parent_id, sequence, call_type, fingerprint,
                 request, response, duration_ms, status, error,
                 recorded_at, build_hash, service_name)
            VALUES ($1,$2,$3,$4,$5::call_type,$6,$7,$8,$9,$10::call_status,$11,$12,$13,$14)
            "#,
        )
        .bind(i.id)
        .bind(i.record_id)
        .bind(i.parent_id)
        .bind(i.sequence as i32)
        .bind(call_type_str(&i.call_type))
        .bind(&i.fingerprint)
        .bind(&i.request)
        .bind(&i.response)
        .bind(i.duration_ms as i64)
        .bind(call_status_str(&i.status))
        .bind(&i.error)
        .bind(i.recorded_at)
        .bind(&i.build_hash)
        .bind(&i.service_name)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Database(e.to_string()))?;
        Ok(())
    }

    async fn write_batch(&self, interactions: &[Interaction]) -> Result<()> {
        if interactions.is_empty() {
            return Ok(());
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| StoreError::Database(e.to_string()))?;

        for i in interactions {
            sqlx::query(
                r#"
                INSERT INTO interactions
                    (id, record_id, parent_id, sequence, call_type, fingerprint,
                     request, response, duration_ms, status, error,
                     recorded_at, build_hash, service_name)
                VALUES ($1,$2,$3,$4,$5::call_type,$6,$7,$8,$9,$10::call_status,$11,$12,$13,$14)
                "#,
            )
            .bind(i.id)
            .bind(i.record_id)
            .bind(i.parent_id)
            .bind(i.sequence as i32)
            .bind(call_type_str(&i.call_type))
            .bind(&i.fingerprint)
            .bind(&i.request)
            .bind(&i.response)
            .bind(i.duration_ms as i64)
            .bind(call_status_str(&i.status))
            .bind(&i.error)
            .bind(i.recorded_at)
            .bind(&i.build_hash)
            .bind(&i.service_name)
            .execute(&mut *tx)
            .await
            .map_err(|e| StoreError::Database(e.to_string()))?;
        }

        tx.commit()
            .await
            .map_err(|e| StoreError::Database(e.to_string()))?;
        Ok(())
    }

    async fn get_by_record_id(&self, record_id: Uuid) -> Result<Vec<Interaction>> {
        let sql = format!(
            "SELECT {} FROM interactions WHERE record_id = $1 ORDER BY sequence ASC",
            SELECT_COLS
        );
        let rows = sqlx::query(&sql)
            .bind(record_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::Database(e.to_string()))?;

        Ok(rows.iter().map(row_to_interaction).collect())
    }

    async fn find_match(
        &self,
        record_id:   Uuid,
        call_type:   CallType,
        fingerprint: &str,
        sequence:    u32,
    ) -> Result<Option<Interaction>> {
        let sql = format!(
            r#"SELECT {} FROM interactions
               WHERE record_id = $1
                 AND call_type = $2::call_type
                 AND fingerprint = $3
                 AND sequence = $4
               LIMIT 1"#,
            SELECT_COLS
        );
        let row = sqlx::query(&sql)
            .bind(record_id)
            .bind(call_type_str(&call_type))
            .bind(fingerprint)
            .bind(sequence as i32)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| StoreError::Database(e.to_string()))?;

        Ok(row.as_ref().map(row_to_interaction))
    }

    async fn find_nearest(
        &self,
        record_id:   Uuid,
        fingerprint: &str,
        sequence:    u32,
    ) -> Result<Option<Interaction>> {
        let sql = format!(
            r#"SELECT {} FROM interactions
               WHERE record_id = $1 AND fingerprint = $2
               ORDER BY ABS(sequence - $3) ASC
               LIMIT 1"#,
            SELECT_COLS
        );
        let row = sqlx::query(&sql)
            .bind(record_id)
            .bind(fingerprint)
            .bind(sequence as i32)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| StoreError::Database(e.to_string()))?;

        Ok(row.as_ref().map(row_to_interaction))
    }

    async fn get_recent_record_ids(&self, limit: usize) -> Result<Vec<Uuid>> {
        let rows = sqlx::query(
            r#"SELECT DISTINCT record_id FROM interactions
               ORDER BY record_id LIMIT $1"#,
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::Database(e.to_string()))?;

        Ok(rows.iter().map(|r| r.get::<Uuid, _>("record_id")).collect())
    }
}

// ── MacroStore impl ───────────────────────────────────────────────────────────

#[async_trait]
impl MacroStore for PostgresStore {
    async fn store_fn_call(&self, interaction: &Interaction) {
        let _ = self.write(interaction).await;
    }

    async fn load_fn_response(
        &self,
        record_id:   Uuid,
        fingerprint: &str,
        sequence:    u32,
    ) -> Option<Value> {
        let exact = self
            .find_match(record_id, CallType::Function, fingerprint, sequence)
            .await
            .ok()
            .flatten();

        if exact.is_some() {
            return exact.map(|i| i.response);
        }

        self.find_nearest(record_id, fingerprint, sequence)
            .await
            .ok()
            .flatten()
            .map(|i| i.response)
    }
}
