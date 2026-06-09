mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use common::{seed_package, setup_test_app_with_storage};
use tower::util::ServiceExt;

/// A full download (no Range header) must advertise byte-range support so that
/// clients like pacman know they can resume an interrupted transfer.
#[tokio::test]
async fn full_download_advertises_accept_ranges() {
    let (app, storage) = setup_test_app_with_storage().await;
    let (data, filename) = seed_package(&storage, "sw1nn", "rangepkg", "1.0.0-1", "x86_64").await;

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/sw1nn/os/x86_64/{filename}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::ACCEPT_RANGES)
            .and_then(|v| v.to_str().ok()),
        Some("bytes"),
        "full responses must advertise Accept-Ranges: bytes"
    );

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body.as_ref(), data.as_slice());
}

/// A `Range: bytes=N-` request (what pacman sends to resume a partial download)
/// must return 206 Partial Content with the requested tail of the file.
#[tokio::test]
async fn range_request_returns_partial_content() {
    let (app, storage) = setup_test_app_with_storage().await;
    let (data, filename) = seed_package(&storage, "sw1nn", "rangepkg", "1.0.0-1", "x86_64").await;

    let total = data.len();
    let start = total / 2;

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/sw1nn/os/x86_64/{filename}"))
                .header(header::RANGE, format!("bytes={start}-"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok()),
        Some(format!("bytes {start}-{}/{total}", total - 1).as_str()),
        "Content-Range must describe the served slice"
    );

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body.as_ref(), &data[start..]);
}

/// An unsatisfiable range (start past the end of the file) must return
/// 416 Range Not Satisfiable rather than silently serving the whole file.
#[tokio::test]
async fn unsatisfiable_range_returns_416() {
    let (app, storage) = setup_test_app_with_storage().await;
    let (data, filename) = seed_package(&storage, "sw1nn", "rangepkg", "1.0.0-1", "x86_64").await;

    let past_end = data.len() + 100;

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/sw1nn/os/x86_64/{filename}"))
                .header(header::RANGE, format!("bytes={past_end}-"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
}
