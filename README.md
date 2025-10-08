# git-remote-walrus

A custom Git remote helper that stores repository data in a content-addressed, immutable storage system.

## Overview

`git-remote-walrus` is a Git remote helper that implements the `walrus::` protocol, allowing you to push and pull Git repositories to/from decentralized storage using Sui blockchain and the Walrus network. It also supports a local filesystem backend for testing.

## Features

- **Decentralized storage**: Uses Sui blockchain for state and Walrus network for data
- **Content-addressed storage**: All objects are stored using SHA-256 hashes
- **Immutable storage**: Once written, blobs never change
- **Shared remotes**: Support for multi-user repositories with access control
- **Blob lifecycle management**: Configurable storage durations with epoch-based expiration
- **Local caching**: Automatic caching of Walrus blobs for performance
- **Pluggable backends**: Storage layer is abstracted via traits
- **Standard Git workflow**: Works with existing Git commands

## Installation

Build and install the binary:

```bash
cargo build --release
sudo cp target/release/git-remote-walrus /usr/local/bin/
```

Or just run from the target directory:

```bash
cargo build --release
export PATH="$PWD/target/release:$PATH"
```

## Configuration

Before using git-remote-walrus with Walrus, you need to configure your wallet paths:

```bash
# View current configuration
git-remote-walrus config

# Edit configuration (opens in $EDITOR)
git-remote-walrus config --edit
```

Required configuration settings:
- `sui_wallet_path`: Path to your Sui wallet config (e.g., `~/.sui/sui_config/client.yaml`)
- `walrus_config_path`: Path to your Walrus config (e.g., `~/.config/walrus/client.yaml`)
- `cache_dir`: Directory for caching Walrus blobs (e.g., `~/.cache/git-remote-walrus`)
- `default_epochs`: Number of epochs to store blobs (default: 5)
- `expiration_warning_threshold`: Warn when blobs expire within N epochs (default: 10)

You can also use environment variables:
- `SUI_WALLET`
- `WALRUS_CONFIG`
- `WALRUS_REMOTE_CACHE_DIR`
- `WALRUS_REMOTE_BLOB_EPOCHS`
- `WALRUS_EXPIRATION_WARNING_THRESHOLD`

## Usage

### Setup: Deploy and Initialize

**One-time setup**: Deploy the Move package to Sui (only needed once per network):

```bash
# Step 1: Deploy the Move package to Sui (one-time per network)
git-remote-walrus deploy

# This outputs a Package ID, for example:
# Package ID: 0x1234abcd...
```

**Note**: The Package ID can be shared across all users on the same network. For production use, consider deploying once and documenting a canonical Package ID that all users can reference, rather than having each user deploy their own package.

**Per-repository setup**: Create a new RemoteState object for each Git repository:

```bash
# Step 2: Create a private remote (only you can access)
git-remote-walrus init 0x1234abcd...

# Or create a shared remote (accessible by multiple users)
git-remote-walrus init 0x1234abcd... --shared --allow 0xaddress1 --allow 0xaddress2

# This outputs an Object ID, for example:
# ✓ Success! Your git remote is ready.
# To use this remote:
#   git remote add storage walrus::0x5678ef...
#   git push storage main
```

Note: You only need to deploy once, but you run `init` for each new Git repository you want to store in Walrus.

### Push to a Walrus remote

```bash
# Create a test repository
git init myrepo
cd myrepo
echo "Hello, World!" > file.txt
git add .
git commit -m "Initial commit"

# Add walrus remote (using Object ID from init command)
git remote add storage walrus::0x5678ef...
git push storage main
```

### Clone from a Walrus remote

```bash
git clone walrus::0x5678ef... myclone
cd myclone
# Your repository is now cloned from Walrus!
```

### Incremental push

```bash
# Make more commits
echo "More content" >> file.txt
git commit -am "Update file"

# Push updates
git push storage main
```

### Local filesystem storage (for testing)

You can also use local filesystem storage without Sui/Walrus:

```bash
# Add filesystem remote
git remote add storage walrus::/tmp/mystorage
git push storage main

# Clone from filesystem
git clone walrus::/tmp/mystorage myclone
```

## Storage Structure

### Walrus Backend (Sui + Walrus)

