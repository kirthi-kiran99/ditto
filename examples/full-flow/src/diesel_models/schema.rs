// Diesel schema definition
// Generated for ditto full-flow example

diesel::table! {
    diesel_orders (order_id) {
        order_id -> Text,
        idempotency_key -> Text,
        amount -> Float8,
        status -> Text,
        created_at -> Timestamptz,
    }
}
