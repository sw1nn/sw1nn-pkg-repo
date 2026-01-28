use clap::{Parser, Subcommand};
use std::path::PathBuf;
use sw1nn_pkg_repo::run_service;
use tokio::fs;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser, Debug)]
#[command(name = "sw1nn-pkg-repod")]
#[command(about = "Package repository server", long_about = None)]
#[command(version = VERSION)]
struct Args {
    /// Path to configuration file
    #[arg(short, long, value_name = "FILE", global = true)]
    config: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run the server (default)
    Serve,
    /// Migrate from old storage structure to new flat structure
    Migrate {
        /// Data directory path (defaults to config value)
        #[arg(short, long)]
        data_path: Option<PathBuf>,
        /// Dry run - show what would be done without making changes
        #[arg(long)]
        dry_run: bool,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    match args.command {
        Some(Commands::Migrate { data_path, dry_run }) => {
            run_migration(args.config.as_deref(), data_path, dry_run).await
        }
        Some(Commands::Serve) | None => run_service(args.config.as_deref()).await,
    }
}

/// Run the storage migration from old structure to new flat structure
async fn run_migration(
    config_path: Option<&str>,
    data_path_override: Option<PathBuf>,
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Load config to get data path
    let config = sw1nn_pkg_repo::config::Config::load(config_path)?;
    let data_path = data_path_override.unwrap_or_else(|| config.storage.data_path.clone());

    tracing::info!(data_path = %data_path.display(), dry_run, "Starting storage migration");

    if !data_path.exists() {
        tracing::info!("Data directory does not exist, nothing to migrate");
        return Ok(());
    }

    // Iterate through all repos
    let mut repo_entries = fs::read_dir(&data_path).await?;
    while let Some(repo_entry) = repo_entries.next_entry().await? {
        if !repo_entry.path().is_dir() {
            continue;
        }

        let repo_name = repo_entry.file_name().to_string_lossy().into_owned();
        let repo_path = repo_entry.path();

        // Check for old structure: {repo}/os/{arch}/
        let os_dir = repo_path.join("os");
        if !os_dir.exists() {
            tracing::debug!(repo = repo_name, "No os/ directory, skipping");
            continue;
        }

        // Check if already migrated (has packages/ directory)
        let packages_dir = repo_path.join("packages");
        let metadata_dir = repo_path.join("metadata");

        if packages_dir.exists() && metadata_dir.exists() {
            tracing::info!(
                repo = repo_name,
                "Already migrated (has packages/ and metadata/ dirs)"
            );
            continue;
        }

        tracing::info!(repo = repo_name, "Migrating repository");

        // Create new directories
        if !dry_run {
            fs::create_dir_all(&packages_dir).await?;
            fs::create_dir_all(&metadata_dir).await?;
        }
        tracing::info!(
            repo = repo_name,
            "Created packages/ and metadata/ directories"
        );

        // Iterate through architectures
        let mut arch_entries = fs::read_dir(&os_dir).await?;
        while let Some(arch_entry) = arch_entries.next_entry().await? {
            if !arch_entry.path().is_dir() {
                continue;
            }

            let arch_name = arch_entry.file_name().to_string_lossy().into_owned();
            let arch_path = arch_entry.path();

            tracing::info!(
                repo = repo_name,
                arch = arch_name,
                "Processing architecture"
            );

            // Move package files
            let mut pkg_entries = fs::read_dir(&arch_path).await?;
            while let Some(pkg_entry) = pkg_entries.next_entry().await? {
                let pkg_path = pkg_entry.path();
                let filename = pkg_entry.file_name().to_string_lossy().into_owned();

                // Skip metadata directory and database files
                if filename == "metadata" {
                    continue;
                }
                if filename.ends_with(".db")
                    || filename.ends_with(".db.tar.gz")
                    || filename.ends_with(".files")
                    || filename.ends_with(".files.tar.gz")
                {
                    tracing::debug!(filename, "Keeping database file in place");
                    continue;
                }

                // Move .pkg.tar.zst and .sig files
                if filename.ends_with(".pkg.tar.zst") || filename.ends_with(".pkg.tar.zst.sig") {
                    let dest_path = packages_dir.join(&filename);
                    if dest_path.exists() {
                        tracing::warn!(filename, "Destination already exists, skipping");
                        continue;
                    }

                    tracing::info!(
                        src = %pkg_path.display(),
                        dest = %dest_path.display(),
                        "Moving package file"
                    );
                    if !dry_run {
                        fs::rename(&pkg_path, &dest_path).await?;
                    }
                }
            }

            // Move metadata files
            let old_metadata_dir = arch_path.join("metadata");
            if old_metadata_dir.exists() {
                let mut meta_entries = fs::read_dir(&old_metadata_dir).await?;
                while let Some(meta_entry) = meta_entries.next_entry().await? {
                    let meta_path = meta_entry.path();
                    let filename = meta_entry.file_name().to_string_lossy().into_owned();

                    if filename.ends_with(".json") {
                        let dest_path = metadata_dir.join(&filename);
                        if dest_path.exists() {
                            tracing::warn!(filename, "Metadata file already exists, skipping");
                            continue;
                        }

                        tracing::info!(
                            src = %meta_path.display(),
                            dest = %dest_path.display(),
                            "Moving metadata file"
                        );
                        if !dry_run {
                            fs::rename(&meta_path, &dest_path).await?;
                        }
                    }
                }

                // Remove empty old metadata directory
                if !dry_run && let Err(e) = fs::remove_dir(&old_metadata_dir).await {
                    tracing::debug!(
                        path = %old_metadata_dir.display(),
                        error = %e,
                        "Could not remove old metadata directory (may not be empty)"
                    );
                }
            }
        }
    }

    if dry_run {
        tracing::info!("Dry run complete - no changes were made");
    } else {
        tracing::info!("Migration complete");
        tracing::info!("Run the server to rebuild databases with the new structure");
    }

    Ok(())
}
