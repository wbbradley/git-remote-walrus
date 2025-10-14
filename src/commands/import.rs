use std::{
    io::Write,
    process::{Command, Stdio},
};

use anyhow::{Context, Result};
use tempfile::TempDir;

use crate::{
    pack::objects::{write_loose_object, GitObject},
    storage::StorageBackend,
};

/// Handle the import command (fetch)
/// Reconstructs Git repo from pack objects and uses git fast-export
pub fn handle<S: StorageBackend, W: Write>(
    storage: &S,
    output: &mut W,
    refs: &[String],
) -> Result<()> {
    tracing::info!("Import requested for refs: {:?}", refs);

    let state = storage.read_state()?;

    // Create temporary git repository
    let temp_dir = TempDir::new().context("Failed to create temp directory")?;
    let git_dir = temp_dir.path().join("repo.git");
    std::fs::create_dir(&git_dir).context("Failed to create git dir")?;
    init_bare_repo(&git_dir)?;

    // Write all objects as loose objects to temp repo
    let objects_dir = git_dir.join("objects");
    for (obj_id, content_id) in &state.objects {
        let content = storage
            .read_object(content_id)
            .with_context(|| format!("Failed to read object {} from storage", obj_id))?;

        let obj = GitObject::from_loose_format(&content)
            .with_context(|| format!("Failed to parse object {}", obj_id))?;

        write_loose_object(&obj, &objects_dir)
            .with_context(|| format!("Failed to write loose object {}", obj_id))?;
    }

    // Update refs in temp repo
    for (ref_name, commit_id) in &state.refs {
        if refs.contains(ref_name) {
            let ref_path = git_dir.join(ref_name);
            std::fs::create_dir_all(ref_path.parent().unwrap())?;
            std::fs::write(&ref_path, format!("{}\n", commit_id))?;
            tracing::debug!("Created ref {} -> {}", ref_name, commit_id);
        }
    }

    // Use git fast-export to generate stream
    let fast_export = Command::new("git")
        .arg("--git-dir")
        .arg(&git_dir)
        .arg("fast-export")
        .arg("--all")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn git fast-export")?;

    let export_output = fast_export
        .wait_with_output()
        .context("Failed to wait for git fast-export")?;

    if !export_output.status.success() {
        tracing::error!(
            "git fast-export stderr: {}",
            String::from_utf8_lossy(&export_output.stderr)
        );
        anyhow::bail!("git fast-export failed");
    }

    // Write fast-export stream to output
    output.write_all(&export_output.stdout)?;

    // Signal completion
    writeln!(output, "done")?;
    writeln!(output)?;

    Ok(())
}

/// Initialize minimal bare repository structure
fn init_bare_repo(git_dir: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(git_dir.join("objects")).context("Failed to create objects dir")?;
    std::fs::create_dir_all(git_dir.join("refs")).context("Failed to create refs dir")?;

    std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n")
        .context("Failed to write HEAD")?;

    Ok(())
}
