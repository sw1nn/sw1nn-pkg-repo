use clap::{Parser, Subcommand};
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::path::{Path, PathBuf};
use std::process;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

// Re-use the Package struct from the lib
use sw1nn_pkg_repo::models::Package;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const BIN_NAME: &str = env!("CARGO_BIN_NAME");
const DEFAULT_CHUNK_SIZE: usize = 1024 * 1024; // 1 MiB

#[derive(Parser, Debug)]
#[command(name = BIN_NAME)]
#[command(about = "Upload and manage packages in sw1nn package repository", long_about = None)]
#[command(version = VERSION)]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path(s) to package file(s) (.pkg.tar.zst) - for backwards compatibility
    #[arg(trailing_var_arg = true)]
    package_files: Vec<String>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Upload package(s) to the repository
    Upload {
        /// Path(s) to package file(s) (.pkg.tar.zst)
        package_files: Vec<String>,
    },
    /// Delete package version(s) from the repository
    Delete {
        /// Package name
        #[arg(short, long)]
        name: String,
        /// Version(s) to delete - can be exact (1.0.0-1) or semver ranges (^1.0.0)
        #[arg(short, long, required = true)]
        version: Vec<String>,
        /// Repository name (optional)
        #[arg(short, long)]
        repo: Option<String>,
        /// Architecture (optional)
        #[arg(short, long)]
        arch: Option<String>,
    },
}

#[derive(Debug, Serialize)]
struct InitiateUploadRequest {
    filename: String,
    size: u64,
    sha256: Option<String>,
    repo: Option<String>,
    arch: Option<String>,
    chunk_size: Option<usize>,
    has_signature: bool,
}

#[derive(Debug, Deserialize)]
struct InitiateUploadResponse {
    upload_id: String,
    #[allow(dead_code)]
    expires_at: String,
    chunk_size: usize,
    total_chunks: u32,
}

#[derive(Debug, Deserialize)]
struct UploadChunkResponse {
    chunk_number: u32,
    checksum: String,
    #[allow(dead_code)]
    received_size: usize,
}

