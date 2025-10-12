use crate::error::{Error, Result};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub server: ServerConfig,
    pub storage: StorageConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,

    #[serde(default = "default_port")]
    pub port: u16,
}

#[derive(Debug, Deserialize, Clone)]
pub struct StorageConfig {
    #[serde(default = "default_data_path")]
    pub data_path: PathBuf,

    #[serde(default = "default_repo_name")]
    pub default_repo: String,

    #[serde(default = "default_arch")]
    pub default_arch: String,
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    3000
}

fn default_data_path() -> PathBuf {
    PathBuf::from("./data")
}

fn default_repo_name() -> String {
    "sw1nn".to_string()
}

fn default_arch() -> String {
    "x86_64".to_string()
}

impl Config {
    pub fn load() -> Result<Self> {
        let config = config::Config::builder()
            .add_source(config::File::with_name("config").required(false))
            .add_source(config::Environment::with_prefix("PKG_REPO"))
            .build()
            .map_err(|e| Error::Config {
                msg: format!("Failed to load configuration: {}", e),
            })?;

        config.try_deserialize().map_err(|e| Error::Config {
            msg: format!("Failed to deserialize configuration: {}", e),
        })
    }

    pub fn default() -> Self {
        Self {
            server: ServerConfig {
                host: default_host(),
                port: default_port(),
            },
            storage: StorageConfig {
                data_path: default_data_path(),
                default_repo: default_repo_name(),
                default_arch: default_arch(),
            },
        }
    }
}
