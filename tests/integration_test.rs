//! End-to-end integration tests for git-remote-walrus
//!
//! These tests require the git-remote-walrus binary to be built.
//! Run with: cargo test --release

use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Once,
};

use tempfile::TempDir;

static INIT: Once = Once::new();

/// Setup git-remote-walrus in PATH for tests
/// This ensures Git can find our custom remote helper
fn setup_git_remote() {
    INIT.call_once(|| {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let binary_path = PathBuf::from(manifest_dir).join("target/release");

        // Verify the binary exists
        let binary = binary_path.join("git-remote-walrus");
        if !binary.exists() {
            panic!(
                "git-remote-walrus binary not found at: {}\n\
                 Please build it first with: cargo build --release",
                binary.display()
            );
        }

        // Add our binary directory to PATH
        let current_path = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{}", binary_path.display(), current_path);
        std::env::set_var("PATH", new_path);

        eprintln!("✓ git-remote-walrus added to PATH for testing");
    });
}

/// Helper to run git commands in a directory
fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("failed to execute git");

    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn test_basic_push_clone() {
    setup_git_remote();

    let temp = TempDir::new().unwrap();
    let test_repo = temp.path().join("test-repo");
    let storage = temp.path().join("storage");
    let cloned_repo = temp.path().join("cloned");

    // Create test repository
    std::fs::create_dir(&test_repo).unwrap();
    git(&test_repo, &["init"]);
    git(&test_repo, &["config", "user.name", "Test"]);
    git(&test_repo, &["config", "user.email", "test@test.com"]);

    // Create first commit
    std::fs::write(test_repo.join("file1.txt"), "Hello World").unwrap();
    git(&test_repo, &["add", "file1.txt"]);
    git(&test_repo, &["commit", "-m", "First commit"]);

    // Create second commit
    std::fs::write(test_repo.join("file2.txt"), "Second file").unwrap();
    git(&test_repo, &["add", "file2.txt"]);
    git(&test_repo, &["commit", "-m", "Second commit"]);

    let orig_sha = git(&test_repo, &["rev-parse", "HEAD"]);

    // Push to walrus
    let storage_url = format!("walrus::{}", storage.display());
    git(&test_repo, &["push", &storage_url, "main"]);

    // Clone from walrus
    git(
        temp.path(),
        &["clone", &storage_url, cloned_repo.to_str().unwrap()],
    );

    // Verify SHAs match
    let cloned_sha = git(&cloned_repo, &["rev-parse", "HEAD"]);
    assert_eq!(orig_sha, cloned_sha, "SHA preservation failed");

    // Verify file contents
    let content1 = std::fs::read_to_string(cloned_repo.join("file1.txt")).unwrap();
    let content2 = std::fs::read_to_string(cloned_repo.join("file2.txt")).unwrap();
    assert_eq!(content1, "Hello World");
    assert_eq!(content2, "Second file");
}

#[test]
fn test_multiple_branches() {
    setup_git_remote();

    let temp = TempDir::new().unwrap();
    let test_repo = temp.path().join("test-repo");
    let storage = temp.path().join("storage");
    let cloned_repo = temp.path().join("cloned");

    // Create test repository
    std::fs::create_dir(&test_repo).unwrap();
    git(&test_repo, &["init"]);
    git(&test_repo, &["config", "user.name", "Test"]);
    git(&test_repo, &["config", "user.email", "test@test.com"]);

    // Create main branch commit
    std::fs::write(test_repo.join("main.txt"), "main").unwrap();
    git(&test_repo, &["add", "main.txt"]);
    git(&test_repo, &["commit", "-m", "Main commit"]);
    let main_sha = git(&test_repo, &["rev-parse", "HEAD"]);

    // Create feature branch
    git(&test_repo, &["checkout", "-b", "feature"]);
    std::fs::write(test_repo.join("feature.txt"), "feature").unwrap();
    git(&test_repo, &["add", "feature.txt"]);
    git(&test_repo, &["commit", "-m", "Feature commit"]);
    let feature_sha = git(&test_repo, &["rev-parse", "HEAD"]);

    // Push all branches
    let storage_url = format!("walrus::{}", storage.display());
    git(&test_repo, &["push", &storage_url, "--all"]);

    // Clone and verify
    git(
        temp.path(),
        &["clone", &storage_url, cloned_repo.to_str().unwrap()],
    );

    let cloned_main_sha = git(&cloned_repo, &["rev-parse", "origin/main"]);
    let cloned_feature_sha = git(&cloned_repo, &["rev-parse", "origin/feature"]);

    assert_eq!(main_sha, cloned_main_sha);
    assert_eq!(feature_sha, cloned_feature_sha);
}

