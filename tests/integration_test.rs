//! End-to-end integration tests for git-remote-gitwal
//!
//! These tests require the git-remote-gitwal binary to be built.
//! Run with: cargo test --release

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;
use tempfile::TempDir;

static INIT: Once = Once::new();

/// Setup git-remote-gitwal in PATH for tests
/// This ensures Git can find our custom remote helper
fn setup_git_remote() {
    INIT.call_once(|| {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let binary_path = PathBuf::from(manifest_dir).join("target/release");

        // Verify the binary exists
        let binary = binary_path.join("git-remote-gitwal");
        if !binary.exists() {
            panic!(
                "git-remote-gitwal binary not found at: {}\n\
                 Please build it first with: cargo build --release",
                binary.display()
            );
        }

        // Add our binary directory to PATH
        let current_path = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{}", binary_path.display(), current_path);
        std::env::set_var("PATH", new_path);

        eprintln!("✓ git-remote-gitwal added to PATH for testing");
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

    // Push to gitwal
    let storage_url = format!("gitwal::{}", storage.display());
    git(&test_repo, &["push", &storage_url, "main"]);

    // Clone from gitwal
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
    let storage_url = format!("gitwal::{}", storage.display());
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
    let storage_url = format!("gitwal::{}", storage.display());
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
    let storage_url = format!("gitwal::{}", storage.display());
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

    let storage_url = format!("gitwal::{}", storage.display());
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
