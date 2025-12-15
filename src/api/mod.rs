mod upload;

use crate::config::Config;
use crate::error::{Result, ResultIoExt};
use crate::metadata::{extract_pkginfo, generate_files_db, generate_repo_db};
use crate::models::{Package, PackageQuery};
use crate::storage::Storage;
use crate::upload::UploadSessionStore;
use axum::{
    Json,
    extract::{Path as AxumPath, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use std::sync::Arc;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub struct AppState {
    pub storage: Storage,
    pub config: Config,
    pub upload_store: UploadSessionStore,
}

/// List packages with optional filtering
#[utoipa::path(
    get,
    path = "/packages",
    params(
        ("name" = Option<String>, Query, description = "Filter by package name"),
        ("repo" = Option<String>, Query, description = "Filter by repository"),
        ("arch" = Option<String>, Query, description = "Filter by architecture")
    ),
    responses(
        (status = 200, description = "List of packages", body = Vec<Package>),
        (status = 500, description = "Internal server error")
    ),
    tag = "packages"
)]
pub async fn list_packages(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PackageQuery>,
) -> Result<Json<Vec<Package>>> {
    // If both repo and arch are not specified, list all packages
    // Otherwise use specified repo/arch or defaults
    let mut packages = if query.repo.is_none() && query.arch.is_none() {
        state.storage.list_all_packages().await?
    } else {
        let repo = query
            .repo
            .as_deref()
            .unwrap_or(&state.config.storage.default_repo);
        let arch = query
            .arch
            .as_deref()
            .unwrap_or(&state.config.storage.default_arch);

        state.storage.list_packages(repo, arch).await?
    };

    // Apply filters
    if let Some(ref name_filter) = query.name {
        packages.retain(|p| p.name.contains(name_filter));
    }

    if let Some(ref repo_filter) = query.repo {
        packages.retain(|p| &p.repo == repo_filter);
    }

    if let Some(ref arch_filter) = query.arch {
        packages.retain(|p| &p.arch == arch_filter);
    }

    Ok(Json(packages))
}

/// Delete a package
#[utoipa::path(
    delete,
    path = "/packages/{name}",
    params(
        ("name" = String, Path, description = "Package name to delete"),
        ("repo" = Option<String>, Query, description = "Repository name"),
        ("arch" = Option<String>, Query, description = "Architecture")
    ),
    responses(
        (status = 204, description = "Package deleted successfully"),
        (status = 404, description = "Package not found"),
        (status = 500, description = "Internal server error")
    ),
    tag = "packages"
)]
pub async fn delete_package(
    State(state): State<Arc<AppState>>,
    AxumPath(name): AxumPath<String>,
    Query(query): Query<PackageQuery>,
) -> Result<impl IntoResponse> {
    let repo = query
        .repo
        .unwrap_or_else(|| state.config.storage.default_repo.clone());
    let arch = query
        .arch
        .unwrap_or_else(|| state.config.storage.default_arch.clone());

    // Load package metadata
    let package = state.storage.load_package(&repo, &arch, &name).await?;

    // Delete package
    state.storage.delete_package(&package).await?;

    // Regenerate repository database
    regenerate_repo_db(&state.storage, &repo, &arch).await?;

    Ok(StatusCode::NO_CONTENT)
}

/// Regenerate repository database for a given repo/arch
pub(crate) async fn regenerate_repo_db(storage: &Storage, repo: &str, arch: &str) -> Result<()> {
    let packages = storage.list_packages(repo, arch).await?;

    let repo_dir = storage.repo_dir(repo, arch)?;

    // Load pkginfo for each package
    let mut pkg_data = Vec::new();
    for pkg in packages {
        let pkg_path = storage.package_path(repo, arch, &pkg.filename)?;
        let data = tokio::fs::read(&pkg_path).await.map_io_err(&pkg_path)?;

        // Extract pkginfo in blocking task (CPU-intensive decompression)
        let pkginfo = tokio::task::spawn_blocking(move || extract_pkginfo(&data))
            .await
            .map_err(|e| std::io::Error::other(format!("Task join error: {}", e)))??;

        pkg_data.push((pkg, pkginfo));
    }

    // Generate databases
    generate_repo_db(&repo_dir, repo, &pkg_data).await?;
    generate_files_db(&repo_dir, repo, &pkg_data).await?;

    Ok(())
}

#[derive(OpenApi)]
#[openapi(
    components(
        schemas(
            Package,
            PackageQuery,
            upload::InitiateUploadRequest,
            upload::InitiateUploadResponse,
            upload::UploadChunkResponse,
            upload::UploadSignatureResponse,
            upload::CompleteUploadRequest,
            upload::ChunkInfo,
            upload::AbortUploadResponse
        )
    ),
    tags(
        (name = "packages", description = "Package management endpoints"),
        (name = "chunked-uploads", description = "Chunked upload endpoints")
    )
)]
pub struct ApiDoc;

/// Create the API router with all routes
pub fn create_api_router(state: Arc<AppState>) -> OpenApiRouter {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(list_packages))
        .routes(routes!(delete_package))
        .routes(routes!(upload::initiate_upload))
        .routes(routes!(upload::upload_chunk))
        .routes(routes!(upload::upload_signature))
        .routes(routes!(upload::complete_upload))
        .routes(routes!(upload::abort_upload))
        .with_state(state)
}
