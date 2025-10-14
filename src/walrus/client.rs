use std::{io::Write, path::PathBuf, process::Command};

use anyhow::{Context, Result};
use serde::Deserialize;
use tempfile::NamedTempFile;

/// Information about a stored blob (from walrus store command)
#[derive(Debug, Clone)]
pub struct BlobInfo {
    /// Sui SharedBlob object ID (for querying status)
    pub shared_object_id: String,
    /// Walrus blob ID (for reading content)
    pub blob_id: String,
}

/// Status of a blob on Walrus
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct BlobStatus {
    pub blob_id: String,
    pub status: String,
    pub end_epoch: Option<u64>,
}

/// Walrus epoch information
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(dead_code)]
pub struct EpochInfo {
    pub current_epoch: u64,
    #[serde(default)]
    pub start_of_current_epoch: Option<serde_json::Value>,
    #[serde(default)]
    pub epoch_duration: Option<serde_json::Value>,
    #[serde(default)]
    pub max_epochs_ahead: Option<u64>,
}

/// Client for interacting with Walrus CLI
pub struct WalrusClient {
    config_path: Option<PathBuf>,
    default_epochs: u32,
}

impl WalrusClient {
    /// Create a new Walrus client
    pub fn new(config_path: Option<PathBuf>, default_epochs: u32) -> Self {
        Self {
            config_path,
            default_epochs,
        }
    }

    /// Store content on Walrus and return blob info (object_id and blob_id)
    pub fn store(&self, content: &[u8]) -> Result<BlobInfo> {
        self.store_with_epochs(content, self.default_epochs)
    }

    /// Store content on Walrus with specific epoch duration
    pub fn store_with_epochs(&self, content: &[u8], epochs: u32) -> Result<BlobInfo> {
        // Create a temporary file for the content
        let mut temp_file =
            NamedTempFile::new().context("Failed to create temporary file for Walrus upload")?;

        temp_file
            .write_all(content)
            .context("Failed to write content to temporary file")?;

        temp_file
            .flush()
            .context("Failed to flush temporary file")?;

        // Build walrus store command
        let mut cmd = Command::new("walrus");
        if let Some(config) = &self.config_path {
            cmd.arg("--config").arg(config);
        }
        cmd.arg("store")
            .arg("--json")
            .arg("--share")
            .arg("--permanent")
            .arg("--force") // Always create new blob object to get sharedBlobObject ID
            .arg("--epochs")
            .arg(epochs.to_string())
            .arg(temp_file.path());

        // Execute command
        let output = cmd
            .output()
            .context("Failed to execute walrus store command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("walrus store failed: {}", stderr);
        }

        // Parse JSON output to extract blob info (object_id and blob_id)
        let stdout = String::from_utf8_lossy(&output.stdout);
        let blob_info = self.parse_blob_info(&stdout)?;
        tracing::debug!(
            "Parsed blob_info - shared_object_id: {}, blob_id: {}",
            blob_info.shared_object_id, blob_info.blob_id
        );

        tracing::info!(
            "Stored blob {} at shared object {} (expires in {} epochs)",
            &blob_info.blob_id, &blob_info.shared_object_id, epochs
        );

        Ok(blob_info)
    }

    /// Read blob content from Walrus
    pub fn read(&self, blob_id: &str) -> Result<Vec<u8>> {
        // Build walrus read command
        let mut cmd = Command::new("walrus");
        if let Some(config) = &self.config_path {
            cmd.arg("--config").arg(config);
        }
        cmd.arg("read").arg(blob_id);

        // Execute command
        let output = cmd
            .output()
            .context("Failed to execute walrus read command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("walrus read failed: {}", stderr);
        }

        Ok(output.stdout)
    }

    /// Get blob status from Walrus (legacy - prefer using Sui's get_shared_blob_status)
    #[allow(dead_code)]
    pub fn blob_status(&self, blob_id: &str) -> Result<BlobStatus> {
        // Build walrus blob-status command
        // Use --blob-id flag to avoid blob IDs starting with '-' being interpreted as flags
        let mut cmd = Command::new("walrus");
        if let Some(config) = &self.config_path {
            cmd.arg("--config").arg(config);
        }
        cmd.arg("blob-status").arg("--json");

        // Execute command
        let output = cmd
            .arg("--blob-id")
            .arg(blob_id)
            .output()
            .context("Failed to execute walrus blob-status command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("walrus blob-status failed: {}", stderr);
        }

        // Parse JSON output
        let stdout = String::from_utf8_lossy(&output.stdout);
        let status: BlobStatus =
            serde_json::from_str(&stdout).context("Failed to parse blob status JSON")?;

        Ok(status)
    }

    /// Get current Walrus epoch information
    pub fn current_epoch(&self) -> Result<EpochInfo> {
        // Build walrus info epoch command
        let mut cmd = Command::new("walrus");
        if let Some(config) = &self.config_path {
            cmd.arg("--config").arg(config);
        }
        cmd.arg("info").arg("epoch").arg("--json");

        // Execute command
        let output = cmd
            .output()
            .context("Failed to execute walrus info epoch command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("walrus info epoch failed: {}", stderr);
        }

        // Parse JSON output
        let stdout = String::from_utf8_lossy(&output.stdout);
        let epoch_info: EpochInfo =
            serde_json::from_str(&stdout).context("Failed to parse epoch info JSON")?;

        Ok(epoch_info)
    }

