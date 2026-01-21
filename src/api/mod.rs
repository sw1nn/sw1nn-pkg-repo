pub mod cleanup_policy;
pub mod delete_versions;
mod upload;

use crate::config::Config;
use crate::db_actor::DbUpdateHandle;
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
    pub storage: Arc<Storage>,
    pub config: Config,
    pub upload_store: UploadSessionStore,
    pub db_update: DbUpdateHandle,
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

    // Request database update (debounced, coalesced)
    state.db_update.request_update(&repo, &arch).await;

    Ok(StatusCode::NO_CONTENT)
}

/// Force rebuild of repository database
#[utoipa::path(
    post,
    path = "/repos/{repo}/os/{arch}/rebuild",
    params(
        ("repo" = String, Path, description = "Repository name"),
        ("arch" = String, Path, description = "Architecture")
    ),
    responses(
        (status = 202, description = "Database rebuild initiated"),
        (status = 500, description = "Internal server error")
    ),
    tag = "packages"
)]
pub async fn rebuild_db(
    State(state): State<Arc<AppState>>,
    AxumPath((repo, arch)): AxumPath<(String, String)>,
) -> Result<impl IntoResponse> {
    tracing::info!(repo = %repo, arch = %arch, "Force rebuild requested via API");

    // Force immediate rebuild (bypass debounce)
    state.db_update.force_rebuild(&repo, &arch).await;

    Ok(StatusCode::ACCEPTED)
}

/// Regenerate repository database for a given repo/arch
pub(crate) async fn regenerate_repo_db(storage: &Storage, repo: &str, arch: &str) -> Result<()> {
    let packages = storage.list_packages(repo, arch).await?;

    let repo_dir = storage.repo_dir(repo, arch)?;

    // Group packages by name and keep only the latest version of each
    let latest_packages = select_latest_versions(packages);

    tracing::info!(
        repo = %repo,
        arch = %arch,
        package_count = latest_packages.len(),
        "Regenerating database with latest package versions"
    );

    // Load pkginfo for each package
    let mut pkg_data = Vec::new();
    for pkg in latest_packages {
        let pkg_path = storage.package_path(repo, arch, &pkg.filename)?;

        // Read package file, skipping if missing (orphaned metadata)
        let data = match tokio::fs::read(&pkg_path).await {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::warn!(
                    path = %pkg_path.display(),
                    package = %pkg.name,
                    "Orphaned metadata - package file missing, skipping"
                );
                continue;
            }
            Err(e) => return Err(e).map_io_err(&pkg_path),
        };

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

/// Select only the latest version of each package
fn select_latest_versions(packages: Vec<Package>) -> Vec<Package> {
    use std::collections::HashMap;

    let mut latest_by_name: HashMap<String, Package> = HashMap::new();

    for pkg in packages {
        let dominated = latest_by_name
            .get(&pkg.name)
            .is_some_and(|existing| compare_versions(&existing.version, &pkg.version).is_ge());

        if !dominated {
            latest_by_name.insert(pkg.name.clone(), pkg);
        }
    }

    latest_by_name.into_values().collect()
}

/// Compare two Arch Linux package versions
/// Returns Ordering::Greater if v1 > v2, etc.
fn compare_versions(v1: &str, v2: &str) -> std::cmp::Ordering {
    // Try to parse as semver for comparison
    // Format: [epoch:]pkgver-pkgrel
    let parse = |v: &str| -> Option<(u64, semver::Version, u64)> {
        let (epoch, rest) = if let Some((e, r)) = v.split_once(':') {
            (e.parse::<u64>().ok()?, r)
        } else {
            (0, v)
        };

        let (pkgver, pkgrel) = rest.rsplit_once('-')?;
        let pkgrel_num = pkgrel.parse::<u64>().ok()?;
        let semver_ver = semver::Version::parse(pkgver).ok()?;

        Some((epoch, semver_ver, pkgrel_num))
    };

    match (parse(v1), parse(v2)) {
        (Some((e1, sv1, pr1)), Some((e2, sv2, pr2))) => {
            e1.cmp(&e2)
                .then_with(|| sv1.cmp(&sv2))
                .then_with(|| pr1.cmp(&pr2))
        }
        // Fall back to string comparison if parsing fails
        _ => v1.cmp(v2),
    }
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
            upload::AbortUploadResponse,
            delete_versions::DeleteVersionsRequest,
            delete_versions::DeleteVersionsResponse,
            cleanup_policy::CleanupPolicyRequest,
            cleanup_policy::CleanupPolicyResponse,
            cleanup_policy::PackageCleanupDetail
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
    use axum::routing::post;

    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(list_packages))
        .routes(routes!(delete_package))
        .routes(routes!(rebuild_db))
        .route(
            "/packages/:name/versions/delete",
            post(delete_versions::delete_versions),
        )
        .routes(routes!(cleanup_policy::apply_cleanup_policy))
        .routes(routes!(upload::initiate_upload))
        .routes(routes!(upload::upload_chunk))
        .routes(routes!(upload::upload_signature))
        .routes(routes!(upload::complete_upload))
        .routes(routes!(upload::abort_upload))
        .with_state(state)
}
