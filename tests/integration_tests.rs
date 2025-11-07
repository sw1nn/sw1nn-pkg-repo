use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::util::ServiceExt;

mod common;
use common::{create_test_package, setup_test_app};

#[tokio::test]
async fn test_server_starts_and_routes_registered() {
    let app = setup_test_app().await;

    // Test that API docs endpoint exists
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api-docs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_list_packages_empty() {
    let app = setup_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/packages")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let packages: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
    assert_eq!(packages.len(), 0);
}

#[tokio::test]
async fn test_serve_file_not_found() {
    let app = setup_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/custom/os/x86_64/nonexistent.pkg.tar.zst")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_delete_package_not_found() {
    let app = setup_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/packages/nonexistent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_upload_duplicate_package_returns_409() {
    let app = setup_test_app().await;

    // Create a test package
    let package_data = create_test_package("test-pkg", "1.0.0", "x86_64");

    // Create multipart form data manually with proper binary handling
    let boundary = "------------------------boundary123456789";
    let mut body = Vec::new();

    // Start boundary
    body.extend_from_slice(b"--");
    body.extend_from_slice(boundary.as_bytes());
    body.extend_from_slice(b"\r\n");

    // Content disposition and type
    body.extend_from_slice(br#"Content-Disposition: form-data; name="file"; filename="test-pkg-1.0.0-x86_64.pkg.tar.zst""#);
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");

    // Binary data
    body.extend_from_slice(&package_data);

    // End boundary
    body.extend_from_slice(b"\r\n--");
    body.extend_from_slice(boundary.as_bytes());
    body.extend_from_slice(b"--\r\n");

    // First upload should succeed
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages")
                .header(
                    "Content-Type",
                    format!("multipart/form-data; boundary={}", boundary),
                )
                .body(Body::from(body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);

    // Second upload of the same package should return 409 Conflict
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages")
                .header(
                    "Content-Type",
                    format!("multipart/form-data; boundary={}", boundary),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);

    // Verify the error message
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let error_response: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        error_response["error"]
            .as_str()
            .unwrap()
            .contains("Package already exists")
    );
}

#[tokio::test]
async fn test_upload_payload_too_large() {
    let app = setup_test_app().await;

    // Create a test package
    let package_data = create_test_package("test-pkg", "1.0.0", "x86_64");

    // Create multipart form data
    let boundary = "------------------------boundary123456789";
    let mut body = Vec::new();

    // Start boundary
    body.extend_from_slice(b"--");
    body.extend_from_slice(boundary.as_bytes());
    body.extend_from_slice(b"\r\n");

    // Content disposition and type
    body.extend_from_slice(br#"Content-Disposition: form-data; name="file"; filename="test-pkg-1.0.0-x86_64.pkg.tar.zst""#);
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");

    // Binary data
    body.extend_from_slice(&package_data);

    // End boundary
    body.extend_from_slice(b"\r\n--");
    body.extend_from_slice(boundary.as_bytes());
    body.extend_from_slice(b"--\r\n");

    // Set Content-Length to exceed max_payload_size (default is 512 MiB)
    let fake_content_length = (1024 * 1024 * 1024).to_string(); // 1 GiB

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages")
                .header(
                    "Content-Type",
                    format!("multipart/form-data; boundary={}", boundary),
                )
                .header("Content-Length", fake_content_length)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

    // Verify the error message
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let error_response: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        error_response["error"]
            .as_str()
            .unwrap()
            .contains("Payload too large")
    );
    assert!(
        error_response["error"]
            .as_str()
            .unwrap()
            .contains("exceeds maximum allowed size")
    );
}

#[tokio::test]
async fn test_upload_invalid_filename_extension() {
    let app = setup_test_app().await;

    // Create a test package
    let package_data = create_test_package("test-pkg", "1.0.0", "x86_64");

    // Create multipart form data with wrong extension
    let boundary = "------------------------boundary123456789";
    let mut body = Vec::new();

    // Start boundary
    body.extend_from_slice(b"--");
    body.extend_from_slice(boundary.as_bytes());
    body.extend_from_slice(b"\r\n");

    // Content disposition with .tar.gz extension instead of .pkg.tar.zst
    body.extend_from_slice(br#"Content-Disposition: form-data; name="file"; filename="test-pkg-1.0.0-x86_64.tar.gz""#);
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");

    // Binary data
    body.extend_from_slice(&package_data);

    // End boundary
    body.extend_from_slice(b"\r\n--");
    body.extend_from_slice(boundary.as_bytes());
    body.extend_from_slice(b"--\r\n");

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages")
                .header(
                    "Content-Type",
                    format!("multipart/form-data; boundary={}", boundary),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // Verify the error message
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let error_response: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        error_response["error"]
            .as_str()
            .unwrap()
            .contains("Invalid file extension")
    );
    assert!(
        error_response["error"]
            .as_str()
            .unwrap()
            .contains(".pkg.tar.zst")
    );
}

#[tokio::test]
async fn test_path_traversal_in_repo_name() {
    let app = setup_test_app().await;
    let package_data = create_test_package("test-pkg", "1.0.0", "x86_64");

    // Create multipart form data with path traversal in filename
    let boundary = "------------------------boundary123456789";
    let mut body = Vec::new();

    body.extend_from_slice(b"--");
    body.extend_from_slice(boundary.as_bytes());
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(
        br#"Content-Disposition: form-data; name="file"; filename="test-pkg-1.0.0-x86_64.pkg.tar.zst""#,
    );
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(&package_data);
    body.extend_from_slice(b"\r\n--");
    body.extend_from_slice(boundary.as_bytes());
    body.extend_from_slice(b"\r\n");

    // Add repo field with path traversal
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"repo\"\r\n\r\n");
    body.extend_from_slice(b"../../../tmp");
    body.extend_from_slice(b"\r\n--");
    body.extend_from_slice(boundary.as_bytes());
    body.extend_from_slice(b"--\r\n");

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages")
                .header(
                    "Content-Type",
                    format!("multipart/form-data; boundary={}", boundary),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    // Should reject with 400 Bad Request
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let error_response: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(error_response["error"]
        .as_str()
        .unwrap()
        .contains("Path component cannot contain path separators"));
}

#[tokio::test]
async fn test_path_validation_unit() {
    // This is a unit test to verify our path validation logic works correctly
    // We test the Storage layer directly
    use sw1nn_pkg_repo::storage::Storage;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let storage = Storage::new(temp_dir.path());

    // Test that path traversal in repo name is rejected
    let result = storage.package_path("../../../etc", "x86_64", "test.pkg.tar.zst");
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Path component cannot contain path separators"));

    // Test that path traversal in arch is rejected
    let result = storage.package_path("myrepo", "../etc", "test.pkg.tar.zst");
    assert!(result.is_err());

    // Test that ".." is rejected
    let result = storage.package_path("..", "x86_64", "test.pkg.tar.zst");
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Invalid path component"));

    // Test that "." is rejected
    let result = storage.package_path(".", "x86_64", "test.pkg.tar.zst");
    assert!(result.is_err());

    // Test that valid paths work
    let result = storage.package_path("myrepo", "x86_64", "test.pkg.tar.zst");
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_path_traversal_with_dots_in_serve() {
    let app = setup_test_app().await;

    // Try to access a file using path traversal with ".." in path components
    // Note: We can't use ".." directly in the URL path as HTTP clients normalize it
    // But we can try repo name with dots
    let response = app
        .oneshot(
            Request::builder()
                .uri("/../os/x86_64/somefile.db")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Axum normalizes the path, but our validation should catch ".." in repo/arch names
    // This actually results in 404 because axum resolves the path before routing
    // The real protection is in our validate_path_component function
    assert!(response.status() == StatusCode::NOT_FOUND || response.status() == StatusCode::BAD_REQUEST);
}
