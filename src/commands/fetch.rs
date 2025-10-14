//! Handle fetch command - write objects to .git/objects (no fast-export)

use std::{
    io::Write,
    process::{Command, Stdio},
};

use anyhow::{Context, Result};

use crate::{pack::send_pack, storage::StorageBackend};

/// Handle fetch command - write objects to .git/objects for requested refs
/// This replaces the old import handler and eliminates fast-export
///
/// The fetch capability requires us to write objects to .git/objects, not to stdout.
/// We do this by creating a packfile and piping it to `git index-pack --stdin`.
pub fn handle<S: StorageBackend, W: Write>(
    storage: &S,
    output: &mut W,
    refs: &[String],
) -> Result<()> {
    tracing::info!("Fetch requested for refs: {:?}", refs);

    // Create packfile in memory
    let mut packfile = Vec::new();
    send_pack(refs, storage, &mut packfile)?;

    // Write packfile to .git/objects using git index-pack
    let git_dir = std::env::var("GIT_DIR").unwrap_or_else(|_| ".git".to_string());

    let mut index_pack = Command::new("git")
        .arg("--git-dir")
        .arg(&git_dir)
        .arg("index-pack")
        .arg("--stdin")
        .arg("--fix-thin")
        .arg("-v")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn git index-pack")?;

    // Write packfile to stdin
    index_pack
        .stdin
        .as_mut()
        .unwrap()
        .write_all(&packfile)
        .context("Failed to write packfile to git index-pack")?;
    drop(index_pack.stdin.take());

    // Wait for git index-pack to complete
    let result = index_pack
        .wait_with_output()
        .context("Failed to wait for git index-pack")?;

    if !result.status.success() {
        tracing::error!(
            "git index-pack stderr: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        anyhow::bail!(
            "git index-pack failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    tracing::debug!(
        "git index-pack output: {}",
        String::from_utf8_lossy(&result.stdout)
    );
    tracing::debug!(
        "git index-pack stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );

    // Output blank line to signal completion
    writeln!(output)?;
    output.flush()?;

    tracing::info!("Fetch completed");
    Ok(())
}
