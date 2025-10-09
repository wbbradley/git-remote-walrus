use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use super::{
    traits::{ContentId, ImmutableStore, MutableState, StorageBackend},
    CacheIndex, FilesystemStorage, State,
};
use crate::{
    config::WalrusRemoteConfig,
    sui::SuiClient,
    walrus::{BlobTracker, WalrusClient},
};

/// Storage backend using Walrus for immutable objects and Sui for mutable state
///
/// Architecture:
/// - Git objects -> Walrus blobs (with local filesystem cache)
/// - Git refs -> Sui on-chain (RemoteState.refs table)
/// - Objects map -> Walrus blob (RemoteState.objects_blob_id points to it)
/// - Lock -> Sui on-chain (RemoteState.lock)
pub struct WalrusStorage {
    /// Configuration
    config: WalrusRemoteConfig,

    /// Sui object ID for RemoteState
    state_object_id: String,

    /// Local filesystem cache
    cache: FilesystemStorage,

    /// Walrus client for blob operations
    walrus_client: WalrusClient,

    /// Sui client for on-chain state (currently stub)
    sui_client: SuiClient,

    /// Tokio runtime for async operations
    runtime: tokio::runtime::Runtime,

    /// Cache index (blob_id ↔ sha256) path
    cache_index_path: PathBuf,

    /// Blob tracker path
    blob_tracker_path: PathBuf,
}

impl WalrusStorage {
    /// Create a new WalrusStorage instance
    pub fn new(state_object_id: String) -> Result<Self> {
        // Load configuration
        let walrus_remote_config =
            WalrusRemoteConfig::load().context("Failed to load configuration")?;

        // Ensure cache directory exists
        let cache_dir = walrus_remote_config.ensure_cache_dir()?;

        // Create cache storage
        let cache = FilesystemStorage::new(&cache_dir).context("Failed to create cache storage")?;

        // Create Walrus client
        let walrus_client = WalrusClient::new(
            walrus_remote_config.walrus_config_path.clone(),
            walrus_remote_config.default_epochs,
        );

        // Create tokio runtime for async operations
        let runtime = tokio::runtime::Runtime::new().context("Failed to create tokio runtime")?;

        // Create Sui client (need to block on async constructor)
        let sui_client = runtime.block_on(SuiClient::new(
            state_object_id.clone(),
            walrus_remote_config.sui_wallet_path.clone(),
        ))?;

        // Set up paths
        let cache_index_path = cache_dir.join("cache_index.yaml");
        let blob_tracker_path = cache_dir.join("blob_tracker.yaml");

        Ok(Self {
            config: walrus_remote_config,
            state_object_id,
            cache,
            walrus_client,
            sui_client,
            runtime,
            cache_index_path,
            blob_tracker_path,
        })
    }

