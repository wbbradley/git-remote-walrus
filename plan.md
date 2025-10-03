# Git Remote Helper - Design Document

## Project: git-remote-gitwal

### Overview
This document describes the design and implementation plan for a custom Git remote helper that stores data in a content-addressed, immutable storage system. The remote helper will enable Git to push/pull repositories to/from a custom storage backend that enforces specific constraints.

---

## 1. Background: Git Remote Helpers

### What is a Git Remote Helper?
A Git remote helper is an external program that Git invokes to interact with remote repositories using non-native protocols. When you run commands like `git clone gitwal::/path/to/storage`, Git:
1. Looks for an executable named `git-remote-gitwal` in your PATH
2. Spawns it as a subprocess
3. Communicates with it via stdin/stdout using a text-based protocol
4. The helper handles the actual data transfer and storage

### Communication Protocol
Git and the remote helper communicate through a command-response protocol:

**Git sends commands like:**
- `capabilities` - Request list of what the helper can do
- `list` - Request list of remote refs (branches/tags)
- `list for-push` - Request list of refs in preparation for push
- `import <ref>` - Request to fetch specific refs
- `export` - Request to push refs

**Helper responds with:**
- Capability declarations
- Ref lists with SHA-1 hashes
- Status messages
- Empty line to signal completion

### Types of Remote Helpers

There are several capability models, but we'll implement the **import/export** model:

**Import capability:**
- Allows fetching objects from the remote
- Helper receives ref names to import
- Helper outputs a `git fast-import` stream to stdout
- Git reads this stream and imports objects into local repo

**Export capability:**
- Allows pushing objects to the remote
- Git sends a `git fast-export` stream to helper's stdin
- Helper reads this stream and stores objects in remote
- Helper reports success/failure

---

## 2. Storage Constraints

Our implementation must adhere to these constraints:

### Immutable Storage
- **All disk writes must be immutable** - once written, files never change
- **All filenames are SHA-256 hashes** of their contents (content-addressed storage)
- **No appending to files** - if you need to add data, create a new file
- **No seeking within files** - must read entire file into memory

### Mutable State
- A small YAML file (`state.yaml`) acts as mutable registers
- This file stores pointers (SHA-256 hashes) to actual data
- Only this file can be updated in place

### Allowed Operations
- ✅ Write new immutable files (with SHA-256 names)
- ✅ Read entire files into memory
- ✅ Delete files
- ✅ Update state.yaml atomically
- ❌ Append to files
- ❌ Seek within files
- ❌ Modify existing content-addressed files

---

## 3. Architecture Design

### Storage Abstraction Layer

**Key Design Decision:** All storage operations go through a trait-based abstraction to allow swapping backends. The reference implementation uses a filesystem with SHA-256 content addressing, but the system is designed to support other backends (e.g., cloud object storage, custom content stores).

### Directory Structure (Filesystem Implementation)

```
<storage-path>/
├── objects/           # Content-addressed immutable storage
│   ├── ab12cd34...    # Content ID named files (SHA-256 in filesystem impl)
│   ├── ef56gh78...    # Could be: pack files, marks files, or individual objects
│   └── ...
└── state.yaml         # Mutable state file
```

### State File Format

The `state.yaml` file contains pointers to the current state:

```yaml
# Current refs (branches and tags)
refs:
  refs/heads/main: "abc123def456..."      # Git SHA-1 of commit object
  refs/heads/develop: "789012abc345..."
  refs/tags/v1.0.0: "def678901234..."

# Mapping from Git SHA-1 to storage content IDs (opaque backend identifiers)
objects:
  abc123def456...: "content-id-from-backend"  # Could be SHA-256, URI, UUID, etc.
  789012abc345...: "another-content-id"

# Marks for incremental operations (optional)
import_marks: "content-id-of-import-marks-file"
export_marks: "content-id-of-export-marks-file"

# Metadata (optional)
last_modified: "2025-10-02T12:34:56Z"
```

### Component Architecture

