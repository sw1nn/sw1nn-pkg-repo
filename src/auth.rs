//! Bearer-token authentication against Authelia-issued OIDC access tokens.
//!
//! Tokens are RS256 JWTs (RFC 9068, `typ: at+jwt`) verified **offline** against
//! a cached JWKS. The introspection and userinfo endpoints are deliberately not
//! used: everything needed is in the token, and a per-request call would make
//! the identity provider a hard dependency of the upload path.

use crate::api::AppState;
use crate::config::AuthConfig;
use crate::error::Error;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Group required to publish packages.
pub const GROUP_PUBLISH: &str = "pkg-publish";

/// Group required for destructive repository operations.
pub const GROUP_ADMIN: &str = "pkg-admin";

/// Shortest interval between JWKS refetches.
///
/// Without this, a caller sending garbage `kid` values would make the server
/// hammer the identity provider.
const JWKS_MIN_REFETCH_INTERVAL: Duration = Duration::from_secs(60);

/// Claims of an Authelia access token.
///
/// `aud` is deliberately absent: Authelia's device grant returns `aud: []`
/// regardless of client configuration, so the replay boundary between services
/// is `client_id`, not audience. Do not reintroduce an audience check without
/// first confirming against a live token that `aud` is populated.
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// Stable opaque user identifier.
    pub sub: String,
    /// Issuer, checked against the configured issuer.
    pub iss: String,
    /// OIDC client the token was minted for. This is the replay boundary.
    pub client_id: String,
    /// Expiration (unix timestamp).
    pub exp: i64,
    /// Not before (unix timestamp).
    pub nbf: Option<i64>,
    /// Issued at (unix timestamp).
    pub iat: Option<i64>,
    /// Groups the identity provider asserts. The authorization boundary.
    #[serde(default)]
    pub groups: Vec<String>,
    /// Human-readable username, for attributing uploads in the logs.
    pub preferred_username: Option<String>,
}

/// A caller holding a valid access token for this service.
#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub sub: String,
    pub username: String,
    pub groups: Vec<String>,
}

impl AuthenticatedUser {
    /// Anonymous stand-in used when the server runs without an `[auth]` section.
    fn anonymous() -> Self {
        Self {
            sub: "<anonymous>".to_owned(),
            username: "<anonymous>".to_owned(),
            groups: Vec::new(),
        }
    }

    fn is_anonymous(&self) -> bool {
        self.sub == "<anonymous>"
    }

    fn require_group(self, group: &str) -> Result<Self, Error> {
        // An unauthenticated server has no groups to check against; the
        // `[auth]`-absent escape hatch already allowed the request through.
        if self.is_anonymous() || self.groups.iter().any(|g| g == group) {
            return Ok(self);
        }
        Err(Error::Forbidden {
            reason: format!("missing required group '{group}'"),
        })
    }
}

/// A caller authorized to publish packages (`pkg-publish`).
#[derive(Debug, Clone, derive_more::Deref)]
pub struct PkgPublish(pub AuthenticatedUser);

/// A caller authorized for destructive repository operations (`pkg-admin`).
#[derive(Debug, Clone, derive_more::Deref)]
pub struct PkgAdmin(pub AuthenticatedUser);

/// JWKS cache for offline signature verification.
///
/// Keys are fetched once and reused. A refetch happens only when a token
/// presents an unknown `kid`, and is rate-limited. Serving stale keys is safe:
/// key rotation is manual and rare, and a request signed by a genuinely new key
/// triggers exactly one refetch.
pub struct JwksCache {
    uri: String,
    http: reqwest::Client,
    keys: RwLock<HashMap<String, DecodingKey>>,
    /// Held across the network fetch so concurrent misses collapse into one
    /// request rather than a thundering herd.
    last_fetch: Mutex<Option<Instant>>,
}

impl std::fmt::Debug for JwksCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwksCache").field("uri", &self.uri).finish()
    }
}

impl JwksCache {
    pub fn new<S>(uri: S, http: reqwest::Client) -> Self
    where
        S: Into<String>,
    {
        Self {
            uri: uri.into(),
            http,
            keys: RwLock::new(HashMap::new()),
            last_fetch: Mutex::new(None),
        }
    }

    /// Build a cache pre-populated with keys and no reachable endpoint.
    ///
    /// Used by tests, which sign with a locally generated keypair.
    pub fn with_keys<I, S>(keys: I) -> Self
    where
        I: IntoIterator<Item = (S, DecodingKey)>,
        S: Into<String>,
    {
        Self {
            uri: String::new(),
            http: reqwest::Client::new(),
            keys: RwLock::new(keys.into_iter().map(|(k, v)| (k.into(), v)).collect()),
            // Pretend a fetch just happened so the rate limit suppresses any
            // network attempt for an unknown kid.
            last_fetch: Mutex::new(Some(Instant::now())),
        }
    }

