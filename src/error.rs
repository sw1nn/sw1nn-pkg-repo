use derive_more::{Display, From};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Display, From)]
pub enum Error {
    #[from]
    Io(std::io::Error),

    #[display("Package not found: {pkgname}")]
    PackageNotFound { pkgname: String },

    #[display("Invalid package: {pkgname}")]
    InvalidPackage { pkgname: String },

    #[display("Package already exists: {pkgname}")]
    PackageAlreadyExists { pkgname: String },

    #[display("Payload too large: {msg}")]
    PayloadTooLarge { msg: String },

    #[display("Metadata generation failed: {msg}")]
    MetadataGeneration { msg: String },

    #[display("Configuration error: {msg}")]
    Config { msg: String },
}

impl std::error::Error for Error {}

// Implement axum IntoResponse for Error
impl axum::response::IntoResponse for Error {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match &self {
            Error::PackageNotFound { pkgname } => {
                // Safe to expose - just the package name
                (
                    axum::http::StatusCode::NOT_FOUND,
                    format!("Package not found: {}", pkgname),
                )
            }
            Error::InvalidPackage { pkgname } => {
                // Log detailed error internally for debugging
                tracing::warn!("Invalid package request: {}", pkgname);
                // Return sanitized message - avoid exposing internal paths/structure
                let sanitized = if pkgname.contains("Failed to read") {
                    "Invalid multipart request".to_string()
                } else if pkgname.contains("path") || pkgname.contains("Path") {
                    "Invalid request parameters".to_string()
                } else if pkgname.contains(".PKGINFO") || pkgname.contains("parse") {
                    "Invalid package format".to_string()
                } else {
                    // Generic fallback that includes the message if it's safe
                    format!("Invalid package: {}", pkgname)
                };
                (axum::http::StatusCode::BAD_REQUEST, sanitized)
            }
            Error::PackageAlreadyExists { pkgname } => {
                // Safe to expose - just the filename
                (
                    axum::http::StatusCode::CONFLICT,
                    format!("Package already exists: {}", pkgname),
                )
            }
            Error::PayloadTooLarge { msg } => {
                // Safe to expose - contains size limits we configured
                (axum::http::StatusCode::PAYLOAD_TOO_LARGE, msg.clone())
            }
            Error::Io(e) => {
                // Log full error internally for debugging
                tracing::error!("IO error: {}", e);
                // Return generic message - never expose file paths
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal server error".to_string(),
                )
            }
            Error::MetadataGeneration { msg } => {
                // Log full error internally for debugging
                tracing::error!("Metadata generation failed: {}", msg);
                // Return generic message
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to process package metadata".to_string(),
                )
            }
            Error::Config { msg } => {
                // Log full error internally for debugging
                tracing::error!("Configuration error: {}", msg);
                // Return generic message - don't expose config structure
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    "Configuration error".to_string(),
                )
            }
        };

        let body = axum::Json(serde_json::json!({
            "error": message,
        }));

        (status, body).into_response()
    }
}
