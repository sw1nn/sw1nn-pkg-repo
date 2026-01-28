use crate::error::{Error, Result, ResultIoExt};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::HashSet;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use uuid::Uuid;

/// Default chunk size: 1 MiB
pub const DEFAULT_CHUNK_SIZE: usize = 1024 * 1024;

/// Default session expiration: 24 hours
pub const DEFAULT_SESSION_EXPIRATION_SECS: i64 = 86400;

/// Upload session tracking an in-progress chunked upload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadSession {
    pub upload_id: String,
    pub filename: String,
    pub file_size: u64,
    pub sha256: Option<String>,
    pub repo: String,
    pub arch: String,
    pub chunk_size: usize,
    pub total_chunks: u32,
    pub has_signature: bool,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    #[serde(skip)]
    pub uploaded_chunks: HashSet<u32>,
}

impl UploadSession {
    /// Create a new builder for UploadSession
    pub fn builder() -> UploadSessionBuilder<NoFilename, NoFileSize, NoRepo, NoArch> {
        UploadSessionBuilder::new()
    }

    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }

    pub fn is_complete(&self) -> bool {
        self.uploaded_chunks.len() == self.total_chunks as usize
            && (1..=self.total_chunks).all(|n| self.uploaded_chunks.contains(&n))
    }

    pub fn missing_chunks(&self) -> Vec<u32> {
        (1..=self.total_chunks)
            .filter(|n| !self.uploaded_chunks.contains(n))
            .collect()
    }
}

// Typestate marker types for required fields
#[derive(Debug, Default)]
pub struct NoFilename;
#[derive(Debug, Default)]
pub struct HasFilename;

#[derive(Debug, Default)]
pub struct NoFileSize;
#[derive(Debug, Default)]
pub struct HasFileSize;

#[derive(Debug, Default)]
pub struct NoRepo;
#[derive(Debug, Default)]
pub struct HasRepo;

#[derive(Debug, Default)]
pub struct NoArch;
#[derive(Debug, Default)]
pub struct HasArch;

/// Typestate builder for UploadSession
///
/// This builder uses phantom types to enforce at compile time that all required
/// fields (filename, file_size, repo, arch) are set before building.
#[derive(Debug)]
pub struct UploadSessionBuilder<F, S, R, A> {
    filename: Option<String>,
    file_size: Option<u64>,
    sha256: Option<String>,
    repo: Option<String>,
    arch: Option<String>,
    chunk_size: usize,
    has_signature: bool,
    expiration_secs: i64,
    _marker: PhantomData<(F, S, R, A)>,
}

impl Default for UploadSessionBuilder<NoFilename, NoFileSize, NoRepo, NoArch> {
    fn default() -> Self {
        Self::new()
    }
}

impl UploadSessionBuilder<NoFilename, NoFileSize, NoRepo, NoArch> {
    pub fn new() -> Self {
        Self {
            filename: None,
            file_size: None,
            sha256: None,
            repo: None,
            arch: None,
            chunk_size: DEFAULT_CHUNK_SIZE,
            has_signature: false,
            expiration_secs: DEFAULT_SESSION_EXPIRATION_SECS,
            _marker: PhantomData,
        }
    }
}

impl<F, S, R, A> UploadSessionBuilder<F, S, R, A> {
    /// Set the filename (required)
    pub fn filename<T>(self, filename: T) -> UploadSessionBuilder<HasFilename, S, R, A>
    where
        T: Into<String>,
    {
        UploadSessionBuilder {
            filename: Some(filename.into()),
            file_size: self.file_size,
            sha256: self.sha256,
            repo: self.repo,
            arch: self.arch,
            chunk_size: self.chunk_size,
            has_signature: self.has_signature,
            expiration_secs: self.expiration_secs,
            _marker: PhantomData,
        }
    }

    /// Set the file size (required)
    pub fn file_size(self, size: u64) -> UploadSessionBuilder<F, HasFileSize, R, A> {
        UploadSessionBuilder {
            filename: self.filename,
            file_size: Some(size),
            sha256: self.sha256,
            repo: self.repo,
            arch: self.arch,
            chunk_size: self.chunk_size,
            has_signature: self.has_signature,
            expiration_secs: self.expiration_secs,
            _marker: PhantomData,
        }
    }

    /// Set the repository (required)
    pub fn repo<T>(self, repo: T) -> UploadSessionBuilder<F, S, HasRepo, A>
    where
        T: Into<String>,
    {
        UploadSessionBuilder {
            filename: self.filename,
            file_size: self.file_size,
            sha256: self.sha256,
            repo: Some(repo.into()),
            arch: self.arch,
            chunk_size: self.chunk_size,
            has_signature: self.has_signature,
            expiration_secs: self.expiration_secs,
            _marker: PhantomData,
        }
    }

