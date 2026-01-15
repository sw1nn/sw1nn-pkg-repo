use crate::api::{AppState, regenerate_repo_db};
use crate::error::{Error, Result, ResultIoExt};
use crate::metadata::{calculate_sha256, extract_pkginfo};
use crate::models::Package;
use crate::upload::{DEFAULT_CHUNK_SIZE, DEFAULT_SESSION_EXPIRATION_SECS, UploadSession};
use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

/// Request body for initiating a chunked upload
#[derive(Debug, Deserialize, ToSchema)]
pub struct InitiateUploadRequest {
    /// Package filename (e.g., "package-1.0.0-x86_64.pkg.tar.zst")
    pub filename: String,
    /// Total file size in bytes
    pub size: u64,
    /// Pre-calculated SHA256 hash (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Repository name (optional, defaults from config)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Architecture (optional, defaults from config)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    /// Chunk size in bytes (optional, defaults to 1 MiB)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_size: Option<usize>,
    /// Whether a signature file will be uploaded
    #[serde(default)]
    pub has_signature: bool,
}

/// Response from initiating a chunked upload
#[derive(Debug, Serialize, ToSchema)]
pub struct InitiateUploadResponse {
    /// Unique upload session ID
    pub upload_id: String,
    /// Session expiration timestamp
    pub expires_at: String,
    /// Chunk size in bytes
    pub chunk_size: usize,
    /// Total number of chunks
    pub total_chunks: u32,
}

/// Response from uploading a chunk
#[derive(Debug, Serialize, ToSchema)]
pub struct UploadChunkResponse {
    /// Chunk number
    pub chunk_number: u32,
    /// MD5 checksum of the chunk
    pub checksum: String,
    /// Size of the received chunk
    pub received_size: usize,
}

/// Response from uploading a signature
#[derive(Debug, Serialize, ToSchema)]
pub struct UploadSignatureResponse {
    /// Size of the signature file
    pub signature_size: usize,
    /// SHA256 checksum of the signature
    pub checksum: String,
}

/// Request body for completing an upload
#[derive(Debug, Deserialize, ToSchema)]
pub struct CompleteUploadRequest {
    /// List of chunks with their checksums
    pub chunks: Vec<ChunkInfo>,
}

/// Chunk information for verification
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ChunkInfo {
    /// Chunk number (1-indexed)
    pub chunk_number: u32,
    /// MD5 checksum of the chunk
    pub checksum: String,
}

/// Response from aborting an upload
#[derive(Debug, Serialize, ToSchema)]
pub struct AbortUploadResponse {
    /// Upload session ID
    pub upload_id: String,
    /// Number of chunks deleted
    pub deleted_chunks: u32,
    /// Bytes freed
    pub bytes_freed: u64,
}

/// Initiate a chunked upload session
#[utoipa::path(
    post,
    path = "/packages/upload/initiate",
    request_body = InitiateUploadRequest,
    responses(
        (status = 201, description = "Upload session created", body = InitiateUploadResponse),
        (status = 400, description = "Invalid request"),
        (status = 500, description = "Internal server error")
    ),
    tag = "chunked-uploads"
)]
pub async fn initiate_upload(
    State(state): State<Arc<AppState>>,
    Json(req): Json<InitiateUploadRequest>,
) -> Result<impl IntoResponse> {
    // Validate filename
    if !req.filename.ends_with(".pkg.tar.zst") {
        return Err(Error::InvalidPackage {
            pkgname: format!(
                "Invalid file extension: '{}'. Only .pkg.tar.zst packages are allowed",
                req.filename
            ),
        });
    }

    // Validate size
    let max_size = state.config.server.max_payload_size.as_u64();
    if req.size > max_size {
        return Err(Error::PayloadTooLarge {
            msg: format!(
                "File size {} exceeds maximum allowed size of {}",
                byte_unit::Byte::from_u64(req.size),
                state.config.server.max_payload_size
            ),
        });
    }

    if req.size == 0 {
        return Err(Error::InvalidPackage {
            pkgname: "File size cannot be zero".to_string(),
        });
    }

    let repo = req
        .repo
        .unwrap_or_else(|| state.config.storage.default_repo.clone());
    let arch = req
        .arch
        .unwrap_or_else(|| state.config.storage.default_arch.clone());
    let chunk_size = req.chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE);

    // Validate chunk size (must be at least 1 byte, at most file size)
    if chunk_size == 0 || chunk_size as u64 > req.size {
        return Err(Error::InvalidPackage {
            pkgname: format!("Invalid chunk size: {}", chunk_size),
        });
    }

    // Create upload session
    let session = UploadSession::new(
        req.filename,
        req.size,
        req.sha256,
        repo,
        arch,
        chunk_size,
        req.has_signature,
        DEFAULT_SESSION_EXPIRATION_SECS,
    );

    let response = InitiateUploadResponse {
        upload_id: session.upload_id.clone(),
        expires_at: session.expires_at.to_rfc3339(),
        chunk_size: session.chunk_size,
        total_chunks: session.total_chunks,
    };

    state.upload_store.create_session(session).await?;

    Ok((StatusCode::CREATED, Json(response)))
}

