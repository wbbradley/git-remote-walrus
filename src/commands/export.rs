use anyhow::{Context, Result};
use std::io::{BufRead, Write};
use std::process::Command;

use crate::git::fast_export;
use crate::storage::StorageBackend;

/// Handle the export command (push)
/// Reads fast-export stream from stdin, stores it, and updates refs
pub fn handle<S: StorageBackend, W: Write, R: BufRead>(
    storage: &S,
    output: &mut W,
    input: &mut std::io::Lines<R>,
) -> Result<()> {
    // Read the commands from Git
    let (_stream_data, ref_updates) = fast_export::parse_stream(input)?;

    eprintln!("git-remote-gitwal: Extracted ref updates from Git: {:?}", ref_updates);

    // Git doesn't send us the actual commit data in the fast-export stream for remote helpers.
    // Instead, it just tells us which refs to update. We need to run git fast-export ourselves.
    for (refname, _git_sha1) in &ref_updates {
        eprintln!("git-remote-gitwal: Processing ref {}", refname);

        // Get the actual commit SHA that this ref points to locally
        let sha_output = Command::new("git")
            .arg("rev-parse")
            .arg(&refname)
            .output()
            .context("Failed to run git rev-parse")?;

        if !sha_output.status.success() {
            eprintln!("git-remote-gitwal: Could not resolve ref {}", refname);
            continue;
        }

        let git_sha1 = String::from_utf8_lossy(&sha_output.stdout).trim().to_string();
        eprintln!("git-remote-gitwal: Ref {} points to {}", refname, git_sha1);

        // Get the current ref value from storage to determine what we already have
        let state = storage.read_state()?;
        let old_sha = state.refs.get(refname).cloned();

        // Build the git fast-export command
        let export_arg = if let Some(old) = &old_sha {
            // Incremental: export commits from old..new
            format!("{}..{}", old, git_sha1)
        } else {
            // Full export: export the entire ref
            git_sha1.clone()
        };

        eprintln!("git-remote-gitwal: Running git fast-export {}", export_arg);

        // Run git fast-export to get the actual commit data
        // Use --signed-tags=verbatim to preserve GPG signatures on both tags and commits
        let output_result = Command::new("git")
            .arg("fast-export")
            .arg("--all")
            .arg("--signed-tags=verbatim")
            .arg("--tag-of-filtered-object=drop")
            .output()
            .context("Failed to run git fast-export")?;

        if !output_result.status.success() {
            anyhow::bail!(
                "git fast-export failed: {}",
                String::from_utf8_lossy(&output_result.stderr)
            );
        }

        let export_data = output_result.stdout;
        eprintln!("git-remote-gitwal: Exported {} bytes", export_data.len());

        // Store the export data
        let content_id = storage.write_object(&export_data)?;
        eprintln!("git-remote-gitwal: Stored export data as {}", content_id);

        // Update state
        storage.update_state(|state| {
            state.refs.insert(refname.clone(), git_sha1.clone());
            state.objects.insert(git_sha1.clone(), content_id.clone());
            Ok(())
        })?;

        // Report success
        writeln!(output, "ok {}", refname)?;
    }

    // Empty line signals completion
    writeln!(output)?;

    Ok(())
}
