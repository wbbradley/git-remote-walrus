use anyhow::Result;
use std::io::Write;

use crate::storage::StorageBackend;

/// Handle the list command
/// Output all refs with their Git SHA-1 hashes
pub fn handle<S: StorageBackend, W: Write>(
    storage: &S,
    output: &mut W,
    _for_push: bool,
) -> Result<()> {
    let state = storage.read_state()?;

    // For the import/export protocol, we need to list refs but NOT with specific SHAs
    // because Git will create the objects during fast-import.
    // Instead, we use special markers to indicate refs exist but Git should import them.

    // Output each ref with a special marker indicating it should be imported
    for (refname, _git_sha1) in &state.refs {
        // Use @<refname> syntax to indicate this ref exists and needs to be fetched
        writeln!(output, "? {}", refname)?;
    }

    // Output default branch pointer (HEAD)
    // If we have a main branch, point to it, otherwise the first ref
    if state.refs.contains_key("refs/heads/main") {
        writeln!(output, "@refs/heads/main HEAD")?;
    } else if let Some((first_ref, _)) = state.refs.iter().next() {
        writeln!(output, "@{} HEAD", first_ref)?;
    }

    // Empty line signals completion
    writeln!(output)?;

    Ok(())
}