#[derive(Debug, Serialize)]
struct CompleteUploadRequest {
    chunks: Vec<ChunkInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChunkInfo {
    chunk_number: u32,
    checksum: String,
}

#[derive(Debug, Serialize)]
struct DeleteVersionsRequest {
    versions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    arch: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeleteVersionsResponse {
    deleted_count: usize,
    deleted_versions: Vec<String>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| format!("{BIN_NAME}=info").into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("{BIN_NAME} version {VERSION}");

    let args = Args::parse();

    let base_url =
        std::env::var("SW1NN_REPO_URL").unwrap_or_else(|_| "https://repo.sw1nn.net".to_string());

    let client = reqwest::Client::new();

    // Handle subcommands or backwards-compatible positional args
    match args.command {
        Some(Commands::Upload { package_files }) => {
            run_upload(&client, &base_url, package_files).await;
        }
        Some(Commands::Delete {
            name,
            version,
            repo,
            arch,
        }) => {
            run_delete(&client, &base_url, name, version, repo, arch).await;
        }
        None => {
            // Backwards compatibility: treat positional args as upload
            if args.package_files.is_empty() {
                tracing::error!(
                    "No command specified. Use 'upload' or 'delete' subcommand, or provide package files directly."
                );
                process::exit(1);
            }
            run_upload(&client, &base_url, args.package_files).await;
        }
    }
}

async fn run_upload(client: &reqwest::Client, base_url: &str, package_files: Vec<String>) {
    if package_files.is_empty() {
        tracing::error!("No package files specified");
        process::exit(1);
    }

    let total_files = package_files.len();
    let mut successful_uploads = 0;
    let mut failed_uploads = 0;

    tracing::info!("Uploading {total_files} package(s) to {base_url}");

    for (index, pkg_file) in package_files.iter().enumerate() {
        let path = Path::new(pkg_file);

        if !path.exists() {
            tracing::error!(
                "[{}/{}] File '{}' does not exist",
                index + 1,
                total_files,
                pkg_file
            );
            failed_uploads += 1;
            continue;
        }

        if !pkg_file.ends_with(".pkg.tar.zst") {
            tracing::error!(
                "[{}/{}] File '{}' must be a .pkg.tar.zst package",
                index + 1,
                total_files,
                pkg_file
            );
            failed_uploads += 1;
            continue;
        }

        tracing::info!("[{}/{}] Uploading {}", index + 1, total_files, pkg_file);

        // Always use chunked upload
        let result = upload_chunked(client, base_url, path, index + 1, total_files).await;

        match result {
            Ok(package) => {
                print_upload_success(&package, index + 1, total_files);
                successful_uploads += 1;
            }
            Err(e) => {
                tracing::error!("[{}/{}] Upload failed: {}", index + 1, total_files, e);
                failed_uploads += 1;
            }
        }
    }

    println!("\n{}", "=".repeat(50));
    println!("{}", "Upload Summary".bold());
    println!("{}", "=".repeat(50));
    println!("  Total files:       {total_files}");
    println!(
        "  Successful:        {}",
        successful_uploads.to_string().green()
    );
    println!("  Failed:            {}", failed_uploads.to_string().red());
    println!("{}", "=".repeat(50));
    println!();

    if failed_uploads > 0 {
        process::exit(1);
    }
}

async fn run_delete(
    client: &reqwest::Client,
    base_url: &str,
    name: String,
    versions: Vec<String>,
    repo: Option<String>,
    arch: Option<String>,
) {
    tracing::info!(
        package = %name,
        versions = ?versions,
        "Deleting package version(s) from {base_url}"
    );

    let result = delete_versions(client, base_url, &name, versions, repo, arch).await;

    match result {
        Ok(response) => {
            print_delete_success(&name, &response);
        }
        Err(e) => {
            tracing::error!(error = %e, "Delete failed");
            process::exit(1);
        }
    }
}

/// Upload a package using the simple (non-chunked) API
async fn upload_chunked(
    client: &reqwest::Client,
    base_url: &str,
    path: &Path,
    index: usize,
    total: usize,
) -> Result<Package, Box<dyn std::error::Error>> {
    let file_size = tokio::fs::metadata(path).await?.len();
    let filename = path.file_name().unwrap().to_string_lossy().into_owned();

    // Check for signature file
    let sig_path = PathBuf::from(format!("{}.sig", path.display()));
    let has_signature = sig_path.exists();

    // Calculate SHA256 (optional but recommended)
    tracing::info!("[{}/{}] Calculating SHA256...", index, total);
    let file_data = tokio::fs::read(path).await?;
    let sha256 = format!("{:x}", sha2::Sha256::digest(&file_data));

    // Initiate upload
    tracing::info!("[{}/{}] Initiating chunked upload...", index, total);
    // Cap chunk size to file size to avoid server validation errors
    let chunk_size = std::cmp::min(DEFAULT_CHUNK_SIZE, file_size as usize);
    let init_req = InitiateUploadRequest {
        filename: filename.clone(),
        size: file_size,
        sha256: Some(sha256),
        repo: None,
        arch: None,
        chunk_size: Some(chunk_size),
        has_signature,
    };

    let init_url = format!("{}/api/packages/upload/initiate", base_url);
    let response = client.post(&init_url).json(&init_req).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Failed to initiate upload - HTTP {}: {}", status, body).into());
    }

    let init_resp: InitiateUploadResponse = response.json().await?;
    let upload_id = init_resp.upload_id;
    let chunk_size = init_resp.chunk_size;
    let total_chunks = init_resp.total_chunks;

    tracing::info!(
        "[{}/{}] Upload ID: {}, {} chunks of {} bytes",
        index,
        total,
        upload_id,
        total_chunks,
        chunk_size
    );

    // Create progress bar
    let progress = ProgressBar::new(file_size);
    progress.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({eta})"
            )?
            .progress_chars("#>-"),
    );

    // Upload chunks
    let mut file = File::open(path).await?;
    let mut chunk_infos = Vec::new();

    for chunk_num in 1..=total_chunks {
        let mut chunk_data = vec![0u8; chunk_size];
        let bytes_read = file.read(&mut chunk_data).await?;
        chunk_data.truncate(bytes_read);

        // Upload chunk with retry
        let chunk_info =
            upload_chunk_with_retry(client, base_url, &upload_id, chunk_num, &chunk_data, 3)
                .await?;

        chunk_infos.push(chunk_info);
        progress.inc(bytes_read as u64);
    }

    progress.finish_with_message("Upload complete");

    // Upload signature if present
    if has_signature {
        tracing::info!("[{}/{}] Uploading signature...", index, total);
        let sig_data = tokio::fs::read(&sig_path).await?;
        let sig_url = format!("{}/api/packages/upload/{}/signature", base_url, upload_id);

        let response = client
            .post(&sig_url)
            .header("Content-Type", "application/octet-stream")
            .body(sig_data)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            tracing::warn!("Failed to upload signature - HTTP {}: {}", status, body);
        }
    }

    // Complete upload
    tracing::info!("[{}/{}] Completing upload...", index, total);
    let complete_req = CompleteUploadRequest {
        chunks: chunk_infos,
    };

    let complete_url = format!("{}/api/packages/upload/{}/complete", base_url, upload_id);
    let response = client
        .post(&complete_url)
        .json(&complete_req)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Failed to complete upload - HTTP {}: {}", status, body).into());
    }

    let package = response.json::<Package>().await?;
    Ok(package)
}

