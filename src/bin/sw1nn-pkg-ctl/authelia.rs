//! OIDC device authorization grant (RFC 8628) against Authelia.
//!
//! The CLI talks to the identity provider directly; the package repository is
//! no longer in the login path. `sw1nn-pkg-cli` is a public client, so there is
//! no client secret to send.

use crate::token_store::Tokens;
use serde::Deserialize;
use std::time::Duration;

pub const DEFAULT_ISSUER: &str = "https://auth.sw1nn.net";
pub const DEFAULT_CLIENT_ID: &str = "sw1nn-pkg-cli";

const SCOPE: &str = "openid profile groups offline_access";
const DEVICE_CODE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// Endpoints and client id for the device flow.
#[derive(Debug, Clone)]
pub struct Provider {
    pub issuer: String,
    pub client_id: String,
}

impl Default for Provider {
    fn default() -> Self {
        Self {
            issuer: std::env::var("SW1NN_AUTH_ISSUER")
                .unwrap_or_else(|_| DEFAULT_ISSUER.to_owned()),
            client_id: std::env::var("SW1NN_AUTH_CLIENT_ID")
                .unwrap_or_else(|_| DEFAULT_CLIENT_ID.to_owned()),
        }
    }
}

impl Provider {
    fn device_authorization_url(&self) -> String {
        format!("{}/api/oidc/device-authorization", self.issuer)
    }

    fn token_url(&self) -> String {
        format!("{}/api/oidc/token", self.issuer)
    }
}

#[derive(Debug, Deserialize)]
pub struct DeviceAuthorization {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    #[serde(default = "default_interval")]
    pub interval: u64,
}

fn default_interval() -> u64 {
    5
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ErrorResponse {
    error: String,
    error_description: Option<String>,
}

/// Claims read out of the access token for display. The signature is *not*
/// checked here — the server is the only party that verifies tokens.
#[derive(Debug, Deserialize)]
struct DisplayClaims {
    exp: i64,
    preferred_username: Option<String>,
}

#[derive(Debug, derive_more::Display, derive_more::Error, derive_more::From)]
pub enum Error {
    #[display("request to the identity provider failed: {_0}")]
    #[from]
    Http(reqwest::Error),

    /// The provider returned an OAuth error other than the pending states.
    #[display("{code}{}", description.as_deref().map(|d| format!(": {d}")).unwrap_or_default())]
    Oauth {
        code: String,
        description: Option<String>,
    },

    /// The device code expired before the user approved it.
    #[display("the device code expired before it was approved")]
    Expired,

    /// The stored session cannot be renewed; the user must log in again.
    #[display("not logged in or session expired")]
    SessionExpired,
}

/// Step 1: ask the provider for a device code and a URL for the user to visit.
pub async fn start_device_authorization(
    http: &reqwest::Client,
    provider: &Provider,
) -> Result<DeviceAuthorization, Error> {
    // No `audience` parameter: Authelia's device grant accepts one and then
    // ignores it, leaving `aud` empty either way.
    let response = http
        .post(provider.device_authorization_url())
        .form(&[("client_id", provider.client_id.as_str()), ("scope", SCOPE)])
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(oauth_error(response).await);
    }

    Ok(response.json().await?)
}

/// Step 2: poll the token endpoint until the user approves or the code expires.
///
/// Honours the provider's `interval` and backs off on `slow_down`, per RFC 8628.
pub async fn poll_for_tokens(
    http: &reqwest::Client,
    provider: &Provider,
    device: &DeviceAuthorization,
) -> Result<Tokens, Error> {
    let deadline = std::time::Instant::now() + Duration::from_secs(device.expires_in);
    let mut interval = Duration::from_secs(device.interval.max(1));

    loop {
        if std::time::Instant::now() >= deadline {
            return Err(Error::Expired);
        }

        tokio::time::sleep(interval).await;

        let response = http
            .post(provider.token_url())
            .form(&[
                ("client_id", provider.client_id.as_str()),
                ("grant_type", DEVICE_CODE_GRANT),
                ("device_code", device.device_code.as_str()),
            ])
            .send()
            .await?;

        if response.status().is_success() {
            return Ok(into_tokens(response.json().await?));
        }

        match oauth_error(response).await {
            Error::Oauth { code, .. } if code == "authorization_pending" => continue,
            Error::Oauth { code, .. } if code == "slow_down" => {
                interval += Duration::from_secs(5);
            }
            Error::Oauth { code, .. } if code == "expired_token" => return Err(Error::Expired),
            other => return Err(other),
        }
    }
}

