use anyhow::Result;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

use super::traits::{ContentId, ImmutableStore, MutableState, StorageBackend};
use super::State;

/// Filesystem-based storage backend using SHA-256 content addressing
pub struct FilesystemStorage {
    base_path: PathBuf,
}

impl FilesystemStorage {
    /// Create a new filesystem storage backend
    pub fn new<P: AsRef<Path>>(base_path: P) -> Result<Self> {
        Ok(FilesystemStorage {
            base_path: base_path.as_ref().to_path_buf(),
        })
    }

    /// Get the path to the objects directory
    fn objects_dir(&self) -> PathBuf {
        self.base_path.join("objects")
    }

    /// Get the path to the state file
    fn state_path(&self) -> PathBuf {
        self.base_path.join("state.yaml")
    }

    /// Compute SHA-256 hash of content
    fn compute_hash(content: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content);
        hex::encode(hasher.finalize())
    }
}

impl ImmutableStore for FilesystemStorage {
    fn write_object(&self, content: &[u8]) -> Result<ContentId> {
        // 1. Compute SHA-256 hash
        let hash_hex = Self::compute_hash(content);

        // 2. Write to objects/ directory
        let path = self.objects_dir().join(&hash_hex);

        // 3. Only write if doesn't exist (immutable)
        if !path.exists() {
            fs::write(&path, content)?;
        }

        Ok(hash_hex)
    }

    fn write_objects(&self, contents: &[&[u8]]) -> Result<Vec<ContentId>> {
        // Simple implementation: write sequentially
        // Could be optimized with parallel writes if needed
        contents.iter().map(|content| self.write_object(content)).collect()
    }

    fn read_object(&self, id: &str) -> Result<Vec<u8>> {
        let path = self.objects_dir().join(id);
        // Must read entire file into memory (no seeking)
        Ok(fs::read(&path)?)
    }

    fn read_objects(&self, ids: &[&str]) -> Result<Vec<Vec<u8>>> {
        ids.iter().map(|id| self.read_object(id)).collect()
    }

    fn delete_object(&self, id: &str) -> Result<()> {
        let path = self.objects_dir().join(id);
        if path.exists() {
            fs::remove_file(&path)?;
        }
        Ok(())
    }

    fn object_exists(&self, id: &str) -> Result<bool> {
        let path = self.objects_dir().join(id);
        Ok(path.exists())
    }
}

impl MutableState for FilesystemStorage {
    fn read_state(&self) -> Result<State> {
        let state_path = self.state_path();
        if state_path.exists() {
            let content = fs::read_to_string(&state_path)?;
            Ok(serde_yaml::from_str(&content)?)
        } else {
            Ok(State::default())
        }
    }

    fn write_state(&self, state: &State) -> Result<()> {
        let state_path = self.state_path();
        let temp_path = self.base_path.join(".state.yaml.tmp");

        // 1. Write to temp file
        let yaml = serde_yaml::to_string(state)?;
        fs::write(&temp_path, yaml)?;

        // 2. Atomic rename (atomic on POSIX systems)
        fs::rename(&temp_path, &state_path)?;

        Ok(())
    }

    fn update_state<F>(&self, update_fn: F) -> Result<()>
    where
        F: FnOnce(&mut State) -> Result<()>,
    {
        // 1. Read current state
        let mut state = self.read_state()?;

        // 2. Apply updates
        update_fn(&mut state)?;

        // 3. Write atomically
        self.write_state(&state)?;

        Ok(())
    }
}

impl StorageBackend for FilesystemStorage {
    fn initialize(&self) -> Result<()> {
        fs::create_dir_all(self.objects_dir())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_write_and_read_object() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let storage = FilesystemStorage::new(temp_dir.path())?;
        storage.initialize()?;

        let content = b"Hello, World!";
        let id = storage.write_object(content)?;

        let read_content = storage.read_object(&id)?;
        assert_eq!(content.to_vec(), read_content);

        Ok(())
    }

    #[test]
    fn test_object_deduplication() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let storage = FilesystemStorage::new(temp_dir.path())?;
        storage.initialize()?;

        let content = b"Test content";
        let id1 = storage.write_object(content)?;
        let id2 = storage.write_object(content)?;

        assert_eq!(id1, id2);
        Ok(())
    }

    #[test]
    fn test_state_persistence() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let storage = FilesystemStorage::new(temp_dir.path())?;
        storage.initialize()?;

        let mut state = State::default();
        state.refs.insert("refs/heads/main".to_string(), "abc123".to_string());

        storage.write_state(&state)?;

        let read_state = storage.read_state()?;
        assert_eq!(
            read_state.refs.get("refs/heads/main"),
            Some(&"abc123".to_string())
        );

        Ok(())
    }
}
