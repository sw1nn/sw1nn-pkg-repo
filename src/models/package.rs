use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Package {
    /// Package name
    pub name: String,
    /// Package version
    pub version: String,
    /// Architecture (e.g., x86_64, any)
    pub arch: String,
    /// Repository name
    pub repo: String,
    /// Package filename
    pub filename: String,
    /// SHA256 checksum
    pub sha256: String,
    /// Package file size in bytes
    pub size: u64,
    /// Package creation timestamp
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PackageInfo {
    /// Package name
    pub name: String,
    /// Package version
    pub version: String,
    /// Architecture (e.g., x86_64, any)
    pub arch: String,
    /// Repository name
    pub repo: String,
    /// Package filename
    pub filename: String,
    /// Package file size in bytes
    pub size: u64,
    /// SHA256 checksum
    pub sha256: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PackageQuery {
    /// Filter by package name
    pub name: Option<String>,
    /// Filter by repository
    pub repo: Option<String>,
    /// Filter by architecture
    pub arch: Option<String>,
}