    /// Compute SHA-256 hash of content
    fn compute_sha256(content: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content);
        hex::encode(hasher.finalize())
    }

    /// Load cache index
    fn load_cache_index(&self) -> Result<CacheIndex> {
        CacheIndex::load(&self.cache_index_path).context("Failed to load cache index")
    }

    /// Save cache index
    fn save_cache_index(&self, index: &CacheIndex) -> Result<()> {
        index
            .save(&self.cache_index_path)
            .context("Failed to save cache index")
    }

    /// Load blob tracker
    fn load_blob_tracker(&self) -> Result<BlobTracker> {
        BlobTracker::load(&self.blob_tracker_path).context("Failed to load blob tracker")
    }

    /// Save blob tracker
    fn save_blob_tracker(&self, tracker: &BlobTracker) -> Result<()> {
        tracker
            .save(&self.blob_tracker_path)
            .context("Failed to save blob tracker")
    }

    /// Check for blob expiration warnings and emit to stderr
    fn check_blob_expiration(&self) -> Result<()> {
        eprintln!("git-remote-walrus: Checking blob expiration...");
        let tracker = self.load_blob_tracker()?;

        if tracker.count() == 0 {
            return Ok(());
        }

        // Get current Walrus epoch
        let current_epoch = match self.walrus_client.current_epoch() {
            Ok(info) => info.current_epoch,
            Err(e) => {
                eprintln!(
                    "git-remote-walrus: Warning: Failed to get current Walrus epoch: {}",
                    e
                );
                return Ok(());
            }
        };

        // Check for expiration warnings
        let (should_warn, min_epoch, expiring_soon) = tracker
            .check_expiration_warning(current_epoch, self.config.expiration_warning_threshold);

        if should_warn {
            eprintln!(
                "git-remote-walrus: ⚠️  WARNING: {} blob(s) expiring soon!",
                expiring_soon.len()
            );
            eprintln!("  Current Walrus epoch: {}", current_epoch);
            eprintln!(
                "  Warning threshold: {} epochs",
                self.config.expiration_warning_threshold
            );

            if let Some(min) = min_epoch {
                eprintln!("  Earliest expiration: epoch {}", min);
            }

            // List expiring blobs
            for blob in expiring_soon.iter().take(5) {
                let epochs_remaining = blob.end_epoch.saturating_sub(current_epoch);
                eprintln!(
                    "    - {} expires in {} epoch(s)",
                    &blob.blob_id[..16],
                    epochs_remaining
                );
            }

            if expiring_soon.len() > 5 {
                eprintln!("    ... and {} more", expiring_soon.len() - 5);
            }

            eprintln!(
                "  Action required: Re-upload expiring blobs or repository may become inaccessible"
            );
        } else {
            eprintln!("git-remote-walrus: Tracking {} blob(s), earliest expiration at epoch {} (current: {})",
                     tracker.count(), min_epoch.unwrap_or(0), current_epoch);
        }

        Ok(())
    }
}

impl ImmutableStore for WalrusStorage {
    fn write_object(&self, content: &[u8]) -> Result<ContentId> {
        let sha256 = Self::compute_sha256(content);

        // 1. Check if already in cache (by sha256)
        let mut cache_index = self.load_cache_index()?;

        if let Some(object_id) = cache_index.get_object_id(&sha256) {
            // Already cached, return object_id
            eprintln!(
                "git-remote-walrus: Object '{}...' already cached as '{}...'",
                &sha256[..8],
                &object_id[..16]
            );
            return Ok(object_id.clone());
        }

        // 2. Upload to Walrus
        eprintln!(
            "git-remote-walrus: Uploading object '{}...' ({} bytes)",
            &sha256[..8],
            content.len()
        );
        let blob_info = self
            .walrus_client
            .store(content)
            .context("Failed to store object in Walrus")?;

        // 3. Store in local cache
        self.cache
            .write_object(content)
            .context("Failed to cache object locally")?;

        // 4. Update cache index (use shared_object_id as ContentId)
        cache_index.insert(blob_info.shared_object_id.clone(), sha256.clone());
        self.save_cache_index(&cache_index)?;

        // 5. Get blob status from Sui and track expiration
        match self
            .runtime
            .block_on(self.sui_client.get_shared_blob_status(&blob_info.shared_object_id))
        {
            Ok(status) => {
                let mut tracker = self.load_blob_tracker()?;
                tracker.track_blob(
                    status.object_id.clone(),
                    status.blob_id,
                    status.end_epoch,
                    Some(content.len() as u64),
                );
                self.save_blob_tracker(&tracker)?;
            }
            Err(e) => {
                eprintln!(
                    "git-remote-walrus: Warning: Failed to get blob status from Sui: {} [shared_object_id: {}]",
                    e, blob_info.shared_object_id
                );
            }
        }

        Ok(blob_info.shared_object_id)
    }

    fn write_objects(&self, contents: &[&[u8]]) -> Result<Vec<ContentId>> {
        // Simple implementation: write sequentially
        // TODO: Could optimize with parallel uploads in the future
        contents
            .iter()
            .map(|content| self.write_object(content))
            .collect()
    }