When using Walrus storage, git-remote-walrus:
- Stores repository state in a Sui RemoteState object (refs and object mappings)
- Stores Git object data as blobs in the Walrus network
- Caches Walrus blobs locally in `cache_dir` for performance
- Manages blob lifecycles with configurable epoch durations

The RemoteState object on Sui tracks:
- Git refs (branches, tags) mapped to commit SHA-1s
- Git object SHA-1s mapped to Walrus blob IDs
- Blob metadata including expiration epochs

### Filesystem Backend (for testing/development)

The filesystem backend creates the following structure:

```
<storage-path>/
├── objects/           # Content-addressed immutable storage
│   ├── abc123...      # SHA-256 named files
│   └── def456...
└── state.yaml         # Mutable state file
```

state.yaml format:
```yaml
refs:
  refs/heads/main: "abc123..."  # Git SHA-1 of commit
  refs/tags/v1.0.0: "def456..."

objects:
  abc123...: "sha256-hash"  # Git SHA-1 -> Storage ContentId mapping
```

## Architecture

The project is organized into several modules:

- **main.rs**: Entry point, URL parsing, CLI commands (deploy, init, config)
- **protocol.rs**: Git remote helper protocol handler
- **commands/**: Implementation of Git commands (capabilities, list, import, export)
- **storage/**: Storage abstraction layer
  - **traits.rs**: Storage trait definitions
  - **filesystem.rs**: Filesystem backend implementation (for testing)
  - **walrus.rs**: Walrus+Sui backend implementation
  - **state.rs**: State data structure
- **sui/**: Sui blockchain interaction
  - Wallet management
  - RemoteState object operations
  - Transaction building
- **walrus/**: Walrus network integration
  - Blob storage and retrieval
  - Blob lifecycle management
  - Local blob caching
- **git/**: Git format handling
  - **fast_export.rs**: Parse fast-export streams
  - **fast_import.rs**: Generate fast-import streams

## Design Principles

1. **Immutability**: All object storage is immutable and content-addressed
2. **Atomicity**: State updates use atomic file operations (temp + rename)
3. **Abstraction**: Storage backend is pluggable via traits
4. **Simplicity**: Initial implementation stores fast-export streams directly

## Development

Run tests:

```bash
cargo test
```

Build with debug output:

```bash
cargo build
```

The tool logs to stderr, so you can see debug output while Git communicates via stdin/stdout.

## How It Works

### Push Flow

1. Git spawns `git-remote-walrus walrus::/path/to/storage`
2. Git sends commands via stdin (capabilities, list, export)
3. Helper reads fast-export stream from Git
4. Helper stores stream as immutable object (SHA-256 addressed)
5. Helper updates state.yaml with ref → SHA-1 mappings
6. Helper reports success

### Clone/Fetch Flow

1. Git spawns helper and requests capabilities
2. Git requests ref list
3. Helper reads state.yaml and outputs refs
4. Git requests specific refs to import
5. Helper reads stored fast-export streams
6. Helper outputs streams to Git as fast-import format
7. Git imports objects into local repository

## Limitations

### Critical Limitation: GPG Signatures Not Preserved

**⚠️ IMPORTANT**: Due to using `git fast-export/fast-import`, GPG commit signatures are **not preserved**. This means:
- Commits with GPG signatures will lose their signatures when pushed/cloned
- Commit SHAs will change because the signature is part of the commit object
- All child commits will also get new SHAs (cascading effect)

This makes the current implementation **unsuitable for production use** where commit signature verification is required.

**Workaround**: For unsigned commits only, or if you don't need to preserve exact SHAs.

**Future Fix**: To properly preserve GPG signatures and exact Git SHAs, we need to switch from fast-export/fast-import to using Git's native pack format or raw object storage.

### Other Limitations

Current implementation:
- Stores entire fast-export streams (no object deduplication yet)
- Simple fast-export parser (may need improvements for complex repos)
- No automatic blob lifecycle extension (manual extension required before expiration)
- No compression

## References

- [Git Remote Helpers](https://git-scm.com/docs/gitremote-helpers)
- [Git Fast-Import](https://git-scm.com/docs/git-fast-import)
- [Git Fast-Export](https://git-scm.com/docs/git-fast-export)

## License

MIT
