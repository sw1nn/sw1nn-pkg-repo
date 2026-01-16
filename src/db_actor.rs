//! Database Update Actor
//!
//! Serializes and coalesces repository database updates to prevent corruption
//! from concurrent regeneration and improve efficiency.

use crate::api::regenerate_repo_db;
use crate::storage::Storage;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant};

/// Key for tracking updates per repo/arch combination
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct RepoArchKey {
    pub repo: String,
    pub arch: String,
}

impl RepoArchKey {
    pub fn new<R, A>(repo: R, arch: A) -> Self
    where
        R: Into<String>,
        A: Into<String>,
    {
        Self {
            repo: repo.into(),
            arch: arch.into(),
        }
    }
}

/// Message sent to the actor
#[derive(Debug)]
pub enum DbUpdateMessage {
    /// Request a database update for the given repo/arch
    RequestUpdate(RepoArchKey),
    /// Force an immediate database rebuild (bypass debounce)
    ForceRebuild(RepoArchKey),
    /// Shutdown the actor gracefully
    Shutdown,
}

/// Handle for sending messages to the actor
#[derive(Clone)]
pub struct DbUpdateHandle {
    tx: mpsc::Sender<DbUpdateMessage>,
}

impl DbUpdateHandle {
    /// Request a database update for the given repo/arch.
    /// This is fire-and-forget - updates are coalesced with debounce.
    pub async fn request_update<R, A>(&self, repo: R, arch: A)
    where
        R: Into<String>,
        A: Into<String>,
    {
        let key = RepoArchKey::new(repo, arch);
        if let Err(e) = self.tx.send(DbUpdateMessage::RequestUpdate(key)).await {
            tracing::error!(error = %e, "Failed to send database update request");
        }
    }

    /// Force an immediate database rebuild, bypassing the debounce.
    /// This is fire-and-forget - the rebuild will happen as soon as possible.
    pub async fn force_rebuild<R, A>(&self, repo: R, arch: A)
    where
        R: Into<String>,
        A: Into<String>,
    {
        let key = RepoArchKey::new(repo, arch);
        if let Err(e) = self.tx.send(DbUpdateMessage::ForceRebuild(key)).await {
            tracing::error!(error = %e, "Failed to send force rebuild request");
        }
    }

    /// Request graceful shutdown of the actor
    pub async fn shutdown(&self) {
        if let Err(e) = self.tx.send(DbUpdateMessage::Shutdown).await {
            tracing::warn!(error = %e, "Failed to send shutdown message to db actor");
        }
    }
}

/// Pending update state for a repo/arch combination
struct PendingUpdate {
    /// When this update was first requested
    first_requested: Instant,
    /// When the last request came in (for debounce)
    last_requested: Instant,
}

/// Database update actor - serializes and coalesces DB regeneration
pub struct DbUpdateActor {
    rx: mpsc::Receiver<DbUpdateMessage>,
    storage: Arc<Storage>,
    pending: HashMap<RepoArchKey, PendingUpdate>,
    debounce_duration: Duration,
}

impl DbUpdateActor {
    /// Default debounce duration: 10 seconds
    const DEFAULT_DEBOUNCE_SECS: u64 = 10;

    /// Channel capacity
    const CHANNEL_CAPACITY: usize = 100;

    /// Create a new actor and its handle
    pub fn new(storage: Arc<Storage>) -> (Self, DbUpdateHandle) {
        Self::with_debounce(storage, Duration::from_secs(Self::DEFAULT_DEBOUNCE_SECS))
    }

    /// Create with custom debounce duration (useful for testing)
    pub fn with_debounce(
        storage: Arc<Storage>,
        debounce_duration: Duration,
    ) -> (Self, DbUpdateHandle) {
        let (tx, rx) = mpsc::channel(Self::CHANNEL_CAPACITY);

        let actor = Self {
            rx,
            storage,
            pending: HashMap::new(),
            debounce_duration,
        };

        let handle = DbUpdateHandle { tx };

        (actor, handle)
    }