#[test]
fn test_binary_files() {
    setup_git_remote();

    let temp = TempDir::new().unwrap();
    let test_repo = temp.path().join("test-repo");
    let storage = temp.path().join("storage");
    let cloned_repo = temp.path().join("cloned");

    // Create test repository
    std::fs::create_dir(&test_repo).unwrap();
    git(&test_repo, &["init"]);
    git(&test_repo, &["config", "user.name", "Test"]);
    git(&test_repo, &["config", "user.email", "test@test.com"]);

    // Create binary file
    let binary_data: Vec<u8> = (0..256).map(|i| i as u8).collect();
    std::fs::write(test_repo.join("binary.dat"), &binary_data).unwrap();
    git(&test_repo, &["add", "binary.dat"]);
    git(&test_repo, &["commit", "-m", "Add binary"]);

    // Push and clone
    let storage_url = format!("walrus::{}", storage.display());
    git(&test_repo, &["push", &storage_url, "main"]);
    git(
        temp.path(),
        &["clone", &storage_url, cloned_repo.to_str().unwrap()],
    );

    // Verify binary file
    let cloned_data = std::fs::read(cloned_repo.join("binary.dat")).unwrap();
    assert_eq!(binary_data, cloned_data);
}

#[test]
fn test_lightweight_tags() {
    setup_git_remote();

    let temp = TempDir::new().unwrap();
    let test_repo = temp.path().join("test-repo");
    let storage = temp.path().join("storage");
    let cloned_repo = temp.path().join("cloned");

    // Create test repository
    std::fs::create_dir(&test_repo).unwrap();
    git(&test_repo, &["init"]);
    git(&test_repo, &["config", "user.name", "Test"]);
    git(&test_repo, &["config", "user.email", "test@test.com"]);

    // Create commit and tag
    std::fs::write(test_repo.join("file.txt"), "content").unwrap();
    git(&test_repo, &["add", "file.txt"]);
    git(&test_repo, &["commit", "-m", "Commit"]);
    git(&test_repo, &["tag", "v1.0.0"]);

    let commit_sha = git(&test_repo, &["rev-parse", "HEAD"]);

    // Push tag
    let storage_url = format!("walrus::{}", storage.display());
    git(&test_repo, &["push", &storage_url, "main"]);
    git(
        &test_repo,
        &["push", &storage_url, "v1.0.0:refs/tags/v1.0.0"],
    );

    // Clone and verify tag
    git(
        temp.path(),
        &["clone", &storage_url, cloned_repo.to_str().unwrap()],
    );

    let tag_sha = git(&cloned_repo, &["rev-parse", "v1.0.0"]);
    assert_eq!(commit_sha, tag_sha);
}

#[test]
fn test_incremental_push() {
    setup_git_remote();

    let temp = TempDir::new().unwrap();
    let test_repo = temp.path().join("test-repo");
    let storage = temp.path().join("storage");

    // Create test repository
    std::fs::create_dir(&test_repo).unwrap();
    git(&test_repo, &["init"]);
    git(&test_repo, &["config", "user.name", "Test"]);
    git(&test_repo, &["config", "user.email", "test@test.com"]);

    // First push
    std::fs::write(test_repo.join("file1.txt"), "First").unwrap();
    git(&test_repo, &["add", "file1.txt"]);
    git(&test_repo, &["commit", "-m", "First"]);

    let storage_url = format!("walrus::{}", storage.display());
    git(&test_repo, &["push", &storage_url, "main"]);

    // Second push
    std::fs::write(test_repo.join("file2.txt"), "Second").unwrap();
    git(&test_repo, &["add", "file2.txt"]);
    git(&test_repo, &["commit", "-m", "Second"]);
    let second_sha = git(&test_repo, &["rev-parse", "HEAD"]);

    git(&test_repo, &["push", &storage_url, "main"]);

    // Clone and verify second commit is present
    let cloned_repo = temp.path().join("cloned");
    git(
        temp.path(),
        &["clone", &storage_url, cloned_repo.to_str().unwrap()],
    );

    let cloned_sha = git(&cloned_repo, &["rev-parse", "HEAD"]);
    assert_eq!(second_sha, cloned_sha);

    // Verify both files exist
    assert!(cloned_repo.join("file1.txt").exists());
    assert!(cloned_repo.join("file2.txt").exists());
}

