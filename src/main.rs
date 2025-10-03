use anyhow::Result;
use std::env;
use std::path::PathBuf;

mod commands;
mod error;
mod git;
mod protocol;
mod storage;

use storage::StorageBackend;

fn main() -> Result<()> {
    // Git passes the remote URL as the first argument
    // Example: git-remote-gitwal gitwal::/path/to/storage
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        anyhow::bail!("Usage: git-remote-gitwal <remote-url>");
    }

    let remote_url = &args[1];

    // Parse the URL - format is gitwal::<path>
    let storage_path = parse_remote_url(remote_url)?;

    // Log to stderr (not stdout, which is used for Git protocol)
    eprintln!("git-remote-gitwal: Using storage path: {:?}", storage_path);

    // Initialize storage backend
    let storage = storage::FilesystemStorage::new(storage_path)?;
    storage.initialize()?;

    // Start protocol handler
    protocol::handle_commands(storage)?;

    Ok(())
}

fn parse_remote_url(url: &str) -> Result<PathBuf> {
    // Format: gitwal::/path/to/storage or gitwal::relative/path
    if let Some(path_str) = url.strip_prefix("gitwal::") {
        Ok(PathBuf::from(path_str))
    } else {
        anyhow::bail!("Invalid remote URL format. Expected: gitwal::<path>");
    }
}
