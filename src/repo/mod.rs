use axum::{
    extract::{Path, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use std::sync::Arc;
use tower::util::ServiceExt;
use tower_http::services::ServeFile;

use crate::api::AppState;
use crate::error::Result;

/// Serve repository files (packages or database files)
/// This handles both .pkg.tar.zst files and .db/.files database files
pub async fn serve_file(
    State(state): State<Arc<AppState>>,
    Path((repo, arch, filename)): Path<(String, String, String)>,
    request: Request,
) -> Result<Response> {
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

    // Record download metric for package files
    if filename.ends_with(".pkg.tar.zst") && !filename.ends_with(".sig") {
        crate::metrics::record_package_download(&repo, &arch);
    }

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

    // Delegate the actual byte serving to tower-http's `ServeFile`. It honours
    // `Range` requests (responding `206 Partial Content` / `416 Range Not
    // Satisfiable`), advertises `Accept-Ranges: bytes`, supports conditional
    // requests, and streams the file rather than buffering it into memory. This
    // is what lets pacman resume an interrupted package download.
    let mut response = ServeFile::new(&file_path)
        .oneshot(request)
        .await
        .expect("ServeFile responder is infallible")
        .into_response();

    // `ServeFile` guesses the content type from the file extension; override it
    // with the repository's canonical types.
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static(content_type),
    );

    Ok(response)
}
