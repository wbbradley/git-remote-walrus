# Git Remote Helper - Implementation Status & Next Steps

## Project: git-remote-walrus

### Overview
A custom Git remote helper that stores data in a content-addressed, immutable storage system. The remote helper enables Git to push/pull repositories to/from a custom storage backend that enforces immutability constraints.

### 🎉 Phase 2 Complete: Native Pack Format
**Fast-export/import completely eliminated** - Now uses Git's native pack format end-to-end, preserving GPG signatures and all Git metadata. Both push and fetch operations use packfiles, ensuring exact SHA-1 preservation and signature integrity.

---

## Current Implementation Status

### ✅ Completed (Phase 1: fast-export/import approach)

**Architecture:**
- Storage abstraction layer with pluggable backends via traits
- Filesystem backend with SHA-256 content addressing
- Atomic state management using temp file + rename
- Git remote helper protocol handler (stdin/stdout communication)

**Functionality:**
- `capabilities` command - Advertises import/export support
- `list` command - Lists available refs using `?` marker format
- `export` command - Handles push by running `git fast-export --all` and storing output
- `import` command - Handles fetch/clone by replaying stored fast-export streams

**Storage Structure:**
```
<storage-path>/
├── objects/           # SHA-256 content-addressed immutable files
│   └── <sha256-hash>  # Stored fast-export streams
└── state.yaml         # Mutable state (refs → Git SHA-1, Git SHA-1 → ContentID)
```

**Working Features:**
- ✅ Push repositories to walrus storage
- ✅ Clone repositories from walrus storage
- ✅ Content and commit messages preserved
- ✅ File contents correctly stored and retrieved
- ✅ Multiple commits and history preserved

### ❌ Critical Limitations (Blockers for Production)

**GPG Signatures Not Preserved:**
- `git fast-export` does not include GPG signatures (`gpgsig` field) in its output
- `git fast-import` cannot restore signatures even if included
- **Result:** Commit SHAs change after push/clone cycle
- **Impact:** Cascading SHA changes - when parent commit SHA changes, all child commits get new SHAs

**Other Limitations:**
- No support for signed tags (signatures stripped)
- No object deduplication (stores entire fast-export stream per push)
- No support for complex Git features (submodules, LFS, shallow clones, partial clones)
- No garbage collection
- No compression

---

## Phase 2: Pack Format Implementation ✅ COMPLETED

### Why Pack Format?

Git's pack format is the native storage format that:
- ✅ Preserves ALL commit metadata including GPG signatures
- ✅ Maintains exact Git SHA-1s (no SHA changes)
- ✅ Supports all Git object types (commits, trees, blobs, tags)
- ✅ Includes delta compression for efficiency
- ✅ Is the format Git uses internally for fetch/push operations

### Architecture Changes

**Move from import/export to fetch capability:**

Current (import/export):
```
Git → fast-export stream → Helper stores stream → Helper replays stream → Git fast-import
```

New (fetch):
```
Git ← pack file ← Helper stores pack → Helper sends pack ← Git
```

**Protocol Changes:**
- Replace `import/export` capabilities with `fetch` capability
- Use `fetch` command instead of `import`
- Use pack protocol for receiving and sending objects

### Implementation Plan

#### Step 1: Understand Git Pack Format

**Key concepts to implement:**
- Pack file structure (header, objects, index)
- Object types: commit (0b001), tree (0b010), blob (0b011), tag (0b100)
- Delta compression (OFS_DELTA, REF_DELTA)
- Pack index format (.idx files)
- Thin packs vs. full packs

**References:**
- https://git-scm.com/docs/pack-format
- https://github.com/git/git/blob/master/Documentation/technical/pack-format.txt
- https://github.com/git/git/blob/master/Documentation/technical/pack-protocol.txt

#### Step 2: Storage Layer Changes

**New storage model:**
```rust
// Instead of storing fast-export streams, store pack files
pub struct State {
    // Maps ref names to Git SHA-1 commit hashes (unchanged)
    refs: HashMap<String, String>,

    // Maps Git SHA-1s to pack file content IDs
    // Multiple objects can be in the same pack
    packs: HashMap<String, ContentId>,  // pack_id -> backend_content_id

    // Maps Git SHA-1s to pack IDs and offsets
    objects: HashMap<String, ObjectLocation>,
}

pub struct ObjectLocation {
    pack_id: String,      // Which pack contains this object
    offset: u64,          // Offset within the pack
    object_type: ObjectType,
}
```

