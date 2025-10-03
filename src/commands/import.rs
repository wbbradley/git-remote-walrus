use anyhow::Result;
use std::io::Write;

use crate::storage::StorageBackend;

/// Filter a fast-import stream to remove problematic commands
fn filter_fast_import_stream(data: &[u8]) -> Vec<u8> {
    let input = String::from_utf8_lossy(data);
    let mut output = Vec::new();
    let mut lines = input.lines().peekable();

    while let Some(line) = lines.next() {
        // Skip reset commands that don't have a "from" clause
        // These are problematic for fast-import
        if line.trim().starts_with("reset ") {
            // Peek at the next line to see if it has "from"
            if let Some(next_line) = lines.peek() {
                if next_line.trim().starts_with("from ") {
                    // This reset has a from clause, keep it
                    output.extend_from_slice(line.as_bytes());
                    output.push(b'\n');
                } else {
                    // Skip this reset - it has no "from" clause
                    eprintln!("git-remote-gitwal: Filtering out problematic reset: {}", line);
                }
            } else {
                // Last line is a reset, skip it
                eprintln!("git-remote-gitwal: Filtering out problematic reset: {}", line);
            }
        } else {
            // Keep all other lines
            output.extend_from_slice(line.as_bytes());
            output.push(b'\n');
        }
    }

    output
}

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

                eprintln!("git-remote-gitwal: Original fast-import stream:");
                eprintln!("{}", String::from_utf8_lossy(&stream_data));

                // The fast-export stream uses marks, but we need to output actual Git objects.
                // We'll just output the stream and add a 'done' at the end.
                // Git's fast-import will handle creating the objects.

                // Filter the stream to remove problematic reset commands
                let filtered_stream = filter_fast_import_stream(&stream_data);

                eprintln!("git-remote-gitwal: Filtered fast-import stream:");
                eprintln!("{}", String::from_utf8_lossy(&filtered_stream));

                // Output it to stdout (Git will read this as fast-import format)
                output.write_all(&filtered_stream)?;
                output.flush()?;
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