/// Upload a single chunk
#[utoipa::path(
    post,
    path = "/packages/upload/{upload_id}/chunks/{chunk_number}",
    params(
        ("upload_id" = String, Path, description = "Upload session ID"),
        ("chunk_number" = u32, Path, description = "Chunk number (1-indexed)")
    ),
    request_body(content = Vec<u8>, content_type = "application/octet-stream"),
    responses(
        (status = 200, description = "Chunk uploaded successfully", body = UploadChunkResponse),
        (status = 400, description = "Invalid chunk"),
        (status = 404, description = "Upload session not found"),
        (status = 500, description = "Internal server error")
    ),
    tag = "chunked-uploads"
)]
pub async fn upload_chunk(
    State(state): State<Arc<AppState>>,
    Path((upload_id, chunk_number)): Path<(String, u32)>,
    body: Bytes,
) -> Result<impl IntoResponse> {
    // Verify session exists and not expired
    let session = state.upload_store.get_session(&upload_id).await?;

    if session.is_expired() {
        return Err(Error::InvalidPackage {
            pkgname: format!("Upload session {} has expired", upload_id),
        });
    }

    // Store chunk
    let checksum = state
        .upload_store
        .store_chunk(&upload_id, chunk_number, &body)
        .await?;

    let response = UploadChunkResponse {
        chunk_number,
        checksum,
        received_size: body.len(),
    };

    Ok(Json(response))
}

