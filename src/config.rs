use crate::error::{Error, Result};
use byte_unit::Byte;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub server: ServerConfig,
    pub storage: StorageConfig,
}

#[derive(Deserialize, Clone)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,

    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default = "default_max_payload_size")]
    pub max_payload_size: Byte,
}

#[derive(Debug, Deserialize, Clone)]
pub struct StorageConfig {
    #[serde(default = "default_data_path")]
    pub data_path: PathBuf,

    #[serde(default = "default_repo_name")]
    pub default_repo: String,

    #[serde(default = "default_arch")]
    pub default_arch: String,

    #[serde(default = "default_auto_cleanup_enabled")]
    pub auto_cleanup_enabled: bool,
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    3000
}

fn default_max_payload_size() -> Byte {
    Byte::from_u64_with_unit(512, byte_unit::Unit::MiB).unwrap()
}

fn default_data_path() -> PathBuf {
    PathBuf::from("data")
}

fn default_repo_name() -> String {
    "sw1nn".to_string()
}

fn default_arch() -> String {
    "x86_64".to_string()
}

fn default_auto_cleanup_enabled() -> bool {
    true
}

impl Config {
    pub fn load(config_path: Option<&str>) -> Result<Self> {
        let mut builder = config::Config::builder();

        // Add config file sources in order of precedence (lower to higher)
        if config_path.is_none() {
            // Release builds: look in /etc/sw1nn-pkg-repo/
            #[cfg(not(debug_assertions))]
            {
                builder = builder.add_source(
                    config::File::with_name("/etc/sw1nn-pkg-repo/config").required(false),
                );
            }

            // Debug builds: look in current working directory
            #[cfg(debug_assertions)]
            {
                builder = builder.add_source(config::File::with_name("config").required(false));
            }
        }

        // Custom config path (if specified via --config)
        if let Some(path) = config_path {
            builder = builder.add_source(
                config::File::with_name(path)
                    .required(true)
                    .format(config::FileFormat::Toml),
            );
        }

        // Environment variables (highest precedence)
        builder = builder.add_source(config::Environment::with_prefix("PKG_REPO"));

        let config = builder.build().map_err(|e| Error::Config {
            msg: format!("Failed to load configuration: {}", e),
        })?;

        let mut config: Self = config.try_deserialize().map_err(|e| Error::Config {
            msg: format!("Failed to deserialize configuration: {}", e),
        })?;

        // Convert relative data_path to absolute and clean it
        if !config.storage.data_path.is_absolute() {
            let cwd = std::env::current_dir().map_err(|e| Error::Config {
                msg: format!("Failed to get current directory: {}", e),
            })?;
            config.storage.data_path = cwd.join(&config.storage.data_path);
        }

        // Clean up the path (resolve . and .. components)
        // If canonicalize fails (e.g., path doesn't exist yet), keep the absolute path
        if let Ok(canonical) = config.storage.data_path.canonicalize() {
            config.storage.data_path = canonical;
        }

        Ok(config)
    }
}

impl Default for Config {
    fn default() -> Self {
        let mut data_path = default_data_path();

        // Convert relative path to absolute and clean it
        if !data_path.is_absolute() {
            data_path = std::env::current_dir()
                .map(|cwd| cwd.join(&data_path))
                .unwrap_or(data_path);
        }

        // Clean up the path (resolve . and .. components)
        // If canonicalize fails (e.g., path doesn't exist yet), keep the absolute path
        if let Ok(canonical) = data_path.canonicalize() {
            data_path = canonical;
        }

        Self {
            server: ServerConfig {
                host: default_host(),
                port: default_port(),
                max_payload_size: default_max_payload_size(),
            },
            storage: StorageConfig {
                data_path,
                default_repo: default_repo_name(),
                default_arch: default_arch(),
                auto_cleanup_enabled: default_auto_cleanup_enabled(),
            },
        }
    }
}

impl std::fmt::Debug for ServerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field(
                "max_payload_size",
                &format!(
                    "{}",
                    self.max_payload_size
                        .get_appropriate_unit(byte_unit::UnitType::Binary)
                ),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_relative_path_converted_to_absolute() {
        // Create a temporary directory with a config file
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");

        // Write config with relative data_path (other fields will use defaults)
        fs::write(
            &config_path,
            r#"
[server]
host = "127.0.0.1"
port = 3000

[storage]
data_path = "./my_data"
"#,
        )
        .unwrap();

        // Change to temp directory so relative path resolution works
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(&temp_dir).unwrap();

        // Load config
        let config = Config::load(Some(config_path.to_str().unwrap())).unwrap();

        // Restore original directory
        std::env::set_current_dir(original_dir).unwrap();

        // Verify path is now absolute
        assert!(
            config.storage.data_path.is_absolute(),
            "data_path should be absolute but got: {:?}",
            config.storage.data_path
        );

        // Verify it contains the expected components
        let path_str = config.storage.data_path.to_string_lossy();
        assert!(
            path_str.ends_with("my_data"),
            "Expected path to end with 'my_data' but got: {}",
            path_str
        );
    }

    #[test]
    fn test_default_relative_path_converted_to_absolute() {
        let config = Config::default();

        // Verify path is absolute
        assert!(
            config.storage.data_path.is_absolute(),
            "Default data_path should be absolute but got: {:?}",
            config.storage.data_path
        );

        // Verify it ends with 'data'
        let path_str = config.storage.data_path.to_string_lossy();
        assert!(
            path_str.ends_with("data"),
            "Expected default path to end with 'data' but got: {}",
            path_str
        );
    }

    #[test]
    fn test_absolute_path_unchanged() {
        // Create a temporary directory with a config file
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        let absolute_data_path = temp_dir.path().join("absolute_data");

        // Write config with absolute data_path
        fs::write(
            &config_path,
            format!(
                r#"
[server]
host = "127.0.0.1"
port = 3000

[storage]
data_path = "{}"
"#,
                absolute_data_path.display()
            ),
        )
        .unwrap();

        // Load config
        let config = Config::load(Some(config_path.to_str().unwrap())).unwrap();

        // Verify path is still absolute and points to the same location
        assert!(config.storage.data_path.is_absolute());

        // The path might be canonicalized, so we check it contains the key component
        let path_str = config.storage.data_path.to_string_lossy();
        assert!(
            path_str.contains("absolute_data"),
            "Expected path to contain 'absolute_data' but got: {}",
            path_str
        );
    }
}
