//! Verification and authorization of Authelia-issued access tokens.
//!
//! Tokens are signed with a keypair generated for the test run and injected
//! straight into the JWKS cache, so nothing here touches the network.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header};
use rsa::pkcs1::{EncodeRsaPrivateKey, EncodeRsaPublicKey, LineEnding};
use serde_json::json;
use std::sync::LazyLock;
use sw1nn_pkg_repo::auth::JwksCache;
use sw1nn_pkg_repo::config::AuthConfig;
use tower::util::ServiceExt;

mod common;
use common::{setup_test_app, setup_test_app_with_auth};

const ISSUER: &str = "https://auth.sw1nn.net";
const CLIENT_ID: &str = "sw1nn-pkg-cli";
const KID: &str = "main";

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// One RSA keypair per test binary — generating it is slow, and every test
/// needs the same one.
static KEYPAIR: LazyLock<(EncodingKey, DecodingKey)> = LazyLock::new(|| {
    let private = rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048)
        .expect("failed to generate test RSA key");

    let private_pem = private
        .to_pkcs1_pem(LineEnding::LF)
        .expect("failed to encode private key");
    let public_pem = private
        .to_public_key()
        .to_pkcs1_pem(LineEnding::LF)
        .expect("failed to encode public key");

    (
        EncodingKey::from_rsa_pem(private_pem.as_bytes()).expect("bad private key"),
        DecodingKey::from_rsa_pem(public_pem.as_bytes()).expect("bad public key"),
    )
});

fn auth_config() -> AuthConfig {
    AuthConfig {
        issuer: ISSUER.to_owned(),
        // Never reached: the cache is pre-populated with the test key.
        jwks_uri: "http://127.0.0.1:1/jwks.json".to_owned(),
        client_id: CLIENT_ID.to_owned(),
        leeway_secs: 60,
    }
}

/// A JWKS cache holding only the test signing key, under `kid: main`.
fn test_jwks() -> JwksCache {
    JwksCache::with_keys([(KID, KEYPAIR.1.clone())])
}

async fn app() -> Router {
    setup_test_app_with_auth(auth_config(), test_jwks()).await
}

/// Build a token shaped like a real Authelia access token, overridable per test.
struct TokenBuilder {
    issuer: String,
    client_id: String,
    groups: Vec<String>,
    kid: String,
    lifetime_secs: i64,
    /// Seconds to shift `iat`/`nbf` by, for not-yet-valid tokens.
    not_before_in: i64,
}

impl Default for TokenBuilder {
    fn default() -> Self {
        Self {
            issuer: ISSUER.to_owned(),
            client_id: CLIENT_ID.to_owned(),
            groups: Vec::new(),
            kid: KID.to_owned(),
            lifetime_secs: 3600,
            not_before_in: 0,
        }
    }
}

impl TokenBuilder {
    fn groups<I>(mut self, groups: I) -> Self
    where
        I: IntoIterator<Item: Into<String>>,
    {
        self.groups = groups.into_iter().map(Into::into).collect();
        self
    }

    fn build(self) -> String {
        let now = chrono::Utc::now().timestamp();
        let valid_from = now + self.not_before_in;

        let claims = json!({
            "sub": "07ca04a5-8538-4ecf-8fe6-a7c5e133a4df",
            "iss": self.issuer,
            "client_id": self.client_id,
            "iat": valid_from,
            "nbf": valid_from,
            "exp": valid_from + self.lifetime_secs,
            // Authelia's device grant always returns an empty audience.
            "aud": [],
            "groups": self.groups,
            "scp": ["openid", "profile", "groups", "offline_access"],
            "preferred_username": "neale",
        });

        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(self.kid);
        header.typ = Some("at+jwt".to_owned());

        jsonwebtoken::encode(&header, &claims, &KEYPAIR.0).expect("failed to sign test token")
    }
}

fn token() -> String {
    TokenBuilder::default().build()
}

// -- Request helpers, one per protected route class --

fn initiate_upload(auth: Option<&str>) -> Request<Body> {
    let body = json!({
        "filename": "test-pkg-1.0.0-x86_64.pkg.tar.zst",
        "size": 1048576,
        "sha256": "abc123",
        "chunk_size": 1048576,
        "has_signature": false
    });

    build(
        "POST",
        "/api/packages/upload/initiate",
        auth,
        Body::from(body.to_string()),
    )
}

fn delete_package(auth: Option<&str>) -> Request<Body> {
    build("DELETE", "/api/packages/nonexistent", auth, Body::empty())
}

fn rebuild_db(auth: Option<&str>) -> Request<Body> {
    build(
        "POST",
        "/api/repos/sw1nn/os/x86_64/rebuild",
        auth,
        Body::empty(),
    )
}

fn list_packages(auth: Option<&str>) -> Request<Body> {
    build("GET", "/api/packages", auth, Body::empty())
}

fn build(method: &str, uri: &str, auth: Option<&str>, body: Body) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("Content-Type", "application/json");

    if let Some(token) = auth {
        request = request.header("Authorization", format!("Bearer {token}"));
    }

    request.body(body).expect("failed to build test request")
}

