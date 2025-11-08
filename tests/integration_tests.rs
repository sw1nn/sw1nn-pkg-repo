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

    // Verify the error message contains size information (safe to expose)
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let error_response: serde_json::Value = serde_json::from_slice(&body).unwrap();
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
    // Error message is now sanitized to avoid leaking internal implementation details
    assert!(error_response["error"]
        .as_str()
        .unwrap()
        .contains("Invalid request parameters"));
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
async fn test_concurrent_uploads_same_package() {
    // Both requests need to share the same storage to test TOCTOU properly
    let app = setup_test_app().await;
    let package_data = create_test_package("concurrent-test", "1.0.0", "x86_64");

    // Create multipart form data
    let boundary = "------------------------boundary123456789";
    let mut body = Vec::new();

    body.extend_from_slice(b"--");
    body.extend_from_slice(boundary.as_bytes());
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(
        br#"Content-Disposition: form-data; name="file"; filename="concurrent-test-1.0.0-x86_64.pkg.tar.zst""#,
    );
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(&package_data);
    body.extend_from_slice(b"\r\n--");
    body.extend_from_slice(boundary.as_bytes());
    body.extend_from_slice(b"--\r\n");

    // Attempt concurrent uploads of the same package
    let body1 = body.clone();
    let body2 = body.clone();

    // Clone the app to simulate concurrent requests to same server
    let app1 = app.clone();
    let app2 = app;

    let (result1, result2) = tokio::join!(
        app1.oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages")
                .header(
                    "Content-Type",
                    format!("multipart/form-data; boundary={}", boundary),
                )
                .body(Body::from(body1))
                .unwrap(),
        ),
        app2.oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages")
                .header(
                    "Content-Type",
                    format!("multipart/form-data; boundary={}", boundary),
                )
                .body(Body::from(body2))
                .unwrap(),
        )
    );

    let result1 = result1.unwrap();
    let result2 = result2.unwrap();

    // Exactly one should succeed (201), one should fail with 409 (Conflict)
    let statuses = [result1.status(), result2.status()];
    assert!(
        statuses.contains(&StatusCode::CREATED),
        "One request should succeed with 201 CREATED"
    );
    assert!(
        statuses.contains(&StatusCode::CONFLICT),
        "One request should fail with 409 CONFLICT"
    );

    // Verify the error message on the failed request
    let failed_response = if result1.status() == StatusCode::CONFLICT {
        result1
    } else {
        result2
    };

    let body = axum::body::to_bytes(failed_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let error_response: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(error_response["error"]
        .as_str()
        .unwrap()
        .contains("Package already exists"));
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

#[tokio::test]
async fn test_list_all_packages_no_query_params() {
    let app = setup_test_app().await;

    // Upload packages to different repos and arches
    let pkg1 = create_test_package("pkg1", "1.0.0", "x86_64");
    let pkg2 = create_test_package("pkg2", "2.0.0", "aarch64");
    let pkg3 = create_test_package("pkg3", "3.0.0", "x86_64");

    let boundary = "------------------------boundary123456789";

    // Upload to default repo (sw1nn) x86_64
    let mut body1 = Vec::new();
    body1.extend_from_slice(b"--");
    body1.extend_from_slice(boundary.as_bytes());
    body1.extend_from_slice(b"\r\n");
    body1.extend_from_slice(br#"Content-Disposition: form-data; name="file"; filename="pkg1-1.0.0-x86_64.pkg.tar.zst""#);
    body1.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body1.extend_from_slice(&pkg1);
    body1.extend_from_slice(b"\r\n--");
    body1.extend_from_slice(boundary.as_bytes());
    body1.extend_from_slice(b"--\r\n");

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages")
                .header("Content-Type", format!("multipart/form-data; boundary={}", boundary))
                .body(Body::from(body1))
                .unwrap(),
        )
        .await
        .unwrap();

    // Upload to default repo (sw1nn) aarch64
    let mut body2 = Vec::new();
    body2.extend_from_slice(b"--");
    body2.extend_from_slice(boundary.as_bytes());
    body2.extend_from_slice(b"\r\n");
    body2.extend_from_slice(br#"Content-Disposition: form-data; name="file"; filename="pkg2-2.0.0-aarch64.pkg.tar.zst""#);
    body2.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body2.extend_from_slice(&pkg2);
    body2.extend_from_slice(b"\r\n--");
    body2.extend_from_slice(boundary.as_bytes());
    body2.extend_from_slice(b"--\r\n");

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages")
                .header("Content-Type", format!("multipart/form-data; boundary={}", boundary))
                .body(Body::from(body2))
                .unwrap(),
        )
        .await
        .unwrap();

    // Upload to custom repo x86_64
    let mut body3 = Vec::new();
    body3.extend_from_slice(b"--");
    body3.extend_from_slice(boundary.as_bytes());
    body3.extend_from_slice(b"\r\n");
    body3.extend_from_slice(br#"Content-Disposition: form-data; name="file"; filename="pkg3-3.0.0-x86_64.pkg.tar.zst""#);
    body3.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body3.extend_from_slice(&pkg3);
    body3.extend_from_slice(b"\r\n--");
    body3.extend_from_slice(boundary.as_bytes());
    body3.extend_from_slice(b"\r\n");
    body3.extend_from_slice(b"Content-Disposition: form-data; name=\"repo\"\r\n\r\n");
    body3.extend_from_slice(b"custom");
    body3.extend_from_slice(b"\r\n--");
    body3.extend_from_slice(boundary.as_bytes());
    body3.extend_from_slice(b"--\r\n");

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages")
                .header("Content-Type", format!("multipart/form-data; boundary={}", boundary))
                .body(Body::from(body3))
                .unwrap(),
        )
        .await
        .unwrap();

    // List all packages with no query params
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

    // Should return all 3 packages
    assert_eq!(packages.len(), 3);

    // Verify we have all the packages
    let names: Vec<&str> = packages.iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"pkg1"));
    assert!(names.contains(&"pkg2"));
    assert!(names.contains(&"pkg3"));
}