    /// Set the architecture (required)
    pub fn arch<T>(self, arch: T) -> UploadSessionBuilder<F, S, R, HasArch>
    where
        T: Into<String>,
    {
        UploadSessionBuilder {
            filename: self.filename,
            file_size: self.file_size,
            sha256: self.sha256,
            repo: self.repo,
            arch: Some(arch.into()),
            chunk_size: self.chunk_size,
            has_signature: self.has_signature,
            expiration_secs: self.expiration_secs,
            _marker: PhantomData,
        }
    }

    /// Set the SHA256 hash (optional)
    pub fn sha256<T>(mut self, sha256: T) -> Self
    where
        T: Into<String>,
    {
        self.sha256 = Some(sha256.into());
        self
    }

    /// Set the chunk size (optional, defaults to DEFAULT_CHUNK_SIZE)
    pub fn chunk_size(mut self, size: usize) -> Self {
        self.chunk_size = size;
        self
    }

    /// Set whether a signature will be uploaded (optional, defaults to false)
    pub fn has_signature(mut self, has_sig: bool) -> Self {
        self.has_signature = has_sig;
        self
    }

    /// Set the session expiration time in seconds (optional, defaults to DEFAULT_SESSION_EXPIRATION_SECS)
    pub fn expiration_secs(mut self, secs: i64) -> Self {
        self.expiration_secs = secs;
        self
    }
}

impl UploadSessionBuilder<HasFilename, HasFileSize, HasRepo, HasArch> {
    /// Build the UploadSession
    ///
    /// This method is only available when all required fields have been set.
    pub fn build(self) -> UploadSession {
        let file_size = self.file_size.expect("file_size is required");
        let total_chunks = ((file_size as f64) / (self.chunk_size as f64)).ceil() as u32;
        let now = Utc::now();
        let expires_at = now + Duration::seconds(self.expiration_secs);

        UploadSession {
            upload_id: Uuid::new_v4().to_string(),
            filename: self.filename.expect("filename is required"),
            file_size,
            sha256: self.sha256,
            repo: self.repo.expect("repo is required"),
            arch: self.arch.expect("arch is required"),
            chunk_size: self.chunk_size,
            total_chunks,
            has_signature: self.has_signature,
            created_at: now,
            expires_at,
            uploaded_chunks: HashSet::new(),
        }
    }
}

/// In-memory storage for upload sessions
#[derive(Clone)]
pub struct UploadSessionStore {
    sessions: Arc<RwLock<std::collections::HashMap<String, UploadSession>>>,
    base_path: PathBuf,
}

