use anyhow::{Context, Result};
use serde::Deserialize;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use tempfile::NamedTempFile;

/// Status of a blob on Walrus
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct BlobStatus {
    pub blob_id: String,
    pub status: String,
    pub end_epoch: Option<u64>,
}

/// Walrus epoch information
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
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

    /// Store content on Walrus and return blob ID
    pub fn store(&self, content: &[u8]) -> Result<String> {
        self.store_with_epochs(content, self.default_epochs)
    }

    /// Store content on Walrus with specific epoch duration
    pub fn store_with_epochs(&self, content: &[u8], epochs: u32) -> Result<String> {
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
        cmd.arg("store")
            .arg("--json")
            .arg("--epochs")
            .arg(epochs.to_string())
            .arg(temp_file.path());

        if let Some(config) = &self.config_path {
            cmd.arg("--config").arg(config);
        }

        // Execute command
        let output = cmd
            .output()
            .context("Failed to execute walrus store command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("walrus store failed: {}", stderr);
        }

        // Parse JSON output to extract blob_id
        let stdout = String::from_utf8_lossy(&output.stdout);
        let blob_id = self.parse_blob_id(&stdout)?;

        eprintln!(
            "walrus: Stored blob {} (expires in {} epochs)",
            blob_id, epochs
        );

        Ok(blob_id)
    }

    /// Read blob content from Walrus
    pub fn read(&self, blob_id: &str) -> Result<Vec<u8>> {
        // Build walrus read command
        let mut cmd = Command::new("walrus");
        cmd.arg("read").arg(blob_id);

        if let Some(config) = &self.config_path {
            cmd.arg("--config").arg(config);
        }

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

    /// Get blob status from Walrus
    pub fn blob_status(&self, blob_id: &str) -> Result<BlobStatus> {
        // Build walrus blob-status command
        // Use --blob-id flag to avoid blob IDs starting with '-' being interpreted as flags
        let mut cmd = Command::new("walrus");
        cmd.arg("blob-status")
            .arg("--json")
            .arg("--blob-id")
            .arg(blob_id);

        if let Some(config) = &self.config_path {
            cmd.arg("--config").arg(config);
        }

        // Execute command
        let output = cmd
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
        cmd.arg("info").arg("epoch").arg("--json");

        if let Some(config) = &self.config_path {
            cmd.arg("--config").arg(config);
        }

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

    /// Parse blob_id from walrus store output
    fn parse_blob_id(&self, output: &str) -> Result<String> {
        // The walrus store command outputs JSON with the blob_id
        // Format: [{"blobStoreResult": {...}, "path": "..."}]
        // blobStoreResult contains either:
        //   - alreadyCertified: Blob already exists (deduplicated)
        //   - newlyCreated: Blob was just uploaded

        // Try to parse as JSON first
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(output) {
            // Array format with blobStoreResult wrapper
            if let Some(array) = json.as_array() {
                if let Some(first) = array.first() {
                    if let Some(result) = first.get("blobStoreResult") {
                        // Try alreadyCertified (blob was deduplicated)
                        if let Some(blob_id) = result
                            .get("alreadyCertified")
                            .and_then(|ac| ac.get("blobId"))
                            .and_then(|id| id.as_str())
                        {
                            return Ok(blob_id.to_string());
                        }
                        // Try newlyCreated (blob was uploaded)
                        if let Some(blob_id) = result
                            .get("newlyCreated")
                            .and_then(|nc| nc.get("blobObject"))
                            .and_then(|bo| bo.get("blobId"))
                            .and_then(|id| id.as_str())
                        {
                            return Ok(blob_id.to_string());
                        }
                    }
                }
            }

            // Fallback: try direct object access (for compatibility)
            if let Some(blob_id) = json
                .get("newlyCreated")
                .and_then(|nc| nc.get("blobObject"))
                .and_then(|bo| bo.get("blobId"))
                .and_then(|id| id.as_str())
            {
                return Ok(blob_id.to_string());
            }

            if let Some(blob_id) = json
                .get("alreadyCertified")
                .and_then(|ac| ac.get("blobId"))
                .and_then(|id| id.as_str())
            {
                return Ok(blob_id.to_string());
            }

            if let Some(blob_id) = json.get("blob_id").and_then(|id| id.as_str()) {
                return Ok(blob_id.to_string());
            }
        }

        anyhow::bail!("Failed to parse blob_id from walrus output: {}", output)
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
    fn test_parse_blob_id_newly_created() {
        let client = WalrusClient::default();
        let output =
            r#"{"newlyCreated": {"blobObject": {"id": "0x123", "blobId": "test-blob-id-123"}}}"#;
        let blob_id = client.parse_blob_id(output).unwrap();
        assert_eq!(blob_id, "test-blob-id-123");
    }

    #[test]
    fn test_parse_blob_id_already_certified() {
        let client = WalrusClient::default();
        let output = r#"{"alreadyCertified": {"blobId": "existing-blob-id"}}"#;
        let blob_id = client.parse_blob_id(output).unwrap();
        assert_eq!(blob_id, "existing-blob-id");
    }

    #[test]
    fn test_parse_blob_id_simple() {
        let client = WalrusClient::default();
        let output = r#"{"blob_id": "simple-blob-id"}"#;
        let blob_id = client.parse_blob_id(output).unwrap();
        assert_eq!(blob_id, "simple-blob-id");
    }

    #[test]
    fn test_parse_blob_id_new_format_already_certified() {
        let client = WalrusClient::default();
        let output = r#"[{"blobStoreResult": {"alreadyCertified": {"blobId": "new-format-blob-id", "object": "0x123", "endEpoch": 191}}, "path": "/tmp/file"}]"#;
        let blob_id = client.parse_blob_id(output).unwrap();
        assert_eq!(blob_id, "new-format-blob-id");
    }

    #[test]
    fn test_parse_blob_id_new_format_newly_created() {
        let client = WalrusClient::default();
        let output = r#"[{"blobStoreResult": {"newlyCreated": {"blobObject": {"blobId": "newly-created-id"}}}, "path": "/tmp/file"}]"#;
        let blob_id = client.parse_blob_id(output).unwrap();
        assert_eq!(blob_id, "newly-created-id");
    }
}
