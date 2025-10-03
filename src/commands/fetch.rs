//! Handle fetch command - send packfile directly (no fast-export)

use anyhow::Result;
use std::io::Write;

use crate::pack::send_pack;
use crate::storage::StorageBackend;

/// Handle fetch command - send packfile for requested refs
/// This replaces the old import handler and eliminates fast-export
pub fn handle<S: StorageBackend, W: Write>(
    storage: &S,
    output: &mut W,
    refs: &[String],
) -> Result<()> {
    eprintln!("git-remote-gitwal: Fetch requested for refs: {:?}", refs);

    // Send packfile containing all objects for the requested refs
    // This preserves GPG signatures since we're using native pack format
    send_pack(refs, storage, output)?;

    eprintln!("git-remote-gitwal: Fetch completed");
    Ok(())
}