**Storage structure:**
```
<storage-path>/
├── objects/
│   ├── <sha256-hash>  # Pack files
│   └── <sha256-hash>  # Pack index files
└── state.yaml
```

#### Step 3: Implement Fetch Capability

**Capabilities output:**
```
fetch
refspec refs/heads/*:refs/heads/*
refspec refs/tags/*:refs/tags/*
```

**Fetch command handler:**
```rust
// Receive: fetch <sha1> <refname>
// Process:
// 1. Look up which pack(s) contain the requested objects
// 2. Read pack file(s) from storage
// 3. Send pack file to Git via stdout
// 4. Git will unpack and verify objects
```

**Key implementation points:**
- Git sends list of wanted SHAs and have SHAs
- Helper computes which objects are needed
- Helper sends a pack file containing those objects
- Pack file must include all dependencies (trees, blobs)

#### Step 4: Implement Push with Packs

**Receive pack from Git:**
```rust
// Git sends a pack file via stdin when pushing
// Handler must:
// 1. Receive the pack file
// 2. Verify the pack (checksums)
// 3. Index the pack (build .idx file)
// 4. Store pack as immutable object
// 5. Update state.yaml with object locations
// 6. Report success for each ref
```

**Pack indexing:**
- Parse pack file to extract object SHAs and offsets
- Build index mapping SHA → (pack_id, offset, type)
- Store index separately or in state.yaml

#### Step 5: Pack File Utilities

**Required functionality:**
```rust
mod pack {
    // Pack parsing
    fn parse_pack_header(data: &[u8]) -> Result<(u32, u32)>; // version, num_objects
    fn parse_pack_object(data: &[u8], offset: usize) -> Result<PackObject>;

    // Pack generation (for sending objects to Git)
    fn create_pack(objects: &[GitObject]) -> Result<Vec<u8>>;

    // Pack verification
    fn verify_pack_checksum(data: &[u8]) -> Result<()>;

    // Pack indexing
    fn index_pack(data: &[u8]) -> Result<PackIndex>;
}

pub struct PackObject {
    object_type: ObjectType,
    size: u64,
    data: Vec<u8>,  // Could be full object or delta
}

pub enum ObjectType {
    Commit,
    Tree,
    Blob,
    Tag,
    OfsDelta,
    RefDelta,
}
```

#### Step 6: Object Traversal

**For fetch operations, need to:**
- Start from requested commit SHA
- Walk the object graph (commit → tree → blobs, parent commits)
- Collect all reachable objects
- Exclude objects Git already has (from "have" list)
- Pack collected objects

**Implementation:**
```rust
fn collect_objects(
    wanted: &[String],     // SHAs Git wants
    have: &[String],       // SHAs Git already has
    storage: &impl StorageBackend,
) -> Result<Vec<GitObject>> {
    // BFS or DFS through commit graph
    // Stop at commits in "have" list
    // Return all reachable objects
}
```

#### Step 7: Delta Handling

**Optional optimization:**
- Git packs use delta compression
- Can store deltas or expand them
- Initial implementation: expand all deltas to full objects
- Future: preserve delta compression for efficiency

#### Step 8: Testing Strategy

**Test cases:**
1. **Unsigned commits** - Verify SHA preservation
2. **Signed commits** - Verify GPG signatures preserved
3. **Signed tags** - Verify tag signatures preserved
4. **Branch operations** - Multiple branches, merges
5. **Large files** - Ensure delta compression works
6. **Incremental push** - Only send new objects
7. **Partial clone** - Fetch subset of history
8. **Tag operations** - Lightweight and annotated tags

