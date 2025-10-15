use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Walrus network size limits
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SizeInfo {
    /// Storage unit size in bytes
    pub storage_unit_size: u64,
    /// Maximum blob size in bytes
    pub max_blob_size: u64,
}

/// Walrus network information
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalrusNetworkInfo {
    /// Size constraints
    pub size_info: SizeInfo,
    /// Timestamp when this was last queried (for potential cache invalidation)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queried_at: Option<String>,
}

impl WalrusNetworkInfo {
    /// Load network info from cache file
    pub fn load(path: &Path) -> Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read network info from {:?}", path))?;

        let info: WalrusNetworkInfo = serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse network info from {:?}", path))?;

        Ok(Some(info))
    }

    /// Save network info to cache file
    pub fn save(&self, path: &Path) -> Result<()> {
        // Ensure directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory {:?}", parent))?;
        }

        let content = serde_yaml::to_string(self).context("Failed to serialize network info")?;

        fs::write(path, content)
            .with_context(|| format!("Failed to write network info to {:?}", path))?;

        Ok(())
    }

    /// Query network info from Walrus CLI
    pub fn query(walrus_config_path: Option<&PathBuf>) -> Result<Self> {
        let mut cmd = Command::new("walrus");

        if let Some(config_path) = walrus_config_path {
            cmd.arg("--config").arg(config_path);
        }

        cmd.arg("info").arg("--json");

        tracing::debug!("Querying Walrus network info: {:?}", cmd);

        let output = cmd
            .output()
            .context("Failed to execute walrus info command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("walrus info command failed: {}", stderr);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Parse JSON output
        let json: serde_json::Value =
            serde_json::from_str(&stdout).context("Failed to parse walrus info JSON")?;

        // Extract sizeInfo
        let size_info_json = json
            .get("sizeInfo")
            .ok_or_else(|| anyhow::anyhow!("sizeInfo not found in walrus info output"))?;

        let storage_unit_size = size_info_json
            .get("storageUnitSize")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("storageUnitSize not found or invalid"))?;

        let max_blob_size = size_info_json
            .get("maxBlobSize")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("maxBlobSize not found or invalid"))?;

        Ok(WalrusNetworkInfo {
            size_info: SizeInfo {
                storage_unit_size,
                max_blob_size,
            },
            queried_at: Some(chrono::Utc::now().to_rfc3339()),
        })
    }

    /// Get the maximum blob size for this network
    pub fn max_blob_size(&self) -> u64 {
        self.size_info.max_blob_size
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn test_save_and_load() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("network_info.yaml");

        let info = WalrusNetworkInfo {
            size_info: SizeInfo {
                storage_unit_size: 1048576,
                max_blob_size: 1834952,
            },
            queried_at: Some("2025-10-15T03:46:32Z".to_string()),
        };

        info.save(&path).unwrap();

        let loaded = WalrusNetworkInfo::load(&path).unwrap().unwrap();
        assert_eq!(loaded.size_info.max_blob_size, 1834952);
        assert_eq!(loaded.size_info.storage_unit_size, 1048576);
    }

    #[test]
    fn test_load_nonexistent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nonexistent.yaml");

        let loaded = WalrusNetworkInfo::load(&path).unwrap();
        assert!(loaded.is_none());
    }

    #[test]
    fn test_max_blob_size() {
        let info = WalrusNetworkInfo {
            size_info: SizeInfo {
                storage_unit_size: 1048576,
                max_blob_size: 1834952,
            },
            queried_at: None,
        };

        assert_eq!(info.max_blob_size(), 1834952);
    }
}
