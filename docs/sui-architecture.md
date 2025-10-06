# Sui + Walrus Architecture

## Overview

The git-remote-walrus backend uses a hybrid storage model:
- **Walrus**: Immutable blob storage for git objects (content-addressed)
- **Sui**: Mutable on-chain state for refs and objects map
- **Local filesystem**: Performance cache (dual-indexed)

## RemoteState Object Design

### Move Contract Structure

```move
module walrus_remote::remote_state {
    use sui::table::{Self, Table};
    use sui::clock::Clock;
    use sui::vec_set::{Self, VecSet};

    /// Main state object for a git remote
    struct RemoteState has key {
        id: UID,
        owner: address,
        /// Git refs (branch/tag names) -> Git SHA-1 commit hashes
        refs: Table<String, String>,
        /// Walrus blob ID containing the objects map (git_sha1 -> blob_id)
        objects_blob_id: Option<String>,
        /// Lock for atomic updates
        lock: Option<LockInfo>,
        /// Optional allowlist for multi-user repos
        allowlist: Option<VecSet<address>>,
    }

    /// Lock information with time-based expiration
    struct LockInfo has store {
        holder: address,
        expires_ms: u64,
    }

    /// Capability for administrative operations
    struct AdminCap has key, store {
        id: UID,
        remote_id: ID,
    }
}
```

### Object Ownership Models

**Default: Owned Object (Single User)**
- Object is owned by creator's address
- Only owner can perform operations
- Fastest transaction processing
- No additional access control needed

**Optional: Shared Object with Allowlist (Multi-User)**
- Convert owned object to shared via `share_object()`
- Maintain allowlist of authorized addresses
- Each operation checks caller is in allowlist
- Slightly slower due to shared object consensus

## Locking Mechanism

### Design Rationale

The objects map update requires atomic operations to prevent race conditions:
1. Download current objects blob from Walrus
2. Merge new object mappings
3. Upload new blob to Walrus
4. Update on-chain state with new blob ID

Without locking, concurrent pushes could:
- Overwrite each other's object mappings
- Create inconsistent state between Walrus and Sui

### Implementation Using On-Chain Clock

```move
/// Acquire lock with timeout (typically 5 minutes = 300000ms)
public fun acquire_lock(
    state: &mut RemoteState,
    clock: &Clock,
    timeout_ms: u64,
    ctx: &mut TxContext
) {
    let current_time = clock::timestamp_ms(clock);
    let caller = tx_context::sender(ctx);

    // Check if lock exists and is expired
    if (option::is_some(&state.lock)) {
        let lock = option::borrow(&state.lock);
        assert!(
            lock.holder == caller || current_time >= lock.expires_ms,
            ERR_LOCK_HELD
        );
    }

    // Set new lock
    let new_lock = LockInfo {
        holder: caller,
        expires_ms: current_time + timeout_ms,
    };
    option::swap_or_fill(&mut state.lock, new_lock);
}

/// Release lock (caller must be lock holder)
public fun release_lock(
    state: &mut RemoteState,
    ctx: &mut TxContext
) {
    assert!(option::is_some(&state.lock), ERR_NO_LOCK);
    let lock = option::borrow(&state.lock);
    assert!(lock.holder == tx_context::sender(ctx), ERR_NOT_LOCK_HOLDER);

    option::extract(&mut state.lock);
}

/// Update objects blob ID (requires lock)
public fun update_objects_blob(
    state: &mut RemoteState,
    blob_id: String,
    clock: &Clock,
    ctx: &mut TxContext
) {
    let current_time = clock::timestamp_ms(clock);
    assert!(option::is_some(&state.lock), ERR_NOT_LOCKED);

    let lock = option::borrow(&state.lock);
    assert!(lock.holder == tx_context::sender(ctx), ERR_NOT_LOCK_HOLDER);
    assert!(current_time < lock.expires_ms, ERR_LOCK_EXPIRED);

    option::swap_or_fill(&mut state.objects_blob_id, blob_id);
}
```

### Lock Workflow for Push Operations

1. **Acquire Lock** (5 min timeout)
   ```rust
   sui_client.acquire_lock(300_000).await?;
   ```

2. **Perform Updates** (within lock period)
   - Download current objects blob from Walrus
   - Merge new git_sha1 -> blob_id mappings
   - Upload updated objects map to Walrus

3. **Atomic State Update** (single PTB)
   - Batch update refs
   - Update objects_blob_id
   - Release lock

4. **Error Handling**
   - Lock acquisition fails → retry with exponential backoff (3 attempts)
   - Transaction fails → release lock before returning error
   - Lock expires → automatic release, retry from step 1

## Programmable Transaction Blocks (PTBs)

### Why PTBs Are Critical

Git push operations typically update:
- Multiple refs (branches/tags)
- Objects blob ID
- Lock state

Without PTBs, this would require 3+ separate transactions:
- Higher gas costs
- Non-atomic updates (race conditions)
- Slower execution

### PTB Strategy for Ref Updates

