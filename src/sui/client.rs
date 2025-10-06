use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;

use shared_crypto::intent::{Intent, IntentMessage};
use sui_config::{sui_config_dir, PersistedConfig, SUI_CLIENT_CONFIG, SUI_KEYSTORE_FILENAME};
use sui_keys::keystore::{AccountKeystore, FileBasedKeystore};
use sui_sdk::{
    rpc_types::{
        SuiMoveStruct, SuiMoveValue, SuiObjectDataOptions, SuiParsedData,
        SuiTransactionBlockEffectsAPI, SuiTransactionBlockResponseOptions,
    },
    sui_client_config::SuiClientConfig,
    SuiClientBuilder,
};
use sui_types::{
    base_types::{ObjectID, ObjectRef, SuiAddress},
    crypto::Signature,
    dynamic_field::DynamicFieldName,
    programmable_transaction_builder::ProgrammableTransactionBuilder,
    quorum_driver_types::ExecuteTransactionRequestType,
    transaction::{ObjectArg, Transaction, TransactionData},
    Identifier,
};

/// Sui on-chain clock object ID (shared object at 0x6)
const CLOCK_OBJECT_ID: &str = "0x0000000000000000000000000000000000000000000000000000000000000006";

/// Default gas budget for transactions (1 SUI = 1_000_000_000 MIST)
const DEFAULT_GAS_BUDGET: u64 = 10_000_000_000; // 0.1 SUI

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

    /// File-based keystore for signing transactions
    keystore: FileBasedKeystore,
}

impl SuiClient {
    /// Create a new Sui client
    ///
    /// Loads the keystore and active address from Sui client config.
    pub async fn new(
        state_object_id: String,
        rpc_url: String,
        wallet_path: PathBuf,
    ) -> Result<Self> {
        // Parse state object ID
        let state_object_id = ObjectID::from_hex_literal(&state_object_id)
            .with_context(|| format!("Invalid state object ID: {}", state_object_id))?;

        // Build Sui client
        let client = SuiClientBuilder::default()
            .build(rpc_url)
            .await
            .context("Failed to build Sui client")?;

        // Load Sui client config to get active address
        let config_dir = if wallet_path.exists() && wallet_path.is_file() {
            wallet_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .to_path_buf()
        } else {
            sui_config_dir().context("Failed to get Sui config directory")?
        };

        let config_path = config_dir.join(SUI_CLIENT_CONFIG);
        let sui_config: SuiClientConfig = PersistedConfig::read(&config_path)
            .with_context(|| format!("Failed to load Sui config from {:?}", config_path))?;

        // Get active address from config
        let sender = sui_config
            .active_address
            .ok_or_else(|| anyhow::anyhow!("No active address found in Sui config"))?;

        // Load keystore
        let keystore_path = config_dir.join(SUI_KEYSTORE_FILENAME);
        let keystore = FileBasedKeystore::load_or_create(&keystore_path)
            .with_context(|| format!("Failed to load keystore from {:?}", keystore_path))?;

        // Verify the active address exists in the keystore
        if !keystore.addresses().contains(&sender) {
            anyhow::bail!(
                "Active address {} not found in keystore at {:?}",
                sender,
                keystore_path
            );
        }

        // TODO: Load package ID from the RemoteState object
        // For now, we'll extract it when we read the object
        let package_id = ObjectID::ZERO; // Placeholder

        Ok(Self {
            client,
            state_object_id,
            package_id,
            sender,
            keystore,
        })
    }

    /// Get the object reference for the RemoteState
    async fn get_state_object_ref(&self) -> Result<ObjectRef> {
        let object = self
            .client
            .read_api()
            .get_object_with_options(
                self.state_object_id,
                SuiObjectDataOptions::new().with_owner(),
            )
            .await
            .context("Failed to fetch RemoteState object")?;

        let data = object
            .data
            .ok_or_else(|| anyhow::anyhow!("RemoteState object not found"))?;

        Ok(data.object_ref())
    }

