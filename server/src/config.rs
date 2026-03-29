use std::path::PathBuf;

use serde::Deserialize;

/// Server-only fields that can also live in `ditto.toml`.
#[derive(Deserialize, Default)]
struct FileConfig {
    target_url: Option<String>,
    port:       Option<u16>,
    ui_dir:     Option<String>,
}

pub struct Config {
    pub db_url:     String,
    pub target_url: String,
    pub port:       u16,
    pub ui_dir:     PathBuf,
}

impl Config {
    /// Load config with the following precedence (highest → lowest):
    ///   1. Environment variables
    ///   2. `ditto.toml` in the current directory (or path in `DITTO_CONFIG`)
    ///   3. Built-in defaults
    pub fn load() -> Result<Self, String> {
        let file_cfg = Self::load_file();

        // DB URL is resolved by replay-store (shared with minimal-axum and others).
        let db_url = replay_store::db_url()
            .ok_or("REPLAY_DB_URL is required (set it in the environment or in ditto.toml)")?;

        let target_url = std::env::var("REPLAY_TARGET_URL")
            .ok()
            .or(file_cfg.target_url)
            .unwrap_or_else(|| "http://localhost:3000".to_string());

        let port = std::env::var("REPLAY_SERVER_PORT")
            .ok()
            .and_then(|s| s.parse::<u16>().ok())
            .or(file_cfg.port)
            .unwrap_or(4000);

        let ui_dir = std::env::var("REPLAY_UI_DIR")
            .ok()
            .or(file_cfg.ui_dir)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("./ui/dist"));

        Ok(Self { db_url, target_url, port, ui_dir })
    }

    fn load_file() -> FileConfig {
        let path = std::env::var("DITTO_CONFIG")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("ditto.toml"));

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
}
