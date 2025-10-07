use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Dual index for cache lookups
/// Maps blob_id <-> sha256 bidirectionally
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CacheIndex {
    /// Walrus blob_id -> SHA-256 hash
    #[serde(default)]
    blob_to_sha256: BTreeMap<String, String>,

    /// SHA-256 hash -> Walrus blob_id
    #[serde(default)]
    sha256_to_blob: BTreeMap<String, String>,
}

impl CacheIndex {
    /// Create a new empty cache index
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Load cache index from file
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read cache index from {:?}", path))?;

        let index: CacheIndex = serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse cache index from {:?}", path))?;

        Ok(index)
    }

    /// Save cache index to file
    pub fn save(&self, path: &Path) -> Result<()> {
        // Ensure directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory {:?}", parent))?;
        }

        let content = serde_yaml::to_string(self).context("Failed to serialize cache index")?;

        fs::write(path, content)
            .with_context(|| format!("Failed to write cache index to {:?}", path))?;

        Ok(())
    }

    /// Add a mapping between blob_id and sha256
    pub fn insert(&mut self, blob_id: String, sha256: String) {
        self.blob_to_sha256.insert(blob_id.clone(), sha256.clone());
        self.sha256_to_blob.insert(sha256, blob_id);
    }

    /// Get SHA-256 from blob_id
    pub fn get_sha256(&self, blob_id: &str) -> Option<&String> {
        self.blob_to_sha256.get(blob_id)
    }

    /// Get blob_id from SHA-256
    pub fn get_blob_id(&self, sha256: &str) -> Option<&String> {
        self.sha256_to_blob.get(sha256)
    }

    /// Check if blob_id exists in index
    #[allow(dead_code)]
    pub fn contains_blob(&self, blob_id: &str) -> bool {
        self.blob_to_sha256.contains_key(blob_id)
    }

    /// Check if sha256 exists in index
    #[allow(dead_code)]
    pub fn contains_sha256(&self, sha256: &str) -> bool {
        self.sha256_to_blob.contains_key(sha256)
    }

    /// Remove a mapping
    #[allow(dead_code)]
    pub fn remove_by_blob_id(&mut self, blob_id: &str) -> Option<String> {
        if let Some(sha256) = self.blob_to_sha256.remove(blob_id) {
            self.sha256_to_blob.remove(&sha256);
            Some(sha256)
        } else {
            None
        }
    }

    /// Remove a mapping by SHA-256
    #[allow(dead_code)]
    pub fn remove_by_sha256(&mut self, sha256: &str) -> Option<String> {
        if let Some(blob_id) = self.sha256_to_blob.remove(sha256) {
            self.blob_to_sha256.remove(&blob_id);
            Some(blob_id)
        } else {
            None
        }
    }

    /// Get all blob_ids
    #[allow(dead_code)]
    pub fn all_blob_ids(&self) -> impl Iterator<Item = &String> {
        self.blob_to_sha256.keys()
    }

    /// Get all sha256 hashes
    #[allow(dead_code)]
    pub fn all_sha256s(&self) -> impl Iterator<Item = &String> {
        self.sha256_to_blob.keys()
    }

    /// Get count of indexed items
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.blob_to_sha256.len()
    }

    /// Check if index is empty
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.blob_to_sha256.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn test_insert_and_lookup() {
        let mut index = CacheIndex::new();

        index.insert("blob1".to_string(), "sha256_1".to_string());
        index.insert("blob2".to_string(), "sha256_2".to_string());

        assert_eq!(index.get_sha256("blob1"), Some(&"sha256_1".to_string()));
        assert_eq!(index.get_blob_id("sha256_2"), Some(&"blob2".to_string()));
        assert_eq!(index.len(), 2);
    }

    #[test]
    fn test_bidirectional_lookup() {
        let mut index = CacheIndex::new();

        index.insert("blob_abc".to_string(), "sha_xyz".to_string());

        assert!(index.contains_blob("blob_abc"));
        assert!(index.contains_sha256("sha_xyz"));
        assert_eq!(index.get_sha256("blob_abc"), Some(&"sha_xyz".to_string()));
        assert_eq!(index.get_blob_id("sha_xyz"), Some(&"blob_abc".to_string()));
    }

    #[test]
    fn test_remove() {
        let mut index = CacheIndex::new();

        index.insert("blob1".to_string(), "sha1".to_string());
        index.insert("blob2".to_string(), "sha2".to_string());

        assert_eq!(index.remove_by_blob_id("blob1"), Some("sha1".to_string()));
        assert!(!index.contains_blob("blob1"));
        assert!(!index.contains_sha256("sha1"));
        assert_eq!(index.len(), 1);

        assert_eq!(index.remove_by_sha256("sha2"), Some("blob2".to_string()));
        assert!(index.is_empty());
    }

    #[test]
    fn test_save_and_load() {
        let dir = tempdir().unwrap();
        let index_path = dir.path().join("cache_index.yaml");

        let mut index = CacheIndex::new();
        index.insert("blob1".to_string(), "sha1".to_string());
        index.insert("blob2".to_string(), "sha2".to_string());

        index.save(&index_path).unwrap();

        let loaded = CacheIndex::load(&index_path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.get_sha256("blob1"), Some(&"sha1".to_string()));
        assert_eq!(loaded.get_blob_id("sha2"), Some(&"blob2".to_string()));
    }
}
