use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{Context, Result};
use base64::{display::Base64Display, engine::general_purpose::URL_SAFE_NO_PAD};
use num_bigint::BigUint;
use shared_crypto::intent::Intent;
use sui_config::PersistedConfig;
use sui_keys::keystore::AccountKeystore;
use sui_sdk::{
    rpc_types::{
        SuiMoveStruct, SuiMoveValue, SuiObjectDataOptions, SuiParsedData,
        SuiTransactionBlockEffectsAPI, SuiTransactionBlockResponseOptions,
    },
    sui_client_config::SuiClientConfig,
    SuiClientBuilder,
};
use sui_types::{
    base_types::{ObjectID, ObjectRef, SequenceNumber, SuiAddress},
    crypto::Signature,
    dynamic_field::DynamicFieldName,
    programmable_transaction_builder::ProgrammableTransactionBuilder,
    quorum_driver_types::ExecuteTransactionRequestType,
    transaction::{ObjectArg, Transaction, TransactionData},
    Identifier,
};
use tokio::time::Instant;

/// Sui on-chain clock object ID (shared object at 0x6)
const CLOCK_OBJECT_ID: &str = "0x0000000000000000000000000000000000000000000000000000000000000006";

/// Default gas budget for transactions (1 SUI = 1_000_000_000 MIST)
const DEFAULT_GAS_BUDGET: u64 = 10_000_000_000; // 0.1 SUI

/// Status information for a SharedBlob object
#[derive(Debug, Clone)]
pub struct SharedBlobStatus {
    pub object_id: String,
    pub blob_id: String,
    pub end_epoch: u64,
}

/// Sui client for interacting with RemoteState on-chain
pub struct SuiClient {
    /// Sui RPC client
    client: sui_sdk::SuiClient,

    /// RemoteState object ID
    /// Might not yet be initialized if we are in the `init` command.
    state_object_id: Option<ObjectID>,

    /// Package ID where RemoteState module is published
    package_id: ObjectID,

    /// Sender address (from wallet)
    sender: SuiAddress,

    /// Keystore for signing transactions
    sui_client_config: SuiClientConfig,
}

impl SuiClient {
    /// Create a new Sui client
    ///
    /// Loads the keystore and active address from Sui client config.
    pub async fn new(state_object_id: String, wallet_path: PathBuf) -> Result<Self> {
        // Parse state object ID
        let state_object_id = ObjectID::from_hex_literal(&state_object_id)
            .with_context(|| format!("Invalid state object ID: {}", state_object_id))?;

        // Load Sui client config to get active address
        let sui_client_config: SuiClientConfig = PersistedConfig::read(&wallet_path)
            .with_context(|| format!("Failed to load Sui config from {:?}", wallet_path))?;

        // Build Sui client
        let client = SuiClientBuilder::default()
            .build(sui_client_config.get_active_env()?.rpc.clone())
            .await
            .context("Failed to build Sui client")?;

        // Get active address from config
        let active_address = sui_client_config
            .active_address
            .ok_or_else(|| anyhow::anyhow!("No active address found in Sui config"))?;

        // Verify the active address exists in the keystore
        if !sui_client_config
            .keystore
            .addresses()
            .contains(&active_address)
        {
            anyhow::bail!("Active address {} not found in keystore", active_address,);
        }

        // Extract package ID from RemoteState object
        let package_id = Self::extract_package_id(&client, state_object_id)
            .await
            .context("Failed to extract package ID from RemoteState object")?;

        Ok(Self {
            client,
            state_object_id: Some(state_object_id),
            package_id,
            sender: active_address,
            sui_client_config,
        })
    }