#[tokio::test]
async fn test_list_packages_filter_by_name() {
    let app = setup_test_app().await;

    // Upload test packages
    let pkg1 = create_test_package("test-foo", "1.0.0", "x86_64");
    let pkg2 = create_test_package("test-bar", "2.0.0", "x86_64");

    let boundary = "------------------------boundary123456789";

    for (pkg_data, name) in [(pkg1, "test-foo"), (pkg2, "test-bar")] {
        let mut body = Vec::new();
        body.extend_from_slice(b"--");
        body.extend_from_slice(boundary.as_bytes());
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(format!(r#"Content-Disposition: form-data; name="file"; filename="{}-1.0.0-x86_64.pkg.tar.zst""#, name).as_bytes());
        body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
        body.extend_from_slice(&pkg_data);
        body.extend_from_slice(b"\r\n--");
        body.extend_from_slice(boundary.as_bytes());
        body.extend_from_slice(b"--\r\n");

        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/packages")
                    .header("Content-Type", format!("multipart/form-data; boundary={}", boundary))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
    }

    // Filter by name containing "foo"
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/packages?name=foo")
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

    // Should only return test-foo
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0]["name"].as_str().unwrap(), "test-foo");
}

#[tokio::test]
async fn test_list_packages_filter_by_repo() {
    let app = setup_test_app().await;

    // Upload to different repos
    let pkg1 = create_test_package("pkg1", "1.0.0", "x86_64");
    let pkg2 = create_test_package("pkg2", "2.0.0", "x86_64");

    let boundary = "------------------------boundary123456789";

    // Upload to default repo
    let mut body1 = Vec::new();
    body1.extend_from_slice(b"--");
    body1.extend_from_slice(boundary.as_bytes());
    body1.extend_from_slice(b"\r\n");
    body1.extend_from_slice(br#"Content-Disposition: form-data; name="file"; filename="pkg1-1.0.0-x86_64.pkg.tar.zst""#);
    body1.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body1.extend_from_slice(&pkg1);
    body1.extend_from_slice(b"\r\n--");
    body1.extend_from_slice(boundary.as_bytes());
    body1.extend_from_slice(b"--\r\n");

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages")
                .header("Content-Type", format!("multipart/form-data; boundary={}", boundary))
                .body(Body::from(body1))
                .unwrap(),
        )
        .await
        .unwrap();

    // Upload to custom repo
    let mut body2 = Vec::new();
    body2.extend_from_slice(b"--");
    body2.extend_from_slice(boundary.as_bytes());
    body2.extend_from_slice(b"\r\n");
    body2.extend_from_slice(br#"Content-Disposition: form-data; name="file"; filename="pkg2-2.0.0-x86_64.pkg.tar.zst""#);
    body2.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body2.extend_from_slice(&pkg2);
    body2.extend_from_slice(b"\r\n--");
    body2.extend_from_slice(boundary.as_bytes());
    body2.extend_from_slice(b"\r\n");
    body2.extend_from_slice(b"Content-Disposition: form-data; name=\"repo\"\r\n\r\n");
    body2.extend_from_slice(b"testing");
    body2.extend_from_slice(b"\r\n--");
    body2.extend_from_slice(boundary.as_bytes());
    body2.extend_from_slice(b"--\r\n");

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages")
                .header("Content-Type", format!("multipart/form-data; boundary={}", boundary))
                .body(Body::from(body2))
                .unwrap(),
        )
        .await
        .unwrap();

    // Filter by repo=testing
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/packages?repo=testing")
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

    // Should only return pkg2 from testing repo
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0]["name"].as_str().unwrap(), "pkg2");
    assert_eq!(packages[0]["repo"].as_str().unwrap(), "testing");
}