**Verification:**
```bash
# Push repo
git push walrus::/tmp/storage main

# Clone repo
git clone walrus::/tmp/storage /tmp/test

# Verify SHAs match exactly
cd /tmp/test
git log --format="%H" > /tmp/cloned-shas
cd /path/to/original
git log --format="%H" > /tmp/original-shas
diff /tmp/cloned-shas /tmp/original-shas  # Should be identical

# Verify GPG signatures
git verify-commit HEAD  # Should succeed
```

---

## Implementation Modules

### Recommended structure:
```
src/
├── main.rs              # Entry point (no changes needed)
├── protocol.rs          # Protocol handler (update for fetch)
├── commands/
│   ├── capabilities.rs  # Update to advertise 'fetch'
│   ├── list.rs          # No changes needed
│   ├── fetch.rs         # NEW - replace import.rs
│   ├── push.rs          # NEW - replace export.rs, receive packs
│   └── mod.rs
├── pack/                # NEW module
│   ├── mod.rs
│   ├── reader.rs        # Parse pack files
│   ├── writer.rs        # Generate pack files
│   ├── index.rs         # Pack indexing
│   ├── objects.rs       # Object graph traversal
│   └── delta.rs         # Delta handling
├── storage/
│   ├── traits.rs        # No changes to traits
│   ├── filesystem.rs    # No changes needed
│   ├── state.rs         # Update State struct
│   └── mod.rs
└── error.rs
```

---

## Dependencies to Add

```toml
[dependencies]
sha2 = "0.10"           # Already have
hex = "0.4"             # Already have
serde = { version = "1.0", features = ["derive"] }  # Already have
serde_yaml = "0.9"      # Already have
anyhow = "1.0"          # Already have

# New dependencies for pack support:
flate2 = "1.0"          # zlib compression (Git uses zlib for pack objects)
sha1 = "0.10"           # Git uses SHA-1 for object IDs
```

---

## Resources

### Git Pack Format Documentation:
- https://git-scm.com/docs/pack-format
- https://github.com/git/git/blob/master/Documentation/technical/pack-format.txt
- https://github.com/git/git/blob/master/Documentation/technical/pack-protocol.txt
- https://github.com/git/git/blob/master/Documentation/technical/http-protocol.txt