/// Exchange a refresh token for a fresh access token.
///
/// Authelia may rotate the refresh token, so the caller must persist whatever
/// comes back rather than keeping the old one.
pub async fn refresh(
    http: &reqwest::Client,
    provider: &Provider,
    refresh_token: &str,
) -> Result<Tokens, Error> {
    let response = http
        .post(provider.token_url())
        .form(&[
            ("client_id", provider.client_id.as_str()),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(Error::SessionExpired);
    }

    let mut tokens = into_tokens(response.json().await?);

    // A rotating provider returns a new refresh token; a non-rotating one omits
    // it and expects the old one to stay valid.
    if tokens.refresh_token.is_none() {
        tokens.refresh_token = Some(refresh_token.to_owned());
    }

    Ok(tokens)
}

fn into_tokens(response: TokenResponse) -> Tokens {
    let claims = display_claims(&response.access_token);

    // Prefer the token's own `exp`; fall back to `expires_in` if it cannot be
    // decoded, and finally to a conservative one minute.
    let expires_at = claims
        .as_ref()
        .map(|c| c.exp)
        .or_else(|| {
            response
                .expires_in
                .map(|s| chrono::Utc::now().timestamp() + s)
        })
        .unwrap_or_else(|| chrono::Utc::now().timestamp() + 60);

    Tokens {
        access_token: response.access_token,
        refresh_token: response.refresh_token,
        expires_at,
        username: claims.and_then(|c| c.preferred_username),
    }
}

/// Decode the JWT payload without verifying it. Display only — the server does
/// the real verification, and nothing here is trusted for authorization.
fn display_claims(access_token: &str) -> Option<DisplayClaims> {
    use base64::Engine as _;

    let payload = access_token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;

    serde_json::from_slice(&decoded).ok()
}

async fn oauth_error(response: reqwest::Response) -> Error {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();

    match serde_json::from_str::<ErrorResponse>(&body) {
        Ok(e) => Error::Oauth {
            code: e.error,
            description: e.error_description,
        },
        Err(_) => Error::Oauth {
            code: format!("HTTP {status}"),
            description: (!body.is_empty()).then_some(body),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

    fn token_with_payload(payload: &serde_json::Value) -> String {
        let encode = |v: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v);
        format!(
            "{}.{}.{}",
            encode(br#"{"alg":"RS256","typ":"at+jwt","kid":"main"}"#),
            encode(payload.to_string().as_bytes()),
            encode(b"not-a-real-signature"),
        )
    }

    #[test]
    fn expiry_comes_from_the_token_not_expires_in() {
        let access_token = token_with_payload(&serde_json::json!({
            "exp": 1788336345i64,
            "preferred_username": "neale",
        }));

        let tokens = into_tokens(TokenResponse {
            access_token,
            refresh_token: Some("r".to_owned()),
            // Deliberately inconsistent: the token's own `exp` wins.
            expires_in: Some(1),
        });

        assert_eq!(tokens.expires_at, 1788336345);
        assert_eq!(tokens.username.as_deref(), Some("neale"));
    }

    #[test]
    fn opaque_access_token_falls_back_to_expires_in() {
        let before = chrono::Utc::now().timestamp();

        let tokens = into_tokens(TokenResponse {
            access_token: "not-a-jwt".to_owned(),
            refresh_token: None,
            expires_in: Some(3600),
        });

        assert!(tokens.expires_at >= before + 3600);
        assert_eq!(tokens.username, None);
    }

    #[test]
    fn provider_builds_the_documented_endpoints() {
        let provider = Provider {
            issuer: DEFAULT_ISSUER.to_owned(),
            client_id: DEFAULT_CLIENT_ID.to_owned(),
        };

        assert_eq!(
            provider.device_authorization_url(),
            "https://auth.sw1nn.net/api/oidc/device-authorization"
        );
        assert_eq!(
            provider.token_url(),
            "https://auth.sw1nn.net/api/oidc/token"
        );
    }
}