```
┌─────────────────────────────────────┐
│   Git Client (user runs git clone)  │
└───────────────┬─────────────────────┘
                │ spawns process
                ▼
┌─────────────────────────────────────┐
│     git-remote-gitwal binary        │
│  ┌───────────────────────────────┐  │
│  │   Protocol Handler            │  │
│  │   - Parse commands from stdin │  │
│  │   - Write responses to stdout │  │
│  └───────────┬───────────────────┘  │
│              │                       │
│  ┌───────────▼───────────────────┐  │
│  │   Command Implementations     │  │
│  │   - capabilities()            │  │
│  │   - list()                    │  │
│  │   - import_refs()             │  │
│  │   - export_refs()             │  │
│  └───────────┬───────────────────┘  │
│              │                       │
│  ┌───────────▼───────────────────┐  │
│  │   Storage Layer               │  │
│  │   - StorageBackend trait      │  │
│  │   - ImmutableStore operations │  │
│  │   - MutableState management   │  │
│  └───────────┬───────────────────┘  │
└──────────────┼───────────────────────┘
               │
               ▼
       ┌─────────────────────────┐
       │  Backend Implementation │
       │  (Filesystem, Cloud,    │
       │   Custom Store, etc.)   │
       └─────────────────────────┘
```

---

## 4. Protocol Implementation Details

### 4.1 Capabilities Command

**Input:** `capabilities`

**Output:**
```
import
export
refspec refs/heads/*:refs/heads/*
refspec refs/tags/*:refs/tags/*

```

**Explanation:**
- `import` - We support fetching via fast-import stream
- `export` - We support pushing via fast-export stream
- `refspec` - Defines how refs map between local and remote
- Empty line signals completion

### 4.2 List Command

**Input:** `list` or `list for-push`

**Output:**
```
<sha1> <refname>
<sha1> <refname>
@refs/heads/main HEAD
<newline>
```

**Example:**
```
abc123def456... refs/heads/main
789012abc345... refs/heads/develop
def678901234... refs/tags/v1.0.0
@refs/heads/main HEAD

```

**Implementation:**
1. Read `state.yaml`
2. For each ref, output: `<git-sha1> <refname>`
3. Note: Git wants SHA-1, but we store SHA-256 internally. We need to:
   - Store the Git SHA-1 (40 hex chars) in state.yaml, not our SHA-256
   - Or, if we store objects, extract SHA-1 from the Git object itself
4. Output `@<default-branch> HEAD` to indicate default branch
5. Output empty line

### 4.3 Import Command

**Input:**
```
import refs/heads/main
import refs/heads/develop
<newline>
```

**Process:**
1. Git has sent us ref names it wants to import (fetch)
2. We need to output a `git fast-import` stream containing all objects reachable from those refs
3. Git will read our stdout and pipe it to `git fast-import`

**Output Format (fast-import stream):**
```
blob
mark :1
data <length>
<file-content>

commit refs/heads/main
mark :2
author Name <email> <timestamp>
committer Name <email> <timestamp>
data <length>
<commit-message>
from :<parent-mark-or-sha1>
M 100644 :1 path/to/file

done
```

**Implementation:**
1. For each ref requested:
   - Read the commit SHA-1 from state.yaml
   - Recursively find all reachable objects (commits, trees, blobs)
   - Output them in fast-import format
2. Use marks (`:1`, `:2`, etc.) for internal references
3. Optionally save marks to marks file for incremental operations
4. Output `done` and empty line when complete

**Data Storage Strategy:**
- Option A: Store entire fast-import stream as one immutable file per ref
- Option B: Store Git pack files and extract objects
- **Recommended: Option A** for simplicity - store the fast-export stream we received during push

### 4.4 Export Command

**Input:**
```
export
<newline>
```

**Process:**
1. Git will send a `git fast-export` stream to our stdin
2. We need to parse it and store all objects
3. Update refs in state.yaml
4. Report success

**Input Format (fast-export stream from Git):**
```
blob
mark :1
data <length>
<binary-data>

commit refs/heads/main
mark :2
author Name <email> <timestamp>
committer Name <email> <timestamp>
data <length>
<message>
from :<parent>
M 100644 :1 path/to/file

done
```

