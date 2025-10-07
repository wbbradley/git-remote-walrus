use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Information about a tracked blob
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobInfo {
    /// Walrus blob ID
    pub blob_id: String,
    /// Epoch when blob expires
    pub end_epoch: u64,
    /// Optional: size in bytes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

/// Tracks blob expiration epochs
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BlobTracker {
    /// Maps blob_id to expiration info
    #[serde(default)]
    blobs: BTreeMap<String, BlobInfo>,
}

impl BlobTracker {
    /// Create a new blob tracker
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Load blob tracker from file
    pub fn load(path: &Path) -> Result<Self> {
        eprintln!("git-remote-walrus: Loading blob tracker from {:?}", path);
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read blob tracker from {:?}", path))?;

        let tracker: BlobTracker = serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse blob tracker from {:?}", path))?;

        Ok(tracker)
    }

    /// Save blob tracker to file
    pub fn save(&self, path: &Path) -> Result<()> {
        // Ensure directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory {:?}", parent))?;
        }

        let content = serde_yaml::to_string(self).context("Failed to serialize blob tracker")?;

        fs::write(path, content)
            .with_context(|| format!("Failed to write blob tracker to {:?}", path))?;

        Ok(())
    }

    /// Track a new blob
    pub fn track_blob(&mut self, blob_id: String, end_epoch: u64, size: Option<u64>) {
        self.blobs.insert(
            blob_id.clone(),
            BlobInfo {
                blob_id,
                end_epoch,
                size,
            },
        );
    }

    /// Get blob info
    #[allow(dead_code)]
    pub fn get_blob(&self, blob_id: &str) -> Option<&BlobInfo> {
        self.blobs.get(blob_id)
    }

    /// Get minimum expiration epoch across all blobs
    pub fn min_end_epoch(&self) -> Option<u64> {
        self.blobs.values().map(|info| info.end_epoch).min()
    }

    /// Get all blobs expiring before or at the given epoch
    pub fn expiring_before(&self, epoch: u64) -> Vec<&BlobInfo> {
        self.blobs
            .values()
            .filter(|info| info.end_epoch <= epoch)
            .collect()
    }

    /// Remove blob from tracking
    #[allow(dead_code)]
    pub fn untrack_blob(&mut self, blob_id: &str) -> Option<BlobInfo> {
        self.blobs.remove(blob_id)
    }

    /// Get all tracked blobs
    #[allow(dead_code)]
    pub fn all_blobs(&self) -> impl Iterator<Item = &BlobInfo> {
        self.blobs.values()
    }

    /// Get count of tracked blobs
    pub fn count(&self) -> usize {
        self.blobs.len()
    }

    /// Check if we should warn about expiring blobs
    /// Returns (should_warn, min_epoch, blobs_expiring_soon)
    pub fn check_expiration_warning(
        &self,
        current_epoch: u64,
        warning_threshold: u64,
    ) -> (bool, Option<u64>, Vec<&BlobInfo>) {
        let min_epoch = self.min_end_epoch();

        if let Some(min) = min_epoch {
            let warn_epoch = current_epoch + warning_threshold;
            let expiring_soon: Vec<_> = self.expiring_before(warn_epoch).into_iter().collect();

            if !expiring_soon.is_empty() {
                return (true, Some(min), expiring_soon);
            }
        }

        (false, min_epoch, Vec::new())
    }
}

/// Helper to determine blob tracker path from cache directory
#[allow(dead_code)]
pub fn blob_tracker_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join("blob_tracker.yaml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_track_blob() {
        let mut tracker = BlobTracker::new();
        tracker.track_blob("blob1".to_string(), 100, Some(1024));
        tracker.track_blob("blob2".to_string(), 200, Some(2048));

        assert_eq!(tracker.count(), 2);
        assert_eq!(tracker.min_end_epoch(), Some(100));
    }

    #[test]
    fn test_expiring_before() {
        let mut tracker = BlobTracker::new();
        tracker.track_blob("blob1".to_string(), 100, None);
        tracker.track_blob("blob2".to_string(), 200, None);
        tracker.track_blob("blob3".to_string(), 300, None);

        let expiring = tracker.expiring_before(150);
        assert_eq!(expiring.len(), 1);
        assert_eq!(expiring[0].blob_id, "blob1");

        let expiring = tracker.expiring_before(250);
        assert_eq!(expiring.len(), 2);
    }

    #[test]
    fn test_check_expiration_warning() {
        let mut tracker = BlobTracker::new();
        tracker.track_blob("blob1".to_string(), 100, None);
        tracker.track_blob("blob2".to_string(), 200, None);

        // Current epoch 50, warning threshold 60 (warn if expiring within 60 epochs)
        let (should_warn, min_epoch, expiring) = tracker.check_expiration_warning(50, 60);
        assert!(should_warn);
        assert_eq!(min_epoch, Some(100));
        assert_eq!(expiring.len(), 1);

        // Current epoch 50, warning threshold 40 (no blobs expiring within 40 epochs)
        let (should_warn, min_epoch, expiring) = tracker.check_expiration_warning(50, 40);
        assert!(!should_warn);
        assert_eq!(min_epoch, Some(100));
        assert_eq!(expiring.len(), 0);
    }

    #[test]
    fn test_serialization() {
        let mut tracker = BlobTracker::new();
        tracker.track_blob("blob1".to_string(), 100, Some(1024));

        let yaml = serde_yaml::to_string(&tracker).unwrap();
        let deserialized: BlobTracker = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(deserialized.count(), 1);
        assert_eq!(deserialized.min_end_epoch(), Some(100));
    }
}