#[test]
fn test_object_deduplication_across_pushes() {
    setup_git_remote();

    let temp = TempDir::new().unwrap();
    let test_repo = temp.path().join("test-repo");
    let storage = temp.path().join("storage");

    // Create test repository
    std::fs::create_dir(&test_repo).unwrap();
    git(&test_repo, &["init"]);
    git(&test_repo, &["config", "user.name", "Test"]);
    git(&test_repo, &["config", "user.email", "test@test.com"]);

    // Step 1: Create first commit with single file
    std::fs::write(test_repo.join("file1.txt"), "line 1\n").unwrap();
    git(&test_repo, &["add", "file1.txt"]);
    git(&test_repo, &["commit", "-m", "First commit"]);

    // Step 2: Push to walrus
    let storage_url = format!("walrus::{}", storage.display());
    git(&test_repo, &["push", &storage_url, "main"]);

    // Step 3: Count objects in state.yaml
    let state_file = storage.join("state.yaml");
    let state_content = std::fs::read_to_string(&state_file).unwrap();
    let state: serde_yaml::Value = serde_yaml::from_str(&state_content).unwrap();
    let objects_after_first_push = state["objects"].as_mapping().map(|m| m.len()).unwrap_or(0);

    eprintln!("Objects after first push: {}", objects_after_first_push);
    eprintln!("State after first push:\n{}", state_content);

    // Step 4: Create second commit with new file
    std::fs::write(test_repo.join("file2.txt"), "line 2\n").unwrap();
    git(&test_repo, &["add", "file2.txt"]);
    git(&test_repo, &["commit", "-m", "Second commit"]);

    // Step 5: Push second commit to walrus
    git(&test_repo, &["push", &storage_url, "main"]);

    // Step 6: Count objects again and investigate
    let state_content = std::fs::read_to_string(&state_file).unwrap();
    let state: serde_yaml::Value = serde_yaml::from_str(&state_content).unwrap();
    let objects_after_second_push = state["objects"].as_mapping().map(|m| m.len()).unwrap_or(0);

    eprintln!("\nObjects after second push: {}", objects_after_second_push);
    eprintln!("State after second push:\n{}", state_content);

    // Expected objects for first commit: 1 blob (file1.txt) + 1 tree + 1 commit = 3
    // Expected objects for second commit: 1 blob (file2.txt) + 1 tree + 1 commit = 3
    // Total unique objects: 6
    // But if there's duplication, we might see more

    let expected_new_objects = 3; // 1 new blob + 1 new tree + 1 new commit
    let actual_new_objects = objects_after_second_push - objects_after_first_push;

    eprintln!("\nExpected new objects: {}", expected_new_objects);
    eprintln!("Actual new objects: {}", actual_new_objects);

    // Get all object keys to check for duplicates
    if let Some(objects) = state["objects"].as_mapping() {
        let mut git_shas: Vec<String> = objects
            .keys()
            .map(|k| k.as_str().unwrap().to_string())
            .collect();
        git_shas.sort();
        eprintln!("\nAll git SHAs in state:");
        for sha in &git_shas {
            eprintln!("  {}", sha);
        }

        // Check for duplicate git SHAs (shouldn't happen)
        let unique_count = git_shas
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len();
        assert_eq!(
            unique_count,
            git_shas.len(),
            "Found duplicate git SHAs in state.objects!"
        );

        // Check content IDs for duplicate content
        let mut content_ids: Vec<String> = objects
            .values()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        content_ids.sort();
        eprintln!("\nAll content IDs:");
        for id in &content_ids {
            eprintln!("  {}", id);
        }

        // Count duplicate content IDs (this is fine - same content, different git objects)
        let unique_content_ids = content_ids
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len();
        eprintln!("\nUnique content IDs: {}", unique_content_ids);
        eprintln!("Total git SHAs: {}", git_shas.len());
    }

    // The key assertion: we should only add ~3 new objects (blob, tree, commit)
    // If we're seeing significantly more, that's the bug
    assert!(
        actual_new_objects <= expected_new_objects + 1, // +1 for tolerance
        "Too many new objects added! Expected ~{}, got {}. This suggests duplicate objects are being stored.",
        expected_new_objects,
        actual_new_objects
    );
}

