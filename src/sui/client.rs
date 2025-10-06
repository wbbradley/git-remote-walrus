use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr;

use sui_sdk::{SuiClientBuilder, rpc_types::SuiObjectDataOptions};
use sui_types::{
    base_types::{ObjectID, ObjectRef, SuiAddress},
    programmable_transaction_builder::ProgrammableTransactionBuilder,
    transaction::{Argument, Command, ObjectArg, TransactionData},
    Identifier,
};

/// Sui on-chain clock object ID (shared object at 0x6)
const CLOCK_OBJECT_ID: &str = "0x0000000000000000000000000000000000000000000000000000000000000006";

/// Default gas budget for transactions (1 SUI = 1_000_000_000 MIST)
const DEFAULT_GAS_BUDGET: u64 = 100_000_000; // 0.1 SUI

/// Sui client for interacting with RemoteState on-chain
pub struct SuiClient {
    /// Sui RPC client
    client: sui_sdk::SuiClient,

    /// RemoteState object ID
    state_object_id: ObjectID,

    /// Package ID where RemoteState module is published
    package_id: ObjectID,

    /// Sender address (from wallet)
    sender: SuiAddress,
}

impl SuiClient {
    /// Create a new Sui client
    ///
    /// Note: This is a simplified implementation. A production version would:
    /// - Load keystore from wallet_path
    /// - Support multiple key types
    /// - Handle gas coin selection
    /// - Implement retry logic
    pub async fn new(
        state_object_id: String,
        rpc_url: String,
        _wallet_path: PathBuf,
    ) -> Result<Self> {
        // Parse state object ID
        let state_object_id = ObjectID::from_hex_literal(&state_object_id)
            .with_context(|| format!("Invalid state object ID: {}", state_object_id))?;

        // Build Sui client
        let client = SuiClientBuilder::default()
            .build(rpc_url)
            .await
            .context("Failed to build Sui client")?;

        // TODO: Load package ID from the RemoteState object
        // For now, we'll extract it when we read the object
        let package_id = ObjectID::ZERO; // Placeholder

        // TODO: Load sender address from wallet
        // For now, use a placeholder address
        let sender = SuiAddress::from_str("0x0")
            .context("Failed to parse sender address")?;

        Ok(Self {
            client,
            state_object_id,
            package_id,
            sender,
        })
    }

    /// Get the object reference for the RemoteState
    async fn get_state_object_ref(&self) -> Result<ObjectRef> {
        let object = self.client
            .read_api()
            .get_object_with_options(
                self.state_object_id,
                SuiObjectDataOptions::new().with_owner(),
            )
            .await
            .context("Failed to fetch RemoteState object")?;

        let data = object.data
            .ok_or_else(|| anyhow::anyhow!("RemoteState object not found"))?;

        Ok(data.object_ref())
    }

    /// Get the Clock object reference (shared object at 0x6)
    async fn get_clock_object_ref(&self) -> Result<ObjectRef> {
        let clock_id = ObjectID::from_hex_literal(CLOCK_OBJECT_ID)
            .context("Failed to parse clock object ID")?;

        let object = self.client
            .read_api()
            .get_object_with_options(
                clock_id,
                SuiObjectDataOptions::new().with_owner(),
            )
            .await
            .context("Failed to fetch Clock object")?;

        let data = object.data
            .ok_or_else(|| anyhow::anyhow!("Clock object not found"))?;

        Ok(data.object_ref())
    }

    /// Read all refs from on-chain state
    pub async fn read_refs(&self) -> Result<BTreeMap<String, String>> {
        // TODO: Implement dynamic field reading for Table<String, String>
        // This requires querying the dynamic fields of the RemoteState object
        // and deserializing the key-value pairs

        // For now, return empty map
        anyhow::bail!("read_refs not yet fully implemented - needs Table dynamic field reading");
    }

    /// Get objects blob ID from on-chain state
    pub async fn get_objects_blob_id(&self) -> Result<Option<String>> {
        // TODO: Implement by reading the RemoteState object's fields
        // Need to deserialize the Move struct to access objects_blob_id field

        anyhow::bail!("get_objects_blob_id not yet fully implemented - needs object deserialization");
    }

    /// Batch upsert refs using PTB
    pub async fn upsert_refs_batch(&self, refs: Vec<(String, String)>) -> Result<()> {
        if refs.is_empty() {
            return Ok(());
        }

        let mut ptb = ProgrammableTransactionBuilder::new();

        // Get state object reference
        let state_ref = self.get_state_object_ref().await?;

        // Add RemoteState object as input
        let state_arg = ptb.obj(ObjectArg::ImmOrOwnedObject(state_ref))?;

        // Add upsert_ref call for each ref
        for (ref_name, git_sha1) in refs {
            let ref_arg = ptb.pure(ref_name)?;
            let sha_arg = ptb.pure(git_sha1)?;

            ptb.programmable_move_call(
                self.package_id,
                Identifier::new("remote_state")?,
                Identifier::new("upsert_ref")?,
                vec![], // no type arguments
                vec![
                    state_arg,
                    ref_arg,
                    sha_arg,
                ],
            );
        }

        // Build and execute transaction
        self.execute_ptb(ptb).await?;

        Ok(())
    }

