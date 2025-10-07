module walrus_remote::remote_state {
    use std::string::String;
    use sui::{clock::{Self, Clock}, table::{Self, Table}, vec_set::{Self, VecSet}};

    // Error codes
    const ERR_LOCK_HELD: u64 = 1;
    const ERR_NO_LOCK: u64 = 2;
    const ERR_NOT_LOCK_HOLDER: u64 = 3;
    const ERR_LOCK_EXPIRED: u64 = 4;
    const ERR_NOT_AUTHORIZED: u64 = 5;
    const ERR_NOT_OWNER: u64 = 6;

    /// Main state object for a git remote repository
    public struct RemoteState has key {
        id: UID,
        /// Owner address (always authorized)
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
    public struct LockInfo has drop, store {
        holder: address,
        expires_ms: u64,
    }

    /// Create a new RemoteState (owned by caller)
    public fun create_remote(ctx: &mut TxContext) {
        let owner = ctx.sender();
        let remote = RemoteState {
            id: object::new(ctx),
            owner,
            refs: table::new(ctx),
            objects_blob_id: option::none(),
            lock: option::none(),
            allowlist: option::none(),
        };

        transfer::transfer(remote, owner);
    }

    /// Convert owned RemoteState to shared with allowlist
    #[lint_allow(share_owned)]
    public fun share_with_allowlist(
        mut state: RemoteState,
        initial_allowlist: vector<address>,
        ctx: &TxContext,
    ) {
        // Only owner can share
        assert!(state.owner == ctx.sender(), ERR_NOT_OWNER);

        // Set up allowlist
        let mut allow_set = vec_set::empty();
        let mut i = 0;
        while (i < vector::length(&initial_allowlist)) {
            vec_set::insert(&mut allow_set, *vector::borrow(&initial_allowlist, i));
            i = i + 1;
        };

        state.allowlist = option::some(allow_set);

        // Convert to shared object
        transfer::share_object(state);
    }

    /// Acquire lock with timeout (typically 5 minutes = 300000ms)
    public entry fun acquire_lock(
        state: &mut RemoteState,
        clock: &Clock,
        lock_timeout_ms: u64,
        ctx: &TxContext,
    ) {
        check_authorized(state, ctx);

        let current_time = clock::timestamp_ms(clock);
        let caller = ctx.sender();

        // Check if lock exists and is expired
        if (option::is_some(&state.lock)) {
            let lock = option::borrow(&state.lock);
            // Allow re-acquisition if: same holder OR lock expired
            assert!(lock.holder == caller || current_time >= lock.expires_ms, ERR_LOCK_HELD);
        };

        // Set new lock
        let new_lock = LockInfo {
            holder: caller,
            expires_ms: current_time + lock_timeout_ms,
        };
        option::swap_or_fill(&mut state.lock, new_lock);
    }

    /// Release lock (caller must be lock holder)
    public fun release_lock(state: &mut RemoteState, ctx: &TxContext) {
        assert!(option::is_some(&state.lock), ERR_NO_LOCK);

        let lock = option::borrow(&state.lock);
        assert!(lock.holder == ctx.sender(), ERR_NOT_LOCK_HOLDER);

        option::extract(&mut state.lock);
    }

    /// Upsert a single ref (insert or update)
    public fun upsert_ref(
        state: &mut RemoteState,
        ref_name: String,
        git_sha1: String,
        ctx: &TxContext,
    ) {
        check_authorized(state, ctx);

        if (table::contains(&state.refs, ref_name)) {
            let value = table::borrow_mut(&mut state.refs, ref_name);
            *value = git_sha1;
        } else {
            table::add(&mut state.refs, ref_name, git_sha1);
        };
    }

    /// Delete a ref
    public fun delete_ref(state: &mut RemoteState, ref_name: String, ctx: &TxContext) {
        check_authorized(state, ctx);

        if (table::contains(&state.refs, ref_name)) {
            table::remove(&mut state.refs, ref_name);
        };
    }

    /// Update objects blob ID (requires lock)
    public fun update_objects_blob(
        state: &mut RemoteState,
        blob_id: String,
        clock: &Clock,
        ctx: &TxContext,
    ) {
        check_lock_held(state, clock, ctx);
        option::swap_or_fill(&mut state.objects_blob_id, blob_id);
    }

    /// Add address to allowlist (owner only)
    public fun add_to_allowlist(state: &mut RemoteState, address_to_add: address, ctx: &TxContext) {
        assert!(state.owner == ctx.sender(), ERR_NOT_OWNER);

        if (option::is_none(&state.allowlist)) {
            state.allowlist = option::some(vec_set::empty());
        };

        let allowlist = option::borrow_mut(&mut state.allowlist);
        if (!vec_set::contains(allowlist, &address_to_add)) {
            vec_set::insert(allowlist, address_to_add);
        };
    }

    /// Remove address from allowlist (owner only)
    public fun remove_from_allowlist(
        state: &mut RemoteState,
        address_to_remove: address,
        ctx: &TxContext,
    ) {
        assert!(state.owner == ctx.sender(), ERR_NOT_OWNER);

        if (option::is_some(&state.allowlist)) {
            let allowlist = option::borrow_mut(&mut state.allowlist);
            if (vec_set::contains(allowlist, &address_to_remove)) {
                vec_set::remove(allowlist, &address_to_remove);
            };
        };
    }

    // === View Functions ===

    /// Get ref value
    public fun get_ref(state: &RemoteState, ref_name: String): Option<String> {
        if (table::contains(&state.refs, ref_name)) {
            option::some(*table::borrow(&state.refs, ref_name))
        } else {
            option::none()
        }
    }

    /// Get objects blob ID
    public fun get_objects_blob_id(state: &RemoteState): Option<String> {
        state.objects_blob_id
    }

    /// Check if address is authorized
    public fun is_authorized(state: &RemoteState, addr: address): bool {
        // Owner always authorized
        if (addr == state.owner) {
            return true
        };

        // Check allowlist if it exists
        if (option::is_some(&state.allowlist)) {
            let allowlist = option::borrow(&state.allowlist);
            vec_set::contains(allowlist, &addr)
        } else {
            false
        }
    }

    /// Get lock status
    public fun get_lock_info(state: &RemoteState): (bool, Option<address>, Option<u64>) {
        if (option::is_some(&state.lock)) {
            let lock = option::borrow(&state.lock);
            (true, option::some(lock.holder), option::some(lock.expires_ms))
        } else {
            (false, option::none(), option::none())
        }
    }

    // === Internal Helper Functions ===

    /// Check if caller is authorized (owner or in allowlist)
    fun check_authorized(state: &RemoteState, ctx: &TxContext) {
        let caller = ctx.sender();

        // Owner always authorized
        if (caller == state.owner) {
            return
        };

        // Check allowlist if it exists
        if (option::is_some(&state.allowlist)) {
            let allowlist = option::borrow(&state.allowlist);
            assert!(vec_set::contains(allowlist, &caller), ERR_NOT_AUTHORIZED);
        } else {
            abort ERR_NOT_AUTHORIZED
        }
    }

    /// Check that caller holds a valid (non-expired) lock
    fun check_lock_held(state: &RemoteState, clock: &Clock, ctx: &TxContext) {
        assert!(option::is_some(&state.lock), ERR_NO_LOCK);

        let current_time = clock::timestamp_ms(clock);
        let lock = option::borrow(&state.lock);

        assert!(lock.holder == ctx.sender(), ERR_NOT_LOCK_HOLDER);
        assert!(current_time < lock.expires_ms, ERR_LOCK_EXPIRED);
    }

    // === Test Functions ===

    #[test_only]
    public fun create_remote_for_testing(ctx: &mut TxContext): RemoteState {
        RemoteState {
            id: object::new(ctx),
            owner: ctx.sender(),
            refs: table::new(ctx),
            objects_blob_id: option::none(),
            lock: option::none(),
            allowlist: option::none(),
        }
    }

    #[test_only]
    public fun destroy_for_testing(state: RemoteState) {
        let RemoteState { id, owner: _, refs, objects_blob_id: _, lock: _, allowlist: _ } = state;
        table::drop(refs);
        object::delete(id);
    }
}