#[test]
fn test_many_objects_then_small_push() {
    setup_git_remote();

    let temp = TempDir::new().unwrap();
    let test_repo = temp.path().join("test-repo");
    let storage = temp.path().join("storage");

    // Create test repository
    std::fs::create_dir(&test_repo).unwrap();
    git(&test_repo, &["init"]);
    git(&test_repo, &["config", "user.name", "Test"]);
    git(&test_repo, &["config", "user.email", "test@test.com"]);

    // Create a bunch of commits (simulating "a repo with many objects")
    for i in 0..10 {
        std::fs::write(
            test_repo.join(format!("file{}.txt", i)),
            format!("content {}\n", i),
        )
        .unwrap();
        git(&test_repo, &["add", &format!("file{}.txt", i)]);
        git(&test_repo, &["commit", "-m", &format!("Commit {}", i)]);
    }

    // Push all to walrus
    let storage_url = format!("walrus::{}", storage.display());
    git(&test_repo, &["push", &storage_url, "main"]);

    // Count objects after initial push
    let state_file = storage.join("state.yaml");
    let state_content = std::fs::read_to_string(&state_file).unwrap();
    let state: serde_yaml::Value = serde_yaml::from_str(&state_content).unwrap();
    let objects_after_initial = state["objects"].as_mapping().map(|m| m.len()).unwrap_or(0);

    eprintln!(
        "Objects after pushing 10 commits: {}",
        objects_after_initial
    );

    // Now make a small change - just append to one file
    std::fs::write(test_repo.join("file0.txt"), "content 0\nappended\n").unwrap();
    git(&test_repo, &["add", "file0.txt"]);
    git(&test_repo, &["commit", "-m", "Small change"]);

    // Push the small change
    git(&test_repo, &["push", &storage_url, "main"]);

    // Count objects after small push
    let state_content = std::fs::read_to_string(&state_file).unwrap();
    let state: serde_yaml::Value = serde_yaml::from_str(&state_content).unwrap();
    let objects_after_small_push = state["objects"].as_mapping().map(|m| m.len()).unwrap_or(0);

    eprintln!("Objects after small push: {}", objects_after_small_push);

    let new_objects = objects_after_small_push - objects_after_initial;
    eprintln!("New objects from small push: {}", new_objects);

    // For a small change, we expect: 1 new blob + 1 new tree + 1 new commit = 3
    // If we see a lot more, that's the bug
    assert!(
        new_objects <= 4, // 3 + tolerance
        "Small push added {} objects, expected ~3. This suggests objects are being re-sent.",
        new_objects
    );

    // Now let's inspect what's actually stored
    eprintln!("\n--- Investigating storage efficiency ---");

    // Check state.yaml file size
    let state_yaml_size = std::fs::metadata(&state_file).unwrap().len();
    eprintln!("state.yaml file size: {} bytes", state_yaml_size);
    eprintln!("Number of objects: {}", objects_after_small_push);
    eprintln!(
        "Average bytes per object in state.yaml: {}",
        state_yaml_size / objects_after_small_push as u64
    );

    // Calculate total object content size
    let mut total_content_size = 0u64;
    let mut total_data_size = 0u64;
    let mut total_header_overhead = 0u64;

    if let Some(objects) = state["objects"].as_mapping() {
        for (git_sha, content_id) in objects.iter().take(3) {
            let git_sha = git_sha.as_str().unwrap();
            let content_id = content_id.as_str().unwrap();

            // Read the actual stored content
            let object_path = storage.join("objects").join(content_id);
            if let Ok(stored_content) = std::fs::read(&object_path) {
                eprintln!("\nGit SHA-1: {}", git_sha);
                eprintln!("Content ID (SHA-256): {}", content_id);
                eprintln!("Stored size: {} bytes", stored_content.len());

                // Parse the header
                if let Some(null_pos) = stored_content.iter().position(|&b| b == 0) {
                    let header = String::from_utf8_lossy(&stored_content[..null_pos]);
                    let data_size = stored_content.len() - null_pos - 1;
                    let header_overhead = null_pos + 1;
                    eprintln!("Header: {:?}", header);
                    eprintln!("Data size: {} bytes", data_size);
                    eprintln!(
                        "Header overhead: {} bytes ({}%)",
                        header_overhead,
                        header_overhead * 100 / stored_content.len()
                    );

                    total_content_size += stored_content.len() as u64;
                    total_data_size += data_size as u64;
                    total_header_overhead += header_overhead as u64;
                }
            }
        }

        eprintln!("\n--- Storage breakdown (sampled from 3 objects) ---");
        eprintln!("Total stored: {} bytes", total_content_size);
        eprintln!("Actual data: {} bytes", total_data_size);
        eprintln!(
            "Header overhead: {} bytes ({}%)",
            total_header_overhead,
            if total_content_size > 0 {
                total_header_overhead * 100 / total_content_size
            } else {
                0
            }
        );

        eprintln!("\n--- Redundancy analysis ---");
        eprintln!("Git SHA-1 is stored in state.yaml: 40 hex chars");
        eprintln!("Git SHA-1 is computable from content: YES (redundant metadata)");
        eprintln!("SHA-256 content ID is stored in state.yaml: 64 hex chars");
        eprintln!("SHA-256 is computable from content: YES (but needed for content-addressing)");
        eprintln!("Header (type + size) is stored in each object: ~10 bytes");
        eprintln!("Header is redundant: Partially (size is computable, type could be in metadata)");
    }
}