    /// Create a new Sui client for init command (without state object ID)
    pub async fn new_for_init(package_id: String, wallet_path: PathBuf) -> Result<Self> {
        // Parse package ID
        let package_id = ObjectID::from_hex_literal(&package_id)
            .with_context(|| format!("Invalid package ID: {}", package_id))?;

        // Load Sui client config to get active address
        let sui_client_config: SuiClientConfig = PersistedConfig::read(&wallet_path)
            .with_context(|| format!("Failed to load Sui config from {:?}", wallet_path))?;

        // Build Sui client
        let client = SuiClientBuilder::default()
            .build(sui_client_config.get_active_env()?.rpc.clone())
            .await
            .context("Failed to build Sui client")?;

        // Get active address from config
        let active_address = sui_client_config
            .active_address
            .ok_or_else(|| anyhow::anyhow!("No active address found in Sui config"))?;

        // Verify the active address exists in the keystore
        if !sui_client_config
            .keystore
            .addresses()
            .contains(&active_address)
        {
            anyhow::bail!("Active address {} not found in keystore", active_address,);
        }

        Ok(Self {
            client,
            state_object_id: None,
            package_id,
            sender: active_address,
            sui_client_config,
        })
    }

    /// Create a new RemoteState object and return its ID
    pub async fn create_remote(&self) -> Result<String> {
        let mut ptb = ProgrammableTransactionBuilder::new();

        // Call create_remote() which transfers the object to sender
        ptb.programmable_move_call(
            self.package_id,
            Identifier::new("remote_state")?,
            Identifier::new("create_remote")?,
            vec![], // no type arguments
            vec![], // no arguments (uses TxContext)
        );

        // Execute and get created object ID
        let object_id = self
            .execute_ptb_and_get_created_object(ptb, DEFAULT_GAS_BUDGET)
            .await?;

        Ok(object_id.to_hex_literal())
    }

    /// Share a RemoteState object with an allowlist
    pub async fn share_remote(&self, object_id: String, allowlist: Vec<String>) -> Result<()> {
        // Parse object ID
        let object_id = ObjectID::from_hex_literal(&object_id)
            .with_context(|| format!("Invalid object ID: {}", object_id))?;

        // Parse allowlist addresses
        let mut allowlist_addrs = Vec::new();
        for addr_str in allowlist {
            let addr: SuiAddress = addr_str
                .parse()
                .with_context(|| format!("Invalid address: {}", addr_str))?;
            allowlist_addrs.push(addr);
        }

        let mut ptb = ProgrammableTransactionBuilder::new();

        // Get object reference (it's owned by sender)
        let object = self
            .client
            .read_api()
            .get_object_with_options(object_id, SuiObjectDataOptions::new().with_owner())
            .await
            .context("Failed to fetch RemoteState object")?;

        let data = object
            .data
            .ok_or_else(|| anyhow::anyhow!("RemoteState object not found"))?;

        let object_ref = data.object_ref();

        // Add object as input (receiving by value)
        let state_arg = ptb.obj(ObjectArg::ImmOrOwnedObject(object_ref))?;

        // Create vector of addresses for allowlist
        let allowlist_arg = ptb.pure(allowlist_addrs)?;

        // Call share_with_allowlist(state, allowlist)
        ptb.programmable_move_call(
            self.package_id,
            Identifier::new("remote_state")?,
            Identifier::new("share_with_allowlist")?,
            vec![], // no type arguments
            vec![state_arg, allowlist_arg],
        );

        // Execute transaction
        self.execute_ptb(ptb, DEFAULT_GAS_BUDGET).await?;

        Ok(())
    }

    /// Extract package ID from RemoteState object type
    async fn extract_package_id(
        client: &sui_sdk::SuiClient,
        state_object_id: ObjectID,
    ) -> Result<ObjectID> {
        let object = client
            .read_api()
            .get_object_with_options(state_object_id, SuiObjectDataOptions::new().with_type())
            .await
            .context("Failed to fetch RemoteState object")?;

        let data = object
            .data
            .ok_or_else(|| anyhow::anyhow!("RemoteState object not found"))?;

        let type_str = data
            .type_
            .ok_or_else(|| anyhow::anyhow!("RemoteState object has no type"))?
            .to_string();

        // Type format: "0xPACKAGE_ID::remote_state::RemoteState"
        let package_hex = type_str
            .split("::")
            .next()
            .ok_or_else(|| anyhow::anyhow!("Invalid RemoteState type format: {}", type_str))?;

        ObjectID::from_hex_literal(package_hex)
            .with_context(|| format!("Failed to parse package ID from type: {}", type_str))
    }

