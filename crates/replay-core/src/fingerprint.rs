pub struct FingerprintBuilder;

impl FingerprintBuilder {
    /// "GET /api/v1/payments/pay_abc123" → "GET /api/v1/payments/{id}"
    pub fn http(method: &str, path: &str) -> String {
        let tpl = replace_ids(path);
        format!("{} {}", method, tpl)
    }

    /// "/package.Service/Method" → as-is (already stable)
    pub fn grpc(path: &str) -> String {
        path.to_string()
    }

    /// "SELECT * FROM pa WHERE id = $1 AND amount = $2" → template with ?
    pub fn sql(query: &str) -> String {
        replace_sql_params(query)
    }

    /// "payment:session:sess_xyz789" → "payment:session:{id}"
    pub fn redis_key(key: &str) -> String {
        key.split(':')
            .map(|seg| if looks_like_id(seg) { "{id}".to_string() } else { seg.to_string() })
            .collect::<Vec<_>>()
            .join(":")
    }

    /// module::path::FnName → as-is
    pub fn function(path: &str) -> String {
        path.to_string()
    }
}

fn replace_ids(s: &str) -> String {
    s.split('/')
        .map(|seg| if looks_like_id(seg) { "{id}".to_string() } else { seg.to_string() })
        .collect::<Vec<_>>()
        .join("/")
}

/// A segment looks like a dynamic ID if it's long enough and contains digits
/// (UUIDs, numeric IDs, opaque tokens like pay_abc123)
fn looks_like_id(s: &str) -> bool {
    if s.len() < 6 {
        return false;
    }
    let all_safe_chars = s.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_');
    let has_digit = s.chars().any(|c| c.is_ascii_digit());
    all_safe_chars && has_digit
}

/// Replace $1, $2, ... positional params with ?
fn replace_sql_params(s: &str) -> String {
    let mut result = s.to_string();
    // Work from high to low so "$10" is replaced before "$1"
    for i in (1..=99).rev() {
        let placeholder = format!("${}", i);
        if result.contains(&placeholder) {
            result = result.replace(&placeholder, "?");
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_strips_ids() {
        assert_eq!(
            FingerprintBuilder::http("GET", "/api/v1/payments/pay_abc123"),
            "GET /api/v1/payments/{id}"
        );
    }

    #[test]
    fn http_preserves_static_paths() {
        assert_eq!(
            FingerprintBuilder::http("POST", "/api/v1/payments"),
            "POST /api/v1/payments"
        );
    }

    #[test]
    fn sql_replaces_placeholders() {
        assert_eq!(
            FingerprintBuilder::sql("SELECT * FROM payments WHERE id = $1"),
            "SELECT * FROM payments WHERE id = ?"
        );
    }

    #[test]
    fn sql_replaces_multiple_placeholders() {
        assert_eq!(
            FingerprintBuilder::sql("SELECT * FROM payments WHERE id = $1 AND amount = $2"),
            "SELECT * FROM payments WHERE id = ? AND amount = ?"
        );
    }

    #[test]
    fn redis_key_strips_ids() {
        assert_eq!(
            FingerprintBuilder::redis_key("payment:session:sess_xyz789"),
            "payment:session:{id}"
        );
    }

    #[test]
    fn redis_key_preserves_static() {
        assert_eq!(
            FingerprintBuilder::redis_key("config:global"),
            "config:global"
        );
    }
}