```rust
use sui_sdk::types::transaction::{ProgrammableTransactionBuilder, Argument};

async fn upsert_refs_and_update_objects(
    &self,
    refs: Vec<(String, String)>,
    objects_blob_id: String,
) -> Result<()> {
    let mut ptb = ProgrammableTransactionBuilder::new();

    // 1. Update each ref
    for (ref_name, git_sha1) in refs {
        let ref_arg = ptb.pure(ref_name)?;
        let sha_arg = ptb.pure(git_sha1)?;

        ptb.command(Command::MoveCall(Box::new(MoveCall {
            package: self.package_id,
            module: Identifier::new("remote_state")?,
            function: Identifier::new("upsert_ref")?,
            type_arguments: vec![],
            arguments: vec![
                Argument::Input(0), // RemoteState object
                ref_arg,
                sha_arg,
            ],
        })));
    }

    // 2. Update objects blob ID
    let blob_arg = ptb.pure(objects_blob_id)?;
    ptb.command(Command::MoveCall(Box::new(MoveCall {
        package: self.package_id,
        module: Identifier::new("remote_state")?,
        function: Identifier::new("update_objects_blob")?,
        type_arguments: vec![],
        arguments: vec![
            Argument::Input(0), // RemoteState object
            blob_arg,
            Argument::Input(1), // Clock object
        ],
    })));

    // 3. Release lock
    ptb.command(Command::MoveCall(Box::new(MoveCall {
        package: self.package_id,
        module: Identifier::new("remote_state")?,
        function: Identifier::new("release_lock")?,
        type_arguments: vec![],
        arguments: vec![
            Argument::Input(0), // RemoteState object
        ],
    })));

    // Execute PTB
    let pt = ptb.finish();
    let tx_data = TransactionData::new_programmable(
        self.sender,
        vec![self.state_object_ref, self.clock_object_ref],
        pt,
        gas_budget,
        gas_price,
    );

    self.client.sign_and_execute_transaction(tx_data).await?;
    Ok(())
}
```

### PTB Best Practices

1. **Batch Related Operations**: Group all ref updates into single PTB
2. **Use Transaction Results**: Pass results from earlier commands to later ones
3. **Gas Estimation**: Calculate gas budget based on number of operations
4. **Error Atomicity**: Entire PTB succeeds or fails together

## Access Control Patterns

### Owned Object (Default)

```move
/// Only owner can push
public fun upsert_ref(
    state: &mut RemoteState,
    ref_name: String,
    git_sha1: String,
    ctx: &mut TxContext
) {
    // RemoteState is owned by sender - automatic access control
    table::upsert(&mut state.refs, ref_name, git_sha1);
}
```

### Shared Object with Allowlist

```move
/// Add address to allowlist (owner only)
public fun add_to_allowlist(
    state: &mut RemoteState,
    address_to_add: address,
    ctx: &mut TxContext
) {
    assert!(state.owner == tx_context::sender(ctx), ERR_NOT_OWNER);

    if (option::is_none(&state.allowlist)) {
        state.allowlist = option::some(vec_set::empty());
    };

    let allowlist = option::borrow_mut(&mut state.allowlist);
    vec_set::insert(allowlist, address_to_add);
}

/// Check if caller is authorized (for shared objects)
fun check_authorized(state: &RemoteState, ctx: &TxContext) {
    let caller = tx_context::sender(ctx);

    // Owner always authorized
    if (caller == state.owner) return;

    // Check allowlist if it exists
    if (option::is_some(&state.allowlist)) {
        let allowlist = option::borrow(&state.allowlist);
        assert!(vec_set::contains(allowlist, &caller), ERR_NOT_AUTHORIZED);
    } else {
        abort ERR_NOT_AUTHORIZED
    }
}

/// Convert owned object to shared with allowlist
public fun share_with_allowlist(
    state: RemoteState,
    initial_allowlist: vector<address>,
) {
    // Convert to shared object
    let mut shared_state = state;
    shared_state.allowlist = option::some(vec_set::from_keys(initial_allowlist));
    transfer::share_object(shared_state);
}
```

## URL Schema and Initialization

### URL Format

- **Sui-backed remote**: `walrus::<sui-object-id>`
  - Example: `walrus::0x1234567890abcdef...`
- **Filesystem remote**: `walrus::/path/to/storage`
  - Backward compatibility with existing implementation

### URL Parsing

```rust
fn parse_remote_url(url: &str) -> Result<RemoteType> {
    let path_str = url.strip_prefix("walrus::").unwrap_or(url);

    // Try to parse as Sui object ID (0x prefix + hex)
    if path_str.starts_with("0x") {
        let object_id = ObjectID::from_hex_literal(path_str)?;
        Ok(RemoteType::Sui(object_id))
    } else {
        // Treat as filesystem path
        Ok(RemoteType::Filesystem(PathBuf::from(path_str)))
    }
}
```

### Initialization Command

New CLI command: `git-remote-walrus init [--shared]`

**Owned Object (default)**:
```bash
git-remote-walrus init
# Output: Created RemoteState at 0x1234567890abcdef...
# Usage: git remote add origin walrus::0x1234567890abcdef...
```

