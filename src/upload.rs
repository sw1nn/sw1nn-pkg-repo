use crate::error::{Error, Result, ResultIoExt};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::HashSet;
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
    pub fn new(
        filename: String,
        file_size: u64,
        sha256: Option<String>,
        repo: String,
        arch: String,
        chunk_size: usize,
        has_signature: bool,
        expiration_secs: i64,
    ) -> Self {
        let total_chunks = ((file_size as f64) / (chunk_size as f64)).ceil() as u32;
        let now = Utc::now();
        let expires_at = now + Duration::seconds(expiration_secs);

        Self {
            upload_id: Uuid::new_v4().to_string(),
            filename,
            file_size,
            sha256,
            repo,
            arch,
            chunk_size,
            total_chunks,
            has_signature,
            created_at: now,
            expires_at,
            uploaded_chunks: HashSet::new(),
        }
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
            serde_json::to_string_pretty(&session).map_err(|e| std::io::Error::other(e))?;
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
            serde_json::to_string_pretty(&session).map_err(|e| std::io::Error::other(e))?;
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
                tracing::warn!("Failed to cleanup expired session {}: {}", upload_id, e);
            }
        }

        Ok(expired)
    }
}
