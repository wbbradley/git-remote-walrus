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

    // Output each ref with its Git SHA-1
    for (refname, git_sha1) in &state.refs {
        writeln!(output, "{} {}", git_sha1, refname)?;
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
