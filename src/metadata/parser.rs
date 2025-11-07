use crate::error::{Error, Result};
use crate::models::PkgInfo;
use std::io::Read;
use tar::Archive;
use zstd::stream::read::Decoder;

/// Extract .PKGINFO from a .pkg.tar.zst file
pub fn extract_pkginfo(package_data: &[u8]) -> Result<PkgInfo> {
    // Decompress zstd
    let decoder = Decoder::new(package_data)?;

    // Read tar archive
    let mut archive = Archive::new(decoder);

    // Find and read .PKGINFO file
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;

        if path.to_str() == Some(".PKGINFO") {
            let mut content = String::new();
            entry.read_to_string(&mut content)?;

            return PkgInfo::parse(&content).map_err(|e| Error::InvalidPackage {
                pkgname: format!("Failed to parse .PKGINFO: {}", e),
            });
        }
    }

    Err(Error::InvalidPackage {
        pkgname: ".PKGINFO not found in package".to_string(),
    })
}

/// Calculate MD5 checksum
pub fn calculate_md5(data: &[u8]) -> String {
    let digest = md5::compute(data);
    format!("{:x}", digest)
}

/// Calculate SHA256 checksum
pub fn calculate_sha256(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}
