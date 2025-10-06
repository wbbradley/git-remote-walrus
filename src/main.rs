#![deny(clippy::mod_module_files)]
use anyhow::Result;
use std::env;
use std::path::PathBuf;

mod commands;
mod error;
mod git;
mod pack;
mod protocol;
mod storage;
mod walrus;

use storage::{FilesystemStorage, StorageBackend};

/// Remote storage backend type
enum RemoteType {
    Filesystem(PathBuf),
    Sui(String), // Sui object ID as hex string
}

/// Wrapper enum for different storage backends
/// This allows us to use different storage types with the protocol handler
enum Storage {
    Filesystem(FilesystemStorage),
    // TODO: Add WalrusStorage variant when implemented
    // Walrus(WalrusStorage),
}

// Implement StorageBackend traits for Storage enum by delegating to inner types
impl storage::ImmutableStore for Storage {
    fn write_object(&self, content: &[u8]) -> Result<String> {
        match self {
            Storage::Filesystem(s) => s.write_object(content),
        }
    }

    fn write_objects(&self, contents: &[&[u8]]) -> Result<Vec<String>> {
        match self {
            Storage::Filesystem(s) => s.write_objects(contents),
        }
    }

    fn read_object(&self, id: &str) -> Result<Vec<u8>> {
        match self {
            Storage::Filesystem(s) => s.read_object(id),
        }
    }

    fn read_objects(&self, ids: &[&str]) -> Result<Vec<Vec<u8>>> {
        match self {
            Storage::Filesystem(s) => s.read_objects(ids),
        }
    }

    fn delete_object(&self, id: &str) -> Result<()> {
        match self {
            Storage::Filesystem(s) => s.delete_object(id),
        }
    }

    fn object_exists(&self, id: &str) -> Result<bool> {
        match self {
            Storage::Filesystem(s) => s.object_exists(id),
        }
    }
}

impl storage::MutableState for Storage {
    fn read_state(&self) -> Result<storage::State> {
        match self {
            Storage::Filesystem(s) => s.read_state(),
        }
    }

    fn write_state(&self, state: &storage::State) -> Result<()> {
        match self {
            Storage::Filesystem(s) => s.write_state(state),
        }
    }

    fn update_state<F>(&self, update_fn: F) -> Result<()>
    where
        F: FnOnce(&mut storage::State) -> Result<()>,
    {
        match self {
            Storage::Filesystem(s) => s.update_state(update_fn),
        }
    }
}

impl StorageBackend for Storage {
    fn initialize(&self) -> Result<()> {
        match self {
            Storage::Filesystem(s) => s.initialize(),
        }
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();

    // Check for init command: git-remote-walrus init [--shared] [--allow <addr>]...
    if args.len() >= 2 && args[1] == "init" {
        return handle_init(&args[2..]);
    }

    // Git passes three arguments:
    // 1. Binary path
    // 2. Remote name (e.g., "storage")
    // 3. Remote URL (e.g., "walrus::/tmp/storage" or "walrus::0x123...")
    if args.len() < 3 {
        anyhow::bail!(
            "Usage: git-remote-walrus <remote-name> <remote-url>\n       git-remote-walrus init [--shared] [--allow <address>]..."
        );
    }

    let _remote_name = &args[1];
    let remote_url = &args[2];

    // Parse the URL - format is walrus::<path or object-id>
    let remote_type = parse_remote_url(remote_url)?;

    // Initialize storage backend based on type
    let storage = match remote_type {
        RemoteType::Filesystem(path) => {
            eprintln!("git-remote-walrus: Using filesystem storage: {:?}", path);
            let fs_storage = FilesystemStorage::new(path)?;
            Storage::Filesystem(fs_storage)
        }
        RemoteType::Sui(object_id) => {
            eprintln!("git-remote-walrus: Using Sui storage: {}", object_id);
            // TODO: Implement WalrusStorage
            anyhow::bail!("Sui storage backend not yet implemented. Object ID: {}", object_id);
        }
    };

    storage.initialize()?;

    // Start protocol handler
    protocol::handle_commands(storage)?;

    Ok(())
}

fn parse_remote_url(url: &str) -> Result<RemoteType> {
    eprintln!("git-remote-walrus: Parsing URL: '{}'", url);

    // Git strips the protocol prefix, so we might receive either:
    // - "walrus::/path/to/storage" (user-specified format)
    // - "/path/to/storage" (Git has already stripped "walrus::")
    // - "walrus::0x1234..." (Sui object ID)
    // - "0x1234..." (Git has already stripped "walrus::")
    let path_str = url.strip_prefix("walrus::").unwrap_or(url);

    // Try to parse as Sui object ID (0x prefix + hex chars)
    if path_str.starts_with("0x") && path_str.len() > 2 {
        // Validate hex characters after 0x
        let hex_part = &path_str[2..];
        if hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(RemoteType::Sui(path_str.to_string()));
        }
    }

    // Treat as filesystem path
    Ok(RemoteType::Filesystem(PathBuf::from(path_str)))
}

fn handle_init(args: &[String]) -> Result<()> {
    // TODO: Implement Sui object creation
    // Parse args for --shared and --allow flags
    let mut shared = false;
    let mut allowlist = Vec::new();
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "--shared" => {
                shared = true;
                i += 1;
            }
            "--allow" => {
                if i + 1 >= args.len() {
                    anyhow::bail!("--allow requires an address argument");
                }
                allowlist.push(args[i + 1].clone());
                i += 2;
            }
            other => {
                anyhow::bail!("Unknown argument: {}", other);
            }
        }
    }

    eprintln!("git-remote-walrus: Creating new remote...");
    eprintln!("  Shared: {}", shared);
    if !allowlist.is_empty() {
        eprintln!("  Allowlist: {:?}", allowlist);
    }

    // TODO: Call Sui SDK to create RemoteState object
    anyhow::bail!("Init command not yet implemented. This will create a RemoteState object on Sui.");
}