    /// Get the object reference for the RemoteState
    async fn get_state_object_ref(&self) -> Result<ObjectRef> {
        let state_object_id = self.state_object_id.ok_or_else(|| {
            anyhow::anyhow!("State object ID is not set - cannot get state object reference")
        })?;
        let object = self
            .client
            .read_api()
            .get_object_with_options(state_object_id, SuiObjectDataOptions::new().with_owner())
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
        let state_object_id = self.state_object_id.ok_or_else(|| {
            anyhow::anyhow!("State object ID is not set - cannot get state object reference")
        })?;
        // Get the RemoteState object
        let remote_state = self
            .client
            .read_api()
            .get_object_with_options(
                state_object_id,
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

    /// Get objects blob object ID from on-chain state
    pub async fn get_objects_blob_object_id(&self) -> Result<Option<String>> {
        let state_object_id = self.state_object_id.ok_or_else(|| {
            anyhow::anyhow!("State object ID is not set - cannot get state object reference")
        })?;
        // Get the RemoteState object with content
        let remote_state = self
            .client
            .read_api()
            .get_object_with_options(
                state_object_id,
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

        // Extract objects_blob_object_id from the struct
        self.extract_objects_blob_object_id_from_content(&content)
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

    /// Helper: Extract objects_blob_object_id from RemoteState content
    fn extract_objects_blob_object_id_from_content(
        &self,
        content: &SuiParsedData,
    ) -> Result<Option<String>> {
        let move_obj = match content {
            SuiParsedData::MoveObject(obj) => obj,
            _ => anyhow::bail!("Expected MoveObject"),
        };

        // Extract the "objects_blob_object_id" field which is Option<Address> or Address
        let blob_object_id_field = self
            .get_struct_field(&move_obj.fields, "objects_blob_object_id")
            .context("Failed to get 'objects_blob_object_id' field")?;

        tracing::debug!(
            "sui: Extracting objects_blob_object_id from field: {:?}",
            blob_object_id_field
        );

        // Extract Option<String> - field can be Option<Address> or direct Address
        let result = self
            .extract_option_string_or_address(blob_object_id_field)
            .context("Failed to extract object ID from objects_blob_object_id")?;

        tracing::debug!("sui: Extracted objects_blob_object_id: {:?}", result);
        Ok(result)
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

    /// Helper: Extract String from Address or String SuiMoveValue
    fn extract_string_or_address(&self, value: &SuiMoveValue) -> Result<String> {
        use sui_sdk::rpc_types::SuiMoveValue;

        match value {
            SuiMoveValue::String(s) => Ok(s.clone()),
            SuiMoveValue::Address(addr) => Ok(addr.to_string()),
            _ => anyhow::bail!("Expected String or Address, got {:?}", value),
        }
    }

    /// Helper: Extract Option<String> from Option<Address> or Option<String>
    fn extract_option_string_or_address(&self, value: &SuiMoveValue) -> Result<Option<String>> {
        use sui_sdk::rpc_types::SuiMoveValue;

        match value {
            SuiMoveValue::Option(opt) => match opt.as_ref() {
                Some(inner) => Ok(Some(self.extract_string_or_address(inner)?)),
                None => Ok(None),
            },
            // Handle direct Address (not wrapped in Option) - legacy case
            SuiMoveValue::Address(addr) => Ok(Some(addr.to_string())),
            _ => anyhow::bail!("Expected Option<Address> or Address, got {:?}", value),
        }
    }

    /// Helper: Extract u64 from SuiMoveValue
    fn extract_u64(&self, value: &SuiMoveValue) -> Result<u64> {
        use sui_sdk::rpc_types::SuiMoveValue;

        match value {
            SuiMoveValue::Number(n) => Ok(*n as u64),
            SuiMoveValue::String(s) => s
                .parse::<u64>()
                .with_context(|| format!("Failed to parse u64 from string: {}", s)),
            _ => anyhow::bail!("Expected Number or String for u64, got {:?}", value),
        }
    }

    /// Batch query SharedBlob statuses from Sui with pagination
    /// Returns results in the same order as input, with errors for individual failures
    /// Chunks requests to avoid RPC limits (default: 50 objects per batch)
    /// Calls progress_callback after each chunk if provided
    pub async fn get_shared_blob_statuses_batch<F>(
        &self,
        object_ids: &[String],
        mut progress_callback: Option<F>,
    ) -> Result<Vec<Result<SharedBlobStatus>>>
    where
        F: FnMut(usize),
    {
        if object_ids.is_empty() {
            return Ok(Vec::new());
        }

        // RPC batch size limit - conservative to avoid hitting server limits
        const BATCH_SIZE: usize = 50;

        tracing::debug!(
            "sui: Batch querying {} SharedBlob objects (chunk size: {})",
            object_ids.len(),
            BATCH_SIZE
        );

        let mut all_results = Vec::with_capacity(object_ids.len());

        // Process in chunks to avoid RPC limits
        for (chunk_idx, chunk) in object_ids.chunks(BATCH_SIZE).enumerate() {
            if object_ids.len() > BATCH_SIZE {
                tracing::debug!(
                    "  Processing batch {}/{} ({} objects)",
                    chunk_idx + 1,
                    object_ids.len().div_ceil(BATCH_SIZE),
                    chunk.len()
                );
            }

            let chunk_results = self.query_blob_statuses_single_batch(chunk).await?;
            all_results.extend(chunk_results);

            // Call progress callback after processing this chunk
            if let Some(ref mut callback) = progress_callback {
                callback(chunk.len());
            }
        }

        Ok(all_results)
    }

    /// Query a single batch of SharedBlob statuses (internal helper)
    async fn query_blob_statuses_single_batch(
        &self,
        object_ids: &[String],
    ) -> Result<Vec<Result<SharedBlobStatus>>> {
        // Parse all object IDs
        let parsed_ids: Result<Vec<ObjectID>> = object_ids
            .iter()
            .map(|id| {
                ObjectID::from_hex_literal(id).with_context(|| format!("Invalid object ID: {}", id))
            })
            .collect();
        let parsed_ids = parsed_ids?;

        // Batch query all objects in this chunk
        let objects = self
            .client
            .read_api()
            .multi_get_object_with_options(
                parsed_ids.clone(),
                SuiObjectDataOptions::new()
                    .with_content()
                    .with_bcs()
                    .with_type()
                    .with_owner(),
            )
            .await
            .context("Failed to batch fetch SharedBlob objects")?;

        // Process each result
        let mut results = Vec::new();
        for (i, object_response) in objects.into_iter().enumerate() {
            let object_id_str = &object_ids[i];

            let result = (|| -> Result<SharedBlobStatus> {
                let data = object_response.data.ok_or_else(|| {
                    anyhow::anyhow!(
                        "SharedBlob object not found: {} (error: {:?})",
                        object_id_str,
                        object_response.error
                    )
                })?;

                let content = data.content.ok_or_else(|| {
                    anyhow::anyhow!("SharedBlob has no content: {}", object_id_str)
                })?;

                // Extract fields from the SharedBlob object
                let move_obj = match content {
                    SuiParsedData::MoveObject(obj) => obj,
                    _ => anyhow::bail!("Expected MoveObject for SharedBlob"),
                };

                // Navigate to: content.fields.blob.fields
                let blob_field = self
                    .get_struct_field(&move_obj.fields, "blob")
                    .context("Failed to get 'blob' field from SharedBlob")?;

                let blob_struct = match blob_field {
                    SuiMoveValue::Struct(s) => s,
                    _ => anyhow::bail!("Expected Struct for blob field"),
                };

                // Extract blob_id (stored as u256 decimal, convert to base64)
                let blob_id_value = self
                    .get_struct_field(blob_struct, "blob_id")
                    .context("Failed to get 'blob_id' field from Blob")?;
                let blob_id_u256 = self.extract_string(blob_id_value)?;
                let blob_id = parse_num_blob_id(&blob_id_u256)?;

                // Navigate to: blob.fields.storage.fields
                let storage_field = self
                    .get_struct_field(blob_struct, "storage")
                    .context("Failed to get 'storage' field from Blob")?;

                let storage_struct = match storage_field {
                    SuiMoveValue::Struct(s) => s,
                    _ => anyhow::bail!("Expected Struct for storage field"),
                };

                // Extract end_epoch
                let end_epoch_value = self
                    .get_struct_field(storage_struct, "end_epoch")
                    .context("Failed to get 'end_epoch' field from Storage")?;
                let end_epoch = self.extract_u64(end_epoch_value)?;

                Ok(SharedBlobStatus {
                    object_id: object_id_str.to_string(),
                    blob_id,
                    end_epoch,
                })
            })();

            results.push(result);
        }

        Ok(results)
    }

    /// Get SharedBlob status from Sui
    /// Extracts object_id, blob_id, and end_epoch from a SharedBlob object
    pub async fn get_shared_blob_status(&self, object_id: &str) -> Result<SharedBlobStatus> {
        tracing::debug!("sui: Querying SharedBlob object: {}", object_id);

        // Parse object ID
        let obj_id = ObjectID::from_hex_literal(object_id)
            .with_context(|| format!("Invalid object ID: {}", object_id))?;

        // Get the SharedBlob object with content
        let object = self
            .client
            .read_api()
            .get_object_with_options(
                obj_id,
                SuiObjectDataOptions::new()
                    .with_content()
                    .with_bcs()
                    .with_type()
                    .with_owner(),
            )
            .await
            .with_context(|| format!("Failed to fetch SharedBlob object {}", object_id))?;

        tracing::debug!(
            "sui: Query response - data: {:?}, error: {:?}",
            object.data.is_some(),
            object.error
        );

        let data = object.data.ok_or_else(|| {
            anyhow::anyhow!(
                "SharedBlob object not found: {} (error: {:?})",
                object_id,
                object.error
            )
        })?;

        let content = data
            .content
            .ok_or_else(|| anyhow::anyhow!("SharedBlob has no content: {}", object_id))?;

        // Extract fields from the SharedBlob object
        let move_obj = match content {
            SuiParsedData::MoveObject(obj) => obj,
            _ => anyhow::bail!("Expected MoveObject for SharedBlob"),
        };

        // Navigate to: content.fields.blob.fields
        let blob_field = self
            .get_struct_field(&move_obj.fields, "blob")
            .context("Failed to get 'blob' field from SharedBlob")?;

        let blob_struct = match blob_field {
            SuiMoveValue::Struct(s) => s,
            _ => anyhow::bail!("Expected Struct for blob field"),
        };

        // Extract blob_id (stored as u256 decimal, convert to base64)
        let blob_id_value = self
            .get_struct_field(blob_struct, "blob_id")
            .context("Failed to get 'blob_id' field from Blob")?;
        let blob_id_u256 = self.extract_string(blob_id_value)?;
        let blob_id = parse_num_blob_id(&blob_id_u256)?;
        tracing::debug!("sui: blob_id: {}", blob_id);

        // Navigate to: blob.fields.storage.fields
        let storage_field = self
            .get_struct_field(blob_struct, "storage")
            .context("Failed to get 'storage' field from Blob")?;

        let storage_struct = match storage_field {
            SuiMoveValue::Struct(s) => s,
            _ => anyhow::bail!("Expected Struct for storage field"),
        };

        // Extract end_epoch
        let end_epoch_value = self
            .get_struct_field(storage_struct, "end_epoch")
            .context("Failed to get 'end_epoch' field from Storage")?;
        let end_epoch = self.extract_u64(end_epoch_value)?;

        Ok(SharedBlobStatus {
            object_id: object_id.to_string(),
            blob_id,
            end_epoch,
        })
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
    /// Retries on 504 timeout errors since transaction may have succeeded
    pub async fn acquire_lock(&self, timeout_ms: u64) -> Result<()> {
        const MAX_RETRIES: u32 = 3;
        const RETRY_DELAY_MS: u64 = 200;

        for attempt in 0..MAX_RETRIES {
            if attempt > 0 {
                tracing::info!("  Retry attempt {} after 504 timeout...", attempt);
                tokio::time::sleep(tokio::time::Duration::from_millis(RETRY_DELAY_MS)).await;

                // Check if lock was actually acquired despite the timeout
                if self.check_lock_acquired().await? {
                    tracing::info!("  Lock was already acquired in previous attempt");
                    return Ok(());
                }
            }

            let mut ptb = ProgrammableTransactionBuilder::new();

            // Get object references
            let state_ref = self.get_state_object_ref().await?;
            let clock_ref = self.get_clock_object_ref().await?;

            // Add objects as inputs
            let state_arg = ptb.obj(ObjectArg::ImmOrOwnedObject(state_ref))?;
            // ObjectArg::Receiving(state_ref),
            let clock_arg = ptb.obj(ObjectArg::SharedObject {
                id: clock_ref.0,
                initial_shared_version: SequenceNumber::from(1),
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
            match self.execute_ptb(ptb, DEFAULT_GAS_BUDGET).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    tracing::error!("git-remote-walrus: [acquire_lock(timeout_ms={timeout_ms})] execute_ptb error: {e:?}");
                    let err_str = e.to_string();
                    // Retry only on 504 timeouts
                    if err_str.contains("504") && attempt < MAX_RETRIES - 1 {
                        tracing::warn!(
                            "  Got 504 timeout on attempt {}, will retry...",
                            attempt + 1
                        );
                        continue;
                    }
                    return Err(e);
                }
            }
        }

        anyhow::bail!("Failed to acquire lock after {} retries", MAX_RETRIES)
    }

    /// Check if a lock is currently held on the RemoteState
    async fn check_lock_acquired(&self) -> Result<bool> {
        let state_object_id = self.state_object_id.ok_or_else(|| {
            anyhow::anyhow!("State object ID is not set - cannot get state object reference")
        })?;
        let object = self
            .client
            .read_api()
            .get_object_with_options(state_object_id, SuiObjectDataOptions::new().with_content())
            .await
            .context("Failed to fetch RemoteState object")?;

        let data = object
            .data
            .ok_or_else(|| anyhow::anyhow!("RemoteState object not found"))?;

        if let Some(SuiParsedData::MoveObject(move_obj)) = data.content {
            if let SuiMoveStruct::WithFields(fields) = move_obj.fields {
                if let Some(lock_value) = fields.get("lock") {
                    // If lock field is Some (not null), lock is acquired
                    return Ok(matches!(lock_value, SuiMoveValue::Option(opt) if opt.is_some()));
                }
            }
        }

        Ok(false)
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
            initial_shared_version: SequenceNumber::from(1),
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
        objects_blob_object_id: String,
    ) -> Result<()> {
        tracing::debug!(
            "sui: Storing objects_blob_object_id to RemoteState: {}",
            objects_blob_object_id
        );

        let mut ptb = ProgrammableTransactionBuilder::new();

        // Get object references
        let state_ref = self.get_state_object_ref().await?;
        let clock_ref = self.get_clock_object_ref().await?;

        // Add objects as inputs
        let state_arg = ptb.obj(ObjectArg::ImmOrOwnedObject(state_ref))?;
        let clock_arg = ptb.obj(ObjectArg::SharedObject {
            id: clock_ref.0,
            initial_shared_version: SequenceNumber::from(1),
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

        // 2. Update objects blob object ID
        let objects_blob_object_arg = ptb.pure(objects_blob_object_id)?;

        ptb.programmable_move_call(
            self.package_id,
            Identifier::new("remote_state")?,
            Identifier::new("update_objects_blob")?,
            vec![], // no type arguments
            vec![state_arg, objects_blob_object_arg, clock_arg],
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
        tracing::debug!("sui: Executing programmable transaction...");
        tracing::debug!("  Selecting gas coins for budget: {} MIST", gas_budget);
        // 1. Select enough gas coins to cover the budget
        let coins = self
            .client
            .coin_read_api()
            .get_coins(self.sender, None, None, Some(50))
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

        tracing::debug!("  Fetching current gas price...");
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
        let gas_coin_count = gas_coin_refs.len();
        let tx_data = TransactionData::new_programmable(
            self.sender,
            gas_coin_refs,
            pt,
            gas_budget,
            gas_price,
        );

        // 4. Sign transaction with keystore
        tracing::debug!("  Signing transaction with address: {}", self.sender);
        let signature: Signature = self
            .sui_client_config
            .keystore
            .sign_secure(&self.sender, &tx_data, Intent::sui_transaction())
            .await
            .context("Failed to sign transaction")?;
        tracing::debug!("  Transaction signed successfully");

        // 5. Create signed transaction
        let transaction = Transaction::from_data(tx_data, vec![signature]);

        // 6. Execute transaction
        // Use WaitForEffectsCert for faster response (doesn't wait for local execution)
        tracing::info!("  Executing transaction on-chain [gas_coin_count={gas_coin_count}]...");
        let start = Instant::now();
        let response = self
            .client
            .quorum_driver_api()
            .execute_transaction_block(
                transaction,
                SuiTransactionBlockResponseOptions::default()
                    .with_effects()
                    .with_input()
                    .with_events()
                    .with_object_changes()
                    .with_balance_changes(),
                Some(ExecuteTransactionRequestType::WaitForLocalExecution),
            )
            .await
            .with_context(|| {
                format!("Failed to execute transaction after {:?}", start.elapsed())
            })?;

        // 7. Check for errors in transaction execution
        if let Some(effects) = &response.effects {
            if effects.status().is_err() {
                anyhow::bail!("Transaction execution failed: {:?}", effects.status());
            }
        }

        tracing::info!(
            "sui: Transaction executed successfully: {}",
            response.digest
        );

        Ok(())
    }

    /// Execute a PTB and return the first created object ID
    async fn execute_ptb_and_get_created_object(
        &self,
        ptb: ProgrammableTransactionBuilder,
        gas_budget: u64,
    ) -> Result<ObjectID> {
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
        tracing::debug!("  Signing transaction with address: {}", self.sender);
        let signature: Signature = self
            .sui_client_config
            .keystore
            .sign_secure(&self.sender, &tx_data, Intent::sui_transaction())
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
                SuiTransactionBlockResponseOptions::default()
                    .with_effects()
                    .with_object_changes(),
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

        // 8. Extract created object ID from object changes
        let object_changes = response
            .object_changes
            .ok_or_else(|| anyhow::anyhow!("No object changes in response"))?;

        for change in object_changes {
            if let sui_sdk::rpc_types::ObjectChange::Created {
                object_id,
                object_type,
                ..
            } = change
            {
                // Check if this is a RemoteState object
                if object_type
                    .to_string()
                    .contains("remote_state::RemoteState")
                {
                    return Ok(object_id);
                }
            }
        }

        anyhow::bail!("No RemoteState object was created in transaction")
    }
}

fn parse_num_blob_id(s: &str) -> Result<String> {
    if let Some(number) = BigUint::parse_bytes(s.as_bytes(), 10) {
        let bytes = number.to_bytes_le();

        if bytes.len() <= 32 {
            let mut blob_id = [0; 32];
            blob_id[..bytes.len()].copy_from_slice(&bytes);
            return Ok(Base64Display::new(&blob_id, &URL_SAFE_NO_PAD).to_string());
        }
    }
    anyhow::bail!("Unable to parse numeric blob id: {s}");
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
