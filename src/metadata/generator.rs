use crate::error::Result;
use crate::models::{Package, PkgInfo};
use flate2::Compression;
use flate2::write::GzEncoder;
use std::path::Path;
use tar::Builder;
use tokio::fs;

/// Generate desc file content for a package
pub fn generate_desc(pkg: &Package, pkginfo: &PkgInfo) -> String {
    let mut desc = String::new();

    // Required fields
    desc.push_str("%FILENAME%\n");
    desc.push_str(&format!("{}\n\n", pkg.filename));

    desc.push_str("%NAME%\n");
    desc.push_str(&format!("{}\n\n", pkg.name));

    desc.push_str("%VERSION%\n");
    desc.push_str(&format!("{}\n\n", pkg.version));

    // Optional description
    if let Some(ref pkgdesc) = pkginfo.pkgdesc {
        desc.push_str("%DESC%\n");
        desc.push_str(&format!("{}\n\n", pkgdesc));
    }

    // Architecture
    desc.push_str("%ARCH%\n");
    desc.push_str(&format!("{}\n\n", pkg.arch));

    // Build date
    if let Some(ref builddate) = pkginfo.builddate {
        desc.push_str("%BUILDDATE%\n");
        desc.push_str(&format!("{}\n\n", builddate));
    }

    // Packager
    if let Some(ref packager) = pkginfo.packager {
        desc.push_str("%PACKAGER%\n");
        desc.push_str(&format!("{}\n\n", packager));
    }

    // Compressed size
    desc.push_str("%CSIZE%\n");
    desc.push_str(&format!("{}\n\n", pkg.size));

    // Installed size
    if let Some(size) = pkginfo.size {
        desc.push_str("%ISIZE%\n");
        desc.push_str(&format!("{}\n\n", size));
    }

    // SHA256 checksum
    desc.push_str("%SHA256SUM%\n");
    desc.push_str(&format!("{}\n\n", pkg.sha256));

    // URL
    if let Some(ref url) = pkginfo.url {
        desc.push_str("%URL%\n");
        desc.push_str(&format!("{}\n\n", url));
    }

    // License
    if !pkginfo.license.is_empty() {
        desc.push_str("%LICENSE%\n");
        for license in &pkginfo.license {
            desc.push_str(&format!("{}\n", license));
        }
        desc.push_str("\n");
    }

    // Dependencies
    if !pkginfo.depends.is_empty() {
        desc.push_str("%DEPENDS%\n");
        for dep in &pkginfo.depends {
            desc.push_str(&format!("{}\n", dep));
        }
        desc.push_str("\n");
    }

    // Optional dependencies
    if !pkginfo.optdepends.is_empty() {
        desc.push_str("%OPTDEPENDS%\n");
        for dep in &pkginfo.optdepends {
            desc.push_str(&format!("{}\n", dep));
        }
        desc.push_str("\n");
    }

    // Conflicts
    if !pkginfo.conflicts.is_empty() {
        desc.push_str("%CONFLICTS%\n");
        for conflict in &pkginfo.conflicts {
            desc.push_str(&format!("{}\n", conflict));
        }
        desc.push_str("\n");
    }

    // Provides
    if !pkginfo.provides.is_empty() {
        desc.push_str("%PROVIDES%\n");
        for provides in &pkginfo.provides {
            desc.push_str(&format!("{}\n", provides));
        }
        desc.push_str("\n");
    }

    // Replaces
    if !pkginfo.replaces.is_empty() {
        desc.push_str("%REPLACES%\n");
        for replaces in &pkginfo.replaces {
            desc.push_str(&format!("{}\n", replaces));
        }
        desc.push_str("\n");
    }

    // Groups
    if !pkginfo.groups.is_empty() {
        desc.push_str("%GROUPS%\n");
        for group in &pkginfo.groups {
            desc.push_str(&format!("{}\n", group));
        }
        desc.push_str("\n");
    }

    desc
}

