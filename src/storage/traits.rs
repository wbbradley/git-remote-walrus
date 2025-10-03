use anyhow::Result;

use super::State;

/// Opaque content identifier returned by storage backend.
/// Could be a SHA-256 hash, UUID, URI, or any backend-specific format.
pub type ContentId = String;

/// Trait for immutable, content-addressed storage operations
pub trait ImmutableStore {
    /// Write content and return its content identifier.
    /// If content already exists, returns identifier without writing.
    fn write_object(&self, content: &[u8]) -> Result<ContentId>;

    /// Write multiple objects in a batch operation.
    /// Returns content identifiers in the same order as inputs.
    /// More efficient than multiple write_object calls for some backends.
    fn write_objects(&self, contents: &[&[u8]]) -> Result<Vec<ContentId>>;

    /// Read object by content identifier into memory.
    /// Returns error if object doesn't exist.
    fn read_object(&self, id: &str) -> Result<Vec<u8>>;

    /// Read multiple objects in a batch operation.
    /// Returns objects in the same order as requested ids.
    /// Returns error if any object doesn't exist.
    fn read_objects(&self, ids: &[&str]) -> Result<Vec<Vec<u8>>>;

    /// Delete object by content identifier.
    /// Returns Ok(()) even if object didn't exist.
    fn delete_object(&self, id: &str) -> Result<()>;

    /// Check if object exists by identifier.
    fn object_exists(&self, id: &str) -> Result<bool>;
}

/// Trait for mutable state management
pub trait MutableState {
    /// Read the current state.
    /// Returns default state if none exists.
    fn read_state(&self) -> Result<State>;

    /// Atomically write new state.
    /// Implementation should ensure atomicity (temp file + rename or equivalent).
    fn write_state(&self, state: &State) -> Result<()>;

    /// Atomically update state using a closure.
    /// Handles read-modify-write with proper atomicity.
    fn update_state<F>(&self, update_fn: F) -> Result<()>
    where
        F: FnOnce(&mut State) -> Result<()>;
}

/// Combined storage backend trait
pub trait StorageBackend: ImmutableStore + MutableState {
    /// Initialize storage (create directories, verify access, etc.)
    fn initialize(&self) -> Result<()>;
}