    /// Run the actor loop
    pub async fn run(mut self) {
        tracing::info!(
            debounce_secs = self.debounce_duration.as_secs(),
            "Database update actor started"
        );

        loop {
            // Calculate next timeout based on pending updates
            let timeout = self.next_timeout();

            tokio::select! {
                // Handle incoming messages
                msg = self.rx.recv() => {
                    match msg {
                        Some(DbUpdateMessage::RequestUpdate(key)) => {
                            self.handle_request(key);
                        }
                        Some(DbUpdateMessage::ForceRebuild(key)) => {
                            self.handle_force_rebuild(key).await;
                        }
                        Some(DbUpdateMessage::Shutdown) => {
                            tracing::info!("Database update actor received shutdown signal");
                            self.flush_all_pending().await;
                            break;
                        }
                        None => {
                            tracing::info!("Database update actor channel closed");
                            self.flush_all_pending().await;
                            break;
                        }
                    }
                }
                // Process ready updates when timeout fires
                _ = tokio::time::sleep(timeout) => {
                    self.process_ready_updates().await;
                }
            }
        }

        tracing::info!("Database update actor stopped");
    }

    /// Handle an incoming update request
    fn handle_request(&mut self, key: RepoArchKey) {
        let now = Instant::now();

        self.pending
            .entry(key.clone())
            .and_modify(|pending| {
                pending.last_requested = now;
                tracing::debug!(
                    repo = %key.repo,
                    arch = %key.arch,
                    "Coalesced database update request"
                );
            })
            .or_insert_with(|| {
                tracing::debug!(
                    repo = %key.repo,
                    arch = %key.arch,
                    "New database update request queued"
                );
                PendingUpdate {
                    first_requested: now,
                    last_requested: now,
                }
            });
    }

    /// Handle a force rebuild request - bypass debounce and rebuild immediately
    async fn handle_force_rebuild(&mut self, key: RepoArchKey) {
        // Remove any pending update for this key (we're rebuilding now)
        self.pending.remove(&key);

        tracing::info!(
            repo = %key.repo,
            arch = %key.arch,
            "Force rebuilding database"
        );

        self.regenerate_db(&key).await;
    }

    /// Calculate the next timeout duration
    fn next_timeout(&self) -> Duration {
        if self.pending.is_empty() {
            // No pending updates, wait a long time (will be interrupted by messages)
            Duration::from_secs(3600)
        } else {
            // Find the soonest update that will be ready
            let now = Instant::now();
            self.pending
                .values()
                .map(|p| {
                    let ready_at = p.last_requested + self.debounce_duration;
                    ready_at.saturating_duration_since(now)
                })
                .min()
                .unwrap_or(Duration::from_millis(100))
                .max(Duration::from_millis(100)) // Minimum 100ms to avoid busy loop
        }
    }

    /// Process any updates that have passed their debounce period
    async fn process_ready_updates(&mut self) {
        let now = Instant::now();

        // Find keys that are ready (debounce period has passed)
        let ready_keys: Vec<RepoArchKey> = self
            .pending
            .iter()
            .filter(|(_, pending)| {
                now.duration_since(pending.last_requested) >= self.debounce_duration
            })
            .map(|(key, _)| key.clone())
            .collect();

        // Process each ready update
        for key in ready_keys {
            if let Some(pending) = self.pending.remove(&key) {
                let wait_time = now.duration_since(pending.first_requested);
                tracing::info!(
                    repo = %key.repo,
                    arch = %key.arch,
                    wait_ms = wait_time.as_millis(),
                    "Processing database update"
                );

                self.regenerate_db(&key).await;
            }
        }
    }

    /// Flush all pending updates immediately (used during shutdown)
    async fn flush_all_pending(&mut self) {
        let keys: Vec<RepoArchKey> = self.pending.keys().cloned().collect();

        for key in keys {
            if self.pending.remove(&key).is_some() {
                tracing::info!(
                    repo = %key.repo,
                    arch = %key.arch,
                    "Flushing pending database update during shutdown"
                );
                self.regenerate_db(&key).await;
            }
        }
    }

    /// Perform the actual database regeneration
    async fn regenerate_db(&self, key: &RepoArchKey) {
        if let Err(e) = regenerate_repo_db(&self.storage, &key.repo, &key.arch).await {
            tracing::error!(
                repo = %key.repo,
                arch = %key.arch,
                error = %e,
                "Failed to regenerate repository database"
            );
        } else {
            tracing::info!(
                repo = %key.repo,
                arch = %key.arch,
                "Repository database regenerated successfully"
            );
        }
    }
}