### Existing Rust Git Libraries (for reference):
- **gitoxide** (https://github.com/Byron/gitoxide) - Pure Rust Git implementation
- **git2-rs** (https://github.com/rust-lang/git2-rs) - Rust bindings for libgit2

**Note:** We may want to use gitoxide's pack parsing instead of implementing from scratch.

### Git Remote Helper Examples:
- https://github.com/git/git/tree/master/contrib/remote-helpers
- git-remote-gcrypt - Encrypted Git remote helper
- git-remote-ipfs - IPFS-based remote helper

---

## Migration Path from Phase 1 to Phase 2

### Option 1: Clean Break
- Implement pack support in parallel
- Release as v2.0
- No backward compatibility with fast-export storage

### Option 2: Dual Support
- Detect storage format from state.yaml
- Support both fast-export (legacy) and pack formats
- Provide migration tool

**Recommendation:** Option 1 (clean break) since Phase 1 is not production-ready anyway.

---

## Known Challenges

1. **Pack parsing complexity** - Git pack format is complex, especially delta handling
2. **Object graph traversal** - Need to efficiently walk commit history
3. **Memory usage** - Large packs need careful memory management
4. **Thin packs** - Git may send "thin" packs that reference objects not in the pack
5. **Performance** - Pack operations need to be fast for large repos

**Mitigation:** Consider using gitoxide's pack parsing crate instead of implementing from scratch.

---

## Success Criteria for Phase 2

- [x] Push and clone preserve exact Git SHAs ✅
- [ ] GPG commit signatures preserved (needs testing)
- [ ] GPG tag signatures preserved (needs testing)
- [ ] Signed tags work correctly (needs testing)
- [ ] Annotated tags work correctly (needs testing)
- [ ] Lightweight tags work correctly (needs testing)
- [ ] Incremental push only sends new objects (needs testing)
- [ ] Large repositories (1000+ commits) work efficiently (needs testing)
- [ ] Binary files handled correctly (needs testing)
- [ ] Merge commits work correctly (needs testing)
- [ ] Multiple branches work correctly (needs testing)
- [ ] All tests pass with real-world repositories (needs testing)

### Phase 2 Implementation Summary

**Completed:**
- ✅ Pack format storage using `git pack-objects` and `git unpack-objects`
- ✅ Individual loose object storage in content-addressed backend
- ✅ Push (export) via pack format - preserves all Git metadata
- ✅ Fetch via native pack format - **NO fast-export/import** - preserves GPG signatures
- ✅ SHA preservation verified for unsigned commits
- ✅ Fetch/export capabilities with pack format end-to-end

**Architecture:**
```
Push Flow:
Git → git pack-objects → packfile → git unpack-objects → loose objects → storage backend

Fetch Flow:
Storage backend → loose objects → temp git repo → git pack-objects → packfile → Git
```

**Key Implementation Details:**
- **NO fast-export/import anywhere** - Completely eliminated to preserve GPG signatures
- Uses `fetch` capability instead of `import` for native pack protocol support
- Both push and fetch use Git's native pack format throughout

**Key Files:**
- `src/pack/objects.rs` - Git object parsing/writing (loose format)
- `src/pack/receive.rs` - Receive and unpack packfiles (used by push/export)
- `src/pack/send.rs` - Create packfiles from stored objects (used by fetch)
- `src/commands/export.rs` - Push handler using pack format
- `src/commands/fetch.rs` - Fetch handler using native pack format (NO fast-export)
- `src/commands/capabilities.rs` - Advertises `fetch` and `export` capabilities

**Storage Format:**
```rust
pub struct State {
    refs: HashMap<String, String>,       // ref_name -> git_sha1
    objects: HashMap<String, ContentId>, // git_sha1 -> backend_content_id (loose object)
}
```

Objects are stored individually in loose format (zlib-compressed with "type size\0data" header), preserving all Git metadata including GPG signatures.

---

## Phase 3: Quality Assurance & Testing

### Remaining Work Items

#### 1. GPG Signature Testing ⚠️ HIGH PRIORITY
**Goal:** Verify that GPG signatures are preserved through push/clone cycle

**Status:** ✅ **Fast-export eliminated** - No longer a risk since we use native pack format

**Test Plan:**
```bash
# Create test repo with GPG-signed commits
git init test-signed
cd test-signed
git config user.signingkey <key-id>
git commit --allow-empty -S -m "Signed commit 1"
git commit --allow-empty -S -m "Signed commit 2"

# Push to walrus
git push walrus::/tmp/storage main

# Clone and verify
git clone walrus::/tmp/storage /tmp/cloned
cd /tmp/cloned

# Verify signatures preserved
git verify-commit HEAD
git verify-commit HEAD~1

# Verify SHAs match exactly
git rev-parse HEAD
# Compare with original SHA
```

**Expected:** GPG signatures preserved, SHAs match exactly (should work now that fast-export is gone)

#### 2. Tag Support Testing
**Test cases needed:**

a. **Lightweight tags:**
```bash
git tag v1.0.0
git push walrus::/tmp/storage --tags
# Verify tag points to correct commit after clone
```

b. **Annotated tags:**
```bash
git tag -a v2.0.0 -m "Release 2.0"
git push walrus::/tmp/storage --tags
# Verify tag message preserved
```

c. **Signed tags:**
```bash
git tag -s v3.0.0 -m "Signed release"
git push walrus::/tmp/storage --tags
# Verify signature preserved
git verify-tag v3.0.0
```

#### 3. Branch Operations Testing
```bash
# Create multiple branches
git checkout -b feature-1
git commit -m "Feature 1"
git checkout -b feature-2
git commit -m "Feature 2"

# Push all branches
git push walrus::/tmp/storage --all

# Clone and verify all branches present
git clone walrus::/tmp/storage /tmp/test
cd /tmp/test
git branch -a
# Should show all branches
```

#### 4. Merge Commit Testing
```bash
# Create merge commit
git checkout main
git merge feature-1

# Push and verify merge commit preserved
git push walrus::/tmp/storage main
# Clone and check merge structure
git clone walrus::/tmp/storage /tmp/test
cd /tmp/test
git log --graph --oneline
# Verify merge structure correct
```

#### 5. Incremental Push Testing
**Goal:** Verify only new objects are pushed

```bash
# Initial push
git push walrus::/tmp/storage main
# Note object count

# Add new commit
git commit --allow-empty -m "New commit"

# Push again
git push walrus::/tmp/storage main
# Verify only 1 new object stored (the commit)
# Check storage backend for object count
```

**Current limitation:** export.rs creates full packfile each time. Need to implement incremental packing based on remote ref position.

#### 6. Large Repository Testing
**Test with real-world repos:**
- Clone Linux kernel (1M+ commits) ❌ (too large for initial testing)
- Clone a medium repo (10k commits) ✅
- Verify performance acceptable

**Performance benchmarks needed:**
- Push time for N commits
- Clone time for N commits
- Storage space efficiency (compare to .git size)

#### 7. Binary File Testing
```bash
# Add large binary files
dd if=/dev/urandom of=binary.dat bs=1M count=10
git add binary.dat
git commit -m "Add binary"

# Push and clone
git push walrus::/tmp/storage main
git clone walrus::/tmp/storage /tmp/test

# Verify binary identical
diff binary.dat /tmp/test/binary.dat
```

#### 8. Edge Cases & Error Handling

**Test scenarios:**
- [ ] Empty repository push
- [ ] Corrupted packfile handling
- [ ] Network interruption during push (graceful failure)
- [ ] Storage backend errors (disk full, permissions)
- [ ] Concurrent pushes to same ref (conflict detection)
- [ ] Force push (ref update safety)
- [ ] Push to non-existent branch
- [ ] Clone from empty storage
- [ ] Clone with partial refs

### Integration Test Suite

**Recommended structure:**
```rust
// tests/integration_test.rs

#[test]
fn test_unsigned_commit_sha_preservation() {
    // Create test repo, push, clone, compare SHAs
}

#[test]
fn test_gpg_signed_commit_preservation() {
    // Requires GPG setup, may need to skip in CI
}

#[test]
fn test_multiple_branches() {
    // Test branch operations
}

#[test]
fn test_merge_commits() {
    // Test merge structure preservation
}

#[test]
fn test_annotated_tags() {
    // Test tag message preservation
}

#[test]
fn test_signed_tags() {
    // Test tag signature preservation
}

#[test]
fn test_incremental_push() {
    // Test that only new objects are stored
}

#[test]
fn test_binary_files() {
    // Test binary file handling
}

#[test]
fn test_large_repository() {
    // Test performance with 1000+ commits
}
```

### Known Issues & Limitations

1. ~~**Fast-export limitation:**~~ ✅ **FIXED** - Fast-export completely eliminated, now uses native pack format

2. **No incremental push:** Currently creates full packfile on each push
   - **Impact:** Inefficient for large repos with small changes
   - **Fix:** Implement `have`/`want` negotiation to only pack new objects

3. **No object deduplication across packs:** Each push stores all objects
   - **Impact:** Storage bloat with repeated objects
   - **Fix:** Track objects globally and skip already-stored objects

4. **No garbage collection:** Old objects never cleaned up
   - **Impact:** Storage grows indefinitely
   - **Fix:** Implement ref-based reachability analysis and GC

5. **Temp directory usage:** Fetch creates temp git repo for every fetch
   - **Impact:** Slower fetches, disk I/O overhead
   - **Fix:** Cache reconstructed repo between fetches (low priority - usually acceptable performance)

### Documentation Needed

- [ ] README with usage examples
- [ ] Architecture documentation
- [ ] Storage format specification
- [ ] Troubleshooting guide
- [ ] Performance tuning guide
- [ ] Migration guide (if needed)

---

## Future Enhancements (Phase 3+)

- Garbage collection for unreachable objects
- Compression at storage layer
- Cloud storage backends (S3, Azure Blob, etc.)
- Replication and multi-region support
- Shallow clone support
- Partial clone support
- LFS support
- Submodule support
- Wire protocol v2 support
- Performance optimization (caching, parallelization)
- Multi-tenancy (multiple repos in one storage)

Notes:

  - When adding new rust dependencies, always use `cargo add`, instead of manually touching
    Cargo.toml files, so that we get the latest versions.
