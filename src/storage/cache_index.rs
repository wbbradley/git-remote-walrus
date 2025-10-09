use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Dual index for cache lookups
/// Maps object_id <-> sha256 bidirectionally
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CacheIndex {
    /// Sui object_id -> SHA-256 hash
    #[serde(default)]
    object_to_sha256: BTreeMap<String, String>,

    /// SHA-256 hash -> Sui object_id
    #[serde(default)]
    sha256_to_object: BTreeMap<String, String>,
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

    /// Add a mapping between object_id and sha256
    pub fn insert(&mut self, object_id: String, sha256: String) {
        self.object_to_sha256
            .insert(object_id.clone(), sha256.clone());
        self.sha256_to_object.insert(sha256, object_id);
    }

    /// Get SHA-256 from object_id
    pub fn get_sha256(&self, object_id: &str) -> Option<&String> {
        self.object_to_sha256.get(object_id)
    }

    /// Get object_id from SHA-256
    pub fn get_object_id(&self, sha256: &str) -> Option<&String> {
        self.sha256_to_object.get(sha256)
    }

    /// Check if object_id exists in index
    #[allow(dead_code)]
    pub fn contains_object(&self, object_id: &str) -> bool {
        self.object_to_sha256.contains_key(object_id)
    }

    /// Check if sha256 exists in index
    #[allow(dead_code)]
    pub fn contains_sha256(&self, sha256: &str) -> bool {
        self.sha256_to_object.contains_key(sha256)
    }

    /// Remove a mapping by object_id
    #[allow(dead_code)]
    pub fn remove_by_object_id(&mut self, object_id: &str) -> Option<String> {
        if let Some(sha256) = self.object_to_sha256.remove(object_id) {
            self.sha256_to_object.remove(&sha256);
            Some(sha256)
        } else {
            None
        }
    }

    /// Remove a mapping by SHA-256
    #[allow(dead_code)]
    pub fn remove_by_sha256(&mut self, sha256: &str) -> Option<String> {
        if let Some(object_id) = self.sha256_to_object.remove(sha256) {
            self.object_to_sha256.remove(&object_id);
            Some(object_id)
        } else {
            None
        }
    }

    /// Get all object_ids
    #[allow(dead_code)]
    pub fn all_object_ids(&self) -> impl Iterator<Item = &String> {
        self.object_to_sha256.keys()
    }

    /// Get all sha256 hashes
    #[allow(dead_code)]
    pub fn all_sha256s(&self) -> impl Iterator<Item = &String> {
        self.sha256_to_object.keys()
    }

    /// Get count of indexed items
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.object_to_sha256.len()
    }

    /// Check if index is empty
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.object_to_sha256.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn test_insert_and_lookup() {
        let mut index = CacheIndex::new();

        index.insert("0x1".to_string(), "sha256_1".to_string());
        index.insert("0x2".to_string(), "sha256_2".to_string());

        assert_eq!(index.get_sha256("0x1"), Some(&"sha256_1".to_string()));
        assert_eq!(index.get_object_id("sha256_2"), Some(&"0x2".to_string()));
        assert_eq!(index.len(), 2);
    }

    #[test]
    fn test_bidirectional_lookup() {
        let mut index = CacheIndex::new();

        index.insert("0xabc".to_string(), "sha_xyz".to_string());

        assert!(index.contains_object("0xabc"));
        assert!(index.contains_sha256("sha_xyz"));
        assert_eq!(index.get_sha256("0xabc"), Some(&"sha_xyz".to_string()));
        assert_eq!(index.get_object_id("sha_xyz"), Some(&"0xabc".to_string()));
    }

    #[test]
    fn test_remove() {
        let mut index = CacheIndex::new();

        index.insert("0x1".to_string(), "sha1".to_string());
        index.insert("0x2".to_string(), "sha2".to_string());

        assert_eq!(index.remove_by_object_id("0x1"), Some("sha1".to_string()));
        assert!(!index.contains_object("0x1"));
        assert!(!index.contains_sha256("sha1"));
        assert_eq!(index.len(), 1);

        assert_eq!(index.remove_by_sha256("sha2"), Some("0x2".to_string()));
        assert!(index.is_empty());
    }

    #[test]
    fn test_save_and_load() {
        let dir = tempdir().unwrap();
        let index_path = dir.path().join("cache_index.yaml");

        let mut index = CacheIndex::new();
        index.insert("0x1".to_string(), "sha1".to_string());
        index.insert("0x2".to_string(), "sha2".to_string());

        index.save(&index_path).unwrap();

        let loaded = CacheIndex::load(&index_path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.get_sha256("0x1"), Some(&"sha1".to_string()));
        assert_eq!(loaded.get_object_id("sha2"), Some(&"0x2".to_string()));
    }
}
