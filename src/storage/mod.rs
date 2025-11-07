use crate::error::{Error, Result};
use crate::models::Package;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;

/// Validate a path component to prevent directory traversal attacks
fn validate_path_component(component: &str) -> Result<()> {
    // Reject empty, ".", "..", or components containing path separators
    if component.is_empty() {
        return Err(Error::InvalidPackage {
            pkgname: "Path component cannot be empty".to_string(),
        });
    }

    if component == "." || component == ".." {
        return Err(Error::InvalidPackage {
            pkgname: format!("Invalid path component: '{}'", component),
        });
    }

    if component.contains('/') || component.contains('\\') {
        return Err(Error::InvalidPackage {
            pkgname: "Path component cannot contain path separators".to_string(),
        });
    }

    if component.contains('\0') {
        return Err(Error::InvalidPackage {
            pkgname: "Path component cannot contain null bytes".to_string(),
        });
    }

    Ok(())
}

/// Validate that a constructed path is within the base directory
fn validate_path_within_base(base: &Path, path: &Path) -> Result<()> {
    // Canonicalize both paths to resolve symlinks and relative components
    let canonical_base = base
        .canonicalize()
        .map_err(|e| Error::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Base path does not exist: {}", e),
        )))?;

    // For the constructed path, we need to check if it would be within base
    // even if it doesn't exist yet. We check the parent if the path doesn't exist.
    let path_to_check = if path.exists() {
        path.canonicalize()?
    } else if let Some(parent) = path.parent() {
        if parent.exists() {
            parent.canonicalize()?.join(
                path.file_name()
                    .ok_or_else(|| Error::InvalidPackage {
                        pkgname: "Invalid path structure".to_string(),
                    })?,
            )
        } else {
            // Parent doesn't exist yet, just verify the logical structure
            return Ok(());
        }
    } else {
        return Err(Error::InvalidPackage {
            pkgname: "Invalid path structure".to_string(),
        });
    };

    if !path_to_check.starts_with(&canonical_base) {
        return Err(Error::InvalidPackage {
            pkgname: "Path traversal detected".to_string(),
        });
    }

    Ok(())
}

/// Storage layer for managing package files and metadata
/// Structure: data/{repo}/os/{arch}/{package-file}
///            data/{repo}/os/{arch}/metadata/{package-name}.json
pub struct Storage {
    base_path: PathBuf,
}

impl Storage {
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        Self {
            base_path: base_path.into(),
        }
    }

    /// Get the path for a package file
    pub fn package_path(&self, repo: &str, arch: &str, filename: &str) -> Result<PathBuf> {
        validate_path_component(repo)?;
        validate_path_component(arch)?;
        validate_path_component(filename)?;

        let path = self.base_path
            .join(repo)
            .join("os")
            .join(arch)
            .join(filename);

        validate_path_within_base(&self.base_path, &path)?;

        Ok(path)
    }

    /// Get the path for package metadata
    pub fn metadata_path(&self, repo: &str, arch: &str, package_name: &str) -> Result<PathBuf> {
        validate_path_component(repo)?;
        validate_path_component(arch)?;
        validate_path_component(package_name)?;

        let path = self.base_path
            .join(repo)
            .join("os")
            .join(arch)
            .join("metadata")
            .join(format!("{}.json", package_name));

        validate_path_within_base(&self.base_path, &path)?;

        Ok(path)
    }

    /// Get the directory path for a repo/arch combination
    pub fn repo_dir(&self, repo: &str, arch: &str) -> Result<PathBuf> {
        validate_path_component(repo)?;
        validate_path_component(arch)?;

        let path = self.base_path.join(repo).join("os").join(arch);

        validate_path_within_base(&self.base_path, &path)?;

        Ok(path)
    }

    /// Store a package file and its metadata
    ///
    /// Uses atomic file creation to prevent TOCTOU race conditions.
    /// If the package already exists, returns PackageAlreadyExists error.
    pub async fn store_package(&self, package: &Package, data: &[u8]) -> Result<()> {
        let pkg_path = self.package_path(&package.repo, &package.arch, &package.filename)?;
        let meta_path = self.metadata_path(&package.repo, &package.arch, &package.name)?;

        // Create directories
        if let Some(parent) = pkg_path.parent() {
            fs::create_dir_all(parent).await?;
        }
        if let Some(parent) = meta_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        // Atomic write with exclusive creation flag (prevents TOCTOU races)
        use tokio::fs::OpenOptions;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true) // Fails atomically if file exists
            .open(&pkg_path)
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    Error::PackageAlreadyExists {
                        pkgname: package.filename.clone(),
                    }
                } else {
                    Error::Io(e)
                }
            })?;

        file.write_all(data).await?;
        file.sync_all().await?;

        // Write metadata
        let metadata_json = serde_json::to_string_pretty(package)
            .map_err(|e| Error::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
        fs::write(&meta_path, metadata_json).await?;

        Ok(())
    }

    /// Load package metadata
    pub async fn load_package(
        &self,
        repo: &str,
        arch: &str,
        package_name: &str,
    ) -> Result<Package> {
        let meta_path = self.metadata_path(repo, arch, package_name)?;

        if !meta_path.exists() {
            return Err(Error::PackageNotFound {
                pkgname: package_name.to_string(),
            });
        }

        let content = fs::read_to_string(&meta_path).await?;
        let package: Package = serde_json::from_str(&content)
            .map_err(|e| Error::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;

        Ok(package)
    }

    /// List all packages in a repo/arch
    pub async fn list_packages(&self, repo: &str, arch: &str) -> Result<Vec<Package>> {
        let meta_dir = self.repo_dir(repo, arch)?.join("metadata");

        if !meta_dir.exists() {
            return Ok(Vec::new());
        }

        let mut packages = Vec::new();
        let mut entries = fs::read_dir(&meta_dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                let content = fs::read_to_string(&path).await?;
                if let Ok(package) = serde_json::from_str::<Package>(&content) {
                    packages.push(package);
                }
            }
        }

        Ok(packages)
    }

    /// Delete a package and its metadata
    pub async fn delete_package(&self, package: &Package) -> Result<()> {
        let pkg_path = self.package_path(&package.repo, &package.arch, &package.filename)?;
        let meta_path = self.metadata_path(&package.repo, &package.arch, &package.name)?;

        // Delete package file
        if pkg_path.exists() {
            fs::remove_file(&pkg_path).await?;
        }

        // Delete metadata
        if meta_path.exists() {
            fs::remove_file(&meta_path).await?;
        }

        Ok(())
    }

    /// Check if a package file exists
    pub async fn package_exists(&self, repo: &str, arch: &str, filename: &str) -> Result<bool> {
        Ok(self.package_path(repo, arch, filename)?.exists())
    }
}