    fn cached(&self, kid: &str) -> Option<DecodingKey> {
        self.keys
            .read()
            .expect("jwks cache poisoned")
            .get(kid)
            .cloned()
    }

    /// Resolve a `kid` to a decoding key, refetching the JWKS at most once and
    /// no more often than [`JWKS_MIN_REFETCH_INTERVAL`].
    async fn key_for(&self, kid: &str) -> Result<DecodingKey, Error> {
        if let Some(key) = self.cached(kid) {
            return Ok(key);
        }

        {
            let mut last_fetch = self.last_fetch.lock().await;

            // Another task may have refreshed while we waited for the lock.
            if let Some(key) = self.cached(kid) {
                return Ok(key);
            }

            let due = last_fetch.is_none_or(|at| at.elapsed() >= JWKS_MIN_REFETCH_INTERVAL);
            if !due {
                return Err(Error::TokenVerification {
                    msg: format!("unknown signing key '{kid}' (refetch rate-limited)"),
                });
            }

            *last_fetch = Some(Instant::now());
            self.refetch().await?;
        }

        self.cached(kid).ok_or_else(|| Error::TokenVerification {
            msg: format!("unknown signing key '{kid}'"),
        })
    }

    async fn refetch(&self) -> Result<(), Error> {
        tracing::info!(uri = %self.uri, "Fetching JWKS");

        let jwks: JwkSet = self
            .http
            .get(&self.uri)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|e| Error::Jwks {
                msg: format!("failed to fetch JWKS: {e}"),
            })?
            .json()
            .await
            .map_err(|e| Error::Jwks {
                msg: format!("failed to parse JWKS: {e}"),
            })?;

        let fetched = decoding_keys(&jwks);

        if fetched.is_empty() {
            return Err(Error::Jwks {
                msg: "JWKS contained no usable keys".to_owned(),
            });
        }

        tracing::info!(key_count = fetched.len(), "Loaded JWKS keys");
        *self.keys.write().expect("jwks cache poisoned") = fetched;

        Ok(())
    }
}

/// Build a `kid` -> key map from a JWKS, skipping entries that have no key id
/// or that cannot be turned into a decoding key.
fn decoding_keys(jwks: &JwkSet) -> HashMap<String, DecodingKey> {
    let mut keys = HashMap::new();

    for jwk in &jwks.keys {
        let Some(kid) = jwk.common.key_id.clone() else {
            continue;
        };
        match DecodingKey::from_jwk(jwk) {
            Ok(key) => {
                keys.insert(kid, key);
            }
            Err(e) => tracing::warn!(kid, error = %e, "Skipping unusable JWKS key"),
        }
    }

    keys
}

/// Verify a bearer token and return its claims.
///
/// Checks, all required, in order: signature, issuer, client id, then
/// expiry/not-before with a small clock leeway. Group membership is the
/// caller's responsibility — see [`AuthenticatedUser::require_group`].
pub async fn verify_token(
    config: &AuthConfig,
    jwks: &JwksCache,
    token: &str,
) -> Result<Claims, Error> {
    let header = jsonwebtoken::decode_header(token).map_err(|e| Error::TokenVerification {
        msg: format!("malformed token header: {e}"),
    })?;

    let kid = header.kid.ok_or_else(|| Error::TokenVerification {
        msg: "token header has no 'kid'".to_owned(),
    })?;

    let key = jwks.key_for(&kid).await?;

    let mut validation = Validation::new(Algorithm::RS256);
    validation.leeway = config.leeway_secs;
    validation.set_issuer(&[&config.issuer]);
    validation.set_required_spec_claims(&["exp", "iss", "sub"]);
    // Not on by default in jsonwebtoken.
    validation.validate_nbf = true;
    // Authelia's device grant returns `aud: []`, so an audience check would
    // reject every real token. `client_id` is the replay boundary instead.
    validation.validate_aud = false;

    let claims = jsonwebtoken::decode::<Claims>(token, &key, &validation)
        .map_err(|e| Error::TokenVerification {
            msg: format!("token rejected: {e}"),
        })?
        .claims;

    if claims.client_id != config.client_id {
        return Err(Error::TokenVerification {
            msg: format!("token was minted for client '{}'", claims.client_id),
        });
    }

    Ok(claims)
}

// -- Axum extractors --

impl FromRequestParts<Arc<AppState>> for AuthenticatedUser {
    type Rejection = Error;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        // Auth not configured — allow all requests through.
        let Some(auth_config) = &state.config.auth else {
            return Ok(AuthenticatedUser::anonymous());
        };

        let token = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or(Error::Unauthorized)?;

        let claims = verify_token(auth_config, &state.jwks, token).await?;

