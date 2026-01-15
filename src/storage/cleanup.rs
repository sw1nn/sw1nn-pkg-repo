use crate::error::Result;
use crate::models::Package;
use crate::storage::Storage;
use std::collections::HashMap;

/// Parse semantic version and pkgrel from Arch Linux package version string
///
/// Format: `[epoch:]major.minor.patch-pkgrel`
/// Returns: `Some((major, minor, patch, pkgrel))` or `None` if invalid
///
/// Examples:
/// - "1.5.3-1" → Some((1, 5, 3, 1))
/// - "2:1.5.3-2" → Some((1, 5, 3, 2))
/// - "1.5.3-12" → Some((1, 5, 3, 12))
fn parse_semver_from_pkgver(version_str: &str) -> Option<(u64, u64, u64, u64)> {
    // Remove epoch if present (strip "N:" prefix)
    let without_epoch = if let Some(colon_pos) = version_str.find(':') {
        &version_str[colon_pos + 1..]
    } else {
        version_str
    };

    // Split on last '-' to separate pkgver and pkgrel
    let last_dash = without_epoch.rfind('-')?;
    let pkgver = &without_epoch[..last_dash];
    let pkgrel = &without_epoch[last_dash + 1..];

    // Parse pkgrel as u64
    let pkgrel_num = pkgrel.parse::<u64>().ok()?;

    // Parse pkgver as semver
    let semver = semver::Version::parse(pkgver).ok()?;

    Some((semver.major, semver.minor, semver.patch, pkgrel_num))
}