**Implementation:**
1. Read fast-export stream from stdin until `done`
2. Parse the stream to extract:
   - All blob/commit/tag objects
   - Final ref mappings (which SHA-1 each ref points to)
3. Store the entire stream as an immutable object
4. Update state.yaml with new ref → SHA-1 mappings
5. Output: `ok <refname>` for each successfully pushed ref
6. Output empty line

**Response:**
```
ok refs/heads/main
ok refs/heads/develop

```

---

## 5. Storage Layer Implementation

### 5.1 Storage Trait Abstraction

**Core Design Philosophy:** All storage operations go through traits to enable backend pluggability. The content addressing scheme is backend-specific and treated as opaque by the rest of the system.

```rust
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
```

### 5.2 Filesystem Implementation

**Reference implementation using SHA-256 content addressing:**

```rust
pub struct FilesystemStorage {
    base_path: PathBuf,
}

impl ImmutableStore for FilesystemStorage {
    fn write_object(&self, content: &[u8]) -> Result<ContentId> {
        // 1. Compute SHA-256 hash
        let hash = sha256(content);
        let hash_hex = hex::encode(hash);

        // 2. Write to objects/ directory
        let path = self.base_path.join("objects").join(&hash_hex);

        // 3. Only write if doesn't exist (immutable)
        if !path.exists() {
            std::fs::write(&path, content)?;
        }

        Ok(hash_hex)
    }

    fn write_objects(&self, contents: &[&[u8]]) -> Result<Vec<ContentId>> {
        // Simple implementation: write sequentially
        // Could be optimized with parallel writes if needed
        contents.iter()
            .map(|content| self.write_object(content))
            .collect()
    }

    fn read_object(&self, id: &str) -> Result<Vec<u8>> {
        let path = self.base_path.join("objects").join(id);
        // Must read entire file into memory (no seeking)
        Ok(std::fs::read(&path)?)
    }

    fn read_objects(&self, ids: &[&str]) -> Result<Vec<Vec<u8>>> {
        ids.iter()
            .map(|id| self.read_object(id))
            .collect()
    }

    fn delete_object(&self, id: &str) -> Result<()> {
        let path = self.base_path.join("objects").join(id);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }

    fn object_exists(&self, id: &str) -> Result<bool> {
        let path = self.base_path.join("objects").join(id);
        Ok(path.exists())
    }
}

impl MutableState for FilesystemStorage {
    // Implementation in next section
}

impl StorageBackend for FilesystemStorage {
    fn initialize(&self) -> Result<()> {
        std::fs::create_dir_all(self.base_path.join("objects"))?;
        Ok(())
    }
}
```

**Why This Design:**
- **Backend Agnostic:** ContentId is opaque; backends can use any addressing scheme
- **Batch Operations:** Supports efficient batch uploads for remote storage backends
- **Simple Interface:** Clear separation between immutable objects and mutable state
- **Future-Proof:** Easy to add cloud storage, databases, or custom backends

### 5.3 State Management (MutableState Implementation)

**Atomic Update Pattern for Filesystem:**
```rust
impl MutableState for FilesystemStorage {
    fn read_state(&self) -> Result<State> {
        let state_path = self.base_path.join("state.yaml");
        if state_path.exists() {
            let content = std::fs::read_to_string(&state_path)?;
            Ok(serde_yaml::from_str(&content)?)
        } else {
            Ok(State::default())
        }
    }

    fn write_state(&self, state: &State) -> Result<()> {
        let state_path = self.base_path.join("state.yaml");
        let temp_path = self.base_path.join(".state.yaml.tmp");

        // 1. Write to temp file
        let yaml = serde_yaml::to_string(state)?;
        std::fs::write(&temp_path, yaml)?;

        // 2. Atomic rename (atomic on POSIX systems)
        std::fs::rename(&temp_path, &state_path)?;

        Ok(())
    }

    fn update_state<F>(&self, update_fn: F) -> Result<()>
    where
        F: FnOnce(&mut State) -> Result<()>
    {
        // 1. Read current state
        let mut state = self.read_state()?;

        // 2. Apply updates
        update_fn(&mut state)?;

        // 3. Write atomically
        self.write_state(&state)?;

        Ok(())
    }
}
```

