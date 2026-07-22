//! Detached OpenPGP signature inspection for packages.
//!
//! Package signatures are detached `.sig` files sitting next to the
//! `.pkg.tar.zst`. We report two things about them:
//!
//!   * always: the issuer key ID and creation time, read straight from
//!     the signature packet (no keyring needed);
//!   * when a trust keyring is configured: a good/bad verdict and the
//!     signer's user-id, from verifying the signature over the package.

use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use sequoia_openpgp::cert::CertParser;
use sequoia_openpgp::packet::{Packet, Signature};
use sequoia_openpgp::parse::Parse;
use sequoia_openpgp::parse::stream::{
    DetachedVerifierBuilder, MessageLayer, MessageStructure, VerificationHelper,
};
use sequoia_openpgp::policy::StandardPolicy;
use sequoia_openpgp::{Cert, KeyHandle};

use crate::models::SignatureInfo;

/// A set of trusted signing certificates, loaded once at startup.
#[derive(Clone, Default)]
pub struct Keyring {
    certs: Arc<Vec<Cert>>,
}

impl Keyring {
    /// Load trusted certs from an ASCII-armored (or binary) keyring file.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let certs = CertParser::from_file(path)?.collect::<sequoia_openpgp::Result<Vec<Cert>>>()?;
        Ok(Self {
            certs: Arc::new(certs),
        })
    }

    fn is_empty(&self) -> bool {
        self.certs.is_empty()
    }

    /// Find a cert whose keys match the given handle (primary or subkey).
    fn find(&self, handle: &KeyHandle) -> Option<&Cert> {
        self.certs
            .iter()
            .find(|cert| cert.keys().any(|k| k.key().key_handle().aliases(handle)))
    }
}

/// Inspect a package's detached signature.
///
/// Returns `None` when no `.sig` sits next to the package, or when the
/// signature cannot be parsed at all. Otherwise returns what we could
/// determine — key ID always, verdict/signer only when `keyring` is
/// populated and verification runs.
pub fn inspect(pkg_path: &Path, keyring: &Keyring) -> Option<SignatureInfo> {
    let sig_path = pkg_path.with_extension(format!(
        "{}.sig",
        pkg_path.extension().and_then(|e| e.to_str()).unwrap_or("")
    ));
    if !sig_path.exists() {
        return None;
    }

    let sig_bytes = std::fs::read(&sig_path).ok()?;
    let sig = parse_signature(&sig_bytes)?;

    let key_id = sig
        .issuers()
        .next()
        .map(|h| h.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let created_at = sig.signature_creation_time().map(DateTime::<Utc>::from);

    // Without a keyring we stop at what the packet self-reports.
    if keyring.is_empty() {
        return Some(SignatureInfo {
            key_id,
            signer: None,
            created_at,
            valid: None,
        });
    }

    let (valid, signer) = verify(pkg_path, &sig_bytes, keyring);
    Some(SignatureInfo {
        key_id,
        signer,
        created_at,
        valid: Some(valid),
    })
}

/// Pull the first Signature packet out of a detached `.sig` blob.
fn parse_signature(sig_bytes: &[u8]) -> Option<Signature> {
    use sequoia_openpgp::parse::PacketParserResult;

    let mut ppr = sequoia_openpgp::parse::PacketParser::from_bytes(sig_bytes).ok()?;
    while let PacketParserResult::Some(pp) = ppr {
        let (packet, next) = pp.recurse().ok()?;
        if let Packet::Signature(sig) = packet {
            return Some(sig);
        }
        ppr = next;
    }
    None
}

/// Verify the detached signature over the package file. Returns the
/// verdict and, when good, the signer's primary user-id.
fn verify(pkg_path: &Path, sig_bytes: &[u8], keyring: &Keyring) -> (bool, Option<String>) {
    let policy = StandardPolicy::new();
    let helper = Helper {
        keyring,
        signer: None,
    };

    let builder = match DetachedVerifierBuilder::from_bytes(sig_bytes) {
        Ok(b) => b,
        Err(_) => return (false, None),
    };
    let mut verifier = match builder.with_policy(&policy, None, helper) {
        Ok(v) => v,
        Err(_) => return (false, None),
    };

    let file = match std::fs::File::open(pkg_path) {
        Ok(f) => f,
        Err(_) => return (false, None),
    };
    match verifier.verify_reader(file) {
        Ok(()) => (true, verifier.into_helper().signer),
        Err(_) => (false, None),
    }
}

struct Helper<'a> {
    keyring: &'a Keyring,
    signer: Option<String>,
}

impl VerificationHelper for Helper<'_> {
    fn get_certs(&mut self, ids: &[KeyHandle]) -> sequoia_openpgp::Result<Vec<Cert>> {
        Ok(ids
            .iter()
            .filter_map(|id| self.keyring.find(id).cloned())
            .collect())
    }

    fn check(&mut self, structure: MessageStructure) -> sequoia_openpgp::Result<()> {
        for layer in structure {
            if let MessageLayer::SignatureGroup { results } = layer
                && let Some(result) = results.into_iter().next()
            {
                return match result {
                    Ok(good) => {
                        self.signer =
                            good.ka.cert().userids().next().map(|uid| {
                                String::from_utf8_lossy(uid.userid().value()).into_owned()
                            });
                        Ok(())
                    }
                    Err(e) => Err(anyhow::anyhow!("signature not valid: {e}")),
                };
            }
        }
        Err(anyhow::anyhow!("no valid signature found"))
    }
}
