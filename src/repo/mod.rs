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
    let repo_dir = state.storage.repo_dir(&repo, &arch)?;

    // Check if it's a database file (in repo root) or package file
    let file_path = if filename.ends_with(".db")
        || filename.ends_with(".files")
        || filename.ends_with(".db.tar.gz")
        || filename.ends_with(".files.tar.gz")
    {
        // Database files are in the repo directory root
        repo_dir.join(&filename)
    } else {
        // Package files
        state.storage.package_path(&repo, &arch, &filename)?
    };

    if !file_path.exists() {
        return Ok((StatusCode::NOT_FOUND, "File not found").into_response());
    }

    let data = tokio::fs::read(&file_path).await.map_io_err(&file_path)?;

    // Determine content type based on extension
    let content_type = if filename.ends_with(".pkg.tar.zst") {
        "application/zstd"
    } else if filename.ends_with(".tar.gz") {
        "application/gzip"
    } else if filename.ends_with(".db") || filename.ends_with(".files") {
        "application/gzip" // These are usually symlinks to .tar.gz
    } else {
        "application/octet-stream"
    };

    Ok((StatusCode::OK, [(header::CONTENT_TYPE, content_type)], data).into_response())
}
