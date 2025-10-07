#![deny(clippy::mod_module_files)]
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

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

#[derive(Parser)]
#[command(name = "git-remote-walrus")]
#[command(about = "Git remote helper for Walrus storage", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Remote name (passed by git)
    #[arg(value_name = "REMOTE_NAME", hide = true)]
    remote_name: Option<String>,

    /// Remote URL (passed by git)
    #[arg(value_name = "REMOTE_URL", hide = true)]
    remote_url: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Deploy the Move package to Sui
    Deploy,
    /// Initialize a new remote repository
    Init {
        /// Package ID of the deployed Walrus Move package
        package_id: String,
        /// Create a shared object (accessible by multiple users)
        #[arg(long)]
        shared: bool,
        /// Add addresses to the allowlist (can be specified multiple times)
        #[arg(long, value_name = "ADDRESS")]
        allow: Vec<String>,
    },
    /// Display or edit configuration
    Config {
        /// Open configuration file in $EDITOR
        #[arg(short, long)]
        edit: bool,
    },
}

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
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Deploy) => handle_deploy(),
        Some(Command::Init {
            package_id,
            shared,
            allow,
        }) => handle_init(package_id, shared, allow),
        Some(Command::Config { edit }) => handle_config(edit),
        None => {
            // Git passes remote name and URL as positional arguments
            let remote_url = cli
                .remote_url
                .ok_or_else(|| anyhow::anyhow!("Missing remote URL"))?;

            // Parse the URL - format is walrus::<path or object-id>
            let remote_type = parse_remote_url(&remote_url)?;

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
    }
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

fn handle_deploy() -> Result<()> {
    eprintln!("git-remote-walrus: Deploying Move package to Sui...\n");

    // Load configuration
    let config = config::WalrusRemoteConfig::load()?;

    eprintln!(
        "Hint: You can run `sui client --client.config {} faucet` to get test SUI if you are on a localnet.",
        config.sui_wallet_path.display()
    );
    eprintln!("Configuration:");
    eprintln!("  Wallet: {:?}\n", config.sui_wallet_path);

    // Get the move package directory
    let move_package_dir = std::env::current_dir()?.join("move").join("walrus_remote");

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

fn handle_init(package_id: String, shared: bool, allowlist: Vec<String>) -> Result<()> {
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

fn handle_config(edit: bool) -> Result<()> {
    let config_path = config::WalrusRemoteConfig::config_file_path()?;

    if edit {
        // Open config file in $EDITOR
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());

        eprintln!("Opening config file in {}: {:?}", editor, config_path);

        let status = std::process::Command::new(&editor)
            .arg(&config_path)
            .status()
            .with_context(|| format!("Failed to execute editor: {}", editor))?;

        if !status.success() {
            anyhow::bail!("Editor exited with non-zero status");
        }

        Ok(())
    } else {
        // Display current configuration
        println!("Configuration file: {:?}\n", config_path);

        if !config_path.exists() {
            println!("Config file does not exist yet.");
            println!(
                "\nCreate a config file at {:?} with contents like:\n",
                config_path
            );
            println!("sui_wallet_path: /path/to/.sui/sui_config/client.yaml");
            println!("walrus_config_path: /path/to/.config/walrus/client.yaml");
            println!("cache_dir: /path/to/.cache/git-remote-walrus");
            println!("default_epochs: 5");
            println!("expiration_warning_threshold: 10");
            return Ok(());
        }

        // Load and display config
        let config = config::WalrusRemoteConfig::load()?;

        println!("Current configuration:");
        println!("  sui_wallet_path: {:?}", config.sui_wallet_path);
        println!("  walrus_config_path: {:?}", config.walrus_config_path);
        println!("  cache_dir: {:?}", config.cache_dir);
        println!("  default_epochs: {}", config.default_epochs);
        println!(
            "  expiration_warning_threshold: {}",
            config.expiration_warning_threshold
        );

        println!("\nEnvironment variable overrides:");
        println!("  SUI_WALLET: {:?}", std::env::var("SUI_WALLET").ok());
        println!("  WALRUS_CONFIG: {:?}", std::env::var("WALRUS_CONFIG").ok());
        println!(
            "  WALRUS_REMOTE_CACHE_DIR: {:?}",
            std::env::var("WALRUS_REMOTE_CACHE_DIR").ok()
        );
        println!(
            "  WALRUS_REMOTE_BLOB_EPOCHS: {:?}",
            std::env::var("WALRUS_REMOTE_BLOB_EPOCHS").ok()
        );
        println!(
            "  WALRUS_EXPIRATION_WARNING_THRESHOLD: {:?}",
            std::env::var("WALRUS_EXPIRATION_WARNING_THRESHOLD").ok()
        );

        Ok(())
    }
}