### 5.4 Data Model

**What to Store:**

For maximum simplicity, we'll store:
1. **Fast-import streams** - Complete fast-import output for each push
2. **Refs** - Mapping of ref names to Git SHA-1 commit hashes
3. **Marks files** (optional) - For incremental operations

**State Structure:**
```rust
#[derive(Serialize, Deserialize, Default)]
struct State {
    /// Maps Git ref names to Git SHA-1 commit hashes (40 hex chars)
    #[serde(default)]
    refs: HashMap<String, String>,  // ref_name -> git_sha1

    /// Maps Git SHA-1 hashes to backend content identifiers (opaque)
    /// Content IDs could be SHA-256, URIs, UUIDs - backend-specific
    #[serde(default)]
    objects: HashMap<String, ContentId>, // git_sha1 -> backend_content_id

    /// Optional: content ID of import marks file for incremental operations
    #[serde(skip_serializing_if = "Option::is_none")]
    import_marks: Option<ContentId>,

    /// Optional: content ID of export marks file for incremental operations
    #[serde(skip_serializing_if = "Option::is_none")]
    export_marks: Option<ContentId>,
}
```

**Storage Strategy:**

When receiving a push (export):
1. Read entire fast-export stream from stdin
2. Parse it to extract:
   - Final commit SHA-1s for each ref
   - The complete object graph
3. Store entire stream as immutable object via `backend.write_object()` → get ContentId
4. Update `state.refs` with ref → Git SHA-1 mappings
5. Update `state.objects` with Git SHA-1 → ContentId mappings

When handling a fetch (import):
1. Look up requested refs in `state.refs` → get Git SHA-1s
2. Look up SHA-1s in `state.objects` → get ContentIds
3. Read stored fast-import streams via `backend.read_object(content_id)`
4. Output to stdout for Git to import

**Batch Operations:**
When storing multiple objects (e.g., multiple branches pushed at once), use `write_objects()` for efficiency. This is particularly beneficial for remote backends that support batch uploads.

---

## 6. Git Object Handling

### 6.1 Fast-Import Format

The fast-import format is a text-based streaming format. Key elements:

**Blob (file content):**
```
blob
mark :<n>
data <byte-count>
<raw-data>
```

**Commit:**
```
commit <ref>
mark :<n>
author <name> <email> <timestamp> <timezone>
committer <name> <email> <timestamp> <timezone>
data <byte-count>
<commit-message>
from :<parent-mark-or-sha1>
merge :<parent2>
M <mode> <mark-or-sha> <path>
D <path>
```

**Tag:**
```
tag <name>
from :<mark-or-sha>
tagger <name> <email> <timestamp> <timezone>
data <byte-count>
<tag-message>
```

**Key Commands:**
- `M <mode> <dataref> <path>` - Modify file (mode: 100644, 100755, 120000, 160000)
- `D <path>` - Delete file
- `C <src> <dst>` - Copy file
- `R <src> <dst>` - Rename file
- `from <commit-ish>` - Parent commit
- `merge <commit-ish>` - Additional parent (for merge commits)

### 6.2 Parsing Strategy

**For Export (receiving from Git):**
- Read line by line from stdin
- State machine parser:
  - Look for `blob`, `commit`, `tag`, `reset`, `done`
  - For `data <n>`, read exactly n bytes
  - Track marks → SHA-1 mappings
  - Extract final ref → SHA-1 mappings

**For Import (sending to Git):**
- Option 1: Replay stored fast-export stream verbatim
- Option 2: Generate fast-import commands from stored objects

**Recommended: Store and replay** for simplicity

### 6.3 Marks Files

Marks files enable incremental operations. They map marks to SHA-1s:

```
:<mark> <sha1>
:1 abc123def456...
:2 789012abc345...
```

