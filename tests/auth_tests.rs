use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;
use tower::util::ServiceExt;

mod common;
use common::{setup_test_app, setup_test_app_with_auth};

const TEST_JWT_SECRET: &str = "test-secret-that-is-at-least-32-characters-long!!";

fn test_auth_config() -> sw1nn_pkg_repo::config::AuthConfig {
    sw1nn_pkg_repo::config::AuthConfig {
        github_client_id: "test-client-id".to_string(),
        allowed_users: vec!["testuser".to_string()],
        jwt_secret: TEST_JWT_SECRET.to_string(),
        jwt_expiration_secs: 3600,
    }
}

fn create_test_token(username: &str) -> String {
    let auth = test_auth_config();
    sw1nn_pkg_repo::auth::create_jwt(&auth, username, "admin").unwrap()
}

// -- Tests with auth disabled (backward compatibility) --

#[tokio::test]
async fn test_write_endpoints_work_without_auth_config() -> Result<(), Box<dyn std::error::Error>> {
    let app = setup_test_app().await;

    // Initiate upload should succeed without auth header when auth is not configured
    let request_body = json!({
        "filename": "test-pkg-1.0.0-x86_64.pkg.tar.zst",
        "size": 1048576,
        "chunk_size": 1048576,
        "has_signature": false
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages/upload/initiate")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&request_body)?))?,
        )
        .await?;

    assert_eq!(response.status(), StatusCode::CREATED);
    Ok(())
}

#[tokio::test]
async fn test_read_endpoints_work_without_auth() -> Result<(), Box<dyn std::error::Error>> {
    let app = setup_test_app_with_auth(test_auth_config()).await;

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/packages")
                .body(Body::empty())?,
        )
        .await?;

    assert_eq!(response.status(), StatusCode::OK);
    Ok(())
}

// -- Tests with auth enabled --

#[tokio::test]
async fn test_write_endpoint_requires_auth_when_configured()
-> Result<(), Box<dyn std::error::Error>> {
    let app = setup_test_app_with_auth(test_auth_config()).await;

    let request_body = json!({
        "filename": "test-pkg-1.0.0-x86_64.pkg.tar.zst",
        "size": 1048576,
        "chunk_size": 1048576,
        "has_signature": false
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages/upload/initiate")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&request_body)?))?,
        )
        .await?;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn test_write_endpoint_rejects_invalid_token() -> Result<(), Box<dyn std::error::Error>> {
    let app = setup_test_app_with_auth(test_auth_config()).await;

    let request_body = json!({
        "filename": "test-pkg-1.0.0-x86_64.pkg.tar.zst",
        "size": 1048576,
        "chunk_size": 1048576,
        "has_signature": false
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages/upload/initiate")
                .header("Content-Type", "application/json")
                .header("Authorization", "Bearer invalid-token")
                .body(Body::from(serde_json::to_vec(&request_body)?))?,
        )
        .await?;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn test_write_endpoint_rejects_non_allowlisted_user() -> Result<(), Box<dyn std::error::Error>>
{
    let auth = test_auth_config();
    let app = setup_test_app_with_auth(auth.clone()).await;

    // Create token for a user not in the allowlist
    let token = sw1nn_pkg_repo::auth::create_jwt(&auth, "eviluser", "admin")?;

    let request_body = json!({
        "filename": "test-pkg-1.0.0-x86_64.pkg.tar.zst",
        "size": 1048576,
        "chunk_size": 1048576,
        "has_signature": false
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages/upload/initiate")
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::from(serde_json::to_vec(&request_body)?))?,
        )
        .await?;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    Ok(())
}

#[tokio::test]
async fn test_write_endpoint_succeeds_with_valid_token() -> Result<(), Box<dyn std::error::Error>> {
    let app = setup_test_app_with_auth(test_auth_config()).await;
    let token = create_test_token("testuser");

    let request_body = json!({
        "filename": "test-pkg-1.0.0-x86_64.pkg.tar.zst",
        "size": 1048576,
        "chunk_size": 1048576,
        "has_signature": false
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages/upload/initiate")
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::from(serde_json::to_vec(&request_body)?))?,
        )
        .await?;

    assert_eq!(response.status(), StatusCode::CREATED);
    Ok(())
}

#[tokio::test]
async fn test_delete_endpoint_requires_auth() -> Result<(), Box<dyn std::error::Error>> {
    let app = setup_test_app_with_auth(test_auth_config()).await;

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/packages/nonexistent")
                .body(Body::empty())?,
        )
        .await?;

    // Should get 401, not 404 — auth checked before route logic
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn test_rebuild_endpoint_requires_auth() -> Result<(), Box<dyn std::error::Error>> {
    let app = setup_test_app_with_auth(test_auth_config()).await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/repos/sw1nn/os/x86_64/rebuild")
                .body(Body::empty())?,
        )
        .await?;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    Ok(())
}

// -- JWT unit tests --

#[test]
fn test_create_and_validate_jwt() -> Result<(), Box<dyn std::error::Error>> {
    let auth = test_auth_config();
    let token = sw1nn_pkg_repo::auth::create_jwt(&auth, "testuser", "admin")?;
    let claims = sw1nn_pkg_repo::auth::validate_jwt(&auth, &token)?;

    assert_eq!(claims.sub, "testuser");
    assert_eq!(claims.token_type, "admin");
    assert_eq!(claims.iss, "sw1nn-pkg-repo");
    assert!(claims.exp > claims.iat);
    Ok(())
}

#[test]
fn test_validate_jwt_with_wrong_secret() {
    let auth = test_auth_config();
    let token = sw1nn_pkg_repo::auth::create_jwt(&auth, "testuser", "admin").unwrap();

    let mut wrong_auth = auth;
    wrong_auth.jwt_secret = "a-different-secret-that-is-also-at-least-32-chars!!".to_string();

    let result = sw1nn_pkg_repo::auth::validate_jwt(&wrong_auth, &token);
    assert!(result.is_err());
}
