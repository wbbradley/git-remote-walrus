use anyhow::Result;
use std::io::Write;

use crate::storage::StorageBackend;

/// Handle the import command (fetch)
/// Outputs fast-import stream to stdout for requested refs
pub fn handle<S: StorageBackend, W: Write>(
    storage: &S,
    output: &mut W,
    refs: &[String],
) -> Result<()> {
    let state = storage.read_state()?;

    eprintln!("git-remote-gitwal: Import requested for refs: {:?}", refs);

    // For each requested ref, output its stored fast-import stream
    for refname in refs {
        if let Some(git_sha1) = state.refs.get(refname) {
            eprintln!(
                "git-remote-gitwal: Found ref {} -> {}",
                refname, git_sha1
            );

            // Look up the content ID for this Git SHA-1
            if let Some(content_id) = state.objects.get(git_sha1) {
                eprintln!(
                    "git-remote-gitwal: Reading object {} from storage",
                    content_id
                );

                // Read the stored fast-export stream
                let stream_data = storage.read_object(content_id)?;

                // Output it to stdout (Git will read this as fast-import format)
                output.write_all(&stream_data)?;
            } else {
                eprintln!(
                    "git-remote-gitwal: Warning - no object found for SHA-1 {}",
                    git_sha1
                );
            }
        } else {
            eprintln!("git-remote-gitwal: Warning - ref {} not found", refname);
        }
    }

    // Output 'done' to signal end of fast-import stream
    writeln!(output, "done")?;

    // Empty line signals completion
    writeln!(output)?;

    Ok(())
}
