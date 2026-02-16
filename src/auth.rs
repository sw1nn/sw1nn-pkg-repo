use crate::api::AppState;
use crate::config::AuthConfig;
use crate::error::Error;
use async_trait::async_trait;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// JWT claims
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// Subject (GitHub username)
    pub sub: String,
    /// Issued at (unix timestamp)
    pub iat: i64,
    /// Expiration (unix timestamp)
    pub exp: i64,
    /// Issuer
    pub iss: String,
    /// Token type: "user" for interactive login, "admin" for generated tokens
    pub token_type: String,
}

/// Authenticated user extracted from JWT
#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub username: String,
    pub token_type: String,
}

const ISSUER: &str = "sw1nn-pkg-repo";

/// Create a JWT for the given username
pub fn create_jwt(
    auth_config: &AuthConfig,
    username: &str,
    token_type: &str,
) -> Result<String, Error> {
    let now = chrono::Utc::now().timestamp();
    let claims = Claims {
        sub: username.to_string(),
        iat: now,
        exp: now + auth_config.jwt_expiration_secs,
        iss: ISSUER.to_string(),
        token_type: token_type.to_string(),
    };

    jsonwebtoken::encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(auth_config.jwt_secret.as_bytes()),
    )
    .map_err(|e| Error::Jwt {
        msg: format!("failed to create token: {e}"),
    })
}

/// Validate a JWT and return the claims
pub fn validate_jwt(auth_config: &AuthConfig, token: &str) -> Result<Claims, Error> {
    let mut validation = Validation::default();
    validation.set_issuer(&[ISSUER]);

    jsonwebtoken::decode::<Claims>(
        token,
        &DecodingKey::from_secret(auth_config.jwt_secret.as_bytes()),
        &validation,
    )
    .map(|data| data.claims)
    .map_err(|e| Error::Jwt {
        msg: format!("invalid token: {e}"),
    })
}

// -- GitHub Device Flow --

#[derive(Debug, Serialize, Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GitHubAccessToken {
    pub access_token: String,
    pub token_type: String,
    pub scope: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GitHubUser {
    pub login: String,
}

/// Request a device code from GitHub
pub async fn request_device_code(
    http_client: &reqwest::Client,
    client_id: &str,
) -> Result<DeviceCodeResponse, Error> {
    let response = http_client
        .post("https://github.com/login/device/code")
        .header("Accept", "application/json")
        .form(&[("client_id", client_id), ("scope", "read:user")])
        .send()
        .await
        .map_err(|e| Error::GitHubApi {
            msg: format!("failed to request device code: {e}"),
        })?;

    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(Error::GitHubApi {
            msg: format!("device code request failed: {body}"),
        });
    }

    response
        .json::<DeviceCodeResponse>()
        .await
        .map_err(|e| Error::GitHubApi {
            msg: format!("failed to parse device code response: {e}"),
        })
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PollResponse {
    Success(GitHubAccessToken),
    Error { error: String },
}

/// Poll GitHub for the access token (returns Ok(None) if still pending)
pub async fn poll_device_token(
    http_client: &reqwest::Client,
    client_id: &str,
    device_code: &str,
) -> Result<Option<GitHubAccessToken>, Error> {
    let response = http_client
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .form(&[
            ("client_id", client_id),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .await
        .map_err(|e| Error::GitHubApi {
            msg: format!("failed to poll for token: {e}"),
        })?;

    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(Error::GitHubApi {
            msg: format!("token poll failed: {body}"),
        });
    }

    let poll_response: PollResponse = response.json().await.map_err(|e| Error::GitHubApi {
        msg: format!("failed to parse poll response: {e}"),
    })?;

    match poll_response {
        PollResponse::Success(token) => Ok(Some(token)),
        PollResponse::Error { error } => match error.as_str() {
            "authorization_pending" | "slow_down" => Ok(None),
            "expired_token" => Err(Error::GitHubApi {
                msg: "device code expired, please restart login".to_string(),
            }),
            "access_denied" => Err(Error::Forbidden {
                reason: "user denied the authorization request".to_string(),
            }),
            _ => Err(Error::GitHubApi {
                msg: format!("unexpected error from GitHub: {error}"),
            }),
        },
    }
}

/// Get the authenticated GitHub user's login name
pub async fn get_github_user(
    http_client: &reqwest::Client,
    access_token: &str,
) -> Result<GitHubUser, Error> {
    let response = http_client
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {access_token}"))
        .header("User-Agent", "sw1nn-pkg-repo")
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| Error::GitHubApi {
            msg: format!("failed to get GitHub user: {e}"),
        })?;

    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(Error::GitHubApi {
            msg: format!("GitHub user API returned error: {body}"),
        });
    }

    response
        .json::<GitHubUser>()
        .await
        .map_err(|e| Error::GitHubApi {
            msg: format!("failed to parse GitHub user response: {e}"),
        })
}

// -- Axum Extractor --

#[async_trait]
impl FromRequestParts<Arc<AppState>> for AuthenticatedUser {
    type Rejection = Error;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let auth_config = match &state.config.auth {
            Some(config) => config,
            // Auth not configured — allow all requests through
            None => {
                return Ok(AuthenticatedUser {
                    username: "<anonymous>".to_string(),
                    token_type: "none".to_string(),
                });
            }
        };

        let auth_header = parts
            .headers
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or(Error::Unauthorized)?;

        let token = auth_header
            .strip_prefix("Bearer ")
            .ok_or(Error::Unauthorized)?;

        let claims = validate_jwt(auth_config, token)?;

        // Check allowlist
        if !auth_config.allowed_users.iter().any(|u| u == &claims.sub) {
            return Err(Error::Forbidden {
                reason: format!("user '{}' is not in the allowed users list", claims.sub),
            });
        }

        Ok(AuthenticatedUser {
            username: claims.sub,
            token_type: claims.token_type,
        })
    }
}