    /// Acquire lock with timeout
    pub async fn acquire_lock(&self, timeout_ms: u64) -> Result<()> {
        let mut ptb = ProgrammableTransactionBuilder::new();

        // Get object references
        let state_ref = self.get_state_object_ref().await?;
        let clock_ref = self.get_clock_object_ref().await?;

        // Add objects as inputs
        let state_arg = ptb.obj(ObjectArg::ImmOrOwnedObject(state_ref))?;
        let clock_arg = ptb.obj(ObjectArg::SharedObject {
            id: clock_ref.0,
            initial_shared_version: clock_ref.1,
            mutable: false,
        })?;

        // Call acquire_lock
        let timeout_arg = ptb.pure(timeout_ms)?;

        ptb.programmable_move_call(
            self.package_id,
            Identifier::new("remote_state")?,
            Identifier::new("acquire_lock")?,
            vec![], // no type arguments
            vec![
                state_arg,
                clock_arg,
                timeout_arg,
            ],
        );

        // Build and execute transaction
        self.execute_ptb(ptb).await?;

        Ok(())
    }

    /// Update objects blob ID (requires lock)
    pub async fn update_objects_blob(&self, blob_id: &str) -> Result<()> {
        let mut ptb = ProgrammableTransactionBuilder::new();

        // Get object references
        let state_ref = self.get_state_object_ref().await?;
        let clock_ref = self.get_clock_object_ref().await?;

        // Add objects as inputs
        let state_arg = ptb.obj(ObjectArg::ImmOrOwnedObject(state_ref))?;
        let clock_arg = ptb.obj(ObjectArg::SharedObject {
            id: clock_ref.0,
            initial_shared_version: clock_ref.1,
            mutable: false,
        })?;

        // Call update_objects_blob
        let blob_arg = ptb.pure(blob_id.to_string())?;

        ptb.programmable_move_call(
            self.package_id,
            Identifier::new("remote_state")?,
            Identifier::new("update_objects_blob")?,
            vec![], // no type arguments
            vec![
                state_arg,
                blob_arg,
                clock_arg,
            ],
        );

        // Build and execute transaction
        self.execute_ptb(ptb).await?;

        Ok(())
    }

    /// Release lock
    pub async fn release_lock(&self) -> Result<()> {
        let mut ptb = ProgrammableTransactionBuilder::new();

        // Get state object reference
        let state_ref = self.get_state_object_ref().await?;

        // Add RemoteState object as input
        let state_arg = ptb.obj(ObjectArg::ImmOrOwnedObject(state_ref))?;

        // Call release_lock
        ptb.programmable_move_call(
            self.package_id,
            Identifier::new("remote_state")?,
            Identifier::new("release_lock")?,
            vec![], // no type arguments
            vec![state_arg],
        );

        // Build and execute transaction
        self.execute_ptb(ptb).await?;

        Ok(())
    }

    /// Combined operation: upsert refs and update objects blob atomically via PTB
    ///
    /// This is the most important operation - it ensures that ref updates and
    /// objects blob updates happen atomically in a single transaction.
    pub async fn upsert_refs_and_update_objects(
        &self,
        refs: Vec<(String, String)>,
        objects_blob_id: String,
    ) -> Result<()> {
        let mut ptb = ProgrammableTransactionBuilder::new();

        // Get object references
        let state_ref = self.get_state_object_ref().await?;
        let clock_ref = self.get_clock_object_ref().await?;

        // Add objects as inputs
        let state_arg = ptb.obj(ObjectArg::ImmOrOwnedObject(state_ref))?;
        let clock_arg = ptb.obj(ObjectArg::SharedObject {
            id: clock_ref.0,
            initial_shared_version: clock_ref.1,
            mutable: false,
        })?;

        // 1. Batch upsert all refs
        for (ref_name, git_sha1) in refs {
            let ref_arg = ptb.pure(ref_name)?;
            let sha_arg = ptb.pure(git_sha1)?;

            ptb.programmable_move_call(
                self.package_id,
                Identifier::new("remote_state")?,
                Identifier::new("upsert_ref")?,
                vec![], // no type arguments
                vec![
                    state_arg,
                    ref_arg,
                    sha_arg,
                ],
            );
        }

        // 2. Update objects blob ID
        let objects_blob_arg = ptb.pure(objects_blob_id)?;

        ptb.programmable_move_call(
            self.package_id,
            Identifier::new("remote_state")?,
            Identifier::new("update_objects_blob")?,
            vec![], // no type arguments
            vec![
                state_arg,
                objects_blob_arg,
                clock_arg,
            ],
        );

        // 3. Release lock
        ptb.programmable_move_call(
            self.package_id,
            Identifier::new("remote_state")?,
            Identifier::new("release_lock")?,
            vec![], // no type arguments
            vec![state_arg],
        );

        // Build and execute transaction (all operations atomic)
        self.execute_ptb(ptb).await?;

        Ok(())
    }

    /// Execute a PTB with proper gas handling
    async fn execute_ptb(&self, ptb: ProgrammableTransactionBuilder) -> Result<()> {
        // TODO: Implement proper gas coin selection and transaction signing
        // This requires:
        // 1. Loading keystore from wallet
        // 2. Selecting a gas coin with sufficient balance
        // 3. Getting current gas price
        // 4. Building TransactionData
        // 5. Signing with private key
        // 6. Executing transaction
        // 7. Waiting for transaction effects

        anyhow::bail!("execute_ptb not yet fully implemented - needs keystore and gas handling");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clock_object_id() {
        let clock_id = ObjectID::from_hex_literal(CLOCK_OBJECT_ID).unwrap();
        assert_eq!(clock_id.to_string(), CLOCK_OBJECT_ID);
    }
}
