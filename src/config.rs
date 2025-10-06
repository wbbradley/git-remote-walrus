use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::env;
use std::path::PathBuf;

/// Configuration for git-remote-walrus
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalrusConfig {
    /// Sui RPC URL (e.g., https://fullnode.testnet.sui.io:443)
    pub sui_rpc_url: String,

    /// Path to Sui wallet configuration
    pub sui_wallet_path: PathBuf,

    /// Optional path to Walrus CLI config
    #[serde(skip_serializing_if = "Option::is_none")]
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

impl WalrusConfig {
    /// Load configuration from environment variables and config file
    pub fn load() -> Result<Self> {
        // Start with defaults
        let mut config = Self::default();

        // Try to load from config file
        if let Some(config_path) = Self::config_file_path() {
            if config_path.exists() {
                let file_config = Self::load_from_file(&config_path)?;
                config = file_config;
            }
        }

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

        Ok(config)
    }

    /// Load configuration from a file
    pub fn load_from_file(path: &PathBuf) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {:?}", path))?;

        let config: WalrusConfig = serde_yaml::from_str(&content)
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
    pub fn config_file_path() -> Option<PathBuf> {
        dirs::home_dir().map(|home| home.join(".config/git-remote-walrus/config.yaml"))
    }

    /// Get cache directory, creating it if necessary
    pub fn ensure_cache_dir(&self) -> Result<PathBuf> {
        std::fs::create_dir_all(&self.cache_dir)
            .with_context(|| format!("Failed to create cache directory: {:?}", self.cache_dir))?;
        Ok(self.cache_dir.clone())
    }
}

impl Default for WalrusConfig {
    fn default() -> Self {
        let home = dirs::home_dir().expect("Could not determine home directory");

        Self {
            sui_rpc_url: "https://fullnode.testnet.sui.io:443".to_string(),
            sui_wallet_path: home.join(".sui/sui_config/client.yaml"),
            walrus_config_path: None,
            cache_dir: home.join(".cache/git-remote-walrus"),
            default_epochs: 5,
            expiration_warning_threshold: 10,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_default_config() {
        let config = WalrusConfig::default();
        assert_eq!(config.sui_rpc_url, "https://fullnode.testnet.sui.io:443");
        assert_eq!(config.default_epochs, 5);
        assert_eq!(config.expiration_warning_threshold, 10);
    }

    #[test]
    fn test_save_and_load() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.yaml");

        let config = WalrusConfig::default();
        config.save(&config_path).unwrap();

        let loaded = WalrusConfig::load_from_file(&config_path).unwrap();
        assert_eq!(loaded.sui_rpc_url, config.sui_rpc_url);
        assert_eq!(loaded.default_epochs, config.default_epochs);
    }

    #[test]
    fn test_env_override() {
        env::set_var("SUI_RPC_URL", "https://custom.rpc.url");
        env::set_var("WALRUS_BLOB_EPOCHS", "10");

        let config = WalrusConfig::load().unwrap();
        assert_eq!(config.sui_rpc_url, "https://custom.rpc.url");
        assert_eq!(config.default_epochs, 10);

        env::remove_var("SUI_RPC_URL");
        env::remove_var("WALRUS_BLOB_EPOCHS");
    }
}
