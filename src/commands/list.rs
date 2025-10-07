use std::io::Write;

use anyhow::Result;

use crate::storage::StorageBackend;

/// Handle the list command
/// Output all refs with their Git SHA-1 hashes
pub fn handle<S: StorageBackend, W: Write>(
    storage: &S,
    output: &mut W,
    _for_push: bool,
) -> Result<()> {
    let state = storage.read_state()?;

    // For the fetch capability, we MUST output actual SHA-1 hashes
    // Git can only fetch objects that were listed with a SHA-1 hash
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
