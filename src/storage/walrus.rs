use std::{cell::RefCell, collections::BTreeMap, path::PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use super::{
    traits::{ContentId, ImmutableStore, MutableState, StorageBackend},
    CacheIndex, FilesystemStorage, ParsedContentId, State,
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
/// - Objects map -> Walrus blob (RemoteState.objects_blob_object_id points to it)
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

    /// Cached state to avoid redundant reads during single operation
    /// (e.g., list followed by fetch both need state)
    cached_state: RefCell<Option<State>>,
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
            cached_state: RefCell::new(None),
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

    /// Extract unique blob_object_ids from ContentIds (handles batched format)
    fn extract_blob_object_ids(content_ids: &[&str]) -> Vec<String> {
        use std::collections::HashSet;

        let mut blob_ids: HashSet<String> = HashSet::new();

        for content_id in content_ids {
            if let Ok(parsed) = ParsedContentId::parse(content_id) {
                blob_ids.insert(parsed.blob_object_id().to_string());
            }
        }

        blob_ids.into_iter().collect()
    }

    /// Rehydrate blob_tracker from objects map (lazy discovery)
    /// This allows any client to discover blob expiration info from on-chain state
    fn rehydrate_blob_tracker(&self, objects: &BTreeMap<String, ContentId>) -> Result<()> {
        if objects.is_empty() {
            return Ok(());
        }

        // Extract all unique blob_object_ids from the objects map
        let content_ids: Vec<&str> = objects.values().map(|s| s.as_str()).collect();
        let blob_object_ids = Self::extract_blob_object_ids(&content_ids);

        if blob_object_ids.is_empty() {
            return Ok(());
        }

        tracing::info!(
            "  Rehydrating blob tracker from {} unique blob(s)...",
            blob_object_ids.len()
        );

        // Load current tracker to check what we already have
        let mut tracker = self.load_blob_tracker()?;

        // Query Sui for blob statuses we don't already have
        let mut discovered_count = 0;
        for blob_object_id in blob_object_ids {
            // Skip if we already track this blob
            if tracker.get_blob(&blob_object_id).is_some() {
                continue;
            }

            // Query Sui for this blob's status
            match self
                .runtime
                .block_on(self.sui_client.get_shared_blob_status(&blob_object_id))
            {
                Ok(status) => {
                    tracker.track_blob(
                        status.object_id,
                        status.blob_id,
                        status.end_epoch,
                        None, // We don't know size from just object ID
                    );
                    discovered_count += 1;
                }
                Err(e) => {
                    tracing::debug!(
                        "Could not get blob status for {}: {}",
                        &blob_object_id[..std::cmp::min(blob_object_id.len(), 16)],
                        e
                    );
                }
            }
        }

        if discovered_count > 0 {
            tracing::info!("  Discovered {} new blob(s) for tracking", discovered_count);
            self.save_blob_tracker(&tracker)?;
        }

        Ok(())
    }

    /// Check for blob expiration warnings and emit to stderr
    /// If `relevant_blob_ids` is provided, only check those specific blobs
    fn check_blob_expiration(&self, relevant_blob_ids: Option<&Vec<String>>) -> Result<()> {
        tracing::debug!("Checking blob expiration...");
        let tracker = self.load_blob_tracker()?;

        if tracker.count() == 0 {
            return Ok(());
        }

        // Get current Walrus epoch
        let current_epoch = match self.walrus_client.current_epoch() {
            Ok(info) => info.current_epoch,
            Err(e) => {
                tracing::warn!(
                    "Failed to get current Walrus epoch: {}",
                    e
                );
                return Ok(());
            }
        };

        // Check for expiration warnings (filtered to relevant blobs if provided)
        let (should_warn, min_epoch, expiring_soon) = tracker
            .check_expiration_warning(current_epoch, self.config.expiration_warning_threshold, relevant_blob_ids);

        if should_warn {
            tracing::warn!(
                "WARNING: {} blob(s) expiring soon!",
                expiring_soon.len()
            );
            tracing::warn!("  Current Walrus epoch: {}", current_epoch);
            tracing::warn!(
                "  Warning threshold: {} epochs",
                self.config.expiration_warning_threshold
            );

            if let Some(min) = min_epoch {
                tracing::warn!("  Earliest expiration: epoch {}", min);
            }

            // List expiring blobs
            for blob in expiring_soon.iter().take(5) {
                let epochs_remaining = blob.end_epoch.saturating_sub(current_epoch);
                tracing::warn!(
                    "    - {} expires in {} epoch(s)",
                    &blob.blob_id[..16],
                    epochs_remaining
                );
            }

            if expiring_soon.len() > 5 {
                tracing::warn!("    ... and {} more", expiring_soon.len() - 5);
            }

            tracing::warn!(
                "  Action required: Re-upload expiring blobs or repository may become inaccessible"
            );
        } else {
            tracing::info!("Tracking {} blob(s), earliest expiration at epoch {} (current: {})",
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
            tracing::debug!(
                "Object '{}...' already cached as '{}...'",
                &sha256[..8],
                &object_id[..16]
            );
            return Ok(object_id.clone());
        }

        // 2. Upload to Walrus
        tracing::info!(
            "Uploading object '{}...' ({} bytes)",
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
                tracing::warn!(
                    "Failed to get blob status from Sui: {} [shared_object_id: {}]",
                    e, blob_info.shared_object_id
                );
            }
        }

        Ok(blob_info.shared_object_id)
    }

    fn write_objects(&self, contents: &[&[u8]]) -> Result<Vec<ContentId>> {
        if contents.is_empty() {
            return Ok(Vec::new());
        }

        // If batching is disabled, fall back to sequential writes
        if !self.config.enable_batching {
            tracing::debug!("Batching disabled, using sequential writes");
            return contents
                .iter()
                .map(|content| self.write_object(content))
                .collect();
        }

        tracing::info!(
            "Processing {} objects (batching enabled, max batch size: {} MB)",
            contents.len(),
            self.config.max_batch_blob_size / (1024 * 1024)
        );

        // Load cache index once for all lookups
        let mut cache_index = self.load_cache_index()?;
        let mut blob_tracker = self.load_blob_tracker()?;

        // Track result ContentIds (in same order as input)
        let mut result_content_ids: Vec<Option<ContentId>> = vec![None; contents.len()];

        // Separate already-cached objects from those that need uploading
        let mut objects_to_upload: Vec<(usize, &[u8], String)> = Vec::new(); // (index, content, sha256)

        for (i, content) in contents.iter().enumerate() {
            let sha256 = Self::compute_sha256(content);

            if let Some(existing_content_id) = cache_index.get_object_id(&sha256) {
                // Already cached
                tracing::debug!("Object {}... already cached", &sha256[..8]);
                result_content_ids[i] = Some(existing_content_id.clone());
            } else {
                // Needs uploading
                objects_to_upload.push((i, content, sha256));
            }
        }

        if objects_to_upload.is_empty() {
            tracing::info!("All {} objects already cached", contents.len());
            return Ok(result_content_ids.into_iter().map(|id| id.unwrap()).collect());
        }

        tracing::info!(
            "Need to upload {} new objects ({} already cached)",
            objects_to_upload.len(),
            contents.len() - objects_to_upload.len()
        );

        // Group objects into batches respecting max_batch_blob_size
        let mut batches: Vec<Vec<(usize, &[u8], String)>> = Vec::new();
        let mut current_batch: Vec<(usize, &[u8], String)> = Vec::new();
        let mut current_batch_size: u64 = 0;

        for (idx, content, sha256) in objects_to_upload {
            let content_len = content.len() as u64;

            // If adding this object would exceed max batch size AND we have objects in the batch,
            // finalize the current batch and start a new one
            if current_batch_size + content_len > self.config.max_batch_blob_size
                && !current_batch.is_empty()
            {
                batches.push(std::mem::take(&mut current_batch));
                current_batch_size = 0;
            }

            current_batch.push((idx, content, sha256));
            current_batch_size += content_len;
        }

        // Add the last batch if non-empty
        if !current_batch.is_empty() {
            batches.push(current_batch);
        }

        tracing::info!(
            "Created {} batch(es) for upload",
            batches.len()
        );

        // Upload each batch
        for (batch_num, batch) in batches.iter().enumerate() {
            let batch_size: usize = batch.iter().map(|(_, content, _)| content.len()).sum();
            tracing::info!(
                "Uploading batch {}/{} ({} objects, {} bytes)",
                batch_num + 1,
                batches.len(),
                batch.len(),
                batch_size
            );

            if batch.len() == 1 {
                // Single object in batch - use legacy format (no batching overhead)
                let (idx, content, sha256) = &batch[0];

                let blob_info = self
                    .walrus_client
                    .store(content)
                    .context("Failed to store object in Walrus")?;

                let content_id = ParsedContentId::legacy(blob_info.shared_object_id.clone()).encode();

                // Cache locally
                let _ = self.cache.write_object(content); // Ignore errors

                // Update cache index
                cache_index.insert(blob_info.shared_object_id.clone(), sha256.clone());

                // Track blob expiration
                if let Ok(status) = self
                    .runtime
                    .block_on(self.sui_client.get_shared_blob_status(&blob_info.shared_object_id))
                {
                    blob_tracker.track_blob(
                        status.object_id,
                        status.blob_id,
                        status.end_epoch,
                        Some(content.len() as u64),
                    );
                }

                result_content_ids[*idx] = Some(content_id);
            } else {
                // Multiple objects in batch - concatenate and use batched format
                let mut concatenated = Vec::with_capacity(batch_size);
                let mut offsets: Vec<(usize, u64, u64, String)> = Vec::new(); // (index, offset, length, sha256)

                for (idx, content, sha256) in batch {
                    let offset = concatenated.len() as u64;
                    let length = content.len() as u64;
                    concatenated.extend_from_slice(content);
                    offsets.push((*idx, offset, length, sha256.clone()));

                    // Cache individual object locally
                    let _ = self.cache.write_object(content); // Ignore errors
                }

                // Upload concatenated batch to Walrus
                let blob_info = self
                    .walrus_client
                    .store(&concatenated)
                    .context("Failed to store batched blob in Walrus")?;

                // Create batched ContentIds for each object
                for (idx, offset, length, sha256) in offsets {
                    let content_id = ParsedContentId::batched(
                        blob_info.shared_object_id.clone(),
                        offset,
                        length,
                    )
                    .encode();

                    // Update cache index with batched ContentId
                    cache_index.insert(content_id.clone(), sha256);

                    result_content_ids[idx] = Some(content_id);
                }

                // Track blob expiration for the batched blob
                if let Ok(status) = self
                    .runtime
                    .block_on(self.sui_client.get_shared_blob_status(&blob_info.shared_object_id))
                {
                    blob_tracker.track_blob(
                        status.object_id,
                        status.blob_id,
                        status.end_epoch,
                        Some(concatenated.len() as u64),
                    );
                }

                tracing::info!(
                    "Batch {}/{} uploaded to {} ({} objects batched)",
                    batch_num + 1,
                    batches.len(),
                    &blob_info.shared_object_id[..16],
                    batch.len()
                );
            }
        }

        // Save updated cache index and blob tracker
        self.save_cache_index(&cache_index)?;
        self.save_blob_tracker(&blob_tracker)?;

        // Ensure all results are populated
        Ok(result_content_ids
            .into_iter()
            .map(|id| id.expect("All ContentIds should be populated"))
            .collect())
    }

    fn read_object(&self, id: &str) -> Result<Vec<u8>> {
        // Parse ContentId to detect batched vs legacy format
        let parsed_id = ParsedContentId::parse(id)
            .with_context(|| format!("Invalid ContentId format: {}", id))?;

        // 1. Try to read from cache (by sha256)
        let cache_index = self.load_cache_index()?;

        if let Some(sha256) = cache_index.get_sha256(id) {
            // Try cache hit
            match self.cache.read_object(sha256) {
                Ok(content) => {
                    tracing::debug!("Cache hit for ContentId {}", &id[..std::cmp::min(id.len(), 16)]);
                    return Ok(content);
                }
                Err(_) => {
                    // Cache miss, continue to Walrus
                    tracing::debug!("Cache miss for ContentId {}", &id[..std::cmp::min(id.len(), 16)]);
                }
            }
        }

        // 2. Get the blob_object_id (same for both legacy and batched)
        let blob_object_id = parsed_id.blob_object_id();

        // 3. Get blob_id from Sui object
        tracing::debug!(
            "Querying Sui for blob_id (object: {})",
            &blob_object_id[..std::cmp::min(blob_object_id.len(), 16)]
        );
        let blob_status = self
            .runtime
            .block_on(self.sui_client.get_shared_blob_status(blob_object_id))
            .with_context(|| format!("Failed to get SharedBlob status for object {}", blob_object_id))?;

        // 4. Read from Walrus using blob_id
        tracing::info!(
            "Downloading from Walrus: {}",
            &blob_status.blob_id[..std::cmp::min(blob_status.blob_id.len(), 16)]
        );
        let full_blob = self
            .walrus_client
            .read(&blob_status.blob_id)
            .with_context(|| {
                format!(
                    "Failed to read blob {} from Walrus (object: {})",
                    blob_status.blob_id, blob_object_id
                )
            })?;

        // 5. Extract the appropriate content based on ContentId format
        let content = match parsed_id {
            ParsedContentId::Legacy { .. } => {
                // Legacy format: entire blob is the object
                full_blob
            }
            ParsedContentId::Batched { offset, length, .. } => {
                // Batched format: extract slice from concatenated blob
                let start = offset as usize;
                let end = (offset + length) as usize;

                if end > full_blob.len() {
                    anyhow::bail!(
                        "Batched ContentId specifies range {}..{} but blob is only {} bytes",
                        start,
                        end,
                        full_blob.len()
                    );
                }

                tracing::debug!(
                    "Extracting batched object: bytes {}..{} from blob of {} bytes",
                    start,
                    end,
                    full_blob.len()
                );

                full_blob[start..end].to_vec()
            }
        };

        // 6. Cache it locally
        let sha256 = Self::compute_sha256(&content);
        let _ = self.cache.write_object(&content); // Ignore errors on cache write

        // 7. Update cache index
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
        // Check if we have a cached state
        if let Some(cached) = self.cached_state.borrow().as_ref() {
            tracing::debug!(
                "git-remote-walrus: Using cached state ({} refs, {} objects)",
                cached.refs.len(),
                cached.objects.len()
            );
            return Ok(cached.clone());
        }

        tracing::info!(
            "git-remote-walrus: Reading state from {}",
            &self.state_object_id
        );

        // Read refs from Sui on-chain
        let refs = self
            .runtime
            .block_on(self.sui_client.read_refs())
            .context("Failed to read refs from Sui")?;

        tracing::info!("  Retrieved {} refs from Sui", refs.len());

        // Get objects_blob_object_id from Sui
        let objects_object_id = self
            .runtime
            .block_on(self.sui_client.get_objects_blob_object_id())
            .context("Failed to get objects object ID from Sui")?;

        // Download objects map from Walrus if it exists
        let objects = if let Some(object_id) = objects_object_id {
            tracing::info!(
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
            tracing::info!("  No objects object ID found, starting with empty objects map");
            BTreeMap::new()
        };

        tracing::info!("  Retrieved {} objects mappings", objects.len());

        // Lazy rehydration: discover blob expiration info from objects map
        // This allows any client (including fresh clones) to track blob expiration
        if !objects.is_empty() {
            let _ = self.rehydrate_blob_tracker(&objects); // Best effort, don't fail on errors
        }

        let state = State { refs, objects };

        // Cache the state for subsequent reads
        *self.cached_state.borrow_mut() = Some(state.clone());

        Ok(state)
    }

    fn write_state(&self, state: &State) -> Result<()> {
        tracing::info!(
            "git-remote-walrus: Writing state to {} ({} refs, {} objects)",
            self.state_object_id,
            state.refs.len(),
            state.objects.len()
        );

        // Invalidate cached state since we're writing new state
        *self.cached_state.borrow_mut() = None;

        // Check for blob expiration warnings (scoped to this repo's blobs)
        let content_ids: Vec<&str> = state.objects.values().map(|s| s.as_str()).collect();
        let relevant_blob_ids = Self::extract_blob_object_ids(&content_ids);
        let _ = self.check_blob_expiration(Some(&relevant_blob_ids));

        // Step 1: Acquire lock on RemoteState (5 minute timeout)
        // This ensures no one else can modify the state while we upload to Walrus
        tracing::info!("  Acquiring lock on RemoteState...");
        self.runtime
            .block_on(self.sui_client.acquire_lock(300_000))
            .context("Failed to acquire lock on RemoteState")?;

        // Step 2: Serialize and upload objects map to Walrus (while holding lock)
        tracing::info!("  Serializing objects map...");
        let objects_yaml_str = serde_yaml::to_string(&state.objects)
            .context("Failed to serialize objects map to YAML")?;
        let objects_yaml = objects_yaml_str.as_bytes();

        tracing::info!(
            "  Uploading objects map to Walrus ({} bytes)...",
            objects_yaml.len()
        );
        let objects_blob_info = self
            .walrus_client
            .store(objects_yaml)
            .context("Failed to upload objects map to Walrus")?;

        tracing::info!(
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

        // Step 4: Execute atomic PTB: update refs + update objects_blob_object_id + release lock
        tracing::info!(
            "  Executing atomic PTB (update {} refs + objects object + release lock)...",
            refs.len()
        );
        self.runtime
            .block_on(
                self.sui_client
                    .upsert_refs_and_update_objects(refs, objects_blob_info.shared_object_id),
            )
            .context("Failed to execute atomic PTB")?;

        tracing::info!("  State successfully written to Sui");

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
        tracing::info!("git-remote-walrus: Initializing Walrus storage");
        tracing::info!("  State object: {}", self.state_object_id);
        tracing::info!("  Cache dir: {:?}", self.config.cache_dir);
        tracing::info!("  Wallet: {:?}", self.config.sui_wallet_path);

        // Initialize cache
        self.cache
            .initialize()
            .context("Failed to initialize cache")?;

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
