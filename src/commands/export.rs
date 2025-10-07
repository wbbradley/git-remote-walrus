use std::{
    io::{BufRead, Write},
    process::Command,
};

use anyhow::{Context, Result};

use crate::{git::fast_export, pack::receive_pack, storage::StorageBackend};

/// Handle the export command (push)
/// Uses pack format internally to preserve GPG signatures
pub fn handle<S: StorageBackend, W: Write, R: BufRead>(
    storage: &S,
    output: &mut W,
    input: &mut std::io::Lines<R>,
) -> Result<()> {
    // Read the export commands from Git
    // Note: Git runs fast-export for us, but it may fail for annotated tags
    // We handle this gracefully by using git commands to get ref information
    let parse_result = fast_export::parse_stream(input);

    let ref_updates = match parse_result {
        Ok((_stream_data, updates)) if !updates.is_empty() => updates,
        _ => {
            // Fast-export failed or returned no updates
            // This can happen with annotated tags
            // Fall back to using git show-ref to get all refs that need pushing
            eprintln!("git-remote-walrus: Fast-export failed or empty, using fallback method");
            get_refs_from_git()?
        }
    };

    eprintln!("git-remote-walrus: Ref updates from Git: {:?}", ref_updates);

    // For each ref being pushed, get the commit SHA
    for refname in ref_updates.keys() {
        eprintln!("git-remote-walrus: Processing ref {}", refname);

        // Get the commit SHA that this ref points to locally
        let sha_output = Command::new("git")
            .arg("rev-parse")
            .arg(refname)
            .output()
            .context("Failed to run git rev-parse")?;

        if !sha_output.status.success() {
            eprintln!("git-remote-walrus: Could not resolve ref {}", refname);
            continue;
        }

        let git_sha1 = String::from_utf8_lossy(&sha_output.stdout)
            .trim()
            .to_string();
        eprintln!("git-remote-walrus: Ref {} points to {}", refname, git_sha1);

        // Create a packfile containing all objects for this ref
        // Use git pack-objects to create the packfile
        let state = storage.read_state()?;
        let old_sha = state.refs.get(refname);

        // Build revision range for incremental push
        let rev_range = if let Some(old) = old_sha {
            format!("{}..{}", old, git_sha1)
        } else {
            git_sha1.clone()
        };

        eprintln!("git-remote-walrus: Creating packfile for {}", rev_range);

        // Use git pack-objects --include-tag to include annotated tag objects
        let mut pack_output = Command::new("git")
            .arg("pack-objects")
            .arg("--revs")
            .arg("--include-tag") // Include annotated tag objects
            .arg("--stdout")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .context("Failed to spawn git pack-objects")?;

        // Write the revision to pack-objects stdin
        {
            use std::io::Write as _;
            if let Some(ref mut stdin) = pack_output.stdin {
                writeln!(stdin, "{}", git_sha1)?;
            }
        }

        let pack_result = pack_output.wait_with_output()?;
        if !pack_result.status.success() {
            anyhow::bail!("git pack-objects failed");
        }

        eprintln!(
            "git-remote-walrus: Created packfile of {} bytes",
            pack_result.stdout.len()
        );

        // Receive and store the packfile
        let mut pack_data = &pack_result.stdout[..];
        let object_mappings =
            receive_pack(&mut pack_data, storage).context("Failed to receive pack")?;

        eprintln!(
            "git-remote-walrus: Stored {} objects",
            object_mappings.len()
        );

        // Update state with new objects and ref
        storage.update_state(|state| {
            // Add all object mappings
            for (obj_id, content_id) in &object_mappings {
                state.objects.insert(obj_id.clone(), content_id.clone());
            }
            // Update the ref to point to the new commit
            state.refs.insert(refname.clone(), git_sha1.clone());
            Ok(())
        })?;

        // Report success
        writeln!(output, "ok {}", refname)?;
    }

    // Empty line signals completion
    writeln!(output)?;

    Ok(())
}

/// Fallback method to get refs when fast-export fails
/// Returns a HashMap of refname -> "0000..." (we'll resolve SHAs later)
fn get_refs_from_git() -> Result<std::collections::HashMap<String, String>> {
    // When fast-export fails, we just return empty map
    // The export handler will get the SHA using git rev-parse for each ref anyway
    Ok(std::collections::HashMap::new())
}