    fn read_object(&self, id: &str) -> Result<Vec<u8>> {
        // id is now an object_id
        // 1. Try to read from cache (by sha256)
        let cache_index = self.load_cache_index()?;

        if let Some(sha256) = cache_index.get_sha256(id) {
            // Try cache hit
            match self.cache.read_object(sha256) {
                Ok(content) => {
                    eprintln!("git-remote-walrus: Cache hit for {}", &id[..16]);
                    return Ok(content);
                }
                Err(_) => {
                    // Cache miss, continue to Walrus
                    eprintln!("git-remote-walrus: Cache miss for {}", &id[..16]);
                }
            }
        }

        // 2. Get blob_id from Sui object
        eprintln!(
            "git-remote-walrus: Querying Sui for blob_id (object: {})",
            &id[..16]
        );
        let blob_status = self
            .runtime
            .block_on(self.sui_client.get_shared_blob_status(id))
            .with_context(|| format!("Failed to get SharedBlob status for object {}", id))?;

        // 3. Read from Walrus using blob_id
        eprintln!(
            "git-remote-walrus: Downloading from Walrus: {}",
            &blob_status.blob_id[..16]
        );
        let content = self
            .walrus_client
            .read(&blob_status.blob_id)
            .with_context(|| {
                format!(
                    "Failed to read blob {} from Walrus (object: {})",
                    blob_status.blob_id, id
                )
            })?;

        // 4. Cache it locally
        let sha256 = Self::compute_sha256(&content);
        let _ = self.cache.write_object(&content); // Ignore errors on cache write

        // 5. Update cache index
        let mut cache_index = self.load_cache_index()?;
        cache_index.insert(id.to_string(), sha256);
        let _ = self.save_cache_index(&cache_index); // Ignore errors on index write

        Ok(content)
    }

    fn read_objects(&self, ids: &[&str]) -> Result<Vec<Vec<u8>>> {
        // Simple implementation: read sequentially
        // TODO: Could optimize with parallel reads in the future
        ids.iter().map(|id| self.read_object(id)).collect()
    }

    fn delete_object(&self, id: &str) -> Result<()> {
        // Walrus is immutable, so we only delete from cache
        let cache_index = self.load_cache_index()?;

        if let Some(sha256) = cache_index.get_sha256(id) {
            self.cache.delete_object(sha256)?;
        }

        // Note: We don't remove from cache_index or blob_tracker
        // as the blob still exists on Walrus

        Ok(())
    }

    fn object_exists(&self, id: &str) -> Result<bool> {
        // Check cache index
        let cache_index = self.load_cache_index()?;

        if cache_index.contains_object(id) {
            return Ok(true);
        }

        // Could query Sui for object, but for now assume not exists
        Ok(false)
    }
}

impl MutableState for WalrusStorage {
    fn read_state(&self) -> Result<State> {
        eprintln!(
            "git-remote-walrus: Reading state from {}",
            &self.state_object_id
        );

        // Read refs from Sui on-chain
        let refs = self
            .runtime
            .block_on(self.sui_client.read_refs())
            .context("Failed to read refs from Sui")?;

        eprintln!("  Retrieved {} refs from Sui", refs.len());

        // Get objects_blob_id (object_id) from Sui
        let objects_object_id = self
            .runtime
            .block_on(self.sui_client.get_objects_blob_id())
            .context("Failed to get objects object ID from Sui")?;

        // Download objects map from Walrus if it exists
        let objects = if let Some(object_id) = objects_object_id {
            eprintln!(
                "  Downloading objects map from Walrus (object_id: {})",
                &object_id
            );

            // Get blob_id from Sui
            let blob_status = self
                .runtime
                .block_on(self.sui_client.get_shared_blob_status(&object_id))
                .with_context(|| {
                    format!(
                        "Failed to get SharedBlob status for objects map (object: {})",
                        object_id
                    )
                })?;

            // Read from Walrus using blob_id
            let objects_yaml = self
                .walrus_client
                .read(&blob_status.blob_id)
                .with_context(|| {
                    format!(
                        "Failed to read objects map from Walrus (blob: {}, object: {})",
                        blob_status.blob_id, object_id
                    )
                })?;
            serde_yaml::from_slice(&objects_yaml).context("Failed to parse objects map YAML")?
        } else {
            eprintln!("  No objects object ID found, starting with empty objects map");
            BTreeMap::new()
        };

        eprintln!("  Retrieved {} objects mappings", objects.len());

        Ok(State { refs, objects })
    }