/// Clean up old package versions, keeping only:
/// 1. Current version (newest overall)
/// 2. Latest of same minor version (excluding current)
/// 3. Latest of previous minor version
///
/// For packages with same pkgver but different pkgrel (e.g., 1.5.3-1, 1.5.3-2),
/// only the newest pkgrel is kept.
///
/// Returns list of deleted packages.
pub async fn cleanup_old_versions(
    storage: &Storage,
    package_name: &str,
    repo: &str,
    arch: &str,
) -> Result<Vec<Package>> {
    // List all packages for this repo/arch
    let all_packages = storage.list_packages(repo, arch).await?;

    // Filter by package name
    let packages: Vec<Package> = all_packages
        .into_iter()
        .filter(|p| p.name == package_name)
        .collect();

    // If 0 or 1 package, nothing to clean up
    if packages.len() <= 1 {
        return Ok(Vec::new());
    }

    // Parse versions and filter out non-semver packages
    let mut packages_with_versions: Vec<(Package, u64, u64, u64, u64)> = Vec::new();

    for package in packages.iter() {
        if let Some((major, minor, patch, pkgrel)) = parse_semver_from_pkgver(&package.version) {
            packages_with_versions.push((package.clone(), major, minor, patch, pkgrel));
        } else {
            tracing::warn!(
                package = %package.name,
                version = %package.version,
                "Skipping package with non-semver version from cleanup"
            );
        }
    }

    // If no parseable versions, nothing to clean up
    if packages_with_versions.is_empty() {
        return Ok(Vec::new());
    }

    // Deduplicate by pkgver: group by (major, minor, patch), keep only newest pkgrel
    let mut pkgver_map: HashMap<(u64, u64, u64), (Package, u64, u64, u64, u64)> = HashMap::new();

    for (pkg, major, minor, patch, pkgrel) in packages_with_versions {
        let key = (major, minor, patch);
        // Check if we already have this pkgver
        if let Some((_, _, _, _, existing_pkgrel)) = pkgver_map.get(&key) {
            // Only insert if the new pkgrel is higher
            if pkgrel > *existing_pkgrel {
                pkgver_map.insert(key, (pkg, major, minor, patch, pkgrel));
            }
        } else {
            // First time seeing this pkgver
            pkgver_map.insert(key, (pkg, major, minor, patch, pkgrel));
        }
    }

    // Convert back to vector
    let mut deduplicated: Vec<(Package, u64, u64, u64, u64)> = pkgver_map.into_values().collect();

    // If only 1 version after deduplication, nothing to clean up
    if deduplicated.len() <= 1 {
        // But we might need to delete old pkgrels
        let kept_versions: Vec<String> = deduplicated
            .iter()
            .map(|(p, _, _, _, _)| p.version.clone())
            .collect();

        let mut to_delete = Vec::new();
        for package in packages {
            if !kept_versions.contains(&package.version) {
                to_delete.push(package);
            }
        }

        for package in &to_delete {
            storage.delete_package(package).await.inspect_err(|e| {
                tracing::error!(
                    package = %package.name,
                    version = %package.version,
                    error = %e,
                    "Failed to delete old pkgrel during cleanup"
                );
            })?;
        }

        return Ok(to_delete);
    }

    // Sort by version descending (newest first)
    // Compare tuples (major, minor, patch, pkgrel) in reverse order
    deduplicated.sort_by(
        |(_, maj_a, min_a, pat_a, rel_a), (_, maj_b, min_b, pat_b, rel_b)| {
            (maj_b, min_b, pat_b, rel_b).cmp(&(maj_a, min_a, pat_a, rel_a))
        },
    );

    // Identify versions to keep
    let (current_pkg, current_major, current_minor, _current_patch, _current_pkgrel) =
        &deduplicated[0];

    let mut versions_to_keep = vec![current_pkg.clone()];

    // Find latest of same minor (excluding current)
    let same_minor: Vec<&(Package, u64, u64, u64, u64)> = deduplicated
        .iter()
        .skip(1) // Skip current
        .filter(|(_, major, minor, _, _)| major == current_major && minor == current_minor)
        .collect();

    if let Some((pkg, _, _, _, _)) = same_minor.first() {
        versions_to_keep.push((*pkg).clone());
    }

    // Find latest of previous minor
    if *current_minor > 0 {
        let previous_minor = current_minor - 1;
        let prev_minor: Vec<&(Package, u64, u64, u64, u64)> = deduplicated
            .iter()
            .filter(|(_, major, minor, _, _)| major == current_major && minor == &previous_minor)
            .collect();

        if let Some((pkg, _, _, _, _)) = prev_minor.first() {
            versions_to_keep.push((*pkg).clone());
        }
    }

    // Identify packages to delete (all packages not in versions_to_keep)
    let kept_versions: Vec<String> = versions_to_keep.iter().map(|p| p.version.clone()).collect();
    let mut to_delete = Vec::new();

    for package in packages {
        if !kept_versions.contains(&package.version) {
            to_delete.push(package);
        }
    }

    // Delete packages
    for package in &to_delete {
        storage.delete_package(package).await.inspect_err(|e| {
            tracing::error!(
                package = %package.name,
                version = %package.version,
                error = %e,
                "Failed to delete package during cleanup"
            );
        })?;
    }

    Ok(to_delete)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_semver_basic() -> Result<()> {
        assert_eq!(parse_semver_from_pkgver("1.5.3-1"), Some((1, 5, 3, 1)));
        assert_eq!(parse_semver_from_pkgver("2.0.0-1"), Some((2, 0, 0, 1)));
        assert_eq!(parse_semver_from_pkgver("0.1.0-1"), Some((0, 1, 0, 1)));
        Ok(())
    }

    #[test]
    fn test_parse_semver_with_epoch() -> Result<()> {
        assert_eq!(parse_semver_from_pkgver("2:1.5.3-1"), Some((1, 5, 3, 1)));
        assert_eq!(parse_semver_from_pkgver("1:2.0.0-3"), Some((2, 0, 0, 3)));
        Ok(())
    }

    #[test]
    fn test_parse_semver_various_pkgrel() -> Result<()> {
        assert_eq!(parse_semver_from_pkgver("1.5.3-1"), Some((1, 5, 3, 1)));
        assert_eq!(parse_semver_from_pkgver("1.5.3-2"), Some((1, 5, 3, 2)));
        assert_eq!(parse_semver_from_pkgver("1.5.3-12"), Some((1, 5, 3, 12)));
        Ok(())
    }

    #[test]
    fn test_parse_semver_invalid() -> Result<()> {
        assert_eq!(parse_semver_from_pkgver("invalid"), None);
        assert_eq!(parse_semver_from_pkgver("1.5"), None); // Missing pkgrel
        assert_eq!(parse_semver_from_pkgver("20250115"), None); // Date format
        Ok(())
    }
}