        let username = claims
            .preferred_username
            .unwrap_or_else(|| claims.sub.clone());
        tracing::debug!(sub = %claims.sub, username, "Authenticated request");

        Ok(AuthenticatedUser {
            sub: claims.sub,
            username,
            groups: claims.groups,
        })
    }
}

impl FromRequestParts<Arc<AppState>> for PkgPublish {
    type Rejection = Error;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        AuthenticatedUser::from_request_parts(parts, state)
            .await?
            .require_group(GROUP_PUBLISH)
            .map(Self)
    }
}

impl FromRequestParts<Arc<AppState>> for PkgAdmin {
    type Rejection = Error;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        AuthenticatedUser::from_request_parts(parts, state)
            .await?
            .require_group(GROUP_ADMIN)
            .map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user<I>(groups: I) -> AuthenticatedUser
    where
        I: IntoIterator<Item: Into<String>>,
    {
        AuthenticatedUser {
            sub: "07ca04a5".to_owned(),
            username: "neale".to_owned(),
            groups: groups.into_iter().map(Into::into).collect(),
        }
    }

    #[test]
    fn require_group_admits_exact_match() -> Result<(), Error> {
        user(["lolcommits", GROUP_PUBLISH]).require_group(GROUP_PUBLISH)?;
        Ok(())
    }

    /// `admins` is not a blanket grant. A token carrying it but not the
    /// specific group must still be refused.
    #[test]
    fn require_group_rejects_admins_as_blanket_grant() {
        for group in [GROUP_PUBLISH, GROUP_ADMIN] {
            let result = user(["admins"]).require_group(group);
            assert!(
                matches!(result, Err(Error::Forbidden { .. })),
                "'admins' must not grant '{group}'"
            );
        }
    }

    /// Holding one group must not imply the other.
    #[test]
    fn require_group_does_not_conflate_publish_and_admin() {
        assert!(user([GROUP_PUBLISH]).require_group(GROUP_ADMIN).is_err());
        assert!(user([GROUP_ADMIN]).require_group(GROUP_PUBLISH).is_err());
    }

    /// The JWKS served by the live deployment, captured 2026-09-02. Public key
    /// material only. Guards against a `jsonwebtoken` upgrade that stops
    /// accepting the shape Authelia actually serves.
    const LIVE_JWKS: &str = r#"{
      "keys": [
        {
          "use": "sig",
          "kty": "RSA",
          "kid": "main",
          "alg": "RS256",
          "n": "r--nIn2ZBVld2wGhIXEAPanNgiX7LgajYFN4KoMvb8q_a85AsKHS0lkXo0TXAOegolauVS4uqYk2LzKk6ygqLLNVrIRKQBCgOZAvSg3le2wFCp98PQ9SEICGQssmCDm-01g3__WN6g6zBHPn_b5LwftMRSHcM4MC_EgYMA90kp-k-1C8ZlzTorey54jnCxeRlCd7v-HucZusCwB0n5GLiijf_60yk1Q0ez-mJNjor4KEfehH-yFN3Os4gkdO9itLRHbYUgb-V9UGdKnUcr0KfPuRoij2qKRpoRO6VEOf1bzFT5uDDHcf6fHvltmtGowIe_ZUuDVnwZl4NWZ-0lJvtt7LIFF41lfkx1h78WjfXOAyGvJzveF0rAYvpb-KGxWX_StgoMxO3eVRJ0ut3wYBnHmezEtn-n1kzWPy_Uz35ywZONZN2C9x8H1WQL_5U7LK929jvT2McRIWg_clrz9q0d5HvjWHB7hq4DaY9p9fuLJRqmo6ecDQZyZRbNZxXVgODKii_EyHEgSswtDhg3ayD6XEQN4Ln7ev3fW6nVz2sKEshTi7pB3DLe8jkGOTmn30I1JefwfG3X0Qc6mOuTlGjaCgAp3ujWVG_RMAgWHgUoxIA9b3CLBf9HRGGF9iuKnihabm0m-V9hzcRkAnsLbg7u1aVH94K_WPlvynmK9c05k",
          "e": "AQAB"
        }
      ]
    }"#;

    #[test]
    fn live_jwks_yields_a_usable_decoding_key() -> Result<(), serde_json::Error> {
        let jwks: JwkSet = serde_json::from_str(LIVE_JWKS)?;
        let keys = decoding_keys(&jwks);

        assert_eq!(keys.len(), 1);
        assert!(keys.contains_key("main"), "expected the 'main' key id");
        Ok(())
    }

    #[test]
    fn anonymous_passes_group_checks_when_auth_is_disabled() -> Result<(), Error> {
        AuthenticatedUser::anonymous().require_group(GROUP_ADMIN)?;
        Ok(())
    }
}