    fn write_state(&self, state: &State) -> Result<()> {
        eprintln!(
            "git-remote-walrus: Writing state to {} ({} refs, {} objects)",
            self.state_object_id,
            state.refs.len(),
            state.objects.len()
        );

        // Check for blob expiration warnings
        let _ = self.check_blob_expiration();

        // Step 1: Acquire lock on RemoteState (5 minute timeout)
        // This ensures no one else can modify the state while we upload to Walrus
        eprintln!("  Acquiring lock on RemoteState...");
        self.runtime
            .block_on(self.sui_client.acquire_lock(300_000))
            .context("Failed to acquire lock on RemoteState")?;

        // Step 2: Serialize and upload objects map to Walrus (while holding lock)
        eprintln!("  Serializing objects map...");
        let objects_yaml_str = serde_yaml::to_string(&state.objects)
            .context("Failed to serialize objects map to YAML")?;
        let objects_yaml = objects_yaml_str.as_bytes();

        eprintln!(
            "  Uploading objects map to Walrus ({} bytes)...",
            objects_yaml.len()
        );
        let objects_blob_info = self
            .walrus_client
            .store(objects_yaml)
            .context("Failed to upload objects map to Walrus")?;

        eprintln!(
            "  Objects shared object ID: {} (blob: {})",
            &objects_blob_info.shared_object_id,
            &objects_blob_info.blob_id
        );

        // Step 3: Convert refs to Vec for PTB
        let refs: Vec<(String, String)> = state
            .refs
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        // Step 4: Execute atomic PTB: update refs + update objects_blob_id + release lock
        eprintln!(
            "  Executing atomic PTB (update {} refs + objects object + release lock)...",
            refs.len()
        );
        self.runtime
            .block_on(
                self.sui_client
                    .upsert_refs_and_update_objects(refs, objects_blob_info.shared_object_id),
            )
            .context("Failed to execute atomic PTB")?;

        eprintln!("  State successfully written to Sui");

        Ok(())
    }

    fn update_state<F>(&self, update_fn: F) -> Result<()>
    where
        F: FnOnce(&mut State) -> Result<()>,
    {
        // Standard read-modify-write pattern
        let mut state = self.read_state()?;
        update_fn(&mut state)?;
        self.write_state(&state)?;
        Ok(())
    }
}

impl StorageBackend for WalrusStorage {
    fn initialize(&self) -> Result<()> {
        eprintln!("git-remote-walrus: Initializing Walrus storage");
        eprintln!("  State object: {}", self.state_object_id);
        eprintln!("  Cache dir: {:?}", self.config.cache_dir);
        eprintln!("  Wallet: {:?}", self.config.sui_wallet_path);

        // Initialize cache
        self.cache
            .initialize()
            .context("Failed to initialize cache")?;

        // Check blob expiration warnings
        let _ = self.check_blob_expiration();

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: These tests are limited until we have:
    // 1. Mock Sui client
    // 2. Mock Walrus client
    // 3. Localnet setup

    #[test]
    fn test_compute_sha256() {
        let content = b"Hello, World!";
        let hash = WalrusStorage::compute_sha256(content);

        // Known SHA-256 of "Hello, World!"
        assert_eq!(
            hash,
            "dffd6021bb2bd5b0af676290809ec3a53191dd81c7f70a4b28688a362182986f"
        );
    }
}
