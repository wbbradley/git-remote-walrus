//! Receive pack files during push operations

use std::{
    io::{Read, Write},
    process::{Command, Stdio},
};

use anyhow::{Context, Result};
use tempfile::TempDir;

use super::objects::{read_loose_object, GitObject, ObjectId};
use crate::storage::{ContentId, StorageBackend};

/// Receive a packfile from stdin, unpack it, and store objects in the backend
///
/// Flow:
/// 1. Receive packfile from stdin
/// 2. Use `git index-pack` to unpack to temporary location
/// 3. Read unpacked loose objects
/// 4. Store each object in immutable storage
/// 5. Return mapping of object IDs to storage content IDs
pub fn receive_pack<R: Read>(
    pack_stream: &mut R,
    storage: &impl StorageBackend,
) -> Result<Vec<(ObjectId, ContentId)>> {
    // Create temporary directory for unpacking
    let temp_dir = TempDir::new().context("Failed to create temp directory")?;
    let git_dir = temp_dir.path().join("repo.git");
    std::fs::create_dir(&git_dir).context("Failed to create git dir")?;

    // Initialize bare git repo structure
    init_bare_repo(&git_dir)?;

    // Read packfile into memory (alternative: use pipe/fifo)
    let mut pack_data = Vec::new();
    pack_stream
        .read_to_end(&mut pack_data)
        .context("Failed to read packfile from stdin")?;

    eprintln!("Received pack of {} bytes", pack_data.len());

    // Unpack using git unpack-objects (creates loose objects, not a pack)
    let mut unpack = Command::new("git")
        .arg("--git-dir")
        .arg(&git_dir)
        .arg("unpack-objects")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped()) // Capture stdout
        .stderr(Stdio::piped()) // Capture stderr
        .spawn()
        .context("Failed to spawn git unpack-objects")?;

    // Write pack data to git unpack-objects stdin
    unpack
        .stdin
        .as_mut()
        .unwrap()
        .write_all(&pack_data)
        .context("Failed to write pack to git unpack-objects")?;

    let output = unpack
        .wait_with_output()
        .context("Failed to wait for git unpack-objects")?;

    if !output.status.success() {
        eprintln!(
            "git unpack-objects stdout: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        eprintln!(
            "git unpack-objects stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        anyhow::bail!("git unpack-objects failed with status: {}", output.status);
    }

    // Log the unpack-objects output to stderr
    eprintln!(
        "git unpack-objects: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Collect all unpacked objects from .git/objects
    let objects = collect_loose_objects(&git_dir)?;
    eprintln!("Unpacked {} objects", objects.len());

    // Store each object in immutable storage
    let mut mappings = Vec::new();
    for obj in objects {
        let content = obj.to_loose_format();
        let content_id = storage
            .write_object(&content)
            .with_context(|| format!("Failed to store object {}", obj.id))?;

        eprintln!("Stored object {} -> {}", obj.id, content_id);
        mappings.push((obj.id, content_id));
    }

    Ok(mappings)
}

/// Initialize minimal bare repository structure
fn init_bare_repo(git_dir: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(git_dir.join("objects")).context("Failed to create objects dir")?;
    std::fs::create_dir_all(git_dir.join("refs")).context("Failed to create refs dir")?;

    // Write minimal HEAD
    std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n")
        .context("Failed to write HEAD")?;

    Ok(())
}

/// Collect all loose objects from a git objects directory
fn collect_loose_objects(git_dir: &std::path::Path) -> Result<Vec<GitObject>> {
    let objects_dir = git_dir.join("objects");
    let mut objects = Vec::new();

    // Iterate over 2-char subdirectories (00..ff)
    for entry in std::fs::read_dir(&objects_dir)
        .with_context(|| format!("Failed to read objects dir: {}", objects_dir.display()))?
    {
        let entry = entry.context("Failed to read directory entry")?;
        let path = entry.path();

        // Skip pack and info directories
        if !path.is_dir()
            || path.file_name().unwrap() == "pack"
            || path.file_name().unwrap() == "info"
        {
            continue;
        }

        let dir_name = path.file_name().unwrap().to_str().unwrap();
        if dir_name.len() != 2 {
            continue;
        }

        // Read objects in this subdirectory
        for obj_entry in std::fs::read_dir(&path)
            .with_context(|| format!("Failed to read object subdir: {}", path.display()))?
        {
            let obj_entry = obj_entry.context("Failed to read object entry")?;
            let obj_path = obj_entry.path();

            if !obj_path.is_file() {
                continue;
            }

            // Read the loose object
            match read_loose_object(&obj_path) {
                Ok(obj) => objects.push(obj),
                Err(e) => {
                    eprintln!(
                        "Warning: Failed to read object {}: {}",
                        obj_path.display(),
                        e
                    );
                }
            }
        }
    }

    Ok(objects)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_bare_repo() {
        let temp = TempDir::new().unwrap();
        let git_dir = temp.path().join("test.git");
        init_bare_repo(&git_dir).unwrap();

        assert!(git_dir.join("objects").exists());
        assert!(git_dir.join("refs").exists());
        assert!(git_dir.join("HEAD").exists());
    }
}
