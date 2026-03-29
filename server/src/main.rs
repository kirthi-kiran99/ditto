mod config;
mod routes;
mod types;

use std::sync::Arc;

use axum::{
    Router,
    routing::{get, post},
};
use replay_core::InteractionStore;
use replay_interceptors::ReplayHarness;
use replay_store::PostgresStore;
use tower_http::{cors::CorsLayer, services::ServeDir};
use tracing::info;

use config::Config;
use routes::{
    recordings::{get_recording, list_recordings, list_tags, list_recordings_by_tag, replay_all_by_tag},
    replay::trigger_replay,
};

/// Shared application state injected into every handler.
#[derive(Clone)]
pub struct AppState {
    pub store:   Arc<dyn InteractionStore>,
    pub harness: Arc<ReplayHarness>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ditto_server=info".into()),
        )
        .init();

    let cfg = Config::load().unwrap_or_else(|e| {
        eprintln!("configuration error: {e}");
        std::process::exit(1);
    });

    info!("connecting to store at {}", &cfg.db_url);
    let store: Arc<dyn InteractionStore> = Arc::new(
        PostgresStore::new(&cfg.db_url)
            .await
            .unwrap_or_else(|e| {
                eprintln!("store connection failed: {e}");
                std::process::exit(1);
            }),
    );

    let harness = Arc::new(ReplayHarness::new(store.clone(), cfg.target_url.clone()));

    info!("UI assets: {:?}", &cfg.ui_dir);
    info!("replay target: {}", &cfg.target_url);

    let state = AppState { store, harness };

    let app = Router::new()
        // ── API routes ──────────────────────────────────────────────────────────
        .route("/api/recordings", get(list_recordings))
        .route("/api/recordings/:id", get(get_recording))
        .route("/api/recordings/:id/replay", post(trigger_replay))
        .route("/api/tags", get(list_tags))
        .route("/api/tags/recordings", get(list_recordings_by_tag))
        .route("/api/tags/replay-all", post(replay_all_by_tag))
        // ── State ───────────────────────────────────────────────────────────────
        .with_state(state)
        // ── Static files (React app) — fallback for everything else ─────────────
        .fallback_service(ServeDir::new(&cfg.ui_dir))
        // ── CORS (permissive for dev; restrict in production) ───────────────────
        .layer(CorsLayer::permissive());

    let addr = format!("0.0.0.0:{}", cfg.port)
        .parse::<std::net::SocketAddr>()
        .unwrap();

    info!("ditto-server listening on http://{addr}");

    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await?;

    Ok(())
}