**Usage:**
- `--export-marks=<file>` in fast-export: Save marks after export
- `--import-marks=<file>` in fast-export: Skip already-exported commits
- Similar for fast-import

We can optionally implement marks support for efficiency.

---

## 7. Implementation Plan

### Phase 1: Project Setup
1. Create Rust project: `cargo init --name git-remote-gitwal`
2. Add dependencies:
   ```toml
   [dependencies]
   sha2 = "0.10"
   hex = "0.4"
   serde = { version = "1.0", features = ["derive"] }
   serde_yaml = "0.9"
   anyhow = "1.0"
   ```
3. Set up binary that Git can invoke
4. Parse command-line args (Git passes URL as argument)

### Phase 2: Storage Traits and Filesystem Implementation
1. Define storage trait module (`storage/traits.rs`):
   - `ContentId` type alias
   - `ImmutableStore` trait
   - `MutableState` trait
   - `StorageBackend` trait
2. Implement `FilesystemStorage` backend:
   - SHA-256 hashing utilities
   - `ImmutableStore` implementation (write_object, write_objects, read_object, read_objects, etc.)
   - `MutableState` implementation (atomic state updates with temp file + rename)
   - `StorageBackend` implementation (initialize)
3. Implement State struct with serde YAML
4. Add comprehensive tests for all storage operations
5. Document trait contract and requirements for future implementers

### Phase 3: Protocol Handler
1. Set up stdin/stdout communication
2. Implement command parser (read lines until empty line)
3. Implement command dispatcher
4. Add debug logging (to stderr, not stdout!)
5. Test with manual protocol interaction

### Phase 4: Capabilities and List
1. Implement `capabilities` command
   - Output: `import\nexport\nrefspec ...\n\n`
2. Implement `list` command
   - Read state.yaml
   - Output all refs with SHA-1s
   - Handle empty repo case
3. Test: `echo "list" | git-remote-gitwal gitwal::/tmp/test`

### Phase 5: Export (Push Support)
1. Implement fast-export stream parser
   - Parse `blob`, `commit`, `tag` commands
   - Handle `data <n>` and read exactly n bytes
   - Track marks and final ref mappings
2. Implement `export` command handler
   - Read stream from stdin
   - Store entire stream as immutable object
   - Extract final ref → SHA-1 mappings
   - Update state.yaml
   - Output `ok <refname>` for each ref
3. Test: `git push gitwal::/tmp/test main`

### Phase 6: Import (Fetch Support)
1. Implement fast-import stream generation
   - Read stored streams from objects/
   - Output to stdout
2. Implement `import` command handler
   - Receive list of refs to import
   - Look up refs in state.yaml
   - Retrieve and output stored fast-import data
3. Test: `git clone gitwal::/tmp/test`

### Phase 7: Incremental Operations (Optional)
1. Implement marks file support
2. Store marks files as immutable objects
3. Track marks in state.yaml
4. Use marks for incremental push/fetch

### Phase 8: Error Handling & Robustness
1. Handle corrupted state.yaml
2. Handle missing objects (garbage collection)
3. Handle concurrent access (file locking on state.yaml?)
4. Proper error messages to stderr
5. Graceful degradation

### Phase 9: Testing & Validation
1. Test basic workflow:
   - Create a git repo
   - Push to gitwal remote
   - Clone from gitwal remote
   - Verify contents match
2. Test incremental operations:
   - Push additional commits
   - Fetch updates
3. Test edge cases:
   - Empty repo
   - Binary files
   - Large files
   - Merge commits
   - Tags
4. Test with real-world repos

### Phase 10: Optimization & Polish
1. Garbage collection for orphaned objects
2. Compression (gzip streams before storing?)
3. Performance profiling
4. Documentation
5. Installation script

---

## 8. Key Design Decisions

### Decision 1: Store Fast-Export Streams vs Git Objects

**Option A: Store fast-export streams**
- ✅ Simple to implement
- ✅ Preserves all information
- ✅ Easy to replay
- ❌ Duplicates objects on multiple pushes
- ❌ Larger storage