    /// Get the Clock object reference (shared object at 0x6)
    async fn get_clock_object_ref(&self) -> Result<ObjectRef> {
        let clock_id = ObjectID::from_hex_literal(CLOCK_OBJECT_ID)
            .context("Failed to parse clock object ID")?;

        let object = self
            .client
            .read_api()
            .get_object_with_options(clock_id, SuiObjectDataOptions::new().with_owner())
            .await
            .context("Failed to fetch Clock object")?;

        let data = object
            .data
            .ok_or_else(|| anyhow::anyhow!("Clock object not found"))?;

        Ok(data.object_ref())
    }

    /// Read all refs from on-chain state
    pub async fn read_refs(&self) -> Result<BTreeMap<String, String>> {
        // Get the RemoteState object
        let remote_state = self
            .client
            .read_api()
            .get_object_with_options(
                self.state_object_id,
                SuiObjectDataOptions::new().with_content().with_bcs(),
            )
            .await
            .context("Failed to fetch RemoteState object")?;

        let data = remote_state
            .data
            .ok_or_else(|| anyhow::anyhow!("RemoteState object not found"))?;

        // Extract the refs Table UID from the object content
        // The Table is stored as a dynamic field container
        let content = data
            .content
            .ok_or_else(|| anyhow::anyhow!("RemoteState has no content"))?;

        // Get the refs Table's ObjectID from the struct
        let table_id = self
            .extract_table_id_from_content(&content)
            .context("Failed to extract refs table ID")?;

        // Query all dynamic fields of the Table
        let mut refs = BTreeMap::new();
        let mut cursor = None;

        loop {
            let page = self
                .client
                .read_api()
                .get_dynamic_fields(table_id, cursor, Some(100))
                .await
                .context("Failed to get dynamic fields")?;

            for field in page.data {
                // Extract ref name from field.name
                let ref_name = self.extract_string_from_dynamic_field_name(&field.name)?;

                // Get the field value (git SHA1)
                let field_value = self
                    .client
                    .read_api()
                    .get_dynamic_field_object(table_id, field.name.clone())
                    .await
                    .context("Failed to get dynamic field value")?;

                if let Some(data) = field_value.data {
                    if let Some(content) = data.content {
                        let git_sha1 = self.extract_string_value_from_content(&content)?;
                        refs.insert(ref_name, git_sha1);
                    }
                }
            }

            if page.has_next_page {
                cursor = page.next_cursor;
            } else {
                break;
            }
        }

        Ok(refs)
    }

    /// Get objects blob ID from on-chain state
    pub async fn get_objects_blob_id(&self) -> Result<Option<String>> {
        // Get the RemoteState object with content
        let remote_state = self
            .client
            .read_api()
            .get_object_with_options(
                self.state_object_id,
                SuiObjectDataOptions::new().with_content().with_bcs(),
            )
            .await
            .context("Failed to fetch RemoteState object")?;

        let data = remote_state
            .data
            .ok_or_else(|| anyhow::anyhow!("RemoteState object not found"))?;

        let content = data
            .content
            .ok_or_else(|| anyhow::anyhow!("RemoteState has no content"))?;

        // Extract objects_blob_id from the struct
        self.extract_objects_blob_id_from_content(&content)
    }

    /// Helper: Extract the Table ID from RemoteState content
    fn extract_table_id_from_content(&self, content: &SuiParsedData) -> Result<ObjectID> {
        use sui_sdk::rpc_types::SuiParsedData;

        // Get the MoveObject variant
        let move_obj = match content {
            SuiParsedData::MoveObject(obj) => obj,
            _ => anyhow::bail!("Expected MoveObject, got package"),
        };

        // Access the fields
        let fields = &move_obj.fields;

        // Extract the "refs" field which should be a Table (UID with id)
        let refs_field = self
            .get_struct_field(fields, "refs")
            .context("Failed to get 'refs' field from RemoteState")?;

        // The Table is represented as a UID { id: ObjectID }
        let table_id = self
            .extract_uid_id(refs_field)
            .context("Failed to extract ObjectID from refs Table UID")?;

        Ok(table_id)
    }

