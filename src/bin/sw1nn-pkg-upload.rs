use clap::Parser;
use colored::Colorize;
use std::path::Path;
use std::process;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

// Re-use the Package struct from the lib
use sw1nn_pkg_repo::models::Package;

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

    let url = std::env::var("SW1NN_REPO_URL")
        .unwrap_or_else(|_| "https://repo.sw1nn.net/api/packages".to_string());

    tracing::info!("Uploading {} to {}", pkg_file, url);

    let client = reqwest::Client::new();
    let file = match tokio::fs::read(pkg_file).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("Error reading file: {}", e);
            process::exit(1);
        }
    };

    let file_name = path.file_name().unwrap().to_string_lossy().into_owned();
    let part = reqwest::multipart::Part::bytes(file).file_name(file_name);

    let form = reqwest::multipart::Form::new().part("file", part);

    match client.post(url).multipart(form).send().await {
        Ok(response) => {
            if response.status().is_success() {
                match response.json::<Package>().await {
                    Ok(package) => {
                        println!("\n{}", "✓ Package uploaded successfully".green().bold());
                        println!();
                        println!("  {}  {}", "Name:".cyan().bold(), package.name);
                        println!("  {}  {}", "Version:".cyan().bold(), package.version);
                        println!("  {}  {}", "Arch:".cyan().bold(), package.arch);
                        println!("  {}  {}", "Repo:".cyan().bold(), package.repo);
                        println!("  {}  {}", "Filename:".cyan().bold(), package.filename);
                        println!("  {}  {} bytes", "Size:".cyan().bold(), package.size.to_string().yellow());
                        println!("  {}  {}", "SHA256:".cyan().bold(), package.sha256.bright_black());
                        println!("  {}  {}", "Created:".cyan().bold(), package.created_at.format("%Y-%m-%d %H:%M:%S UTC"));
                        println!();
                    }
                    Err(e) => {
                        tracing::warn!("Successfully uploaded but failed to parse response: {}", e);
                    }
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
