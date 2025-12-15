use derive_more::Display;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Display)]
pub enum Error {
    #[display("IO error at {path}: {error}")]
    Io { error: std::io::Error, path: String },

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

    #[display("Permission denied: {path}")]
    PermissionDenied { path: String },
}

impl std::error::Error for Error {}

// Implement From<std::io::Error> for cases where path context is not available
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Error::Io {
            error,
            path: "<unknown>".to_string(),
        }
    }
}

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
            Error::Io { error, path } => {
                // Log full error with path internally for debugging
                tracing::error!("IO error at path {}: {}", path, error);
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
            Error::PermissionDenied { path } => {
                // Log full error with path internally for debugging
                tracing::error!("Permission denied writing to path: {}", path);
                // Return generic message - don't expose file paths to client
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal server error".to_string(),
                )
            }
        };

        let body = axum::Json(serde_json::json!({
            "error": message,
        }));

        (status, body).into_response()
    }
}

/// Extension trait for converting I/O errors to custom errors with path context
pub trait ResultIoExt<T> {
    /// Map I/O errors with path context, creating PermissionDenied variant when appropriate
    fn map_io_err(self, path: &std::path::Path) -> Result<T>;
}

impl<T> ResultIoExt<T> for std::result::Result<T, std::io::Error> {
    fn map_io_err(self, path: &std::path::Path) -> Result<T> {
        self.map_err(|error| match error.kind() {
            std::io::ErrorKind::PermissionDenied => Error::PermissionDenied {
                path: path.display().to_string(),
            },
            _ => Error::Io {
                error,
                path: path.display().to_string(),
            },
        })
    }
}
