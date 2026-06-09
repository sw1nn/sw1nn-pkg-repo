// Shared helpers are compiled into each integration-test binary separately, so
// any helper a given binary doesn't use trips `dead_code`. Allow it here.
#![allow(dead_code)]

use axum::Router;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;
use sw1nn_pkg_repo::api::{AppState, create_api_router};
use sw1nn_pkg_repo::config::Config;
use sw1nn_pkg_repo::db_actor::DbUpdateActor;
use sw1nn_pkg_repo::repo::serve_file;
use sw1nn_pkg_repo::storage::Storage;
use sw1nn_pkg_repo::upload::UploadSessionStore;
use tar::{Builder, Header};
use tempfile::TempDir;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use utoipa_rapidoc::RapiDoc;
use zstd::stream::write::Encoder;

pub async fn setup_test_app() -> Router {
    let (router, _storage) = setup_test_app_with_storage().await;
    router
}

/// Build the test app and also return the backing [`Storage`] so tests can seed
/// packages directly without going through the upload API.
pub async fn setup_test_app_with_storage() -> (Router, Arc<Storage>) {
    // Create temporary directory for test data
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path().to_path_buf();

    // Don't drop temp_dir - leak it so it persists for the test
    std::mem::forget(temp_dir);

    let mut config = Config::default();
    config.storage.data_path = temp_path.clone();
    config.storage.auto_cleanup_enabled = false; // Disable auto-cleanup for tests

    let storage = Arc::new(Storage::new(&config.storage.data_path));
    let upload_store = UploadSessionStore::new(temp_path);

    // Create database update actor with short debounce for tests
    let (db_actor, db_update_handle) =
        DbUpdateActor::with_debounce(Arc::clone(&storage), Duration::from_millis(100));

    // Spawn actor task (will run for duration of test)
    tokio::spawn(db_actor.run());

    let state = Arc::new(AppState {
        storage: Arc::clone(&storage),
        config: config.clone(),
        upload_store,
        db_update: db_update_handle,
        http_client: reqwest::Client::new(),
    });

    // Build API routes
    let (api_router, api_doc) = create_api_router(state.clone()).split_for_parts();

    // Build repository routes
    let repo_routes = Router::new()
        .route(
            "/{repo}/os/{arch}/{filename}",
            axum::routing::get(serve_file),
        )
        .with_state(state.clone());

    // Build documentation routes
    let doc_routes = Router::new()
        .merge(RapiDoc::with_openapi("/api-docs/openapi.json", api_doc).path("/api-docs"));

    // Combine all routes
    let router = Router::new()
        .nest("/api", api_router)
        .merge(repo_routes)
        .merge(doc_routes)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    (router, storage)
}

pub async fn setup_test_app_with_auth(auth: sw1nn_pkg_repo::config::AuthConfig) -> Router {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path().to_path_buf();
    std::mem::forget(temp_dir);

    let mut config = Config::default();
    config.storage.data_path = temp_path.clone();
    config.storage.auto_cleanup_enabled = false;
    config.auth = Some(auth);

    let storage = Arc::new(Storage::new(&config.storage.data_path));
    let upload_store = UploadSessionStore::new(temp_path);

    let (db_actor, db_update_handle) =
        DbUpdateActor::with_debounce(Arc::clone(&storage), Duration::from_millis(100));
    tokio::spawn(db_actor.run());

    let state = Arc::new(AppState {
        storage,
        config: config.clone(),
        upload_store,
        db_update: db_update_handle,
        http_client: reqwest::Client::new(),
    });

    let (api_router, api_doc) = create_api_router(state.clone()).split_for_parts();
    let repo_routes = Router::new()
        .route(
            "/{repo}/os/{arch}/{filename}",
            axum::routing::get(serve_file),
        )
        .with_state(state.clone());
    let doc_routes = Router::new()
        .merge(RapiDoc::with_openapi("/api-docs/openapi.json", api_doc).path("/api-docs"));

    Router::new()
        .nest("/api", api_router)
        .merge(repo_routes)
        .merge(doc_routes)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}

/// Create a test package with the given name, version, and architecture
pub fn create_test_package(pkgname: &str, pkgver: &str, arch: &str) -> Vec<u8> {
    // Create .PKGINFO content
    let pkginfo_content = format!(
        "pkgname = {}\npkgver = {}\narch = {}\n",
        pkgname, pkgver, arch
    );

    // Create a tar archive in memory
    let mut tar_data = Vec::new();
    {
        let mut tar = Builder::new(&mut tar_data);

        // Add .PKGINFO file
        let mut header = Header::new_gnu();
        header.set_path(".PKGINFO").unwrap();
        header.set_size(pkginfo_content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append(&header, pkginfo_content.as_bytes()).unwrap();

        tar.finish().unwrap();
    }

    // Compress with zstd
    let mut compressed = Vec::new();
    {
        let mut encoder = Encoder::new(&mut compressed, 3).unwrap();
        encoder.write_all(&tar_data).unwrap();
        encoder.finish().unwrap();
    }

    compressed
}

/// Seed a package (file + metadata) directly into storage and return both the
/// raw package bytes and the filename it was stored under.
pub async fn seed_package(
    storage: &Storage,
    repo: &str,
    name: &str,
    version: &str,
    arch: &str,
) -> (Vec<u8>, String) {
    use sw1nn_pkg_repo::models::Package;

    let data = create_test_package(name, version, arch);
    let filename = format!("{name}-{version}-{arch}.pkg.tar.zst");
    let package = Package {
        name: name.to_owned(),
        version: version.to_owned(),
        arch: arch.to_owned(),
        repo: repo.to_owned(),
        filename: filename.clone(),
        sha256: String::new(),
        size: data.len() as u64,
        created_at: chrono::Utc::now(),
    };
    storage.store_package(&package, &data).await.unwrap();
    (data, filename)
}
