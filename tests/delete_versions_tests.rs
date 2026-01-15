use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;
use sha2::Digest;
use tower::Service;
use tower::util::ServiceExt;

mod common;
use common::{create_test_package, setup_test_app};
use sw1nn_pkg_repo::models::Package;

/// Helper to upload a package to the test repo
async fn upload_test_package(
    app: &mut axum::Router,
    pkgname: &str,
    pkgver: &str,
    arch: &str,
) -> Package {
    let package_data = create_test_package(pkgname, pkgver, arch);
    let sha256 = format!("{:x}", sha2::Sha256::digest(&package_data));

    // Initiate upload
    let init_body = serde_json::json!({
        "filename": format!("{}-{}-{}.pkg.tar.zst", pkgname, pkgver, arch),
        "size": package_data.len(),
        "sha256": sha256,
        "chunk_size": package_data.len(), // Single chunk
        "has_signature": false
    });

    let response = app
        .as_service()
        .ready()
        .await
        .unwrap()
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/packages/upload/initiate")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&init_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let init_response: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let upload_id = init_response["upload_id"].as_str().unwrap();

    // Upload chunk
    let response = app
        .as_service()
        .ready()
        .await
        .unwrap()
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/api/packages/upload/{}/chunks/1", upload_id))
                .header("Content-Type", "application/octet-stream")
                .body(Body::from(package_data))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let chunk_response: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let checksum = chunk_response["checksum"].as_str().unwrap();

    // Complete upload
    let complete_body = serde_json::json!({
        "chunks": [
            {
                "chunk_number": 1,
                "checksum": checksum
            }
        ]
    });

    let response = app
        .as_service()
        .ready()
        .await
        .unwrap()
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/api/packages/upload/{}/complete", upload_id))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&complete_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn test_delete_specific_version() {
    let mut app = setup_test_app().await;

    // Upload two versions
    upload_test_package(&mut app, "test-pkg", "1.0.0-1", "x86_64").await;
    upload_test_package(&mut app, "test-pkg", "1.1.0-1", "x86_64").await;

    // Delete specific version
    let delete_body = json!({"versions": ["1.0.0-1"]});
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages/test-pkg/versions/delete")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&delete_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // Verify response body
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(response_json["deleted_count"], 1);
    assert_eq!(response_json["deleted_versions"][0], "1.0.0-1");

    // Verify only 1.1.0-1 remains
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/packages?name=test-pkg")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let packages: Vec<Package> = serde_json::from_slice(&body).unwrap();

    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].version, "1.1.0-1");
}

#[tokio::test]
async fn test_delete_multiple_versions() {
    let mut app = setup_test_app().await;

    // Upload three versions
    upload_test_package(&mut app, "test-pkg", "1.0.0-1", "x86_64").await;
    upload_test_package(&mut app, "test-pkg", "1.1.0-1", "x86_64").await;
    upload_test_package(&mut app, "test-pkg", "1.2.0-1", "x86_64").await;

    // Delete two specific versions
    let delete_body = json!({"versions": ["1.0.0-1", "1.1.0-1"]});
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages/test-pkg/versions/delete")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&delete_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // Verify response body
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(response_json["deleted_count"], 2);

    // Verify only 1.2.0-1 remains
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/packages?name=test-pkg")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let packages: Vec<Package> = serde_json::from_slice(&body).unwrap();

    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].version, "1.2.0-1");
}

#[tokio::test]
async fn test_delete_semver_range() {
    let mut app = setup_test_app().await;

    // Upload versions 1.x and 2.x
    upload_test_package(&mut app, "test-pkg", "1.0.0-1", "x86_64").await;
    upload_test_package(&mut app, "test-pkg", "1.1.0-1", "x86_64").await;
    upload_test_package(&mut app, "test-pkg", "2.0.0-1", "x86_64").await;

    // Delete all 1.x versions using semver range
    let delete_body = json!({"versions": ["^1.0.0"]});
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages/test-pkg/versions/delete")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&delete_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // Verify response body
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(response_json["deleted_count"], 2);

    // Verify only 2.0.0-1 remains
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/packages?name=test-pkg")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let packages: Vec<Package> = serde_json::from_slice(&body).unwrap();

    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].version, "2.0.0-1");
}

#[tokio::test]
async fn test_delete_mixed_exact_and_range() {
    let mut app = setup_test_app().await;

    // Upload various versions
    upload_test_package(&mut app, "test-pkg", "1.0.0-1", "x86_64").await;
    upload_test_package(&mut app, "test-pkg", "1.1.0-1", "x86_64").await;
    upload_test_package(&mut app, "test-pkg", "2.0.0-1", "x86_64").await;
    upload_test_package(&mut app, "test-pkg", "2.1.0-1", "x86_64").await;

    // Delete 1.0.0-1 (exact) and all 2.x (range)
    let delete_body = json!({"versions": ["1.0.0-1", "^2.0.0"]});
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages/test-pkg/versions/delete")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&delete_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // Verify response body
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(response_json["deleted_count"], 3);

    // Verify only 1.1.0-1 remains
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/packages?name=test-pkg")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let packages: Vec<Package> = serde_json::from_slice(&body).unwrap();

    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].version, "1.1.0-1");
}

#[tokio::test]
async fn test_delete_nonexistent_package() {
    let app = setup_test_app().await;

    let delete_body = json!({"versions": ["1.0.0-1"]});
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages/nonexistent/versions/delete")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&delete_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_delete_nonexistent_version() {
    let mut app = setup_test_app().await;

    // Upload one version
    upload_test_package(&mut app, "test-pkg", "1.0.0-1", "x86_64").await;

    // Try to delete a version that doesn't exist
    let delete_body = json!({"versions": ["9.9.9-1"]});
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages/test-pkg/versions/delete")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&delete_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
