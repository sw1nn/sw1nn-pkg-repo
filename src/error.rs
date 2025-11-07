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
            Error::PackageNotFound { .. } => (axum::http::StatusCode::NOT_FOUND, self.to_string()),
            Error::InvalidPackage { .. } => (axum::http::StatusCode::BAD_REQUEST, self.to_string()),
            Error::PackageAlreadyExists { .. } => {
                (axum::http::StatusCode::CONFLICT, self.to_string())
            }
            Error::PayloadTooLarge { .. } => {
                (axum::http::StatusCode::PAYLOAD_TOO_LARGE, self.to_string())
            }
            Error::Io(_) | Error::MetadataGeneration { .. } | Error::Config { .. } => (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                self.to_string(),
            ),
        };

        let body = axum::Json(serde_json::json!({
            "error": message,
        }));

        (status, body).into_response()
    }
}
