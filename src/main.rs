#![deny(clippy::mod_module_files)]
use anyhow::{Context, Result};
use std::env;
use std::path::PathBuf;

mod commands;
mod config;
mod error;
mod git;
mod pack;
mod protocol;
mod storage;
mod sui;
mod walrus;

use storage::{FilesystemStorage, StorageBackend, WalrusStorage};

/// Remote storage backend type
enum RemoteType {
    Filesystem(PathBuf),
    Sui(String), // Sui object ID as hex string
}

/// Wrapper enum for different storage backends
/// This allows us to use different storage types with the protocol handler
enum Storage {
    Filesystem(FilesystemStorage),
    Walrus(Box<WalrusStorage>),
}

// Implement StorageBackend traits for Storage enum by delegating to inner types
impl storage::ImmutableStore for Storage {
    fn write_object(&self, content: &[u8]) -> Result<String> {
        match self {
            Storage::Filesystem(s) => s.write_object(content),
            Storage::Walrus(s) => s.write_object(content),
        }
    }

    fn write_objects(&self, contents: &[&[u8]]) -> Result<Vec<String>> {
        match self {
            Storage::Filesystem(s) => s.write_objects(contents),
            Storage::Walrus(s) => s.write_objects(contents),
        }
    }

    fn read_object(&self, id: &str) -> Result<Vec<u8>> {
        match self {
            Storage::Filesystem(s) => s.read_object(id),
            Storage::Walrus(s) => s.read_object(id),
        }
    }

    fn read_objects(&self, ids: &[&str]) -> Result<Vec<Vec<u8>>> {
        match self {
            Storage::Filesystem(s) => s.read_objects(ids),
            Storage::Walrus(s) => s.read_objects(ids),
        }
    }

    fn delete_object(&self, id: &str) -> Result<()> {
        match self {
            Storage::Filesystem(s) => s.delete_object(id),
            Storage::Walrus(s) => s.delete_object(id),
        }
    }

    fn object_exists(&self, id: &str) -> Result<bool> {
        match self {
            Storage::Filesystem(s) => s.object_exists(id),
            Storage::Walrus(s) => s.object_exists(id),
        }
    }
}

impl storage::MutableState for Storage {
    fn read_state(&self) -> Result<storage::State> {
        match self {
            Storage::Filesystem(s) => s.read_state(),
            Storage::Walrus(s) => s.read_state(),
        }
    }

    fn write_state(&self, state: &storage::State) -> Result<()> {
        match self {
            Storage::Filesystem(s) => s.write_state(state),
            Storage::Walrus(s) => s.write_state(state),
        }
    }

    fn update_state<F>(&self, update_fn: F) -> Result<()>
    where
        F: FnOnce(&mut storage::State) -> Result<()>,
    {
        match self {
            Storage::Filesystem(s) => s.update_state(update_fn),
            Storage::Walrus(s) => s.update_state(update_fn),
        }
    }
}

