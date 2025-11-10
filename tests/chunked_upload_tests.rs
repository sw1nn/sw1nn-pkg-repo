use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;
use tower::util::ServiceExt;

mod common;
use common::{create_test_package, setup_test_app};

#[tokio::test]
async fn test_chunked_upload_initiate() {
    let app = setup_test_app().await;

    let request_body = json!({
        "filename": "test-pkg-1.0.0-x86_64.pkg.tar.zst",
        "size": 10485760, // 10 MiB
        "sha256": "abc123",
        "chunk_size": 1048576, // 1 MiB
        "has_signature": false
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages/upload/initiate")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&request_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(response_json["upload_id"].is_string());
    assert_eq!(response_json["chunk_size"], 1048576);
    assert_eq!(response_json["total_chunks"], 10); // 10 MiB / 1 MiB = 10 chunks
}

#[tokio::test]
async fn test_chunked_upload_invalid_filename() {
    let app = setup_test_app().await;

    let request_body = json!({
        "filename": "test-pkg-1.0.0-x86_64.tar.gz", // Wrong extension
        "size": 1024,
        "has_signature": false
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages/upload/initiate")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&request_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(error["error"].as_str().unwrap().contains(".pkg.tar.zst"));
}

#[tokio::test]
async fn test_chunked_upload_size_too_large() {
    let app = setup_test_app().await;

    let request_body = json!({
        "filename": "huge-pkg-1.0.0-x86_64.pkg.tar.zst",
        "size": 2147483648u64, // 2 GiB - exceeds default 512 MiB limit
        "has_signature": false
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages/upload/initiate")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&request_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn test_chunked_upload_complete_workflow() {
    let app = setup_test_app().await;

    // Create a small test package
    let package_data = create_test_package("chunked-test", "1.0.0", "x86_64");
    let file_size = package_data.len() as u64;
    // Use smaller chunk size that's guaranteed to be less than file size
    let chunk_size = std::cmp::min(256, file_size as usize / 2).max(1);

    // Calculate SHA256
    use sha2::{Digest, Sha256};
    let sha256 = format!("{:x}", Sha256::digest(&package_data));

    // Step 1: Initiate upload
    let init_request = json!({
        "filename": "chunked-test-1.0.0-x86_64.pkg.tar.zst",
        "size": file_size,
        "sha256": sha256,
        "chunk_size": chunk_size,
        "has_signature": false
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages/upload/initiate")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&init_request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    if status != StatusCode::CREATED {
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let error_text = String::from_utf8_lossy(&body);
        panic!(
            "Initiate failed - Expected 201 but got {}: {}",
            status, error_text
        );
    }

    assert_eq!(status, StatusCode::CREATED);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let init_response: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let upload_id = init_response["upload_id"].as_str().unwrap();
    let total_chunks = init_response["total_chunks"].as_u64().unwrap() as usize;

    // Step 2: Upload chunks
    let mut chunk_infos = Vec::new();
    for chunk_num in 1..=total_chunks {
        let start = (chunk_num - 1) * chunk_size;
        let end = std::cmp::min(start + chunk_size, package_data.len());
        let chunk_data = &package_data[start..end];

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/packages/upload/{}/chunks/{}",
                        upload_id, chunk_num
                    ))
                    .header("Content-Type", "application/octet-stream")
                    .body(Body::from(chunk_data.to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let chunk_response: serde_json::Value = serde_json::from_slice(&body).unwrap();

        chunk_infos.push(json!({
            "chunk_number": chunk_response["chunk_number"],
            "checksum": chunk_response["checksum"]
        }));
    }

    // Step 3: Complete upload
    let complete_request = json!({
        "chunks": chunk_infos
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/packages/upload/{}/complete", upload_id))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&complete_request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    if status != StatusCode::CREATED {
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let error_text = String::from_utf8_lossy(&body);
        panic!("Expected 201 but got {}: {}", status, error_text);
    }

    assert_eq!(status, StatusCode::CREATED);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let package: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(package["name"], "chunked-test");
    assert_eq!(package["version"], "1.0.0");
    assert_eq!(package["arch"], "x86_64");
    assert_eq!(package["sha256"], sha256);
}

#[tokio::test]
async fn test_chunked_upload_wrong_chunk_size() {
    let app = setup_test_app().await;

    // Initiate upload
    let init_request = json!({
        "filename": "test-pkg-1.0.0-x86_64.pkg.tar.zst",
        "size": 2048,
        "chunk_size": 1024,
        "has_signature": false
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages/upload/initiate")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&init_request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let init_response: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let upload_id = init_response["upload_id"].as_str().unwrap();

    // Upload chunk with wrong size (not last chunk)
    let wrong_chunk = vec![0u8; 512]; // Should be 1024

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/packages/upload/{}/chunks/1", upload_id))
                .header("Content-Type", "application/octet-stream")
                .body(Body::from(wrong_chunk))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(error["error"].as_str().unwrap().contains("size mismatch"));
}

#[tokio::test]
async fn test_chunked_upload_invalid_chunk_number() {
    let app = setup_test_app().await;

    // Initiate upload with 2 chunks
    let init_request = json!({
        "filename": "test-pkg-1.0.0-x86_64.pkg.tar.zst",
        "size": 2048,
        "chunk_size": 1024,
        "has_signature": false
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages/upload/initiate")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&init_request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let init_response: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let upload_id = init_response["upload_id"].as_str().unwrap();

    // Try to upload chunk 3 (out of range)
    let chunk_data = vec![0u8; 1024];

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/packages/upload/{}/chunks/3", upload_id))
                .header("Content-Type", "application/octet-stream")
                .body(Body::from(chunk_data))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(error["error"].as_str().unwrap().contains("out of range"));
}

#[tokio::test]
async fn test_chunked_upload_abort() {
    let app = setup_test_app().await;

    // Initiate upload
    let init_request = json!({
        "filename": "test-pkg-1.0.0-x86_64.pkg.tar.zst",
        "size": 1024,
        "chunk_size": 512,
        "has_signature": false
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages/upload/initiate")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&init_request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let init_response: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let upload_id = init_response["upload_id"].as_str().unwrap();

    // Upload one chunk
    let chunk_data = vec![0u8; 512];
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/packages/upload/{}/chunks/1", upload_id))
                .header("Content-Type", "application/octet-stream")
                .body(Body::from(chunk_data))
                .unwrap(),
        )
        .await
        .unwrap();

    // Abort upload
    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/packages/upload/{}", upload_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let abort_response: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(abort_response["upload_id"], upload_id);
    assert!(abort_response["deleted_chunks"].as_u64().unwrap() >= 1);
}

#[tokio::test]
async fn test_chunked_upload_complete_missing_chunks() {
    let app = setup_test_app().await;

    // Initiate upload
    let init_request = json!({
        "filename": "test-pkg-1.0.0-x86_64.pkg.tar.zst",
        "size": 2048,
        "chunk_size": 1024,
        "has_signature": false
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages/upload/initiate")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&init_request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let init_response: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let upload_id = init_response["upload_id"].as_str().unwrap();

    // Upload only chunk 1 (missing chunk 2)
    let chunk_data = vec![0u8; 1024];
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/packages/upload/{}/chunks/1", upload_id))
                .header("Content-Type", "application/octet-stream")
                .body(Body::from(chunk_data))
                .unwrap(),
        )
        .await
        .unwrap();

    // Try to complete upload
    let complete_request = json!({
        "chunks": [
            {"chunk_number": 1, "checksum": "abc123"}
        ]
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/packages/upload/{}/complete", upload_id))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&complete_request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(error["error"].as_str().unwrap().contains("incomplete"));
}

#[tokio::test]
async fn test_chunked_upload_concurrent_sessions() {
    let app = setup_test_app().await;

    // Create two different packages
    let pkg1 = create_test_package("pkg1", "1.0.0", "x86_64");
    let pkg2 = create_test_package("pkg2", "2.0.0", "x86_64");

    // Use chunk size that's less than file size
    let chunk_size = std::cmp::min(256, pkg1.len() / 2).max(1);

    // Calculate SHA256s
    use sha2::{Digest, Sha256};
    let sha256_1 = format!("{:x}", Sha256::digest(&pkg1));
    let sha256_2 = format!("{:x}", Sha256::digest(&pkg2));

    // Initiate both uploads
    let init1 = json!({
        "filename": "pkg1-1.0.0-x86_64.pkg.tar.zst",
        "size": pkg1.len(),
        "sha256": sha256_1,
        "chunk_size": chunk_size,
        "has_signature": false
    });

    let init2 = json!({
        "filename": "pkg2-2.0.0-x86_64.pkg.tar.zst",
        "size": pkg2.len(),
        "sha256": sha256_2,
        "chunk_size": chunk_size,
        "has_signature": false
    });

    let response1 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages/upload/initiate")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&init1).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let response2 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/packages/upload/initiate")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&init2).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let body1 = axum::body::to_bytes(response1.into_body(), usize::MAX)
        .await
        .unwrap();
    let init_response1: serde_json::Value = serde_json::from_slice(&body1).unwrap();
    let upload_id1 = init_response1["upload_id"].as_str().unwrap().to_string();

    let body2 = axum::body::to_bytes(response2.into_body(), usize::MAX)
        .await
        .unwrap();
    let init_response2: serde_json::Value = serde_json::from_slice(&body2).unwrap();
    let upload_id2 = init_response2["upload_id"].as_str().unwrap().to_string();

    // Verify upload IDs are different
    assert_ne!(upload_id1, upload_id2);

    // Both sessions should be able to upload chunks independently
    let chunk1 = vec![0u8; chunk_size];
    let chunk2 = vec![1u8; chunk_size];

    let response1 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/packages/upload/{}/chunks/1", upload_id1))
                .header("Content-Type", "application/octet-stream")
                .body(Body::from(chunk1))
                .unwrap(),
        )
        .await
        .unwrap();

    let response2 = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/packages/upload/{}/chunks/1", upload_id2))
                .header("Content-Type", "application/octet-stream")
                .body(Body::from(chunk2))
                .unwrap(),
        )
        .await
        .unwrap();

    // Both should succeed
    assert_eq!(response1.status(), StatusCode::OK);
    assert_eq!(response2.status(), StatusCode::OK);
}