/// Upload a chunk with retry logic
async fn upload_chunk_with_retry(
    client: &reqwest::Client,
    base_url: &str,
    upload_id: &str,
    chunk_number: u32,
    data: &[u8],
    max_retries: u32,
) -> Result<ChunkInfo, Box<dyn std::error::Error>> {
    let mut retries = 0;

    loop {
        let url = format!(
            "{}/api/packages/upload/{}/chunks/{}",
            base_url, upload_id, chunk_number
        );

        let response = client
            .post(&url)
            .header("Content-Type", "application/octet-stream")
            .body(data.to_vec())
            .send()
            .await;

        match response {
            Ok(resp) if resp.status().is_success() => {
                let chunk_resp: UploadChunkResponse = resp.json().await?;
                return Ok(ChunkInfo {
                    chunk_number: chunk_resp.chunk_number,
                    checksum: chunk_resp.checksum,
                });
            }
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();

                if retries < max_retries {
                    retries += 1;
                    let delay = std::time::Duration::from_millis(1000 * retries as u64);
                    tracing::warn!(
                        "Chunk {} upload failed (HTTP {}), retrying in {:?}... ({}/{})",
                        chunk_number,
                        status,
                        delay,
                        retries,
                        max_retries
                    );
                    tokio::time::sleep(delay).await;
                    continue;
                } else {
                    return Err(format!(
                        "Chunk {} upload failed after {} retries: HTTP {}: {}",
                        chunk_number, max_retries, status, body
                    )
                    .into());
                }
            }
            Err(e) => {
                if retries < max_retries {
                    retries += 1;
                    let delay = std::time::Duration::from_millis(1000 * retries as u64);
                    tracing::warn!(
                        "Chunk {} upload error: {}, retrying in {:?}... ({}/{})",
                        chunk_number,
                        e,
                        delay,
                        retries,
                        max_retries
                    );
                    tokio::time::sleep(delay).await;
                    continue;
                } else {
                    return Err(format!(
                        "Chunk {} upload failed after {} retries: {}",
                        chunk_number, max_retries, e
                    )
                    .into());
                }
            }
        }
    }
}

/// Delete package versions from the repository
async fn delete_versions(
    client: &reqwest::Client,
    base_url: &str,
    name: &str,
    versions: Vec<String>,
    repo: Option<String>,
    arch: Option<String>,
) -> Result<DeleteVersionsResponse, Box<dyn std::error::Error>> {
    let url = format!("{base_url}/api/packages/{name}/versions/delete");

    let request = DeleteVersionsRequest {
        versions,
        repo,
        arch,
    };

    let response = client.post(&url).json(&request).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Failed to delete versions - HTTP {status}: {body}").into());
    }

    let delete_response = response.json::<DeleteVersionsResponse>().await?;
    Ok(delete_response)
}

/// Print success message for deleted versions
fn print_delete_success(name: &str, response: &DeleteVersionsResponse) {
    println!(
        "\n{}",
        format!(
            "✓ Successfully deleted {} version(s)",
            response.deleted_count
        )
        .green()
        .bold()
    );
    println!();
    println!("  {:>9}  {}", "Package:".cyan().bold(), name);
    println!(
        "  {:>9}  {}",
        "Deleted:".cyan().bold(),
        response.deleted_count.to_string().yellow()
    );

    if !response.deleted_versions.is_empty() {
        println!("  {:>9}", "Versions:".cyan().bold());
        for version in &response.deleted_versions {
            println!("    - {}", version);
        }
    }
    println!();
}

/// Print success message for uploaded package
fn print_upload_success(package: &Package, index: usize, total: usize) {
    println!(
        "\n{}",
        format!("[{}/{}] ✓ Package uploaded successfully", index, total)
            .green()
            .bold()
    );
    println!();
    println!("  {:>9}  {}", "Name:".cyan().bold(), package.name);
    println!("  {:>9}  {}", "Version:".cyan().bold(), package.version);
    println!("  {:>9}  {}", "Arch:".cyan().bold(), package.arch);
    println!("  {:>9}  {}", "Repo:".cyan().bold(), package.repo);
    println!("  {:>9}  {}", "Filename:".cyan().bold(), package.filename);
    println!(
        "  {:>9}  {} bytes",
        "Size:".cyan().bold(),
        package.size.to_string().yellow()
    );
    println!(
        "  {:>9}  {}",
        "SHA256:".cyan().bold(),
        package.sha256.bright_black()
    );
    println!(
        "  {:>9}  {}",
        "Created:".cyan().bold(),
        package.created_at.format("%Y-%m-%d %H:%M:%S UTC")
    );
    println!();
}
