//! Diesel models for the full-flow example.

use diesel::prelude::*;
use serde::{Deserialize, Serialize};
use sqlx::types::chrono;

/// Diesel order model - equivalent to the sqlx OrderRow
#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Insertable, Selectable)]
#[diesel(table_name = super::schema::diesel_orders)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct DieselOrder {
    pub order_id: String,
    pub idempotency_key: String,
    pub amount: f64,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// New order for insertion (without created_at which is auto-set)
#[derive(Debug, Clone, Insertable)]
#[diesel(table_name = super::schema::diesel_orders)]
pub struct NewDieselOrder {
    pub order_id: String,
    pub idempotency_key: String,
    pub amount: f64,
    pub status: String,
}
