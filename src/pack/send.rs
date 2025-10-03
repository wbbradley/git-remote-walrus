//! Send pack files during fetch operations

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tempfile::TempDir;

use crate::storage::{State, StorageBackend};

use super::objects::{write_loose_object, GitObject, ObjectId};

/// Send a packfile to stdout for the requested refs
///
/// Flow:
/// 1. Determine which objects are needed (from wanted refs)
/// 2. Retrieve objects from storage
/// 3. Write objects as loose files to temporary git repo
/// 4. Use `git pack-objects` to create packfile
/// 5. Stream packfile to stdout
pub fn send_pack<W: Write>(
    wanted_refs: &[String],
    storage: &impl StorageBackend,
    output: &mut W,
) -> Result<()> {
    let state = storage.read_state()?;

    // Collect object IDs for all wanted refs
    let wanted_objects = collect_wanted_objects(&wanted_refs, &state)?;
    eprintln!("Need to send {} objects", wanted_objects.len());

    if wanted_objects.is_empty() {
        eprintln!("No objects to send");
        return Ok(());
    }

    // Create temporary git repository
    let temp_dir = TempDir::new().context("Failed to create temp directory")?;
    let git_dir = temp_dir.path().join("repo.git");
    std::fs::create_dir(&git_dir).context("Failed to create git dir")?;
    init_bare_repo(&git_dir)?;

    // Retrieve objects from storage and write as loose objects
    let objects_dir = git_dir.join("objects");
    for obj_id in &wanted_objects {
        // Get storage content ID from state
        let content_id = state
            .objects
            .get(obj_id)
            .with_context(|| format!("Object {} not found in state", obj_id))?;

        // Read from storage
        let content = storage
            .read_object(content_id)
            .with_context(|| format!("Failed to read object {} from storage", obj_id))?;

        // Parse and write as loose object
        let obj = GitObject::from_loose_format(&content)
            .with_context(|| format!("Failed to parse object {}", obj_id))?;

        write_loose_object(&obj, &objects_dir)
            .with_context(|| format!("Failed to write loose object {}", obj_id))?;

        eprintln!("Wrote object {} to temp repo", obj_id);
    }

    // Create packfile using git pack-objects
    create_packfile(&git_dir, &wanted_objects, output)?;

    Ok(())
}

/// Collect all objects reachable from wanted refs
fn collect_wanted_objects(wanted_refs: &[String], state: &State) -> Result<Vec<ObjectId>> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();

    for ref_name in wanted_refs {
        if let Some(commit_id) = state.refs.get(ref_name) {
            // For now, we'll do a simple approach: collect all objects in state
            // TODO: Implement proper graph traversal
            if seen.insert(commit_id.clone()) {
                result.push(commit_id.clone());
            }
        }
    }

    // For now, return all objects in state (simplification)
    // TODO: Implement proper reachability analysis
    for obj_id in state.objects.keys() {
        if seen.insert(obj_id.clone()) {
            result.push(obj_id.clone());
        }
    }

    Ok(result)
}

/// Initialize minimal bare repository structure
fn init_bare_repo(git_dir: &PathBuf) -> Result<()> {
    std::fs::create_dir_all(git_dir.join("objects"))
        .context("Failed to create objects dir")?;
    std::fs::create_dir_all(git_dir.join("refs"))
        .context("Failed to create refs dir")?;

    std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n")
        .context("Failed to write HEAD")?;

    Ok(())
}

/// Create packfile from loose objects using git pack-objects
fn create_packfile<W: Write>(
    git_dir: &PathBuf,
    object_ids: &[ObjectId],
    output: &mut W,
) -> Result<()> {
    // git pack-objects reads object IDs from stdin, one per line
    let mut pack_objects = Command::new("git")
        .arg("--git-dir")
        .arg(git_dir)
        .arg("pack-objects")
        .arg("--stdout")
        .arg("--revs")
        .arg("--thin")
        .arg("--delta-base-offset")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("Failed to spawn git pack-objects")?;

    // Write object IDs to stdin
    {
        let stdin = pack_objects.stdin.as_mut().unwrap();
        for obj_id in object_ids {
            writeln!(stdin, "{}", obj_id).context("Failed to write object ID to pack-objects")?;
        }
    }

    // Read packfile from stdout and write to output
    let mut pack_stdout = pack_objects.stdout.take().unwrap();
    std::io::copy(&mut pack_stdout, output).context("Failed to copy packfile to output")?;

    let status = pack_objects
        .wait()
        .context("Failed to wait for git pack-objects")?;

    if !status.success() {
        anyhow::bail!("git pack-objects failed with status: {}", status);
    }

    eprintln!("Packfile created successfully");
    Ok(())
}