**Shared Object with allowlist**:
```bash
git-remote-walrus init --shared --allow 0xabc... --allow 0xdef...
# Output: Created shared RemoteState at 0x1234567890abcdef...
```

### Initialization Flow

1. Load Sui wallet and active address
2. Create RemoteState object on-chain
3. Initialize empty refs Table
4. Set objects_blob_id to None
5. No lock initially
6. Optional: Set allowlist and share object
7. Return object ID to user

## Objects Map Storage Strategy

### Why Store in Walrus

The objects map (`BTreeMap<String, String>`) maps git SHA-1 → Walrus blob ID.
- Can grow large (thousands of objects)
- Expensive to store entirely on-chain
- Infrequently accessed (only during push/fetch)

**Solution**: Store serialized map in Walrus, keep only blob ID on-chain.

### Objects Map Update Flow

1. **Read current state**:
   ```rust
   let objects_blob_id = sui_client.get_objects_blob_id().await?;
   let mut objects = if let Some(blob_id) = objects_blob_id {
       let blob_data = walrus_client.read(&blob_id)?;
       serde_yaml::from_slice(&blob_data)?
   } else {
       BTreeMap::new()
   };
   ```

2. **Merge new objects**:
   ```rust
   for (git_sha1, blob_id) in new_objects {
       objects.insert(git_sha1, blob_id);
   }
   ```

3. **Upload to Walrus**:
   ```rust
   let objects_yaml = serde_yaml::to_string(&objects)?;
   let new_blob_id = walrus_client.store(objects_yaml.as_bytes())?;
   ```

4. **Update on-chain state** (via PTB with lock):
   ```rust
   sui_client.upsert_refs_and_update_objects(refs, new_blob_id).await?;
   ```

### Optimizations

- **Lazy Loading**: Only download objects map when needed (push/fetch)
- **Cache Locally**: Store objects map in cache_dir/state.yaml
- **Incremental Updates**: Merge only new objects instead of full replacement
- **Compression**: YAML format is human-readable; consider msgpack for production

## Network Configuration

### Sui Networks

- **Localnet**: `http://127.0.0.1:9000` (for testing)
- **Testnet**: `https://fullnode.testnet.sui.io:443` (default)
- **Mainnet**: `https://fullnode.mainnet.sui.io:443` (production)

### Wallet Configuration

Default wallet location: `~/.sui/sui_config/client.yaml`

Structure:
```yaml
keystore:
  File: ~/.sui/sui_config/sui.keystore
envs:
  - alias: testnet
    rpc: https://fullnode.testnet.sui.io:443
    ...
active_address: "0x..."
```

### Clock Object Reference

The Sui Clock is a singleton shared object at address `0x6`.
- Always accessible via immutable reference
- No ownership required
- Used in all time-based operations

## Gas Considerations

### Operation Costs (Testnet estimates)

- Create RemoteState: ~0.01 SUI
- Update single ref: ~0.001 SUI
- Batch update 10 refs (PTB): ~0.003 SUI
- Acquire/release lock: ~0.001 SUI each
- Full push operation: ~0.005-0.01 SUI

### Gas Budget Strategy

Calculate based on operation count:
```rust
fn estimate_gas_budget(num_refs: usize) -> u64 {
    let base_cost = 1_000_000; // 0.001 SUI in MIST
    let per_ref_cost = 100_000; // 0.0001 SUI per ref
    base_cost + (num_refs as u64 * per_ref_cost)
}
```

### Gas Payment

Use `Argument::GasCoin` in PTBs to automatically use gas coin for payment.

## Error Handling

### Lock Errors

- `ERR_LOCK_HELD`: Lock is held by another user → retry with backoff
- `ERR_LOCK_EXPIRED`: Lock expired during operation → re-acquire and retry
- `ERR_NOT_LOCK_HOLDER`: Attempting operation without lock → acquire lock first

### Transaction Errors

- Network failure → retry up to 3 times with exponential backoff
- Insufficient gas → increase gas budget and retry
- Object version mismatch → refresh object state and retry

### Walrus Errors

- Blob not found → may have expired, warn user
- Upload timeout → retry with longer timeout
- Network error → check Walrus availability

### Rollback Strategy

If PTB fails after Walrus upload:
1. Blob remains in Walrus (no harm - will expire)
2. On-chain state unchanged (transaction reverted)
3. Retry entire operation from lock acquisition

## Future Enhancements

### Post-MVP Features

1. **Quilts Integration**
   - Batch small objects into single Walrus blob
   - Optimal for repos with many small files
   - Reduces per-blob overhead

2. **Blob Auto-Renewal**
   - Monitor blob expiration epochs
   - Automatically extend before expiration
   - Configurable renewal policy

3. **Gas Optimization**
   - Dry-run mode for gas estimation
   - Batch multiple pushes into single PTB
   - Gas sponsorship for shared repos

4. **Advanced Multi-User**
   - Role-based access control (admin/writer/reader)
   - Per-ref permissions
   - Audit log of all operations

5. **Performance**
   - Async Walrus uploads (background thread)
   - Parallel blob downloads
   - Compression for large objects maps
