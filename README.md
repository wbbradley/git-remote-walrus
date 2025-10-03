# git-remote-gitwal

A custom Git remote helper that stores repository data in a content-addressed, immutable storage system.

## Overview

`git-remote-gitwal` is a Git remote helper that implements the `gitwal::` protocol, allowing you to push and pull Git repositories to/from a custom storage backend that enforces immutability constraints.

## Features

- **Content-addressed storage**: All objects are stored using SHA-256 hashes
- **Immutable storage**: Once written, files never change
- **Pluggable backends**: Storage layer is abstracted via traits
- **Standard Git workflow**: Works with existing Git commands

## Installation

Build and install the binary:

```bash
cargo build --release
sudo cp target/release/git-remote-gitwal /usr/local/bin/
```

Or just run from the target directory:

```bash
cargo build --release
export PATH="$PWD/target/release:$PATH"
```

## Usage

### Push to a gitwal remote

```bash
# Create a test repository
git init myrepo
cd myrepo
echo "Hello, World!" > file.txt
git add .
git commit -m "Initial commit"

# Add gitwal remote and push
git remote add storage gitwal::/tmp/mystorage
git push storage main
```

### Clone from a gitwal remote

```bash
git clone gitwal::/tmp/mystorage myclone
cd myclone
# Your repository is now cloned!
```

### Incremental push

```bash
# Make more commits
echo "More content" >> file.txt
git commit -am "Update file"

# Push updates
git push storage main
```

## Storage Structure

The filesystem backend creates the following structure:

```
<storage-path>/
├── objects/           # Content-addressed immutable storage
│   ├── abc123...      # SHA-256 named files
│   └── def456...
└── state.yaml         # Mutable state file
```

### state.yaml Format

```yaml
refs:
  refs/heads/main: "abc123..."  # Git SHA-1 of commit
  refs/tags/v1.0.0: "def456..."

objects:
  abc123...: "sha256-hash"  # Git SHA-1 -> Storage ContentId mapping
```

## Architecture

The project is organized into several modules:

- **main.rs**: Entry point and URL parsing
- **protocol.rs**: Git remote helper protocol handler
- **commands/**: Implementation of Git commands (capabilities, list, import, export)
- **storage/**: Storage abstraction layer
  - **traits.rs**: Storage trait definitions
  - **filesystem.rs**: Filesystem backend implementation
  - **state.rs**: State data structure
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

1. Git spawns `git-remote-gitwal gitwal::/path/to/storage`
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
- No garbage collection
- No compression
- Filesystem backend only

See plan.md for future enhancements.

## References

- [Git Remote Helpers](https://git-scm.com/docs/gitremote-helpers)
- [Git Fast-Import](https://git-scm.com/docs/git-fast-import)
- [Git Fast-Export](https://git-scm.com/docs/git-fast-export)

## License

MIT
