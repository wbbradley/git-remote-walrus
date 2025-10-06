use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::env;
use std::path::PathBuf;
use sui_config::{sui_config_dir, SUI_KEYSTORE_FILENAME};

/// Configuration for git-remote-walrus
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalrusRemoteConfig {
    /// Sui RPC URL (e.g., https://fullnode.testnet.sui.io:443)
    pub sui_rpc_url: String,

    /// Path to Sui wallet configuration
    pub sui_wallet_path: PathBuf,

    /// Path to Walrus CLI config
    pub walrus_config_path: Option<PathBuf>,

    /// Cache directory for local storage
    pub cache_dir: PathBuf,

    /// Default number of epochs for blob storage
    #[serde(default = "defaults::default_epochs")]
    pub default_epochs: u32,

    /// Warning threshold for blob expiration (epochs)
    #[serde(default = "defaults::default_warning_threshold")]
    pub expiration_warning_threshold: u64,
}

mod defaults {
    pub(crate) fn default_epochs() -> u32 {
        5
    }

    pub(crate) fn default_warning_threshold() -> u64 {
        10
    }
}

impl WalrusRemoteConfig {
    /// Load configuration from environment variables and config file
    pub fn load() -> Result<Self> {
        // Try to load from config file
        let config_path = Self::config_file_path()?;
        let mut config = if config_path.exists() {
            Self::load_from_file(&config_path)?
        } else {
            anyhow::bail!("Config file not found at {:?}", config_path);
        };

        // Override with environment variables if present
        if let Ok(url) = env::var("SUI_RPC_URL") {
            config.sui_rpc_url = url;
        }

        if let Ok(path) = env::var("SUI_WALLET") {
            config.sui_wallet_path = PathBuf::from(path);
        }

        if let Ok(path) = env::var("WALRUS_CONFIG") {
            config.walrus_config_path = Some(PathBuf::from(path));
        }

        if let Ok(path) = env::var("WALRUS_CACHE_DIR") {
            config.cache_dir = PathBuf::from(path);
        }

        if let Ok(epochs) = env::var("WALRUS_BLOB_EPOCHS") {
            config.default_epochs = epochs
                .parse()
                .context("Failed to parse WALRUS_BLOB_EPOCHS as u32")?;
        }

        if let Ok(threshold) = env::var("WALRUS_EXPIRATION_WARNING_THRESHOLD") {
            config.expiration_warning_threshold = threshold
                .parse()
                .context("Failed to parse WALRUS_EXPIRATION_WARNING_THRESHOLD as u64")?;
        }

        eprintln!("Using Walrus config: {:?}", config);
        Ok(config)
    }

    /// Load configuration from a file
    pub fn load_from_file(path: &PathBuf) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {:?}", path))?;

        let config: WalrusRemoteConfig = serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {:?}", path))?;

        Ok(config)
    }

    /// Save configuration to file
    #[allow(dead_code)]
    pub fn save(&self, path: &PathBuf) -> Result<()> {
        // Ensure directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create config directory: {:?}", parent))?;
        }

        let content = serde_yaml::to_string(self).context("Failed to serialize config")?;

        std::fs::write(path, content)
            .with_context(|| format!("Failed to write config file: {:?}", path))?;

        Ok(())
    }

    /// Get default config file path
    pub fn config_file_path() -> Result<PathBuf> {
        dirs::home_dir()
            .map(|home| home.join(".config/git-remote-walrus/config.yaml"))
            .context("Could not determine home directory for config file")
    }

    /// Get cache directory, creating it if necessary
    pub fn ensure_cache_dir(&self) -> Result<PathBuf> {
        std::fs::create_dir_all(&self.cache_dir)
            .with_context(|| format!("Failed to create cache directory: {:?}", self.cache_dir))?;
        Ok(self.cache_dir.clone())
    }
}

/*
impl Default for WalrusConfig {
    fn default() -> Self {
        let home = dirs::home_dir().expect("Could not determine home directory");

        Self {
            sui_rpc_url: "https://fullnode.testnet.sui.io:443".to_string(),
            sui_wallet_path: sui_config_dir()
                .context("Failed to get Sui config directory")
                .expect("Sui config directory is required")
                .join(SUI_KEYSTORE_FILENAME),
            walrus_config_path: None,
            cache_dir: home.join(".cache/git-remote-walrus"),
            default_epochs: 5,
            expiration_warning_threshold: 10,
        }
    }
}
*/

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_default_config() {
        let config = WalrusRemoteConfig::default();
        assert_eq!(config.sui_rpc_url, "https://fullnode.testnet.sui.io:443");
        assert_eq!(config.default_epochs, 5);
        assert_eq!(config.expiration_warning_threshold, 10);
    }

    #[test]
    fn test_save_and_load() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.yaml");

        let config = WalrusRemoteConfig {
            sui_rpc_url: "https://example.com".to_string(),
            sui_wallet_path: PathBuf::from("/path/to/wallet"),
            walrus_config_path: Some(PathBuf::from("/path/to/walrus/config")),
            cache_dir: dir.path().join("cache"),
            default_epochs: 7,
            expiration_warning_threshold: 15,
        };
        config.save(&config_path).unwrap();

        let loaded = WalrusRemoteConfig::load_from_file(&config_path).unwrap();
        assert_eq!(loaded.sui_rpc_url, config.sui_rpc_url);
        assert_eq!(loaded.default_epochs, config.default_epochs);
    }

    #[test]
    fn test_env_override() {
        env::set_var("SUI_RPC_URL", "https://custom.rpc.url");
        env::set_var("WALRUS_BLOB_EPOCHS", "10");

        let config = WalrusRemoteConfig::load().unwrap();
        assert_eq!(config.sui_rpc_url, "https://custom.rpc.url");
        assert_eq!(config.default_epochs, 10);

        env::remove_var("SUI_RPC_URL");
        env::remove_var("WALRUS_BLOB_EPOCHS");
    }
}