/// Generate repository database
pub async fn generate_repo_db(
    repo_dir: &Path,
    repo_name: &str,
    packages: &[(Package, PkgInfo)],
) -> Result<()> {
    let db_path = repo_dir.join(format!("{}.db.tar.gz", repo_name));
    let db_link = repo_dir.join(format!("{}.db", repo_name));

    // Clone data needed for blocking task
    let packages = packages.to_vec();
    let db_path_clone = db_path.clone();

    // Create tar.gz archive in blocking task (CPU-intensive compression)
    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::create(&db_path_clone)?;
        let encoder = GzEncoder::new(file, Compression::default());
        let mut tar = Builder::new(encoder);

        // Add each package's desc file
        for (pkg, pkginfo) in &packages {
            let desc_content = generate_desc(pkg, pkginfo);
            let entry_path = format!("{}-{}/desc", pkg.name, pkg.version);

            let mut header = tar::Header::new_gnu();
            header.set_path(&entry_path)?;
            header.set_size(desc_content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();

            tar.append(&header, desc_content.as_bytes())?;
        }

        tar.finish()?;
        Ok::<_, crate::error::Error>(())
    })
    .await
    .map_err(|e| crate::error::Error::Io(std::io::Error::new(
        std::io::ErrorKind::Other,
        format!("Task join error: {}", e),
    )))??;

    // Create symlink
    if db_link.exists() {
        fs::remove_file(&db_link).await?;
    }

    let repo_name = repo_name.to_string();
    let db_link_clone = db_link.clone();

    // Symlink creation in blocking task
    tokio::task::spawn_blocking(move || {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(format!("{}.db.tar.gz", repo_name), &db_link_clone)?;
        }

        #[cfg(not(unix))]
        {
            // On non-Unix systems, just copy the file
            std::fs::copy(&db_path, &db_link_clone)?;
        }
        Ok::<_, std::io::Error>(())
    })
    .await
    .map_err(|e| std::io::Error::new(
        std::io::ErrorKind::Other,
        format!("Task join error: {}", e),
    ))??;

    Ok(())
}

/// Generate files database (simplified version - just contains filenames for now)
pub async fn generate_files_db(
    repo_dir: &Path,
    repo_name: &str,
    packages: &[(Package, PkgInfo)],
) -> Result<()> {
    let files_path = repo_dir.join(format!("{}.files.tar.gz", repo_name));
    let files_link = repo_dir.join(format!("{}.files", repo_name));

    // Clone data needed for blocking task
    let packages = packages.to_vec();
    let files_path_clone = files_path.clone();

    // Create tar.gz archive in blocking task (CPU-intensive compression)
    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::create(&files_path_clone)?;
        let encoder = GzEncoder::new(file, Compression::default());
        let mut tar = Builder::new(encoder);

        // Add each package's files entry (simplified - would need full file listing)
        for (pkg, pkginfo) in &packages {
            let mut files_content = String::new();

            // Add desc content
            files_content.push_str(&generate_desc(pkg, pkginfo));

            // Add placeholder files section
            files_content.push_str("%FILES%\n\n");

            let entry_path = format!("{}-{}/files", pkg.name, pkg.version);

            let mut header = tar::Header::new_gnu();
            header.set_path(&entry_path)?;
            header.set_size(files_content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();

            tar.append(&header, files_content.as_bytes())?;
        }

        tar.finish()?;
        Ok::<_, crate::error::Error>(())
    })
    .await
    .map_err(|e| crate::error::Error::Io(std::io::Error::new(
        std::io::ErrorKind::Other,
        format!("Task join error: {}", e),
    )))??;

    // Create symlink
    if files_link.exists() {
        fs::remove_file(&files_link).await?;
    }

    let repo_name = repo_name.to_string();
    let files_link_clone = files_link.clone();

    // Symlink creation in blocking task
    tokio::task::spawn_blocking(move || {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(format!("{}.files.tar.gz", repo_name), &files_link_clone)?;
        }

        #[cfg(not(unix))]
        {
            // On non-Unix systems, just copy the file
            std::fs::copy(&files_path, &files_link_clone)?;
        }
        Ok::<_, std::io::Error>(())
    })
    .await
    .map_err(|e| std::io::Error::new(
        std::io::ErrorKind::Other,
        format!("Task join error: {}", e),
    ))??;

    Ok(())
}
