use axum::{
    extract::{Path, State},
    http::{StatusCode, header},
    response::IntoResponse,
};
use std::sync::Arc;

use crate::api::AppState;
use crate::error::{Result, ResultIoExt};

/// Serve repository files (packages or database files)
/// This handles both .pkg.tar.zst files and .db/.files database files
pub async fn serve_file(
    State(state): State<Arc<AppState>>,
    Path((repo, arch, filename)): Path<(String, String, String)>,
) -> Result<impl IntoResponse> {
    // Check if it's a database file or package file
    let file_path = if filename.ends_with(".db")
        || filename.ends_with(".files")
        || filename.ends_with(".db.tar.gz")
        || filename.ends_with(".files.tar.gz")
    {
        // Database files are in {repo}/os/{arch}/ for URL compatibility
        let db_dir = state.storage.db_dir(&repo, &arch)?;
        db_dir.join(&filename)
    } else if filename.ends_with(".pkg.tar.zst") || filename.ends_with(".pkg.tar.zst.sig") {
        // Package files are in flat storage
        // Verify the package exists and arch matches (or is "any")
        let pkg_filename = filename.trim_end_matches(".sig");

        // Check if package metadata exists and arch matches
        let metadata_name = pkg_filename.trim_end_matches(".pkg.tar.zst");
        match state.storage.load_package(&repo, metadata_name).await {
            Ok(package) => {
                // Verify arch matches or package is "any"
                if package.arch != arch && package.arch != "any" {
                    return Ok((StatusCode::NOT_FOUND, "File not found").into_response());
                }
                // Return path to actual file
                state.storage.package_path(&repo, &filename)?
            }
            Err(_) => {
                // Package metadata not found, try direct file access for .sig files
                // or return not found
                state.storage.package_path(&repo, &filename)?
            }
        }
    } else {
        // Unknown file type
        return Ok((StatusCode::NOT_FOUND, "File not found").into_response());
    };

    if !file_path.exists() {
        return Ok((StatusCode::NOT_FOUND, "File not found").into_response());
    }

    let data = tokio::fs::read(&file_path).await.map_io_err(&file_path)?;

    // Determine content type based on extension
    let content_type = if filename.ends_with(".pkg.tar.zst") {
        "application/zstd"
    } else if filename.ends_with(".tar.gz")
        || filename.ends_with(".db")
        || filename.ends_with(".files")
    {
        "application/gzip"
    } else if filename.ends_with(".sig") {
        "application/pgp-signature"
    } else {
        "application/octet-stream"
    };

    Ok((StatusCode::OK, [(header::CONTENT_TYPE, content_type)], data).into_response())
}
