use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::ContentId;

/// The mutable state stored in state.yaml
#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct State {
    /// Maps Git ref names to Git SHA-1 commit hashes (40 hex chars)
    #[serde(default)]
    pub refs: HashMap<String, String>, // ref_name -> git_sha1

    /// Maps Git SHA-1 hashes to backend content identifiers (opaque)
    /// Content IDs could be SHA-256, URIs, UUIDs - backend-specific
    #[serde(default)]
    pub objects: HashMap<String, ContentId>, // git_sha1 -> backend_content_id

    // Removed import_marks and export_marks - not needed for pack format
}
