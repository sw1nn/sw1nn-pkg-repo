use crate::config::Config;
use crate::error::{Error, Result};
use crate::metadata::{calculate_sha256, extract_pkginfo, generate_files_db, generate_repo_db};
use crate::models::{Package, PackageQuery};
use crate::storage::Storage;
use axum::{
    extract::{Multipart, Path as AxumPath, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::Utc;
use std::sync::Arc;
use utoipa::OpenApi;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub struct AppState {
    pub storage: Storage,
    pub config: Config,
}

/// Request body for package upload
#[derive(ToSchema)]
pub struct PackageUploadRequest {
    /// Package file (.pkg.tar.zst)
    #[schema(format = "binary")]
    pub file: String,
    /// Repository name (optional, defaults to 'sw1nn')
    #[schema(example = "sw1nn")]
    pub repo: Option<String>,
    /// Architecture (optional, defaults to 'x86_64')
    #[schema(example = "x86_64")]
    pub arch: Option<String>,
}

/// Upload a package file to the repository
#[utoipa::path(
    post,
    path = "/packages",
    request_body(content = PackageUploadRequest, content_type = "multipart/form-data"),
    responses(
        (status = 201, description = "Package uploaded successfully", body = Package),
        (status = 400, description = "Invalid package file"),
        (status = 409, description = "Package already exists"),
        (status = 500, description = "Internal server error")
    ),
    tag = "packages"
)]
pub async fn upload_package(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse> {
    let mut package_data: Option<Vec<u8>> = None;
    let mut repo = state.config.storage.default_repo.clone();
    let mut arch = state.config.storage.default_arch.clone();

    // Parse multipart form
    while let Some(field) = multipart.next_field().await.map_err(|e| {
        Error::InvalidPackage {
            pkgname: format!("Failed to read multipart field: {}", e),
        }
    })? {
        let name = field.name().unwrap_or("").to_string();

        match name.as_str() {
            "file" => {
                let data = field.bytes().await.map_err(|e| Error::InvalidPackage {
                    pkgname: format!("Failed to read file data: {}", e),
                })?;
                package_data = Some(data.to_vec());
            }
            "repo" => {
                repo = field.text().await.unwrap_or(repo);
            }
            "arch" => {
                arch = field.text().await.unwrap_or(arch);
            }
            _ => {}
        }
    }

    let package_data = package_data.ok_or_else(|| Error::InvalidPackage {
        pkgname: "No package file provided".to_string(),
    })?;

    // Extract .PKGINFO
    let pkginfo = extract_pkginfo(&package_data)?;

    // Calculate checksums
    let sha256 = calculate_sha256(&package_data);
    let size = package_data.len() as u64;

    // Create filename
    let filename = format!("{}-{}-{}.pkg.tar.zst", pkginfo.pkgname, pkginfo.pkgver, pkginfo.arch);

    // Check if package already exists
    if state.storage.package_exists(&repo, &pkginfo.arch, &filename).await {
        return Err(Error::PackageAlreadyExists {
            pkgname: filename.clone(),
        });
    }

    // Create package record
    let package = Package {
        name: pkginfo.pkgname.clone(),
        version: pkginfo.pkgver.clone(),
        arch: pkginfo.arch.clone(),
        repo: repo.clone(),
        filename: filename.clone(),
        sha256,
        size,
        created_at: Utc::now(),
    };

    // Store package
    state.storage.store_package(&package, &package_data).await?;

    // Regenerate repository database
    regenerate_repo_db(&state.storage, &repo, &pkginfo.arch).await?;

    Ok((StatusCode::CREATED, Json(package)))
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
    let repo = query.repo.unwrap_or_else(|| state.config.storage.default_repo.clone());
    let arch = query.arch.unwrap_or_else(|| state.config.storage.default_arch.clone());

    let mut packages = state.storage.list_packages(&repo, &arch).await?;

    // Filter by name if provided
    if let Some(name) = query.name {
        packages.retain(|p| p.name.contains(&name));
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
    let repo = query.repo.unwrap_or_else(|| state.config.storage.default_repo.clone());
    let arch = query.arch.unwrap_or_else(|| state.config.storage.default_arch.clone());

    // Load package metadata
    let package = state.storage.load_package(&repo, &arch, &name).await?;

    // Delete package
    state.storage.delete_package(&package).await?;

    // Regenerate repository database
    regenerate_repo_db(&state.storage, &repo, &arch).await?;

    Ok(StatusCode::NO_CONTENT)
}

/// Regenerate repository database for a given repo/arch
async fn regenerate_repo_db(storage: &Storage, repo: &str, arch: &str) -> Result<()> {
    let packages = storage.list_packages(repo, arch).await?;

    let repo_dir = storage.repo_dir(repo, arch);

    // Load pkginfo for each package
    let mut pkg_data = Vec::new();
    for pkg in &packages {
        let pkg_path = storage.package_path(repo, arch, &pkg.filename);
        let data = tokio::fs::read(&pkg_path).await?;
        let pkginfo = extract_pkginfo(&data)?;
        pkg_data.push((pkg.clone(), pkginfo));
    }

    // Generate databases
    generate_repo_db(&repo_dir, repo, &pkg_data).await?;
    generate_files_db(&repo_dir, repo, &pkg_data).await?;

    Ok(())
}

#[derive(OpenApi)]
#[openapi(
    components(
        schemas(Package, PackageQuery, PackageUploadRequest)
    ),
    tags(
        (name = "packages", description = "Package management endpoints")
    )
)]
pub struct ApiDoc;

/// Create the API router with all routes
pub fn create_api_router(state: Arc<AppState>) -> OpenApiRouter {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(upload_package))
        .routes(routes!(list_packages))
        .routes(routes!(delete_package))
        .with_state(state)
}