**Option B: Store individual Git objects**
- ✅ Deduplication
- ✅ Smaller storage
- ❌ Complex to implement
- ❌ Need to reconstruct history for fetch

**Chosen: Option A** for initial implementation, optimize later if needed.

### Decision 2: How to Handle Git SHA-1 vs Backend Content IDs

Git uses SHA-1 (40 hex chars) for object identification. Storage backends use opaque content identifiers (could be SHA-256, URIs, UUIDs, etc.).

**Mapping:**
```
Git Object SHA-1 → Backend ContentId (opaque, backend-specific)
```

Store this mapping in state.yaml:
```yaml
objects:
  abc123def456...: "backend-content-id"  # Opaque identifier from storage backend
```

**Why Opaque Content IDs:**
- Different backends have different addressing schemes (hashes, URIs, database keys)
- System doesn't need to know or care about the format
- Easy to swap backends without changing core logic

### Decision 3: Handling Refs

Store refs in state.yaml with Git SHA-1s (not our SHA-256s):
```yaml
refs:
  refs/heads/main: "abc123..."  # Git SHA-1 of commit
```

This makes it easy to output correct SHA-1s in the `list` command.

### Decision 4: Atomic Updates

Use temp file + rename for atomic state.yaml updates:
```
1. Write new state to .state.yaml.tmp
2. fsync
3. Rename to state.yaml (atomic on POSIX)
```

---

## 9. Example Workflows

### Workflow 1: Initial Push

```bash
# User creates a repo and pushes
git init myrepo
cd myrepo
echo "Hello" > file.txt
git add .
git commit -m "Initial commit"
git remote add origin gitwal::/tmp/mystorage
git push origin main
```

**What happens:**
1. Git spawns `git-remote-gitwal gitwal::/tmp/mystorage`
2. Git sends: `capabilities`
3. Helper responds: `import\nexport\n...\n`
4. Git sends: `list for-push\n\n`
5. Helper reads state.yaml (empty), responds: `\n`
6. Git sends: `export\n\n`
7. Git pipes fast-export stream to helper
8. Helper reads stream, stores as `objects/<sha256>`
9. Helper updates state.yaml with refs
10. Helper responds: `ok refs/heads/main\n\n`

**Storage after push (Filesystem backend):**
```
/tmp/mystorage/
├── objects/
│   └── a1b2c3d4...  (ContentId: 64 char SHA-256 in filesystem impl)
└── state.yaml
    refs:
      refs/heads/main: "abc123..."  (40 char Git SHA-1)
    objects:
      abc123...: "a1b2c3d4..."  (ContentId from backend)
```

### Workflow 2: Clone

```bash
git clone gitwal::/tmp/mystorage myrepo2
```

**What happens:**
1. Git spawns helper
2. Git sends: `capabilities`
3. Helper responds with capabilities
4. Git sends: `list\n\n`
5. Helper reads state.yaml, responds: `abc123... refs/heads/main\n@refs/heads/main HEAD\n\n`
6. Git determines what to fetch
7. Git sends: `import refs/heads/main\n\n`
8. Helper looks up ref, retrieves fast-export stream from objects/
9. Helper outputs stream to stdout
10. Git reads stream and imports objects

---

## 10. Testing Strategy

### Unit Tests
- Storage layer: write, read, hash computation
- State management: serialize, deserialize, atomic update
- Fast-export parsing: parse blobs, commits, tags
- Fast-import generation: generate valid streams

### Integration Tests
- End-to-end: push to gitwal remote, clone from it
- Incremental: push, then push again
- Multiple refs: push multiple branches and tags
- Binary data: push binary files
- Large repos: test performance

### Manual Testing Checklist
- [ ] Empty repo push/clone
- [ ] Single commit push/clone
- [ ] Multiple commits push/clone
- [ ] Multiple branches
- [ ] Tags
- [ ] Binary files
- [ ] Large files (>100MB)
- [ ] Merge commits
- [ ] Incremental push
- [ ] Force push
- [ ] Fetch updates
- [ ] Concurrent operations

---

## 11. Future Enhancements