impl StorageBackend for Storage {
    fn initialize(&self) -> Result<()> {
        match self {
            Storage::Filesystem(s) => s.initialize(),
            Storage::Walrus(s) => s.initialize(),
        }
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();

    // Check for deploy command: git-remote-walrus deploy
    if args.len() >= 2 && args[1] == "deploy" {
        return handle_deploy(&args[2..]);
    }

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
            "Usage: git-remote-walrus <remote-name> <remote-url>\n       git-remote-walrus deploy\n       git-remote-walrus init <package-id> [--shared] [--allow <address>]..."
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
            eprintln!("git-remote-walrus: Using Walrus+Sui storage: {}", object_id);
            let walrus_storage = WalrusStorage::new(object_id)?;
            Storage::Walrus(Box::new(walrus_storage))
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

fn handle_deploy(_args: &[String]) -> Result<()> {
    eprintln!("git-remote-walrus: Deploying Move package to Sui...\n");

    // Load configuration
    let config = config::WalrusRemoteConfig::load()?;

    eprintln!("Configuration:");
    eprintln!("  Wallet: {:?}\n", config.sui_wallet_path);

    // Get the move package directory
    let move_package_dir = env::current_dir()?.join("move").join("walrus_remote");

    if !move_package_dir.exists() {
        anyhow::bail!(
            "Move package directory not found: {:?}\n\
             Please run this command from the git-remote-walrus repository root.",
            move_package_dir
        );
    }

    // Step 1: Build the Move package
    eprintln!("Step 1/2: Building Move package...");
    let build_output = std::process::Command::new("sui")
        .arg("move")
        .arg("build")
        .current_dir(&move_package_dir)
        .output()
        .context("Failed to execute 'sui move build'")?;

    if !build_output.status.success() {
        let stderr = String::from_utf8_lossy(&build_output.stderr);
        anyhow::bail!("Move build failed:\n{}", stderr);
    }

    eprintln!("✓ Move package built successfully\n");

    // Step 2: Publish the package
    eprintln!("Step 2/2: Publishing to Sui...");
    let publish_output = std::process::Command::new("sui")
        .arg("client")
        .arg("--client.config")
        .arg(&config.sui_wallet_path)
        .arg("publish")
        .arg("--json")
        .arg("--gas-budget")
        .arg("500000000") // 0.5 SUI
        .current_dir(&move_package_dir)
        .output()
        .context("Failed to execute 'sui client publish'")?;

    if !publish_output.status.success() {
        let stderr = String::from_utf8_lossy(&publish_output.stderr);
        anyhow::bail!("Publish failed:\n{}", stderr);
    }

    // Parse JSON output to extract package ID
    let stdout = String::from_utf8_lossy(&publish_output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&stdout).context("Failed to parse publish output as JSON")?;

    // Extract package ID from objectChanges
    let mut package_id: Option<String> = None;
    if let Some(object_changes) = json.get("objectChanges").and_then(|v| v.as_array()) {
        for change in object_changes {
            if let Some(change_type) = change.get("type").and_then(|v| v.as_str()) {
                if change_type == "published" {
                    if let Some(pkg_id) = change.get("packageId").and_then(|v| v.as_str()) {
                        package_id = Some(pkg_id.to_string());
                        break;
                    }
                }
            }
        }
    }

    let package_id = package_id
        .ok_or_else(|| anyhow::anyhow!("Failed to extract package ID from publish output"))?;

    eprintln!("✓ Package published successfully\n");
    eprintln!("Package ID: {}\n", package_id);

    // Print next steps
    eprintln!("Next steps:");
    eprintln!("  1. Create a remote:");
    eprintln!("       git-remote-walrus init {}", package_id);
    eprintln!("     Or for a shared remote:");
    eprintln!(
        "       git-remote-walrus init {} --shared --allow <address>",
        package_id
    );

    Ok(())
}

fn handle_init(args: &[String]) -> Result<()> {
    // First argument should be package ID
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        anyhow::bail!(
            "Usage: git-remote-walrus init <package-id> [--shared] [--allow <address>]..."
        );
    }

    let package_id = args[0].clone();

    // Parse remaining args for --shared and --allow flags
    let mut shared = false;
    let mut allowlist = Vec::new();
    let mut i = 1;

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
    eprintln!("  Package ID: {}", package_id);
    eprintln!("  Shared: {}", shared);
    if !allowlist.is_empty() {
        eprintln!("  Allowlist: {:?}", allowlist);
    }

    // Load configuration for RPC URL and wallet path
    let config = config::WalrusRemoteConfig::load()?;

    eprintln!("  Wallet: {:?}", config.sui_wallet_path);

    // Create async runtime for Sui operations
    let runtime = tokio::runtime::Runtime::new()?;

    runtime.block_on(async {
        // Create Sui client
        eprintln!("\nInitializing Sui client...");
        let sui_client = sui::SuiClient::new_for_init(package_id, config.sui_wallet_path).await?;

        // Create RemoteState object
        eprintln!("Creating RemoteState object...");
        let object_id = sui_client.create_remote().await?;
        eprintln!("✓ RemoteState created: {}", object_id);

        // Share if requested
        if shared {
            eprintln!("\nConverting to shared object...");
            sui_client
                .share_remote(object_id.clone(), allowlist)
                .await?;
            eprintln!("✓ RemoteState is now shared");
        }

        // Print instructions
        eprintln!("\n✓ Success! Your git remote is ready.");
        eprintln!("\nTo use this remote:");
        eprintln!("  git remote add storage walrus::{}", object_id);
        eprintln!("  git push storage main");

        Ok(())
    })
}
