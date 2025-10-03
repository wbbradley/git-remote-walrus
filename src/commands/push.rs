//! Handle push command (replaces export)

use anyhow::{Context, Result};
use std::io::{BufRead, Write};

use crate::pack::receive_pack;
use crate::storage::StorageBackend;

/// Handle push command - receive packfile and update refs
pub fn handle<S: StorageBackend, W: Write, R: BufRead>(
    storage: &S,
    output: &mut W,
    lines: &mut std::io::Lines<R>,
) -> Result<()> {
    // Read push commands until empty line
    let mut ref_updates = Vec::new();

    while let Some(line) = lines.next() {
        let line = line?;
        let line = line.trim();

        eprintln!("Push line: {}", line);

        if line.is_empty() {
            break;
        }

        // Parse push command: "push <src>:<dst>"
        if let Some(push_spec) = line.strip_prefix("push ") {
            let parts: Vec<&str> = push_spec.split(':').collect();
            if parts.len() == 2 {
                let src = parts[0].to_string();
                let dst = parts[1].to_string();
                eprintln!("Parsed ref update: {} -> {}", src, dst);
                ref_updates.push((src, dst));
            }
        }
    }

    if ref_updates.is_empty() {
        eprintln!("No refs to push");
        writeln!(output)?;
        return Ok(());
    }

    // Receive packfile from stdin
    eprintln!("Receiving packfile...");
    let mut stdin = std::io::stdin();
    let object_mappings = receive_pack(&mut stdin, storage)
        .context("Failed to receive pack")?;

    eprintln!("Stored {} objects", object_mappings.len());

    // Update state with new objects and refs
    storage.update_state(|state| {
        // Add object mappings
        for (obj_id, content_id) in &object_mappings {
            state.objects.insert(obj_id.clone(), content_id.clone());
        }

        // Update refs
        for (_src, dst) in &ref_updates {
            // src is the local ref (e.g., "refs/heads/main")
            // dst is the remote ref (e.g., "refs/heads/main")
            // We need to get the SHA from the source

            // For now, find the commit SHA from the pushed objects
            // In a real implementation, Git sends the old/new SHAs
            if let Some((obj_id, _)) = object_mappings.first() {
                state.refs.insert(dst.clone(), obj_id.clone());
                eprintln!("Updated ref {} to {}", dst, obj_id);
            }
        }

        Ok(())
    })?;

    // Report success for each ref
    for (_, dst) in &ref_updates {
        writeln!(output, "ok {}", dst)?;
    }

    writeln!(output)?; // Empty line signals completion
    eprintln!("Push completed");

    Ok(())
}