async fn status_of(request: Request<Body>) -> Result<StatusCode, Box<dyn std::error::Error>> {
    Ok(app().await.oneshot(request).await?.status())
}

// -- Auth disabled: the server stays open --

#[tokio::test]
async fn write_endpoints_are_open_without_an_auth_section() -> TestResult {
    let response = setup_test_app()
        .await
        .oneshot(initiate_upload(None))
        .await?;

    assert_eq!(response.status(), StatusCode::CREATED);
    Ok(())
}

// -- Token verification --

#[tokio::test]
async fn missing_authorization_header_is_rejected() -> TestResult {
    assert_eq!(
        status_of(initiate_upload(None)).await?,
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}

#[tokio::test]
async fn malformed_bearer_token_is_rejected() -> TestResult {
    assert_eq!(
        status_of(initiate_upload(Some("not-a-jwt"))).await?,
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}

#[tokio::test]
async fn token_from_another_issuer_is_rejected() -> TestResult {
    let token = TokenBuilder {
        issuer: "https://evil.example.com".to_owned(),
        ..TokenBuilder::default()
    }
    .groups(["pkg-publish"])
    .build();

    assert_eq!(
        status_of(initiate_upload(Some(&token))).await?,
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}

/// The replay boundary between services. A token minted for the lolcommits CLI
/// carries that client id and must not work here, whatever groups it holds.
#[tokio::test]
async fn token_minted_for_another_client_is_rejected() -> TestResult {
    let token = TokenBuilder {
        client_id: "lolcommits-cli".to_owned(),
        ..TokenBuilder::default()
    }
    .groups(["pkg-publish", "pkg-admin"])
    .build();

    assert_eq!(
        status_of(initiate_upload(Some(&token))).await?,
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}

#[tokio::test]
async fn expired_token_is_rejected() -> TestResult {
    // Beyond the 60s leeway.
    let token = TokenBuilder {
        lifetime_secs: -3600,
        ..TokenBuilder::default()
    }
    .groups(["pkg-publish"])
    .build();

    assert_eq!(
        status_of(initiate_upload(Some(&token))).await?,
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}

/// `nbf` must be enforced, not merely parsed — `jsonwebtoken` leaves that check
/// off by default.
#[tokio::test]
async fn not_yet_valid_token_is_rejected() -> TestResult {
    // Beyond the 60s leeway.
    let token = TokenBuilder {
        not_before_in: 3600,
        ..TokenBuilder::default()
    }
    .groups(["pkg-publish"])
    .build();

    assert_eq!(
        status_of(initiate_upload(Some(&token))).await?,
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}

/// An unknown `kid` must fail closed. The cache cannot reach its (unroutable)
/// JWKS URI, so this also proves verification does not depend on a live fetch.
#[tokio::test]
async fn token_signed_with_an_unknown_key_is_rejected() -> TestResult {
    let token = TokenBuilder {
        kid: "rotated-out".to_owned(),
        ..TokenBuilder::default()
    }
    .groups(["pkg-publish"])
    .build();

    assert_eq!(
        status_of(initiate_upload(Some(&token))).await?,
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}

// -- Group authorization --

#[tokio::test]
async fn listing_packages_needs_a_valid_token_but_no_group() -> TestResult {
    assert_eq!(
        status_of(list_packages(None)).await?,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status_of(list_packages(Some(&token()))).await?,
        StatusCode::OK
    );
    Ok(())
}

#[tokio::test]
async fn uploading_requires_pkg_publish() -> TestResult {
    let without = token();
    assert_eq!(
        status_of(initiate_upload(Some(&without))).await?,
        StatusCode::FORBIDDEN
    );

    let with = TokenBuilder::default().groups(["pkg-publish"]).build();
    assert_eq!(
        status_of(initiate_upload(Some(&with))).await?,
        StatusCode::CREATED
    );
    Ok(())
}

#[tokio::test]
async fn destructive_routes_require_pkg_admin() -> TestResult {
    // `pkg-publish` lets you add packages, not remove them.
    let publisher = TokenBuilder::default().groups(["pkg-publish"]).build();
    assert_eq!(
        status_of(delete_package(Some(&publisher))).await?,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status_of(rebuild_db(Some(&publisher))).await?,
        StatusCode::FORBIDDEN
    );

    let admin = TokenBuilder::default().groups(["pkg-admin"]).build();
    assert_eq!(
        status_of(rebuild_db(Some(&admin))).await?,
        StatusCode::ACCEPTED
    );
    // Authorized, so the request reaches the handler and fails on the package.
    assert_eq!(
        status_of(delete_package(Some(&admin))).await?,
        StatusCode::NOT_FOUND
    );
    Ok(())
}

/// `admins` is not a blanket grant. A token carrying it and nothing else must
/// be refused on every group-gated route.
#[tokio::test]
async fn admins_group_grants_nothing_on_its_own() -> TestResult {
    let token = TokenBuilder::default().groups(["admins"]).build();

    assert_eq!(
        status_of(initiate_upload(Some(&token))).await?,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status_of(delete_package(Some(&token))).await?,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status_of(rebuild_db(Some(&token))).await?,
        StatusCode::FORBIDDEN
    );
    Ok(())
}