#[tokio::test]
async fn test_list_packages_filter_by_arch() {
    let app = setup_test_app().await;

    // Upload packages with different architectures
    let pkg1 = create_test_package("pkg1", "1.0.0", "x86_64");
    let pkg2 = create_test_package("pkg2", "2.0.0", "aarch64");

    let boundary = "------------------------boundary123456789";

    for (pkg_data, name, arch) in [(pkg1, "pkg1", "x86_64"), (pkg2, "pkg2", "aarch64")] {
        let mut body = Vec::new();
        body.extend_from_slice(b"--");
        body.extend_from_slice(boundary.as_bytes());
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(format!(r#"Content-Disposition: form-data; name="file"; filename="{}-1.0.0-{}.pkg.tar.zst""#, name, arch).as_bytes());
        body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
        body.extend_from_slice(&pkg_data);
        body.extend_from_slice(b"\r\n--");
        body.extend_from_slice(boundary.as_bytes());
        body.extend_from_slice(b"--\r\n");

        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/packages")
                    .header("Content-Type", format!("multipart/form-data; boundary={}", boundary))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
    }

    // Filter by arch=aarch64
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/packages?arch=aarch64")
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

    // Should only return pkg2 with aarch64 arch
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0]["name"].as_str().unwrap(), "pkg2");
    assert_eq!(packages[0]["arch"].as_str().unwrap(), "aarch64");
}

#[tokio::test]
async fn test_upload_streaming_size_exceeds_limit() {
    let app = setup_test_app().await;

    // Create a test package
    let package_data = create_test_package("test-pkg", "1.0.0", "x86_64");

    // Create multipart form data with artificially large padding to exceed 512 MiB
    // This tests that our streaming validation enforces the limit during chunk reading
    let boundary = "------------------------boundary123456789";
    let mut body = Vec::new();

    // Start boundary
    body.extend_from_slice(b"--");
    body.extend_from_slice(boundary.as_bytes());
    body.extend_from_slice(b"\r\n");

    // Content disposition and type
    body.extend_from_slice(br#"Content-Disposition: form-data; name="file"; filename="test-pkg-1.0.0-x86_64.pkg.tar.zst""#);
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");

    // Binary data - actual package
    body.extend_from_slice(&package_data);

    // Add 513 MiB of padding to exceed the 512 MiB limit
    let padding_size = 513 * 1024 * 1024; // 513 MiB
    body.reserve(padding_size);
    body.extend(std::iter::repeat(0u8).take(padding_size));

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
                .header("Content-Length", body.len().to_string())
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    // Should be rejected with 413 Payload Too Large
    // Both Content-Length check and streaming validation should catch this
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
            .contains("exceeds maximum allowed size")
    );
}