impl UploadSessionStore {
    pub fn new(base_path: PathBuf) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
            base_path,
        }
    }

    /// Create a new upload session
    pub async fn create_session(&self, session: UploadSession) -> Result<UploadSession> {
        let upload_id = session.upload_id.clone();

        // Create upload directory
        let upload_dir = self.upload_dir(&upload_id)?;
        fs::create_dir_all(&upload_dir)
            .await
            .map_io_err(&upload_dir)?;

        // Create chunks subdirectory
        let chunks_dir = upload_dir.join("chunks");
        fs::create_dir_all(&chunks_dir)
            .await
            .map_io_err(&chunks_dir)?;

        // Save session metadata
        let metadata_path = upload_dir.join("metadata.json");
        let metadata_json =
            serde_json::to_string_pretty(&session).map_err(std::io::Error::other)?;
        fs::write(&metadata_path, metadata_json)
            .await
            .map_io_err(&metadata_path)?;

        // Store in memory
        let mut sessions = self.sessions.write().await;
        sessions.insert(upload_id.clone(), session.clone());

        Ok(session)
    }

    /// Get an upload session by ID
    pub async fn get_session(&self, upload_id: &str) -> Result<UploadSession> {
        let sessions = self.sessions.read().await;
        sessions
            .get(upload_id)
            .cloned()
            .ok_or_else(|| Error::InvalidPackage {
                pkgname: format!("Upload session not found: {}", upload_id),
            })
    }

    /// Update an upload session
    pub async fn update_session(&self, session: UploadSession) -> Result<()> {
        let upload_id = session.upload_id.clone();

        // Update metadata file
        let upload_dir = self.upload_dir(&upload_id)?;
        let metadata_path = upload_dir.join("metadata.json");
        let metadata_json =
            serde_json::to_string_pretty(&session).map_err(std::io::Error::other)?;
        fs::write(&metadata_path, metadata_json)
            .await
            .map_io_err(&metadata_path)?;

        // Update in memory
        let mut sessions = self.sessions.write().await;
        sessions.insert(upload_id, session);

        Ok(())
    }

    /// Delete an upload session and cleanup files
    pub async fn delete_session(&self, upload_id: &str) -> Result<(u32, u64)> {
        let upload_dir = self.upload_dir(upload_id)?;

        let mut deleted_chunks = 0;
        let mut bytes_freed = 0u64;

        // Calculate freed space
        if upload_dir.exists() {
            let chunks_dir = upload_dir.join("chunks");
            if chunks_dir.exists() {
                let mut entries = fs::read_dir(&chunks_dir).await.map_io_err(&chunks_dir)?;
                while let Some(entry) = entries.next_entry().await.map_io_err(&chunks_dir)? {
                    if let Ok(metadata) = entry.metadata().await {
                        bytes_freed += metadata.len();
                        deleted_chunks += 1;
                    }
                }
            }

            // Delete entire upload directory
            fs::remove_dir_all(&upload_dir)
                .await
                .map_io_err(&upload_dir)?;
        }

        // Remove from memory
        let mut sessions = self.sessions.write().await;
        sessions.remove(upload_id);

        Ok((deleted_chunks, bytes_freed))
    }

    /// Get path to upload directory
    pub fn upload_dir(&self, upload_id: &str) -> Result<PathBuf> {
        // Validate upload_id is a valid UUID to prevent path traversal
        Uuid::parse_str(upload_id).map_err(|_| Error::InvalidPackage {
            pkgname: format!("Invalid upload ID format: {}", upload_id),
        })?;

        Ok(self.base_path.join(".uploads").join(upload_id))
    }

    /// Get path to chunk file
    pub fn chunk_path(&self, upload_id: &str, chunk_number: u32) -> Result<PathBuf> {
        let upload_dir = self.upload_dir(upload_id)?;
        Ok(upload_dir
            .join("chunks")
            .join(format!("chunk_{:03}", chunk_number)))
    }

    /// Get path to signature file
    pub fn signature_path(&self, upload_id: &str) -> Result<PathBuf> {
        let upload_dir = self.upload_dir(upload_id)?;
        Ok(upload_dir.join("signature.sig"))
    }

    /// Store a chunk
    pub async fn store_chunk(
        &self,
        upload_id: &str,
        chunk_number: u32,
        data: &[u8],
    ) -> Result<String> {
        // Validate chunk exists in session
        let mut session = self.get_session(upload_id).await?;

        if chunk_number < 1 || chunk_number > session.total_chunks {
            return Err(Error::InvalidPackage {
                pkgname: format!(
                    "Chunk number {} out of range (1-{})",
                    chunk_number, session.total_chunks
                ),
            });
        }

        // Validate chunk size
        if chunk_number < session.total_chunks {
            // All chunks except the last must be exactly chunk_size
            if data.len() != session.chunk_size {
                return Err(Error::InvalidPackage {
                    pkgname: format!(
                        "Chunk {} size mismatch: expected {}, got {}",
                        chunk_number,
                        session.chunk_size,
                        data.len()
                    ),
                });
            }
        } else {
            // Last chunk can be smaller
            let expected_last_chunk_size = (session.file_size as usize) % session.chunk_size;
            let expected_size = if expected_last_chunk_size == 0 {
                session.chunk_size
            } else {
                expected_last_chunk_size
            };

            if data.len() != expected_size {
                return Err(Error::InvalidPackage {
                    pkgname: format!(
                        "Final chunk {} size mismatch: expected {}, got {}",
                        chunk_number,
                        expected_size,
                        data.len()
                    ),
                });
            }
        }

        // Write chunk to disk
        let chunk_path = self.chunk_path(upload_id, chunk_number)?;
        let mut file = fs::File::create(&chunk_path)
            .await
            .map_io_err(&chunk_path)?;
        file.write_all(data).await.map_io_err(&chunk_path)?;
        file.sync_all().await.map_io_err(&chunk_path)?;

        // Calculate checksum
        let checksum = format!("{:x}", md5::compute(data));

        // Update session
        session.uploaded_chunks.insert(chunk_number);
        self.update_session(session).await?;

        Ok(checksum)
    }

    /// Store signature file
    pub async fn store_signature(&self, upload_id: &str, data: &[u8]) -> Result<String> {
        let sig_path = self.signature_path(upload_id)?;
        let mut file = fs::File::create(&sig_path).await.map_io_err(&sig_path)?;
        file.write_all(data).await.map_io_err(&sig_path)?;
        file.sync_all().await.map_io_err(&sig_path)?;

        // Calculate checksum
        let checksum = format!("{:x}", sha2::Sha256::digest(data));

        Ok(checksum)
    }

    /// Assemble chunks into final package file on disk
    /// Returns the path to the assembled file
    pub async fn assemble_chunks(&self, upload_id: &str) -> Result<PathBuf> {
        let session = self.get_session(upload_id).await?;

        // Verify all chunks are present
        if !session.is_complete() {
            return Err(Error::InvalidPackage {
                pkgname: format!(
                    "Upload incomplete. Missing chunks: {:?}",
                    session.missing_chunks()
                ),
            });
        }

        let upload_dir = self.upload_dir(upload_id)?;
        let assembled_path = upload_dir.join("assembled.pkg.tar.zst");

        // Open output file
        let mut output_file = fs::File::create(&assembled_path)
            .await
            .map_io_err(&assembled_path)?;
        let mut total_size = 0u64;
        let mut hasher = sha2::Sha256::new();

        // Stream chunks to output file
        for chunk_num in 1..=session.total_chunks {
            let chunk_path = self.chunk_path(upload_id, chunk_num)?;
            let chunk_data = fs::read(&chunk_path).await.map_io_err(&chunk_path)?;

            // Update hash
            hasher.update(&chunk_data);

            // Write to output
            output_file
                .write_all(&chunk_data)
                .await
                .map_io_err(&assembled_path)?;
            total_size += chunk_data.len() as u64;
        }

        // Ensure all data is written
        output_file.sync_all().await.map_io_err(&assembled_path)?;
        drop(output_file);

        // Verify size
        if total_size != session.file_size {
            return Err(Error::InvalidPackage {
                pkgname: format!(
                    "Assembled size mismatch: expected {}, got {}",
                    session.file_size, total_size
                ),
            });
        }

        // Verify SHA256 if provided
        if let Some(expected_hash) = &session.sha256 {
            let actual_hash = format!("{:x}", hasher.finalize());
            if &actual_hash != expected_hash {
                return Err(Error::InvalidPackage {
                    pkgname: format!(
                        "Checksum mismatch: expected {}, got {}",
                        expected_hash, actual_hash
                    ),
                });
            }
        }

        Ok(assembled_path)
    }

    /// Get signature data if present
    pub async fn get_signature(&self, upload_id: &str) -> Result<Option<Vec<u8>>> {
        let sig_path = self.signature_path(upload_id)?;

        if sig_path.exists() {
            let data = fs::read(&sig_path).await.map_io_err(&sig_path)?;
            Ok(Some(data))
        } else {
            Ok(None)
        }
    }

    /// Clean up expired sessions
    pub async fn cleanup_expired(&self) -> Result<Vec<String>> {
        let mut expired = Vec::new();

        let sessions = self.sessions.read().await;
        for (upload_id, session) in sessions.iter() {
            if session.is_expired() {
                expired.push(upload_id.clone());
            }
        }
        drop(sessions);

        for upload_id in &expired {
            if let Err(e) = self.delete_session(upload_id).await {
                tracing::warn!(upload_id, error = %e, "Failed to cleanup expired session");
            }
        }

        Ok(expired)
    }

    /// Purge all upload directories on disk.
    /// Called on startup since sessions don't survive restarts.
    pub async fn purge_all(&self) -> Result<u32> {
        let uploads_dir = self.base_path.join(".uploads");

        if !uploads_dir.exists() {
            return Ok(0);
        }

        let mut count = 0u32;
        let mut entries = fs::read_dir(&uploads_dir).await.map_io_err(&uploads_dir)?;

        while let Some(entry) = entries.next_entry().await.map_io_err(&uploads_dir)? {
            let path = entry.path();

            if !path.is_dir() {
                continue;
            }

            let dir_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("<invalid>");

            if let Err(e) = fs::remove_dir_all(&path).await {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "Failed to remove upload directory"
                );
            } else {
                tracing::debug!(upload_id = dir_name, "Removed stale upload directory");
                count += 1;
            }
        }

        Ok(count)
    }
}

/// Default cleanup interval: 1 hour
pub const DEFAULT_CLEANUP_INTERVAL_SECS: u64 = 3600;

/// Spawn a background task that periodically cleans up expired upload sessions.
pub fn spawn_cleanup_task(store: UploadSessionStore, interval_secs: u64) {
    tokio::spawn(async move {
        let interval = std::time::Duration::from_secs(interval_secs);

        // Purge all stale sessions on startup (sessions don't survive restarts)
        match store.purge_all().await {
            Ok(count) if count > 0 => {
                tracing::info!(count, "Purged stale upload directories on startup");
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to purge upload directories on startup");
            }
            _ => {}
        }

        loop {
            tokio::time::sleep(interval).await;

            // Clean up expired in-memory sessions
            match store.cleanup_expired().await {
                Ok(expired) if !expired.is_empty() => {
                    tracing::info!(count = expired.len(), "Cleaned up expired upload sessions");
                }
                Err(e) => {
                    tracing::error!(error = %e, "Failed to cleanup expired uploads");
                }
                _ => {}
            }
        }
    });
}