    /// Parse blob info (shared_object_id and blob_id) from walrus store output
    fn parse_blob_info(&self, output: &str) -> Result<BlobInfo> {
        // The walrus store command outputs JSON with the blob_id and shared object
        // Format: [{"blobStoreResult": {...}, "path": "..."}]
        // blobStoreResult contains either:
        //   - alreadyCertified: Blob already exists (deduplicated)
        //     { "blobId": "...", "sharedBlobObject": "0x..." }
        //   - newlyCreated: Blob was just uploaded
        //     { "blobObject": { "blobId": "..." }, "sharedBlobObject": "0x..." }

        // Try to parse as JSON first
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(output) {
            // Array format with blobStoreResult wrapper
            if let Some(array) = json.as_array() {
                if let Some(first) = array.first() {
                    if let Some(result) = first.get("blobStoreResult") {
                        // Try newlyCreated (blob was uploaded)
                        if let Some(nc) = result.get("newlyCreated") {
                            if let (Some(blob_id), Some(shared_object_id)) = (
                                nc.get("blobObject")
                                    .and_then(|bo| bo.get("blobId"))
                                    .and_then(|id| id.as_str()),
                                nc.get("sharedBlobObject").and_then(|id| id.as_str()),
                            ) {
                                return Ok(BlobInfo {
                                    shared_object_id: shared_object_id.to_string(),
                                    blob_id: blob_id.to_string(),
                                });
                            }
                        }
                        // Try alreadyCertified (blob was deduplicated)
                        if let Some(ac) = result.get("alreadyCertified") {
                            if let (Some(blob_id), Some(shared_object_id)) = (
                                ac.get("blobId").and_then(|id| id.as_str()),
                                ac.get("sharedBlobObject").and_then(|id| id.as_str()),
                            ) {
                                return Ok(BlobInfo {
                                    shared_object_id: shared_object_id.to_string(),
                                    blob_id: blob_id.to_string(),
                                });
                            }
                        }
                    }
                }
            }

            // Fallback: try direct object access (for compatibility)
            if let Some(nc) = json.get("newlyCreated") {
                if let (Some(blob_id), Some(shared_object_id)) = (
                    nc.get("blobObject")
                        .and_then(|bo| bo.get("blobId"))
                        .and_then(|id| id.as_str()),
                    nc.get("sharedBlobObject").and_then(|id| id.as_str()),
                ) {
                    return Ok(BlobInfo {
                        shared_object_id: shared_object_id.to_string(),
                        blob_id: blob_id.to_string(),
                    });
                }
            }

            if let Some(ac) = json.get("alreadyCertified") {
                if let (Some(blob_id), Some(shared_object_id)) = (
                    ac.get("blobId").and_then(|id| id.as_str()),
                    ac.get("sharedBlobObject").and_then(|id| id.as_str()),
                ) {
                    return Ok(BlobInfo {
                        shared_object_id: shared_object_id.to_string(),
                        blob_id: blob_id.to_string(),
                    });
                }
            }
        }

        anyhow::bail!("Failed to parse blob info from walrus output: {}", output)
    }
}

impl Default for WalrusClient {
    fn default() -> Self {
        Self::new(None, 5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_blob_info_newly_created() {
        let client = WalrusClient::default();
        let output = r#"{"newlyCreated": {"blobObject": {"blobId": "test-blob-id-123"}, "sharedBlobObject": "0x123"}}"#;
        let blob_info = client.parse_blob_info(output).unwrap();
        assert_eq!(blob_info.blob_id, "test-blob-id-123");
        assert_eq!(blob_info.shared_object_id, "0x123");
    }

    #[test]
    fn test_parse_blob_info_already_certified() {
        let client = WalrusClient::default();
        let output =
            r#"{"alreadyCertified": {"blobId": "existing-blob-id", "sharedBlobObject": "0x456"}}"#;
        let blob_info = client.parse_blob_info(output).unwrap();
        assert_eq!(blob_info.blob_id, "existing-blob-id");
        assert_eq!(blob_info.shared_object_id, "0x456");
    }

    #[test]
    fn test_parse_blob_info_new_format_already_certified() {
        let client = WalrusClient::default();
        let output = r#"[{"blobStoreResult": {"alreadyCertified": {"blobId": "new-format-blob-id", "sharedBlobObject": "0x789"}}, "path": "/tmp/file"}]"#;
        let blob_info = client.parse_blob_info(output).unwrap();
        assert_eq!(blob_info.blob_id, "new-format-blob-id");
        assert_eq!(blob_info.shared_object_id, "0x789");
    }

    #[test]
    fn test_parse_blob_info_new_format_newly_created() {
        let client = WalrusClient::default();
        let output = r#"[{"blobStoreResult": {"newlyCreated": {"blobObject": {"blobId": "newly-created-id"}, "sharedBlobObject": "0xabc"}}, "path": "/tmp/file"}]"#;
        let blob_info = client.parse_blob_info(output).unwrap();
        assert_eq!(blob_info.blob_id, "newly-created-id");
        assert_eq!(blob_info.shared_object_id, "0xabc");
    }
}