### Phase 2 Features
1. **Compression**: gzip streams before storing
2. **Deduplication**: Store individual objects instead of streams
3. **Garbage collection**: Remove unreachable objects
4. **Encryption**: Encrypt objects at rest
5. **Remote storage**: Support S3, Azure Blob, etc.
6. **Concurrent access**: Proper locking/transactions
7. **Shallow clones**: Support `--depth` flag

### Advanced Features
1. **Partial clone**: Support Git's partial clone protocol
2. **Wire protocol v2**: Support modern Git protocol
3. **LFS support**: Handle Git LFS objects
4. **Submodules**: Proper submodule support
5. **Hooks**: Pre-push, post-receive hooks
6. **Multi-tenancy**: Support multiple repos in one storage
7. **Replication**: Multi-region replication

---

## 12. Security Considerations

1. **Path traversal**: Validate that storage path doesn't escape
2. **Hash collisions**: Use SHA-256 (collision-resistant)
3. **Disk space**: Implement quotas to prevent disk exhaustion
4. **Malicious input**: Validate fast-export stream format
5. **Concurrent writes**: Prevent corruption from simultaneous pushes
6. **Permissions**: Ensure proper file permissions on objects/

---

## 13. Performance Considerations

1. **Memory usage**: Fast-export streams can be large, need streaming parsing
2. **Disk I/O**: Minimize reads/writes, use buffering
3. **CPU**: SHA-256 hashing can be CPU-intensive, consider parallel hashing
4. **Network**: Not applicable for local storage, but relevant for remote
5. **Incremental operations**: Use marks files to avoid re-sending objects

---

## 14. References

- [Git Remote Helpers Documentation](https://git-scm.com/docs/gitremote-helpers)
- [Git Fast-Import Documentation](https://git-scm.com/docs/git-fast-import)
- [Git Fast-Export Documentation](https://git-scm.com/docs/git-fast-export)
- [Git Internals - Transfer Protocols](https://git-scm.com/book/en/v2/Git-Internals-Transfer-Protocols)

---

## Appendix A: Protocol Example Session

**Push session:**
```
→ capabilities
← import
← export
← refspec refs/heads/*:refs/heads/*
←

→ list for-push
←

→ export
→ blob
→ mark :1
→ data 6
→ Hello
→
→ commit refs/heads/main
→ mark :2
→ author John Doe <john@example.com> 1696262400 -0700
→ committer John Doe <john@example.com> 1696262400 -0700
→ data 15
→ Initial commit
→ M 100644 :1 file.txt
→
→ done
← ok refs/heads/main
←
```

**Fetch session:**
```
→ capabilities
← import
← export
← refspec refs/heads/*:refs/heads/*
←

→ list
← abc123def456789012345678901234567890 refs/heads/main
← @refs/heads/main HEAD
←

→ import refs/heads/main
→
← blob
← mark :1
← data 6
← Hello
←
← commit refs/heads/main
← mark :2
← author John Doe <john@example.com> 1696262400 -0700
← committer John Doe <john@example.com> 1696262400 -0700
← data 15
← Initial commit
← M 100644 :1 file.txt
←
← done
←
```

---

## Appendix B: Rust Module Structure

```
git-remote-gitwal/
├── Cargo.toml
├── README.md
└── src/
    ├── main.rs              # Entry point, CLI parsing
    ├── protocol.rs          # Protocol handler (stdin/stdout)
    ├── commands/
    │   ├── mod.rs
    │   ├── capabilities.rs  # Capabilities command
    │   ├── list.rs          # List command
    │   ├── import.rs        # Import command (fetch)
    │   └── export.rs        # Export command (push)
    ├── storage/
    │   ├── mod.rs
    │   ├── traits.rs        # StorageBackend, ImmutableStore, MutableState traits
    │   ├── filesystem.rs    # FilesystemStorage implementation
    │   └── state.rs         # State struct definition
    ├── git/
    │   ├── mod.rs
    │   ├── fast_import.rs   # Fast-import format generation
    │   └── fast_export.rs   # Fast-export format parsing
    └── error.rs             # Error types
```