    /// Helper: Extract string from dynamic field name
    fn extract_string_from_dynamic_field_name(&self, name: &DynamicFieldName) -> Result<String> {
        // The name.value is a serde_json::Value
        // For String keys, it should be a JSON string
        name.value
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Dynamic field name is not a string: {:?}", name.value))
    }

    /// Helper: Extract string value from dynamic field content
    fn extract_string_value_from_content(&self, content: &SuiParsedData) -> Result<String> {
        // Dynamic field values are wrapped in a Field struct
        let move_obj = match content {
            SuiParsedData::MoveObject(obj) => obj,
            _ => anyhow::bail!("Expected MoveObject for dynamic field value"),
        };

        // The value is in a "value" field
        let value_field = self
            .get_struct_field(&move_obj.fields, "value")
            .context("Failed to get 'value' field from dynamic field")?;

        // Extract the string
        self.extract_string(value_field)
            .context("Failed to extract string from dynamic field value")
    }

    /// Helper: Extract objects_blob_id from RemoteState content
    fn extract_objects_blob_id_from_content(
        &self,
        content: &SuiParsedData,
    ) -> Result<Option<String>> {
        let move_obj = match content {
            SuiParsedData::MoveObject(obj) => obj,
            _ => anyhow::bail!("Expected MoveObject"),
        };

        // Extract the "objects_blob_id" field which is Option<String>
        let blob_id_field = self
            .get_struct_field(&move_obj.fields, "objects_blob_id")
            .context("Failed to get 'objects_blob_id' field")?;

        // Extract Option<String>
        self.extract_option_string(blob_id_field)
            .context("Failed to extract Option<String> from objects_blob_id")
    }

