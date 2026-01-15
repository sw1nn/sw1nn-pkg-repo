use crate::AppState;
use crate::api::regenerate_repo_db;
use crate::error::{Error, Result};
use crate::models::Package;
use axum::{
    Json,
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use std::sync::Arc;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
pub struct DeleteVersionsRequest {
    /// List of version specifications - can be exact versions (e.g., "1.5.3-1")
    /// or semver ranges (e.g., "^1.0.0", ">=1.0.0, <2.0.0")
    pub versions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
}

/// Parse Arch Linux version (epoch:pkgver-pkgrel) to semver
fn parse_arch_version_to_semver(version_str: &str) -> Result<semver::Version> {
    // Remove epoch if present
    let without_epoch = if let Some(colon_pos) = version_str.find(':') {
        &version_str[colon_pos + 1..]
    } else {
        version_str
    };

    // Remove pkgrel (everything after last '-')
    let last_dash = without_epoch
        .rfind('-')
        .ok_or_else(|| Error::InvalidPackage {
            pkgname: format!("Invalid version format: {}", version_str),
        })?;
    let pkgver = &without_epoch[..last_dash];

    // Parse as semver
    semver::Version::parse(pkgver).map_err(|_| Error::InvalidPackage {
        pkgname: format!("Invalid semver format: {}", pkgver),
    })
}

/// Check if a version matches a semver range requirement
fn version_matches_range(version_str: &str, range: &semver::VersionReq) -> Result<bool> {
    let version = parse_arch_version_to_semver(version_str)?;
    Ok(range.matches(&version))
}

/// Delete package versions
#[utoipa::path(
    delete,
    path = "/packages/{name}/versions",
    request_body = DeleteVersionsRequest,
    params(
        ("name" = String, Path, description = "Package name")
    ),
    responses(
        (status = 204, description = "Versions deleted successfully"),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Package or version not found"),
        (status = 500, description = "Internal server error")
    ),
    tag = "packages"
)]
pub async fn delete_versions(
    State(state): State<Arc<AppState>>,
    AxumPath(name): AxumPath<String>,
    Json(request): Json<DeleteVersionsRequest>,
) -> Result<impl IntoResponse> {
    // Extract repo/arch with defaults
    let repo = request
        .repo
        .unwrap_or_else(|| state.config.storage.default_repo.clone());
    let arch = request
        .arch
        .unwrap_or_else(|| state.config.storage.default_arch.clone());

    // Get all packages for this name/repo/arch
    let all_packages = state.storage.list_packages(&repo, &arch).await?;
    let packages: Vec<Package> = all_packages
        .into_iter()
        .filter(|p| p.name == name)
        .collect();

    if packages.is_empty() {
        return Err(Error::PackageNotFound {
            pkgname: name.clone(),
        });
    }

    // Determine which versions to delete by processing each version spec
    // Each spec can be either an exact version match or a semver range
    use std::collections::HashSet;
    let mut to_delete_set: HashSet<String> = HashSet::new();

    for version_spec in &request.versions {
        // Try parsing as semver range first
        if let Ok(version_req) = semver::VersionReq::parse(version_spec) {
            // It's a semver range - match all packages against it
            for pkg in &packages {
                if version_matches_range(&pkg.version, &version_req).unwrap_or(false) {
                    to_delete_set.insert(pkg.version.clone());
                }
            }
        } else {
            // Not a valid semver range - treat as exact version match
            to_delete_set.insert(version_spec.clone());
        }
    }

    // Filter packages to only those in our to_delete set
    let to_delete: Vec<Package> = packages
        .into_iter()
        .filter(|p| to_delete_set.contains(&p.version))
        .collect();

    // Check if any versions matched
    if to_delete.is_empty() {
        return Err(Error::PackageNotFound {
            pkgname: format!("No matching versions found for package: {}", name),
        });
    }

    // Delete all matched packages
    for package in &to_delete {
        state.storage.delete_package(package).await?;
        tracing::info!(
            package = %package.name,
            version = %package.version,
            repo = %package.repo,
            arch = %package.arch,
            "Deleted package version"
        );
    }

    // Regenerate repository database
    regenerate_repo_db(&state.storage, &repo, &arch).await?;

    tracing::info!(
        package = %name,
        repo = %repo,
        arch = %arch,
        deleted_count = to_delete.len(),
        "Deleted package versions"
    );

    Ok(StatusCode::NO_CONTENT)
}
