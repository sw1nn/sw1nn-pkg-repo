use crate::AppState;
use crate::error::Result;
use crate::models::Package;
use axum::{Json, extract::State, response::IntoResponse};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
pub struct CleanupPolicyRequest {
    /// Package name pattern (glob-style). Use "*" for all packages.
    /// Examples: "*", "linux-*", "sw1nn-*"
    #[serde(default = "default_pattern")]
    pub package_pattern: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
}

fn default_pattern() -> String {
    "*".to_string()
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CleanupPolicyResponse {
    /// Total number of packages processed
    pub packages_processed: usize,
    /// Total number of versions deleted
    pub versions_deleted: usize,
    /// Details per package
    pub details: Vec<PackageCleanupDetail>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PackageCleanupDetail {
    pub package_name: String,
    pub versions_deleted: usize,
    pub deleted_versions: Vec<String>,
}

/// Apply cleanup policy to packages matching pattern
#[utoipa::path(
    post,
    path = "/packages/cleanup",
    request_body = CleanupPolicyRequest,
    responses(
        (status = 200, description = "Cleanup policy applied successfully", body = CleanupPolicyResponse),
        (status = 500, description = "Internal server error")
    ),
    tag = "packages"
)]
pub async fn apply_cleanup_policy(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CleanupPolicyRequest>,
) -> Result<impl IntoResponse> {
    let repo = request
        .repo
        .unwrap_or_else(|| state.config.storage.default_repo.clone());
    let arch = request
        .arch
        .unwrap_or_else(|| state.config.storage.default_arch.clone());

    // Get all packages for this repo/arch (includes "any" packages)
    let all_packages = state.storage.list_packages_for_arch(&repo, &arch).await?;

    // Group packages by name
    let mut packages_by_name: HashMap<String, Vec<Package>> = HashMap::new();
    for pkg in all_packages {
        packages_by_name
            .entry(pkg.name.clone())
            .or_default()
            .push(pkg);
    }

    // Filter package names by pattern
    let pattern = glob::Pattern::new(&request.package_pattern).map_err(|e| {
        crate::error::Error::InvalidPackage {
            pkgname: format!("Invalid pattern: {}", e),
        }
    })?;

    let matching_packages: Vec<String> = packages_by_name
        .keys()
        .filter(|name| pattern.matches(name))
        .cloned()
        .collect();

    tracing::info!(
        pattern = %request.package_pattern,
        repo = %repo,
        arch = %arch,
        matching_count = matching_packages.len(),
        "Applying cleanup policy to packages"
    );

    // Apply cleanup to each matching package
    let mut details = Vec::new();
    let mut total_deleted = 0;

    for package_name in matching_packages {
        let deleted =
            crate::storage::cleanup_old_versions(&state.storage, &package_name, &repo, &arch)
                .await?;

        if !deleted.is_empty() {
            let deleted_versions: Vec<String> = deleted.iter().map(|p| p.version.clone()).collect();
            let count = deleted.len();

            tracing::info!(
                package = %package_name,
                repo = %repo,
                arch = %arch,
                deleted_count = count,
                "Applied cleanup policy to package"
            );

            details.push(PackageCleanupDetail {
                package_name: package_name.clone(),
                versions_deleted: count,
                deleted_versions,
            });

            total_deleted += count;
        }
    }

    // Request database update (debounced, coalesced with other updates)
    if total_deleted > 0 {
        state.db_update.request_update(&repo, &arch).await;
    }

    let response = CleanupPolicyResponse {
        packages_processed: details.len(),
        versions_deleted: total_deleted,
        details,
    };

    tracing::info!(
        pattern = %request.package_pattern,
        repo = %repo,
        arch = %arch,
        packages_processed = response.packages_processed,
        versions_deleted = response.versions_deleted,
        "Cleanup policy completed"
    );

    Ok(Json(response))
}
