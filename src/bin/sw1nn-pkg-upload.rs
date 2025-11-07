use clap::Parser;
use std::path::Path;
use std::process;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser, Debug)]
#[command(name = "sw1nn-pkg-upload")]
#[command(about = "Upload packages to sw1nn package repository", long_about = None)]
#[command(version = VERSION)]
struct Args {
    /// Path to package file (.pkg.tar.zst)
    package_file: String,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sw1nn_pkg_upload=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("sw1nn-pkg-upload version {}", VERSION);

    let args = Args::parse();
    let pkg_file = &args.package_file;
    let path = Path::new(pkg_file);

    if !path.exists() {
        tracing::error!("File '{}' does not exist", pkg_file);
        process::exit(1);
    }

    if !pkg_file.ends_with(".pkg.tar.zst") {
        tracing::error!("File must be a .pkg.tar.zst package");
        process::exit(1);
    }

    let url = "https://repo.sw1nn.net/api/packages";

    tracing::info!("Uploading {} to {}", pkg_file, url);

    let client = reqwest::Client::new();
    let file = match tokio::fs::read(pkg_file).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("Error reading file: {}", e);
            process::exit(1);
        }
    };

    let file_name = path.file_name().unwrap().to_string_lossy().to_string();
    let part = reqwest::multipart::Part::bytes(file)
        .file_name(file_name);

    let form = reqwest::multipart::Form::new()
        .part("file", part);

    match client.post(url).multipart(form).send().await {
        Ok(response) => {
            if response.status().is_success() {
                tracing::info!("Successfully uploaded package");
                match response.text().await {
                    Ok(body) => tracing::info!("{}", body),
                    Err(e) => tracing::error!("Error reading response: {}", e),
                }
            } else {
                tracing::error!("Upload failed with status: {}", response.status());
                match response.text().await {
                    Ok(body) => tracing::error!("Error: {}", body),
                    Err(e) => tracing::error!("Error reading response: {}", e),
                }
                process::exit(1);
            }
        }
        Err(e) => {
            tracing::error!("Error uploading package: {}", e);
            process::exit(1);
        }
    }
}