#[test]
fn test_clone_modify_push_cycle() {
    setup_git_remote();

    let temp = TempDir::new().unwrap();
    let original_repo = temp.path().join("original");
    let storage = temp.path().join("storage");
    let cloned_repo = temp.path().join("cloned");

    // Create original repo with many objects
    std::fs::create_dir(&original_repo).unwrap();
    git(&original_repo, &["init"]);
    git(&original_repo, &["config", "user.name", "Test"]);
    git(&original_repo, &["config", "user.email", "test@test.com"]);

    for i in 0..10 {
        std::fs::write(
            original_repo.join(format!("file{}.txt", i)),
            format!("content {}\n", i),
        )
        .unwrap();
        git(&original_repo, &["add", &format!("file{}.txt", i)]);
        git(&original_repo, &["commit", "-m", &format!("Commit {}", i)]);
    }

    // Push to walrus
    let storage_url = format!("walrus::{}", storage.display());
    git(&original_repo, &["push", &storage_url, "main"]);

    let state_file = storage.join("state.yaml");
    let state_content = std::fs::read_to_string(&state_file).unwrap();
    let state: serde_yaml::Value = serde_yaml::from_str(&state_content).unwrap();
    let objects_after_initial = state["objects"].as_mapping().map(|m| m.len()).unwrap_or(0);
    eprintln!("Objects after initial push: {}", objects_after_initial);

    // Clone from walrus
    git(
        temp.path(),
        &["clone", &storage_url, cloned_repo.to_str().unwrap()],
    );

    // Make a small change in cloned repo
    std::fs::write(cloned_repo.join("file0.txt"), "content 0\nmodified\n").unwrap();
    git(&cloned_repo, &["add", "file0.txt"]);
    git(&cloned_repo, &["commit", "-m", "Small change"]);

    // Push from cloned repo back to walrus
    git(&cloned_repo, &["push", "origin", "main"]);

    // Check object count
    let state_content = std::fs::read_to_string(&state_file).unwrap();
    let state: serde_yaml::Value = serde_yaml::from_str(&state_content).unwrap();
    let objects_after_push = state["objects"].as_mapping().map(|m| m.len()).unwrap_or(0);

    eprintln!("Objects after clone-modify-push: {}", objects_after_push);
    let new_objects = objects_after_push - objects_after_initial;
    eprintln!("New objects: {}", new_objects);

    // Should still only add ~3 objects
    assert!(
        new_objects <= 4,
        "Clone-modify-push added {} objects, expected ~3!",
        new_objects
    );
}
