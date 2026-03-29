use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize, Default)]
struct FileConfig {
    db_url:    Option<String>,
    redis_url: Option<String>,
}

/// Resolve the Postgres connection string with the following precedence:
///
/// 1. `REPLAY_DB_URL` environment variable
/// 2. `db_url` key in `ditto.toml` (or the path in `DITTO_CONFIG`)
/// 3. `None` — caller decides how to handle the absence
pub fn db_url() -> Option<String> {
    std::env::var("REPLAY_DB_URL").ok().or_else(|| load_file().db_url)
}

/// Resolve the Redis connection string with the following precedence:
///
/// 1. `REDIS_URL` environment variable
/// 2. `redis_url` key in `ditto.toml`
/// 3. `None`
pub fn redis_url() -> Option<String> {
    std::env::var("REDIS_URL").ok().or_else(|| load_file().redis_url)
}

fn load_file() -> FileConfig {
    let path = std::env::var("DITTO_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("ditto.toml"));

    let Ok(text) = std::fs::read_to_string(&path) else {
        return FileConfig::default();
    };

    match toml::from_str::<FileConfig>(&text) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("Warning: could not parse {}: {e}", path.display());
            FileConfig::default()
        }
    }
}
