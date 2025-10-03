//! Handle fetch command (replaces import)

use anyhow::Result;
use std::io::Write;

use crate::pack::send_pack;
use crate::storage::StorageBackend;

/// Handle fetch command - send packfile for requested refs
pub fn handle<S: StorageBackend, W: Write>(
    storage: &S,
    output: &mut W,
    refs: &[String],
) -> Result<()> {
    eprintln!("Fetching refs: {:?}", refs);

    // Send packfile containing all objects for the requested refs
    send_pack(refs, storage, output)?;

    eprintln!("Fetch completed");
    Ok(())
}
