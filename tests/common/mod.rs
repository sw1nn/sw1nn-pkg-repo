use axum::Router;
use sw1nn_pkg_repo::api::{create_api_router, AppState};
use sw1nn_pkg_repo::config::Config;
use sw1nn_pkg_repo::repo::serve_file;
use sw1nn_pkg_repo::storage::Storage;
use std::sync::Arc;
use tempfile::TempDir;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use utoipa_rapidoc::RapiDoc;

pub async fn setup_test_app() -> Router {
    // Create temporary directory for test data
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path().to_path_buf();

    // Don't drop temp_dir - leak it so it persists for the test
    std::mem::forget(temp_dir);

    let mut config = Config::default();
    config.storage.data_path = temp_path;

    let storage = Storage::new(&config.storage.data_path);
    let state = Arc::new(AppState {
        storage,
        config: config.clone(),
    });

    // Build API routes
    let (api_router, api_doc) = create_api_router(state.clone()).split_for_parts();

    // Build repository routes
    let repo_routes = Router::new()
        .route("/:repo/os/:arch/:filename", axum::routing::get(serve_file))
        .with_state(state.clone());

    // Build documentation routes
    let doc_routes =
        Router::new().merge(RapiDoc::with_openapi("/api-docs/openapi.json", api_doc).path("/api-docs"));

    // Combine all routes
    Router::new()
        .nest("/api", api_router)
        .merge(repo_routes)
        .merge(doc_routes)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}
