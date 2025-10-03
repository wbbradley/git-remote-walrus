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
    // Git passes three arguments:
    // 1. Binary path
    // 2. Remote name (e.g., "storage")
    // 3. Remote URL (e.g., "gitwal::/tmp/storage")
    let args: Vec<String> = env::args().collect();

    if args.len() < 3 {
        anyhow::bail!("Usage: git-remote-gitwal <remote-name> <remote-url>");
    }

    let _remote_name = &args[1];
    let remote_url = &args[2];

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
    eprintln!("git-remote-gitwal: Parsing URL: '{}'", url);

    // Git strips the protocol prefix, so we might receive either:
    // - "gitwal::/path/to/storage" (user-specified format)
    // - "/path/to/storage" (Git has already stripped "gitwal::")
    let path_str = url.strip_prefix("gitwal::").unwrap_or(url);

    Ok(PathBuf::from(path_str))
}
