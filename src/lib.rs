pub mod api;
pub mod config;
pub mod error;
pub mod metadata;
pub mod models;
pub mod repo;
pub mod storage;
pub mod upload;

use api::{AppState, create_api_router};
use axum::{Router, routing::get};
use config::Config;
use repo::serve_file;
use std::io::IsTerminal;
use std::sync::Arc;
use storage::Storage;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use utoipa_rapidoc::RapiDoc;

/// Initialize the tracing subscriber for logging
/// Uses journald when running as a service (no terminal), fmt when running interactively
pub fn init_tracing() {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "sw1nn_pkg_repo=info,tower_http=warn".into());

    if std::io::stdout().is_terminal() {
        // Running in a terminal, use formatted output
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    } else {
        // Running as a service, use journald
        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_journald::layer().expect("Failed to connect to journald"))
            .init();
    }
}

/// Run the package repository service
pub async fn run_service(config_path: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    init_tracing();

    // Log version early
    tracing::info!("sw1nn-pkg-repo version {}", env!("CARGO_PKG_VERSION"));

    // Load configuration
    let config = Config::load(config_path).unwrap_or_else(|_| {
        tracing::warn!("Failed to load config, using defaults");
        Config::default()
    });

    tracing::info!("Starting server with config: {:?}", config);

    // Create storage
    let storage = Storage::new(&config.storage.data_path);

    // Create upload session store
    let upload_store = upload::UploadSessionStore::new(config.storage.data_path.clone());

    // Create shared state
    let state = Arc::new(AppState {
        storage,
        config: config.clone(),
        upload_store,
    });

    // Build API routes using utoipa_axum router
    let (api_router, api_doc) = create_api_router(state.clone()).split_for_parts();

    // Build repository routes (pacman interface)
    let repo_routes = Router::new()
        .route("/:repo/os/:arch/:filename", get(serve_file))
        .with_state(state.clone());

    // Build documentation routes
    let doc_routes = Router::new()
        .merge(RapiDoc::with_openapi("/api-docs/openapi.json", api_doc).path("/api-docs"));

    // Combine all routes
    let app = Router::new()
        .nest("/api", api_router)
        .merge(repo_routes)
        .merge(doc_routes)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    // Start server
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    tracing::info!("Server listening on {}", addr);
    tracing::info!("API documentation available at http://{}/api-docs", addr);

    axum::serve(listener, app).await?;

    Ok(())
}
