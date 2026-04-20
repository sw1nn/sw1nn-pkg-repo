pub mod auth;
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
    pub http_client: reqwest::Client,
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
    // List packages from specified repo or all repos
    let mut packages = if let Some(ref repo) = query.repo {
        // If arch filter is specified, use list_packages_for_arch
        if let Some(ref arch) = query.arch {
            state.storage.list_packages_for_arch(repo, arch).await?
        } else {
            state.storage.list_packages(repo).await?
        }
    } else {
        state.storage.list_all_packages().await?
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
    _user: crate::auth::AuthenticatedUser,
    State(state): State<Arc<AppState>>,
    AxumPath(name): AxumPath<String>,
    Query(query): Query<PackageQuery>,
) -> Result<impl IntoResponse> {
    let repo = query
        .repo
        .unwrap_or_else(|| state.config.storage.default_repo.clone());

    // Load package metadata (no arch in path, arch is in metadata)
    let package = state.storage.load_package(&repo, &name).await?;

    // Get the arch from the package for database update
    let arch = package.arch.clone();

    // Delete package
    state.storage.delete_package(&package).await?;

    crate::metrics::record_package_deleted(&repo, 1);

    // Request database update for affected architectures
    // If package arch is "any", update the default arch database
    // Otherwise update the specific arch database
    let update_arch = if arch == "any" {
        query
            .arch
            .unwrap_or_else(|| state.config.storage.default_arch.clone())
    } else {
        arch
    };
    state.db_update.request_update(&repo, &update_arch).await;

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
    _user: crate::auth::AuthenticatedUser,
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
    // List packages for this arch (includes "any" architecture packages)
    let packages = storage.list_packages_for_arch(repo, arch).await?;

    // Database files go in os/{arch}/ for URL compatibility
    let db_dir = storage.db_dir(repo, arch)?;

    // Group packages by name and keep only the latest version of each
    let latest_packages = select_latest_versions(packages);

    tracing::info!(
        repo,
        arch,
        package_count = latest_packages.len(),
        "Regenerating database with latest package versions"
    );

    // Load pkginfo for each package
    let mut pkg_data = Vec::new();
    for pkg in latest_packages {
        // Package files are in flat storage (no arch in path)
        let pkg_path = storage.package_path(repo, &pkg.filename)?;

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
            .map_err(|e| std::io::Error::other(format!("Task join error: {e}")))??;

        pkg_data.push((pkg, pkginfo));
    }

    // Generate databases
    generate_repo_db(&db_dir, repo, &pkg_data).await?;
    generate_files_db(&db_dir, repo, &pkg_data).await?;

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

/// Compare two Arch Linux package versions using the pacman vercmp algorithm
/// (rpmvercmp), as implemented by [`alpm_types::FullVersion`].
///
/// Handles the full `[epoch:]pkgver-pkgrel` form, including AUR-style
/// pkgvers like `0.15.0.r166.gae5dbc9` that aren't valid semver.
///
/// Falls back to plain string comparison only if both inputs fail to parse
/// as an alpm-package-version.
fn compare_versions(v1: &str, v2: &str) -> std::cmp::Ordering {
    use std::str::FromStr;
    match (
        alpm_types::FullVersion::from_str(v1),
        alpm_types::FullVersion::from_str(v2),
    ) {
        (Ok(a), Ok(b)) => a.cmp(&b),
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
            "/packages/{name}/versions/delete",
            post(delete_versions::delete_versions),
        )
        .routes(routes!(cleanup_policy::apply_cleanup_policy))
        .routes(routes!(upload::initiate_upload))
        .routes(routes!(upload::upload_chunk))
        .routes(routes!(upload::upload_signature))
        .routes(routes!(upload::complete_upload))
        .routes(routes!(upload::abort_upload))
        .route("/auth/device/code", post(auth::device_code))
        .route("/auth/device/token", post(auth::device_token))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::cmp::Ordering;

    fn pkg<N, V>(name: N, version: V) -> Package
    where
        N: Into<String>,
        V: Into<String>,
    {
        Package {
            name: name.into(),
            version: version.into(),
            arch: "x86_64".to_owned(),
            repo: "sw1nn".to_owned(),
            filename: "ignored.pkg.tar.zst".to_owned(),
            sha256: String::new(),
            size: 0,
            created_at: Utc::now(),
        }
    }

    /// Regression for AUR-style git pkgvers where the old string-compare
    /// fallback treated `r72` > `r166` (because `'7' > '1'` lexicographically),
    /// causing newly-uploaded packages to be dropped from the generated DB.
    #[test]
    fn compare_versions_orders_aur_git_style_numerically() {
        assert_eq!(
            compare_versions("0.15.0.r166.gae5dbc9-1", "0.15.0.r72.ga024ce7-1"),
            Ordering::Greater
        );
        assert_eq!(
            compare_versions("0.15.0.r72.ga024ce7-1", "0.15.0.r166.gae5dbc9-1"),
            Ordering::Less
        );
    }

    #[test]
    fn compare_versions_orders_basic_semver() {
        assert_eq!(compare_versions("1.0.0-1", "1.0.1-1"), Ordering::Less);
        assert_eq!(compare_versions("2.0.0-1", "1.9.9-1"), Ordering::Greater);
        assert_eq!(compare_versions("1.2.3-1", "1.2.3-1"), Ordering::Equal);
    }

    #[test]
    fn compare_versions_orders_by_pkgrel() {
        assert_eq!(compare_versions("1.0.0-2", "1.0.0-1"), Ordering::Greater);
        assert_eq!(compare_versions("1.0.0-10", "1.0.0-2"), Ordering::Greater);
    }

    #[test]
    fn compare_versions_honours_epoch() {
        // Higher epoch always wins, even if the pkgver looks smaller.
        assert_eq!(compare_versions("1:0.1.0-1", "2.0.0-1"), Ordering::Greater);
        assert_eq!(compare_versions("2.0.0-1", "1:0.1.0-1"), Ordering::Less);
    }

    #[test]
    fn compare_versions_falls_back_for_unparseable_input() {
        // Neither side is a valid alpm-package-version (missing pkgrel).
        // Should not panic; string compare is the documented fallback.
        assert_eq!(compare_versions("garbage", "garbage"), Ordering::Equal);
        assert_eq!(compare_versions("aaa", "bbb"), Ordering::Less);
    }

    #[test]
    fn select_latest_versions_picks_newer_aur_git_version() {
        let old = pkg("sw1nn-waybar-git", "0.15.0.r72.ga024ce7-1");
        let new = pkg("sw1nn-waybar-git", "0.15.0.r166.gae5dbc9-1");

        // Insertion order should not matter: the newer version must always win.
        for packages in [
            vec![old.clone(), new.clone()],
            vec![new.clone(), old.clone()],
        ] {
            let latest = select_latest_versions(packages);
            assert_eq!(latest.len(), 1);
            assert_eq!(latest[0].version, "0.15.0.r166.gae5dbc9-1");
        }
    }
}
