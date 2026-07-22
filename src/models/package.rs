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
    /// Detached signature details, present iff the package is signed.
    /// Populated when listing; never persisted to package metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<SignatureInfo>,
}

/// Details of a package's detached OpenPGP signature.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SignatureInfo {
    /// Issuer key ID (or fingerprint) taken from the signature packet
    pub key_id: String,
    /// Signer user-id, when the issuer key is in the trust keyring
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signer: Option<String>,
    /// Signature creation time, when present in the signature packet
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    /// Verification outcome against the trust keyring:
    /// `Some(true)` = good, `Some(false)` = bad/untrusted,
    /// `None` = not checked (no keyring configured)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid: Option<bool>,
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
