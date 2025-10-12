use serde::{Deserialize, Serialize};

/// Represents the .PKGINFO file contents from an Arch package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PkgInfo {
    pub pkgname: String,
    pub pkgver: String,
    pub pkgdesc: Option<String>,
    pub url: Option<String>,
    pub builddate: Option<String>,
    pub packager: Option<String>,
    pub size: Option<u64>,
    pub arch: String,
    pub license: Vec<String>,
    pub replaces: Vec<String>,
    pub groups: Vec<String>,
    pub conflicts: Vec<String>,
    pub provides: Vec<String>,
    pub backup: Vec<String>,
    pub depends: Vec<String>,
    pub optdepends: Vec<String>,
    pub makedepends: Vec<String>,
    pub checkdepends: Vec<String>,
}

impl PkgInfo {
    /// Parse .PKGINFO content
    pub fn parse(content: &str) -> Result<Self, String> {
        let mut pkgname = None;
        let mut pkgver = None;
        let mut pkgdesc = None;
        let mut url = None;
        let mut builddate = None;
        let mut packager = None;
        let mut size = None;
        let mut arch = None;
        let mut license = Vec::new();
        let mut replaces = Vec::new();
        let mut groups = Vec::new();
        let mut conflicts = Vec::new();
        let mut provides = Vec::new();
        let mut backup = Vec::new();
        let mut depends = Vec::new();
        let mut optdepends = Vec::new();
        let mut makedepends = Vec::new();
        let mut checkdepends = Vec::new();

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            if let Some((key, value)) = line.split_once(" = ") {
                let value = value.trim();
                match key.trim() {
                    "pkgname" => pkgname = Some(value.to_string()),
                    "pkgver" => pkgver = Some(value.to_string()),
                    "pkgdesc" => pkgdesc = Some(value.to_string()),
                    "url" => url = Some(value.to_string()),
                    "builddate" => builddate = Some(value.to_string()),
                    "packager" => packager = Some(value.to_string()),
                    "size" => size = value.parse().ok(),
                    "arch" => arch = Some(value.to_string()),
                    "license" => license.push(value.to_string()),
                    "replaces" => replaces.push(value.to_string()),
                    "group" => groups.push(value.to_string()),
                    "conflict" => conflicts.push(value.to_string()),
                    "provides" => provides.push(value.to_string()),
                    "backup" => backup.push(value.to_string()),
                    "depend" => depends.push(value.to_string()),
                    "optdepend" => optdepends.push(value.to_string()),
                    "makedepend" => makedepends.push(value.to_string()),
                    "checkdepend" => checkdepends.push(value.to_string()),
                    _ => {}
                }
            }
        }

        Ok(PkgInfo {
            pkgname: pkgname.ok_or("Missing pkgname")?,
            pkgver: pkgver.ok_or("Missing pkgver")?,
            pkgdesc,
            url,
            builddate,
            packager,
            size,
            arch: arch.ok_or("Missing arch")?,
            license,
            replaces,
            groups,
            conflicts,
            provides,
            backup,
            depends,
            optdepends,
            makedepends,
            checkdepends,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_pkginfo() {
        let content = r#"
pkgname = test-package
pkgver = 1.0.0-1
arch = x86_64
"#;

        let pkginfo = PkgInfo::parse(content).unwrap();
        assert_eq!(pkginfo.pkgname, "test-package");
        assert_eq!(pkginfo.pkgver, "1.0.0-1");
        assert_eq!(pkginfo.arch, "x86_64");
        assert_eq!(pkginfo.license.len(), 0);
        assert_eq!(pkginfo.depends.len(), 0);
    }

    #[test]
    fn test_parse_full_pkginfo() {
        let content = r#"
pkgname = test-package
pkgver = 1.0.0-1
pkgdesc = A test package
url = https://example.com
builddate = 1234567890
packager = Test Packager <test@example.com>
size = 1024
arch = x86_64
license = MIT
license = GPL
depend = glibc
depend = gcc
optdepend = python: for optional feature
"#;

        let pkginfo = PkgInfo::parse(content).unwrap();
        assert_eq!(pkginfo.pkgname, "test-package");
        assert_eq!(pkginfo.pkgver, "1.0.0-1");
        assert_eq!(pkginfo.pkgdesc.unwrap(), "A test package");
        assert_eq!(pkginfo.url.unwrap(), "https://example.com");
        assert_eq!(pkginfo.license.len(), 2);
        assert_eq!(pkginfo.license[0], "MIT");
        assert_eq!(pkginfo.license[1], "GPL");
        assert_eq!(pkginfo.depends.len(), 2);
        assert_eq!(pkginfo.depends[0], "glibc");
        assert_eq!(pkginfo.optdepends.len(), 1);
    }

    #[test]
    fn test_parse_missing_required_field() {
        let content = r#"
pkgname = test-package
arch = x86_64
"#;

        let result = PkgInfo::parse(content);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Missing pkgver");
    }

    #[test]
    fn test_parse_ignores_comments() {
        let content = r#"
# This is a comment
pkgname = test-package
pkgver = 1.0.0-1
# Another comment
arch = x86_64
"#;

        let pkginfo = PkgInfo::parse(content).unwrap();
        assert_eq!(pkginfo.pkgname, "test-package");
    }
}
