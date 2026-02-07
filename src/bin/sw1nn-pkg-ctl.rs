use byte_unit::{Byte, UnitType};
use clap::{Parser, Subcommand, ValueEnum};
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

    /// Color output mode (also respects NO_COLOR and FORCE_COLOR env vars)
    #[arg(
        long,
        visible_alias = "colour",
        value_enum,
        default_value = "auto",
        global = true
    )]
    color: ColorMode,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ColorMode {
    /// Auto-detect based on terminal
    Auto,
    /// Always use colors
    Always,
    /// Never use colors
    Never,
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
    /// Replace an erroneously uploaded package (interactive confirmation required)
    Replace {
        /// Path to the replacement package file (.pkg.tar.zst)
        package_file: String,
        /// Repository name (required if package exists in multiple repos)
        #[arg(short, long)]
        repo: Option<String>,
    },
    /// List packages in the repository
    List {
        /// Filter packages by name (substring match)
        #[arg(short = 'n', long)]
        name: Option<String>,
        /// Filter by repository name
        #[arg(short = 'R', long)]
        repo: Option<String>,
        /// Filter by architecture
        #[arg(short = 'a', long)]
        arch: Option<String>,
        /// Output as JSON instead of table
        #[arg(short = 'j', long)]
        json: bool,
        /// Size unit format
        #[arg(short = 'u', long, value_enum, default_value = "binary")]
        unit: SizeUnit,
        /// Sort by field
        #[arg(short = 's', long, value_enum, default_value = "name")]
        sort: SortField,
        /// Reverse sort order
        #[arg(short = 'r', long)]
        reverse: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SizeUnit {
    /// Binary units (KiB, MiB, GiB) - base 1024
    Binary,
    /// Decimal units (KB, MB, GB) - base 1000
    Decimal,
    /// Raw bytes
    Bytes,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SortField {
    /// Sort by package name
    Name,
    /// Sort by version
    Version,
    /// Sort by size
    Size,
    /// Sort by creation time
    Created,
    /// Sort by architecture
    Arch,
    /// Sort by repository
    Repo,
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

    // Configure color output
    configure_colors(args.color);

    let base_url =
        std::env::var("SW1NN_REPO_URL").unwrap_or_else(|_| "https://repo.sw1nn.net".to_string());

    let client = reqwest::Client::new();

    // Handle subcommands or backwards-compatible positional args
    match args.command {
        Some(Commands::Upload { package_files }) => {
            run_upload(&client, &base_url, package_files).await;
        }
        Some(Commands::Replace { package_file, repo }) => {
            run_replace(&client, &base_url, &package_file, repo).await;
        }
        Some(Commands::Delete {
            name,
            version,
            repo,
            arch,
        }) => {
            run_delete(&client, &base_url, name, version, repo, arch).await;
        }
        Some(Commands::List {
            name,
            repo,
            arch,
            json,
            unit,
            sort,
            reverse,
        }) => {
            run_list(
                &client, &base_url, name, repo, arch, json, unit, sort, reverse,
            )
            .await;
        }
        None => {
            // Backwards compatibility: treat positional args as upload
            if args.package_files.is_empty() {
                tracing::error!(
                    "No command specified. Use 'upload', 'delete', 'replace', or 'list' subcommand, or provide package files directly."
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

async fn run_replace(
    client: &reqwest::Client,
    base_url: &str,
    package_file: &str,
    repo_filter: Option<String>,
) {
    let path = Path::new(package_file);

    // Validate file exists
    if !path.exists() {
        tracing::error!(path = package_file, "File does not exist");
        process::exit(1);
    }

    // Validate extension
    if !package_file.ends_with(".pkg.tar.zst") {
        tracing::error!(path = package_file, "File must be a .pkg.tar.zst package");
        process::exit(1);
    }

    let filename = path.file_name().unwrap().to_string_lossy().into_owned();

    // Compute SHA256 of the new file
    tracing::info!("Calculating SHA256 of replacement file...");
    let new_file_data = tokio::fs::read(path).await.unwrap_or_else(|e| {
        tracing::error!(error = %e, "Failed to read file");
        process::exit(1);
    });
    let new_sha256 = format!("{:x}", sha2::Sha256::digest(&new_file_data));
    let new_size = new_file_data.len() as u64;

    // Query existing packages
    let packages = list_packages(client, base_url).await.unwrap_or_else(|e| {
        tracing::error!(error = %e, "Failed to query packages");
        process::exit(1);
    });

    // Find matching packages by filename
    let mut matches: Vec<&Package> = packages.iter().filter(|p| p.filename == filename).collect();

    // Apply repo filter if provided
    if let Some(ref repo) = repo_filter {
        matches.retain(|p| &p.repo == repo);
    }

    if matches.is_empty() {
        tracing::error!(
            filename,
            "No existing package found with this filename on the server"
        );
        process::exit(1);
    }

    if matches.len() > 1 {
        tracing::error!(
            filename,
            repos = ?matches.iter().map(|p| &p.repo).collect::<Vec<_>>(),
            "Package exists in multiple repos — use --repo to specify which one"
        );
        process::exit(1);
    }

    let existing = matches[0];

    // Check if files are identical
    if existing.sha256 == new_sha256 {
        println!(
            "\n{}",
            "⚠ Replacement file is identical to the existing package (same SHA256). Aborting."
                .yellow()
                .bold()
        );
        process::exit(1);
    }

    // Display comparison
    let existing_size_str = format_size(existing.size, SizeUnit::Binary);
    let new_size_str = format_size(new_size, SizeUnit::Binary);

    println!();
    println!("{}", "⚠ Package Replacement".yellow().bold());
    println!("{}", "=".repeat(50));
    println!("  {:>11}  {}", "Package:".cyan().bold(), existing.name);
    println!("  {:>11}  {}", "Version:".cyan().bold(), existing.version);
    println!("  {:>11}  {}", "Arch:".cyan().bold(), existing.arch);
    println!("  {:>11}  {}", "Repo:".cyan().bold(), existing.repo);
    println!("  {:>11}  {}", "Filename:".cyan().bold(), existing.filename);
    println!();
    println!("  {}", "Existing package:".bold());
    println!(
        "    {:>9}  {}",
        "SHA256:".cyan(),
        existing.sha256.bright_black()
    );
    println!(
        "    {:>9}  {}",
        "Size:".cyan(),
        existing_size_str.bright_black()
    );
    println!(
        "    {:>9}  {}",
        "Uploaded:".cyan(),
        existing
            .created_at
            .format("%Y-%m-%d %H:%M:%S UTC")
            .to_string()
            .bright_black()
    );
    println!();
    println!("  {}", "Replacement file:".bold());
    println!("    {:>9}  {}", "SHA256:".cyan(), new_sha256.bright_black());
    println!("    {:>9}  {}", "Size:".cyan(), new_size_str.bright_black());
    println!("{}", "=".repeat(50));
    println!();

    // Interactive confirmation
    let confirmation = read_confirmation(&format!(
        "Type the package name ({}) to confirm replacement: ",
        existing.name.red().bold()
    ));

    if confirmation != existing.name {
        println!("{}", "Aborted — package name did not match.".red().bold());
        process::exit(1);
    }

    // Delete existing version
    tracing::info!(
        name = %existing.name,
        version = %existing.version,
        "Deleting existing package version"
    );
    let delete_result = delete_versions(
        client,
        base_url,
        &existing.name,
        vec![existing.version.clone()],
        Some(existing.repo.clone()),
        Some(existing.arch.clone()),
    )
    .await;

    if let Err(e) = delete_result {
        tracing::error!(error = %e, "Failed to delete existing package");
        process::exit(1);
    }

    // Upload replacement
    tracing::info!("Uploading replacement package...");
    let upload_result = upload_chunked(client, base_url, path, 1, 1).await;

    match upload_result {
        Ok(package) => {
            println!("\n{}", "✓ Package replaced successfully".green().bold());
            println!();
            println!("  {:>11}  {}", "Name:".cyan().bold(), package.name);
            println!("  {:>11}  {}", "Version:".cyan().bold(), package.version);
            println!("  {:>11}  {}", "Arch:".cyan().bold(), package.arch);
            println!("  {:>11}  {}", "Repo:".cyan().bold(), package.repo);
            println!(
                "  {:>11}  {}",
                "SHA256:".cyan().bold(),
                package.sha256.bright_black()
            );
            println!(
                "  {:>11}  {}",
                "Size:".cyan().bold(),
                format_size(package.size, SizeUnit::Binary).bright_black()
            );
            println!();
        }
        Err(e) => {
            tracing::error!(
                error = %e,
                "Failed to upload replacement — the original package has been deleted!"
            );
            process::exit(1);
        }
    }
}

fn read_confirmation(prompt: &str) -> String {
    use std::io::Write;
    print!("{prompt}");
    std::io::stdout().flush().expect("failed to flush stdout");
    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .expect("failed to read from stdin");
    input.trim().to_string()
}

fn configure_colors(mode: ColorMode) {
    // Check environment variables first (they take precedence)
    if std::env::var("NO_COLOR").is_ok() {
        colored::control::set_override(false);
        return;
    }
    if std::env::var("FORCE_COLOR").is_ok() {
        colored::control::set_override(true);
        return;
    }

    // Apply CLI option
    match mode {
        ColorMode::Auto => {
            // Let colored crate auto-detect (default behavior)
        }
        ColorMode::Always => {
            colored::control::set_override(true);
        }
        ColorMode::Never => {
            colored::control::set_override(false);
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_list(
    client: &reqwest::Client,
    base_url: &str,
    name_filter: Option<String>,
    repo_filter: Option<String>,
    arch_filter: Option<String>,
    json_output: bool,
    size_unit: SizeUnit,
    sort_field: SortField,
    reverse: bool,
) {
    let result = list_packages(client, base_url).await;

    match result {
        Ok(mut packages) => {
            // Apply filters
            if let Some(ref name) = name_filter {
                let name_lower = name.to_lowercase();
                packages.retain(|p| p.name.to_lowercase().contains(&name_lower));
            }
            if let Some(ref repo) = repo_filter {
                packages.retain(|p| p.repo == *repo);
            }
            if let Some(ref arch) = arch_filter {
                packages.retain(|p| p.arch == *arch);
            }

            // Sort packages
            packages.sort_by(|a, b| {
                let cmp = match sort_field {
                    SortField::Name => a.name.cmp(&b.name).then_with(|| a.version.cmp(&b.version)),
                    SortField::Version => {
                        a.version.cmp(&b.version).then_with(|| a.name.cmp(&b.name))
                    }
                    SortField::Size => a.size.cmp(&b.size).then_with(|| a.name.cmp(&b.name)),
                    SortField::Created => a
                        .created_at
                        .cmp(&b.created_at)
                        .then_with(|| a.name.cmp(&b.name)),
                    SortField::Arch => a.arch.cmp(&b.arch).then_with(|| a.name.cmp(&b.name)),
                    SortField::Repo => a.repo.cmp(&b.repo).then_with(|| a.name.cmp(&b.name)),
                };
                if reverse { cmp.reverse() } else { cmp }
            });

            if json_output {
                print_packages_json(&packages);
            } else {
                print_packages_table(&packages, size_unit);
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to list packages");
            process::exit(1);
        }
    }
}

async fn list_packages(
    client: &reqwest::Client,
    base_url: &str,
) -> Result<Vec<Package>, Box<dyn std::error::Error>> {
    let url = format!("{base_url}/api/packages");
    let response = client.get(&url).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Failed to list packages - HTTP {status}: {body}").into());
    }

    let packages = response.json::<Vec<Package>>().await?;
    Ok(packages)
}

fn print_packages_json(packages: &[Package]) {
    match serde_json::to_string_pretty(packages) {
        Ok(json) => println!("{json}"),
        Err(e) => {
            tracing::error!(error = %e, "Failed to serialize packages to JSON");
            process::exit(1);
        }
    }
}

fn print_packages_table(packages: &[Package], size_unit: SizeUnit) {
    if packages.is_empty() {
        println!("{}", "No packages found.".yellow());
        return;
    }

    // Calculate column widths
    let name_width = packages
        .iter()
        .map(|p| p.name.len())
        .max()
        .unwrap_or(4)
        .max(4);
    // Fixed width for version column
    let version_width = 12;
    let arch_width = packages
        .iter()
        .map(|p| p.arch.len())
        .max()
        .unwrap_or(4)
        .max(4);
    let repo_width = packages
        .iter()
        .map(|p| p.repo.len())
        .max()
        .unwrap_or(4)
        .max(4);

    // Print header
    println!(
        "{:name_width$}  {:version_width$}  {:arch_width$}  {:repo_width$}  {:>10}  {}",
        "NAME".cyan().bold(),
        "VERSION".cyan().bold(),
        "ARCH".cyan().bold(),
        "REPO".cyan().bold(),
        "SIZE".cyan().bold(),
        "CREATED".cyan().bold(),
    );
    println!(
        "{}",
        "-".repeat(name_width + version_width + arch_width + repo_width + 10 + 20 + 12)
            .bright_black()
    );

    // Print rows
    for pkg in packages {
        let size_str = format_size(pkg.size, size_unit);
        let created_str = pkg.created_at.format("%Y-%m-%d %H:%M").to_string();

        let version_str = format_version(&pkg.version, version_width);
        println!(
            "{:name_width$}  {}  {:arch_width$}  {:repo_width$}  {:>10}  {}",
            pkg.name.green(),
            version_str,
            pkg.arch,
            pkg.repo,
            size_str.bright_black(),
            created_str.bright_black(),
        );
    }

    println!();
    println!(
        "{} {} package(s)",
        "Total:".cyan().bold(),
        packages.len().to_string().yellow()
    );
}

fn format_size(bytes: u64, unit: SizeUnit) -> String {
    let byte = Byte::from_u64(bytes);
    match unit {
        SizeUnit::Binary => format!("{:.1}", byte.get_appropriate_unit(UnitType::Binary)),
        SizeUnit::Decimal => format!("{:.1}", byte.get_appropriate_unit(UnitType::Decimal)),
        SizeUnit::Bytes => format!("{bytes} B"),
    }
}

/// Format version string with pkgver in yellow and pkgrel in grey, left-aligned to width
fn format_version(version: &str, width: usize) -> String {
    // Arch package versions are formatted as pkgver-pkgrel
    // e.g., "2.1.0-1" where "2.1.0" is pkgver and "1" is pkgrel
    let colored = if let Some(last_dash_pos) = version.rfind('-') {
        let pkgver = &version[..last_dash_pos];
        let pkgrel = &version[last_dash_pos..]; // includes the '-'
        format!("{}{}", pkgver.yellow(), pkgrel.bright_black())
    } else {
        // No pkgrel separator found, just color the whole thing yellow
        version.yellow().to_string()
    };

    // Left-align by appending spaces (ANSI codes don't count for visible width)
    let padding = width.saturating_sub(version.len());
    format!("{colored}{:padding$}", "")
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
