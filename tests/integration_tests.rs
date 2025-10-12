use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::util::ServiceExt;

mod common;
use common::setup_test_app;

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
