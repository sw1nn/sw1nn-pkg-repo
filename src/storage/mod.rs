use crate::error::{Error, Result};
use crate::models::Package;
use std::path::PathBuf;
use tokio::fs;
use tokio::io::AsyncWriteExt;

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
    pub fn package_path(&self, repo: &str, arch: &str, filename: &str) -> PathBuf {
        self.base_path
            .join(repo)
            .join("os")
            .join(arch)
            .join(filename)
    }

    /// Get the path for package metadata
    pub fn metadata_path(&self, repo: &str, arch: &str, package_name: &str) -> PathBuf {
        self.base_path
            .join(repo)
            .join("os")
            .join(arch)
            .join("metadata")
            .join(format!("{}.json", package_name))
    }

    /// Get the directory path for a repo/arch combination
    pub fn repo_dir(&self, repo: &str, arch: &str) -> PathBuf {
        self.base_path.join(repo).join("os").join(arch)
    }

    /// Store a package file and its metadata
    pub async fn store_package(&self, package: &Package, data: &[u8]) -> Result<()> {
        let pkg_path = self.package_path(&package.repo, &package.arch, &package.filename);
        let meta_path = self.metadata_path(&package.repo, &package.arch, &package.name);

        // Create directories
        if let Some(parent) = pkg_path.parent() {
            fs::create_dir_all(parent).await?;
        }
        if let Some(parent) = meta_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        // Write package file
        let mut file = fs::File::create(&pkg_path).await?;
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
        let meta_path = self.metadata_path(repo, arch, package_name);

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
        let meta_dir = self.repo_dir(repo, arch).join("metadata");

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
        let pkg_path = self.package_path(&package.repo, &package.arch, &package.filename);
        let meta_path = self.metadata_path(&package.repo, &package.arch, &package.name);

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
    pub async fn package_exists(&self, repo: &str, arch: &str, filename: &str) -> bool {
        self.package_path(repo, arch, filename).exists()
    }
}
