//! Persistence for the OIDC tokens obtained by `login`.
//!
//! The Secret Service (KeepassXC on these machines) is the primary store. On a
//! headless host there is no Secret Service to talk to, so a `0600` file under
//! `$XDG_STATE_HOME` is the fallback.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const SERVICE: &str = "sw1nn-pkg-repo";
const ACCOUNT: &str = "authelia";

/// Tokens and the metadata needed to use and display them.
///
/// Never logged, never printed. `status` shows only the username and expiry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Access-token expiry as a unix timestamp.
    pub expires_at: i64,
    /// `preferred_username` from the access token, for display only.
    pub username: Option<String>,
}

impl Tokens {
    /// Whether the access token is expired, or close enough that a request
    /// made now would probably arrive after it lapsed.
    pub fn is_stale(&self, skew_secs: i64) -> bool {
        self.expires_at - skew_secs <= chrono::Utc::now().timestamp()
    }
}

/// Where the file fallback lives: `$XDG_STATE_HOME/sw1nn-pkg-repo/tokens.json`.
///
/// `None` when no absolute state directory can be determined — most likely
/// `HOME` is unset. Guessing a relative path here would scatter plaintext
/// credentials through whatever directory the CLI happened to run from.
fn fallback_path() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(dirs::state_dir)
        .map(|dir| dir.join(SERVICE).join("tokens.json"))
}

/// The pre-Authelia token file. It holds a self-signed HS256 token the server
/// no longer accepts, so it is removed rather than migrated.
fn legacy_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join(SERVICE).join("token"))
}

fn keyring_entry() -> Option<keyring::Entry> {
    keyring::Entry::new(SERVICE, ACCOUNT)
        .inspect_err(|e| tracing::debug!(error = %e, "Secret Service unavailable"))
        .ok()
}

/// Read the stored tokens, preferring the Secret Service.
pub fn load() -> Option<Tokens> {
    let from_keyring = keyring_entry()
        .and_then(|entry| entry.get_password().ok())
        .and_then(|json| parse(&json));

    if from_keyring.is_some() {
        return from_keyring;
    }

    std::fs::read_to_string(fallback_path()?)
        .ok()
        .and_then(|json| parse(&json))
}

fn parse(json: &str) -> Option<Tokens> {
    serde_json::from_str(json)
        .inspect_err(|e| tracing::warn!(error = %e, "Ignoring unreadable stored credentials"))
        .ok()
}

/// Persist tokens, falling back to a `0600` file when the Secret Service
/// cannot be reached.
pub fn save(tokens: &Tokens) -> Result<(), Box<dyn std::error::Error>> {
    let json = serde_json::to_string(tokens)?;

    match keyring_entry().map(|entry| entry.set_password(&json)) {
        Some(Ok(())) => {
            // A stale file copy must not outlive the Secret Service entry.
            if let Some(path) = fallback_path() {
                let _ = std::fs::remove_file(path);
            }
            Ok(())
        }
        Some(Err(e)) => {
            tracing::debug!(error = %e, "Secret Service write failed; using file fallback");
            save_to_file(&json)
        }
        None => save_to_file(&json),
    }
}

fn save_to_file(json: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = fallback_path().ok_or(
        "no Secret Service available and no state directory to fall back to \
         (set XDG_STATE_HOME or HOME)",
    )?;

    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }

    // Created 0600 from the outset: a chmod after the write would leave the
    // tokens world-readable for the gap in between.
    write_private(&path, json)?;

    Ok(())
}

#[cfg(unix)]
fn create_private_dir(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?
        .write_all(contents.as_bytes())
}

#[cfg(not(unix))]
fn create_private_dir(path: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

#[cfg(not(unix))]
fn write_private(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    std::fs::write(path, contents)
}

/// Remove stored credentials from every location, including the pre-Authelia
/// token file. Returns whether anything was actually removed.
pub fn clear() -> bool {
    let mut removed = false;

    if let Some(entry) = keyring_entry()
        && entry.delete_credential().is_ok()
    {
        removed = true;
    }

    for path in [fallback_path(), legacy_path()].into_iter().flatten() {
        if std::fs::remove_file(&path).is_ok() {
            removed = true;
        }
    }

    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_stale_accounts_for_skew() {
        let soon = Tokens {
            access_token: "x".to_owned(),
            refresh_token: None,
            expires_at: chrono::Utc::now().timestamp() + 30,
            username: None,
        };

        assert!(soon.is_stale(60), "expiring inside the skew window");
        assert!(!soon.is_stale(10), "still usable beyond the skew window");
    }

    #[test]
    fn fallback_path_honours_xdg_state_home() {
        // SAFETY: single-threaded test, and the value is restored below.
        let previous = std::env::var_os("XDG_STATE_HOME");
        unsafe { std::env::set_var("XDG_STATE_HOME", "/somewhere/state") };

        assert_eq!(
            fallback_path(),
            Some(PathBuf::from("/somewhere/state/sw1nn-pkg-repo/tokens.json"))
        );

        unsafe {
            match previous {
                Some(v) => std::env::set_var("XDG_STATE_HOME", v),
                None => std::env::remove_var("XDG_STATE_HOME"),
            }
        }
    }
}
