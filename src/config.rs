use std::{
    env,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Expand tilde (~) in path to user's home directory
fn expand_tilde(path: &Path) -> PathBuf {
    if let Some(s) = path.to_str() {
        if let Some(stripped) = s.strip_prefix("~/") {
            if let Some(home) = dirs::home_dir() {
                return home.join(stripped);
            }
        } else if s == "~" {
            if let Some(home) = dirs::home_dir() {
                return home;
            }
        }
    }
    path.to_path_buf()
}

/// Configuration for git-remote-walrus
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalrusRemoteConfig {
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

impl WalrusRemoteConfig {
    /// Load configuration from environment variables and config file
    pub fn load() -> Result<Self> {
        // Try to load from config file
        let config_path = Self::config_file_path()?;
        tracing::debug!("loading git-remote-walrus config from {:?}", config_path);
        let mut config = if config_path.exists() {
            Self::load_from_file(&config_path)?
        } else {
            anyhow::bail!("config file not found at {:?}", config_path);
        };

        if let Ok(path) = env::var("SUI_WALLET") {
            config.sui_wallet_path = expand_tilde(&PathBuf::from(path));
        }

        if let Ok(path) = env::var("WALRUS_CONFIG") {
            config.walrus_config_path = Some(expand_tilde(&PathBuf::from(path)));
        }

        if let Ok(path) = env::var("WALRUS_REMOTE_CACHE_DIR") {
            config.cache_dir = expand_tilde(&PathBuf::from(path));
        }

        if let Ok(epochs) = env::var("WALRUS_REMOTE_BLOB_EPOCHS") {
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

        let mut config: WalrusRemoteConfig = serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {:?}", path))?;

        // Expand tildes in all path fields
        config.sui_wallet_path = expand_tilde(&config.sui_wallet_path);
        config.cache_dir = expand_tilde(&config.cache_dir);
        if let Some(ref walrus_path) = config.walrus_config_path {
            config.walrus_config_path = Some(expand_tilde(walrus_path));
        }

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

mod defaults {
    pub(crate) fn default_epochs() -> u32 {
        5
    }

    pub(crate) fn default_warning_threshold() -> u64 {
        10
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn test_save_and_load() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.yaml");

        let config = WalrusRemoteConfig {
            sui_wallet_path: PathBuf::from("/path/to/wallet"),
            walrus_config_path: Some(PathBuf::from("/path/to/walrus/config")),
            cache_dir: dir.path().join("cache"),
            default_epochs: 7,
            expiration_warning_threshold: 15,
        };
        config.save(&config_path).unwrap();

        let loaded = WalrusRemoteConfig::load_from_file(&config_path).unwrap();
        assert_eq!(loaded.default_epochs, config.default_epochs);
    }

    #[test]
    fn test_env_override() {
        env::set_var("WALRUS_REMOTE_BLOB_EPOCHS", "10");

        let config = WalrusRemoteConfig::load().unwrap();
        assert_eq!(config.default_epochs, 10);

        env::remove_var("WALRUS_REMOTE_BLOB_EPOCHS");
    }

    #[test]
    fn test_tilde_expansion() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.yaml");

        // Create config with tilde paths
        let config_content = r#"
sui_wallet_path: ~/path/to/wallet
cache_dir: ~/cache
walrus_config_path: ~/walrus/config
default_epochs: 5
expiration_warning_threshold: 10
"#;
        std::fs::write(&config_path, config_content).unwrap();

        let loaded = WalrusRemoteConfig::load_from_file(&config_path).unwrap();

        // Verify tilde was expanded
        if let Some(home) = dirs::home_dir() {
            assert_eq!(loaded.sui_wallet_path, home.join("path/to/wallet"));
            assert_eq!(loaded.cache_dir, home.join("cache"));
            assert_eq!(loaded.walrus_config_path, Some(home.join("walrus/config")));
        }
    }

    #[test]
    fn test_tilde_expansion_env_vars() {
        let home = dirs::home_dir().unwrap();

        env::set_var("SUI_WALLET", "~/test/wallet");
        env::set_var("WALRUS_CONFIG", "~/test/walrus");
        env::set_var("WALRUS_REMOTE_CACHE_DIR", "~/test/cache");

        let config = WalrusRemoteConfig::load().unwrap();

        assert_eq!(config.sui_wallet_path, home.join("test/wallet"));
        assert_eq!(config.walrus_config_path, Some(home.join("test/walrus")));
        assert_eq!(config.cache_dir, home.join("test/cache"));

        env::remove_var("SUI_WALLET");
        env::remove_var("WALRUS_CONFIG");
        env::remove_var("WALRUS_REMOTE_CACHE_DIR");
    }
}
