use anyhow::Result;
use std::io::{BufRead, Write};

use crate::git::fast_export;
use crate::storage::StorageBackend;

/// Handle the export command (push)
/// Reads fast-export stream from stdin, stores it, and updates refs
pub fn handle<S: StorageBackend, W: Write, R: BufRead>(
    storage: &S,
    output: &mut W,
    input: &mut std::io::Lines<R>,
) -> Result<()> {
    // Read the entire fast-export stream from stdin
    let (stream_data, ref_updates) = fast_export::parse_stream(input)?;

    // Store the fast-export stream as an immutable object
    let content_id = storage.write_object(&stream_data)?;

    eprintln!("git-remote-gitwal: Stored export stream as {}", content_id);

    // Update state with new refs and object mappings
    storage.update_state(|state| {
        for (refname, git_sha1) in &ref_updates {
            eprintln!(
                "git-remote-gitwal: Updating ref {} to {}",
                refname, git_sha1
            );

            // Update refs mapping
            state.refs.insert(refname.clone(), git_sha1.clone());

            // Map Git SHA-1 to our content ID
            state.objects.insert(git_sha1.clone(), content_id.clone());
        }
        Ok(())
    })?;

    // Report success for each ref
    for (refname, _) in &ref_updates {
        writeln!(output, "ok {}", refname)?;
    }

    // Empty line signals completion
    writeln!(output)?;

    Ok(())
}