    /// Helper: Get a field from SuiMoveStruct
    fn get_struct_field<'a>(
        &self,
        fields: &'a SuiMoveStruct,
        field_name: &str,
    ) -> Result<&'a SuiMoveValue> {
        let field_map = match fields {
            SuiMoveStruct::WithFields(map) | SuiMoveStruct::WithTypes { fields: map, .. } => map,
            SuiMoveStruct::Runtime(_) => anyhow::bail!("Cannot access fields in Runtime variant"),
        };

        field_map
            .get(field_name)
            .ok_or_else(|| anyhow::anyhow!("Field '{}' not found", field_name))
    }

    /// Helper: Extract ObjectID from a UID field
    fn extract_uid_id(&self, value: &SuiMoveValue) -> Result<ObjectID> {
        use sui_sdk::rpc_types::SuiMoveValue;

        match value {
            SuiMoveValue::Struct(sui_struct) => {
                // UID is a struct with an "id" field
                let id_field = self
                    .get_struct_field(sui_struct, "id")
                    .context("Failed to get 'id' field from UID")?;

                // The id field should be a UID { id: ObjectID }
                if let SuiMoveValue::UID { id } = id_field {
                    Ok(*id)
                } else {
                    anyhow::bail!("Expected UID variant, got {:?}", id_field)
                }
            }
            _ => anyhow::bail!("Expected Struct for UID, got {:?}", value),
        }
    }

    /// Helper: Extract String from SuiMoveValue
    fn extract_string(&self, value: &SuiMoveValue) -> Result<String> {
        use sui_sdk::rpc_types::SuiMoveValue;

        match value {
            SuiMoveValue::String(s) => Ok(s.clone()),
            _ => anyhow::bail!("Expected String, got {:?}", value),
        }
    }

    /// Helper: Extract Option<String> from SuiMoveValue
    fn extract_option_string(&self, value: &SuiMoveValue) -> Result<Option<String>> {
        use sui_sdk::rpc_types::SuiMoveValue;

        match value {
            SuiMoveValue::Option(opt) => match opt.as_ref() {
                Some(inner) => Ok(Some(self.extract_string(inner)?)),
                None => Ok(None),
            },
            _ => anyhow::bail!("Expected Option, got {:?}", value),
        }
    }

    /// Batch upsert refs using PTB
    #[allow(dead_code)]
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
                vec![state_arg, ref_arg, sha_arg],
            );
        }

        // Build and execute transaction
        self.execute_ptb(ptb, DEFAULT_GAS_BUDGET).await?;

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
            vec![state_arg, clock_arg, timeout_arg],
        );

        // Build and execute transaction
        self.execute_ptb(ptb, DEFAULT_GAS_BUDGET).await?;

        Ok(())
    }

    /// Update objects blob ID (requires lock)
    #[allow(dead_code)]
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
            vec![state_arg, blob_arg, clock_arg],
        );

        // Build and execute transaction
        self.execute_ptb(ptb, DEFAULT_GAS_BUDGET).await?;

        Ok(())
    }

    /// Release lock
    #[allow(dead_code)]
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
        self.execute_ptb(ptb, DEFAULT_GAS_BUDGET).await?;

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
                vec![state_arg, ref_arg, sha_arg],
            );
        }

        // 2. Update objects blob ID
        let objects_blob_arg = ptb.pure(objects_blob_id)?;

        ptb.programmable_move_call(
            self.package_id,
            Identifier::new("remote_state")?,
            Identifier::new("update_objects_blob")?,
            vec![], // no type arguments
            vec![state_arg, objects_blob_arg, clock_arg],
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
        self.execute_ptb(ptb, DEFAULT_GAS_BUDGET).await?;

        Ok(())
    }

    /// Execute a PTB with proper gas handling
    async fn execute_ptb(
        &self,
        ptb: ProgrammableTransactionBuilder,
        gas_budget: u64,
    ) -> Result<()> {
        // 1. Select enough gas coins to cover the budget
        let coins = self
            .client
            .coin_read_api()
            .get_coins(self.sender, None, None, Some(500))
            .await
            .context("Failed to fetch gas coins")?;

        // Collect coins until we have enough balance
        let mut gas_coins = Vec::new();
        let mut total_balance = 0u64;

        for coin in coins.data {
            total_balance += coin.balance;
            gas_coins.push(coin);

            if total_balance >= gas_budget {
                break;
            }
        }

        if total_balance < gas_budget {
            anyhow::bail!(
                "Insufficient gas: need {} MIST, but only have {} MIST available",
                gas_budget,
                total_balance
            );
        }

        if gas_coins.is_empty() {
            anyhow::bail!("No gas coins available for sender");
        }

        // 2. Get current gas price
        let gas_price = self
            .client
            .read_api()
            .get_reference_gas_price()
            .await
            .context("Failed to get reference gas price")?;

        // 3. Build TransactionData with all selected gas coins
        let pt = ptb.finish();
        let gas_coin_refs: Vec<_> = gas_coins.iter().map(|c| c.object_ref()).collect();
        let tx_data = TransactionData::new_programmable(
            self.sender,
            gas_coin_refs,
            pt,
            gas_budget,
            gas_price,
        );

        // 4. Sign transaction with keystore
        let intent_msg = IntentMessage::new(Intent::sui_transaction(), tx_data.clone());
        let digest = intent_msg.value.digest();
        let signature: Signature = self
            .keystore
            .sign_hashed(&self.sender, digest.as_ref())
            .await
            .context("Failed to sign transaction")?;

        // 5. Create signed transaction
        let transaction = Transaction::from_data(tx_data, vec![signature]);

        // 6. Execute transaction
        let response = self
            .client
            .quorum_driver_api()
            .execute_transaction_block(
                transaction,
                SuiTransactionBlockResponseOptions::default(),
                Some(ExecuteTransactionRequestType::WaitForLocalExecution),
            )
            .await
            .context("Failed to execute transaction")?;

        // 7. Check for errors in transaction execution
        if let Some(effects) = &response.effects {
            if effects.status().is_err() {
                anyhow::bail!("Transaction execution failed: {:?}", effects.status());
            }
        }

        eprintln!(
            "sui: Transaction executed successfully: {}",
            response.digest
        );

        Ok(())
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
