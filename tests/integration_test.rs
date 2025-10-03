use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

/// Helper to run git commands in a directory
fn git_command(dir: &PathBuf, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("Failed to run git command")
}

/// Create a test repository with commits
fn create_test_repo(dir: &PathBuf, signed: bool) -> PathBuf {
    let repo_dir = dir.join("test-repo");
    fs::create_dir(&repo_dir).expect("Failed to create repo dir");

    // Initialize repo
    git_command(&repo_dir, &["init"]);
    git_command(&repo_dir, &["config", "user.name", "Test User"]);
    git_command(&repo_dir, &["config", "user.email", "test@example.com"]);

    // Create some commits
    fs::write(repo_dir.join("file1.txt"), "content 1").unwrap();
    git_command(&repo_dir, &["add", "."]);

    if signed {
        // Try to sign the commit (will fail if no GPG key, that's ok for now)
        git_command(&repo_dir, &["commit", "-S", "-m", "Initial commit"]);
    } else {
        git_command(&repo_dir, &["commit", "-m", "Initial commit"]);
    }

    fs::write(repo_dir.join("file2.txt"), "content 2").unwrap();
    git_command(&repo_dir, &["add", "."]);

    if signed {
        git_command(&repo_dir, &["commit", "-S", "-m", "Second commit"]);
    } else {
        git_command(&repo_dir, &["commit", "-m", "Second commit"]);
    }

    repo_dir
}

#[test]
fn test_fast_export_preserves_commits() {
    let temp = TempDir::new().unwrap();
    let repo = create_test_repo(&temp.path().to_path_buf(), false);

    // Get original commit SHAs
    let log_output = git_command(&repo, &["log", "--format=%H"]);
    let original_shas = String::from_utf8_lossy(&log_output.stdout);

    println!("Original SHAs:\n{}", original_shas);

    // Export without signed-commits flag (current behavior)
    let export1 = git_command(
        &repo,
        &["fast-export", "--all", "--signed-tags=verbatim"],
    );

    // Export with signed-commits flag (proposed fix)
    let export2 = git_command(
        &repo,
        &["fast-export", "--all", "--signed-tags=verbatim", "--signed-commits=verbatim"],
    );

    // Both should work for unsigned commits
    assert!(export1.status.success(), "Export 1 failed");

    if !export2.status.success() {
        eprintln!("Export 2 stderr: {}", String::from_utf8_lossy(&export2.stderr));
    }
    assert!(export2.status.success(), "Export 2 failed");

    println!("Export 1 size: {} bytes", export1.stdout.len());
    println!("Export 2 size: {} bytes", export2.stdout.len());
}

#[test]
fn test_fast_export_import_roundtrip() {
    let temp = TempDir::new().unwrap();
    let repo = create_test_repo(&temp.path().to_path_buf(), false);

    // Get original commit SHAs
    let log_output = git_command(&repo, &["log", "--format=%H", "--reverse"]);
    let original_shas: Vec<String> = String::from_utf8_lossy(&log_output.stdout)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    println!("Original SHAs: {:?}", original_shas);

    // Export
    let export = git_command(
        &repo,
        &["fast-export", "--all", "--signed-commits=verbatim"],
    );
    assert!(export.status.success(), "Export failed");

    // Create new repo for import
    let import_repo = temp.path().join("import-repo");
    fs::create_dir(&import_repo).unwrap();
    git_command(&import_repo, &["init"]);

    // Import
    let mut import_cmd = Command::new("git")
        .current_dir(&import_repo)
        .args(&["fast-import"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    use std::io::Write;
    import_cmd
        .stdin
        .as_mut()
        .unwrap()
        .write_all(&export.stdout)
        .unwrap();

    let import_result = import_cmd.wait().unwrap();
    assert!(import_result.success(), "Import failed");

    // Get imported commit SHAs
    let log_output = git_command(&import_repo, &["log", "--format=%H", "--reverse"]);
    let imported_shas: Vec<String> = String::from_utf8_lossy(&log_output.stdout)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    println!("Imported SHAs: {:?}", imported_shas);

    // Compare SHAs
    assert_eq!(
        original_shas.len(),
        imported_shas.len(),
        "Different number of commits"
    );

    for (i, (orig, imported)) in original_shas.iter().zip(imported_shas.iter()).enumerate() {
        assert_eq!(
            orig, imported,
            "SHA mismatch at commit {}: {} != {}",
            i, orig, imported
        );
    }
}

#[test]
fn test_gitwal_roundtrip() {
    let temp = TempDir::new().unwrap();
    let repo = create_test_repo(&temp.path().to_path_buf(), false);
    let storage = temp.path().join("storage");

    // Get original SHAs
    let log_output = git_command(&repo, &["log", "--format=%H"]);
    let original_shas = String::from_utf8_lossy(&log_output.stdout);

    // Push to gitwal storage
    let push_result = git_command(
        &repo,
        &["push", &format!("gitwal::{}", storage.display()), "main"],
    );

    if !push_result.status.success() {
        eprintln!("Push stderr: {}", String::from_utf8_lossy(&push_result.stderr));
        panic!("Push failed");
    }

    // Clone from gitwal storage
    let clone_dir = temp.path().join("cloned");
    let clone_result = Command::new("git")
        .args(&[
            "clone",
            &format!("gitwal::{}", storage.display()),
            clone_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    if !clone_result.status.success() {
        eprintln!("Clone stderr: {}", String::from_utf8_lossy(&clone_result.stderr));
        panic!("Clone failed");
    }

    // Get cloned SHAs
    let log_output = git_command(&clone_dir, &["log", "--format=%H"]);
    let cloned_shas = String::from_utf8_lossy(&log_output.stdout);

    println!("Original SHAs:\n{}", original_shas);
    println!("Cloned SHAs:\n{}", cloned_shas);

    assert_eq!(
        original_shas.trim(),
        cloned_shas.trim(),
        "SHAs don't match after gitwal roundtrip"
    );
}

#[test]
#[ignore] // Only run if GPG is set up
fn test_signed_commits_preservation() {
    let temp = TempDir::new().unwrap();
    let repo = create_test_repo(&temp.path().to_path_buf(), true);

    // Check if commits are actually signed (will skip if no GPG key)
    let verify_output = git_command(&repo, &["verify-commit", "HEAD"]);
    if !verify_output.status.success() {
        println!("Skipping signed commit test - no GPG key available");
        return;
    }

    // Export with signed-commits flag
    let export = git_command(
        &repo,
        &["fast-export", "--all", "--signed-commits=verbatim"],
    );

    assert!(export.status.success(), "Export failed");

    // Check if gpgsig is in the export stream
    let export_str = String::from_utf8_lossy(&export.stdout);
    assert!(
        export_str.contains("gpgsig"),
        "gpgsig not found in export stream"
    );

    println!("gpgsig found in export stream!");

    // Import to new repo
    let import_repo = temp.path().join("import-repo");
    fs::create_dir(&import_repo).unwrap();
    git_command(&import_repo, &["init"]);

    let mut import_cmd = Command::new("git")
        .current_dir(&import_repo)
        .args(&["fast-import"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    use std::io::Write;
    import_cmd
        .stdin
        .as_mut()
        .unwrap()
        .write_all(&export.stdout)
        .unwrap();

    let import_result = import_cmd.wait().unwrap();
    assert!(import_result.success(), "Import failed");

    // Verify signature is preserved
    let verify_output = git_command(&import_repo, &["verify-commit", "HEAD"]);
    assert!(
        verify_output.status.success(),
        "Signature verification failed after import"
    );

    println!("Signature preserved successfully!");
}
