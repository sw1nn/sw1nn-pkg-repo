use crate::api::AppState;
use crate::auth;
use crate::error::Error;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct DeviceCodeRequest {
    // No fields needed; the server knows the client_id from config
}

#[derive(Debug, Serialize)]
pub struct DeviceCodeApiResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Deserialize)]
pub struct DeviceTokenRequest {
    pub device_code: String,
}

#[derive(Debug, Serialize)]
pub struct DeviceTokenApiResponse {
    pub token: String,
    pub username: String,
    pub expires_at: i64,
}

/// Request a GitHub device code for authentication
pub async fn device_code(State(state): State<Arc<AppState>>) -> Result<impl IntoResponse, Error> {
    let auth_config = state.config.auth.as_ref().ok_or(Error::AuthNotConfigured)?;

    let response =
        auth::request_device_code(&state.http_client, &auth_config.github_client_id).await?;

    Ok(Json(DeviceCodeApiResponse {
        device_code: response.device_code,
        user_code: response.user_code,
        verification_uri: response.verification_uri,
        expires_in: response.expires_in,
        interval: response.interval,
    }))
}

/// Poll for device token — returns 202 if pending, 200 with JWT on success
pub async fn device_token(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DeviceTokenRequest>,
) -> Result<impl IntoResponse, Error> {
    let auth_config = state.config.auth.as_ref().ok_or(Error::AuthNotConfigured)?;

    let github_token = auth::poll_device_token(
        &state.http_client,
        &auth_config.github_client_id,
        &req.device_code,
    )
    .await?;

    let github_token = match github_token {
        Some(token) => token,
        None => {
            return Ok((
                StatusCode::ACCEPTED,
                Json(serde_json::json!({"status": "pending"})),
            )
                .into_response());
        }
    };

    // Get GitHub username
    let github_user = auth::get_github_user(&state.http_client, &github_token.access_token).await?;

    // Check allowlist
    if !auth_config
        .allowed_users
        .iter()
        .any(|u| u == &github_user.login)
    {
        return Err(Error::Forbidden {
            reason: format!(
                "user '{}' is not in the allowed users list",
                github_user.login
            ),
        });
    }

    // Issue JWT
    let jwt = auth::create_jwt(auth_config, &github_user.login, "user")?;
    let claims = auth::validate_jwt(auth_config, &jwt)?;

    Ok(Json(DeviceTokenApiResponse {
        token: jwt,
        username: github_user.login,
        expires_at: claims.exp,
    })
    .into_response())
}