/// Upload signature file
#[utoipa::path(
    post,
    path = "/packages/upload/{upload_id}/signature",
    params(
        ("upload_id" = String, Path, description = "Upload session ID")
    ),
    request_body(content = Vec<u8>, content_type = "application/octet-stream"),
    responses(
        (status = 200, description = "Signature uploaded successfully", body = UploadSignatureResponse),
        (status = 404, description = "Upload session not found"),
        (status = 500, description = "Internal server error")
    ),
    tag = "chunked-uploads"
)]
pub async fn upload_signature(
    State(state): State<Arc<AppState>>,
    Path(upload_id): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse> {
    // Verify session exists
    let session = state.upload_store.get_session(&upload_id).await?;

    if session.is_expired() {
        return Err(Error::InvalidPackage {
            pkgname: format!("Upload session {} has expired", upload_id),
        });
    }

    // Store signature
    let checksum = state
        .upload_store
        .store_signature(&upload_id, &body)
        .await?;

    let response = UploadSignatureResponse {
        signature_size: body.len(),
        checksum,
    };

    Ok(Json(response))
}

/// Complete a chunked upload
#[utoipa::path(
    post,
    path = "/packages/upload/{upload_id}/complete",
    params(
        ("upload_id" = String, Path, description = "Upload session ID")
    ),
    request_body = CompleteUploadRequest,
    responses(
        (status = 201, description = "Package uploaded successfully", body = Package),
        (status = 400, description = "Invalid upload or missing chunks"),
        (status = 404, description = "Upload session not found"),
        (status = 409, description = "Package already exists"),
        (status = 500, description = "Internal server error")
    ),
    tag = "chunked-uploads"
)]
pub async fn complete_upload(
    State(state): State<Arc<AppState>>,
    Path(upload_id): Path<String>,
    Json(req): Json<CompleteUploadRequest>,
) -> Result<impl IntoResponse> {
    // Get session
    let session = state.upload_store.get_session(&upload_id).await?;

    if session.is_expired() {
        return Err(Error::InvalidPackage {
            pkgname: format!("Upload session {} has expired", upload_id),
        });
    }

    // Verify all chunks are present
    if !session.is_complete() {
        return Err(Error::InvalidPackage {
            pkgname: format!(
                "Upload incomplete. Missing chunks: {:?}",
                session.missing_chunks()
            ),
        });
    }

    // Verify chunk count matches
    if req.chunks.len() != session.total_chunks as usize {
        return Err(Error::InvalidPackage {
            pkgname: format!(
                "Chunk count mismatch: expected {}, got {}",
                session.total_chunks,
                req.chunks.len()
            ),
        });
    }

    // TODO: Verify checksums match (future enhancement)

    // Assemble chunks to disk
    let assembled_path = state.upload_store.assemble_chunks(&upload_id).await?;

    // Read assembled file for processing (extract PKGINFO and calculate SHA256)
    // This is done in a blocking task to avoid blocking the async runtime
    let assembled_path_clone = assembled_path.clone();
    let (pkginfo, sha256, size) = tokio::task::spawn_blocking(move || {
        let package_data = std::fs::read(&assembled_path_clone)?;
        let pkginfo = extract_pkginfo(&package_data)?;
        let sha256 = calculate_sha256(&package_data);
        let size = package_data.len() as u64;
        Ok::<_, Error>((pkginfo, sha256, size))
    })
    .await
    .map_err(|e| std::io::Error::other(format!("Task join error: {}", e)))??;

    // Create filename
    let filename = format!(
        "{}-{}-{}.pkg.tar.zst",
        pkginfo.pkgname, pkginfo.pkgver, pkginfo.arch
    );

    // Create package record
    let package = Package {
        name: pkginfo.pkgname,
        version: pkginfo.pkgver,
        arch: pkginfo.arch,
        repo: session.repo.clone(),
        filename,
        sha256,
        size,
        created_at: Utc::now(),
    };

    // Move assembled file to permanent storage (without loading into memory)
    state
        .storage
        .store_package_from_path(&package, &assembled_path)
        .await?;

    // Store signature if present
    if session.has_signature {
        if let Some(sig_data) = state.upload_store.get_signature(&upload_id).await? {
            let sig_filename = format!("{}.sig", package.filename);
            let sig_path =
                state
                    .storage
                    .package_path(&package.repo, &package.arch, &sig_filename)?;

            tokio::fs::write(&sig_path, &sig_data)
                .await
                .map_io_err(&sig_path)?;
        } else {
            tracing::warn!(
                "Session indicated signature but none found for upload {}",
                upload_id
            );
        }
    }

    // Regenerate repository database
    regenerate_repo_db(&state.storage, &package.repo, &package.arch).await?;

    // Auto-cleanup old versions if enabled
    if state.config.storage.auto_cleanup_enabled {
        let deleted = crate::storage::cleanup_old_versions(
            &state.storage,
            &package.name,
            &package.repo,
            &package.arch,
        )
        .await
        .inspect_err(|e| {
            tracing::error!(
                package = %package.name,
                repo = %package.repo,
                arch = %package.arch,
                error = %e,
                "Failed to cleanup old package versions"
            );
        })
        .unwrap_or_default();

        if !deleted.is_empty() {
            tracing::info!(
                package = %package.name,
                repo = %package.repo,
                arch = %package.arch,
                deleted_count = deleted.len(),
                deleted_versions = ?deleted.iter().map(|p| &p.version).collect::<Vec<_>>(),
                "Cleaned up old package versions"
            );

            // Regenerate DB to reflect deletions
            if let Err(e) = regenerate_repo_db(&state.storage, &package.repo, &package.arch).await {
                tracing::error!(
                    repo = %package.repo,
                    arch = %package.arch,
                    error = %e,
                    "Failed to regenerate repository database after cleanup"
                );
            }
        }
    }

    // Cleanup upload session
    if let Err(e) = state.upload_store.delete_session(&upload_id).await {
        tracing::warn!("Failed to cleanup upload session {}: {}", upload_id, e);
    }

    Ok((StatusCode::CREATED, Json(package)))
}

/// Abort a chunked upload
#[utoipa::path(
    delete,
    path = "/packages/upload/{upload_id}",
    params(
        ("upload_id" = String, Path, description = "Upload session ID")
    ),
    responses(
        (status = 200, description = "Upload aborted successfully", body = AbortUploadResponse),
        (status = 404, description = "Upload session not found"),
        (status = 500, description = "Internal server error")
    ),
    tag = "chunked-uploads"
)]
pub async fn abort_upload(
    State(state): State<Arc<AppState>>,
    Path(upload_id): Path<String>,
) -> Result<impl IntoResponse> {
    let (deleted_chunks, bytes_freed) = state.upload_store.delete_session(&upload_id).await?;

    let response = AbortUploadResponse {
        upload_id,
        deleted_chunks,
        bytes_freed,
    };

    Ok(Json(response))
}
