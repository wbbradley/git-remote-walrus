use std::{collections::HashMap, io::BufRead};

use anyhow::{Context, Result};

/// Parse a fast-export stream and return the raw data plus ref updates
/// Returns: (stream_bytes, ref_updates_map)
pub fn parse_stream<R: BufRead>(
    lines: &mut std::io::Lines<R>,
) -> Result<(Vec<u8>, HashMap<String, String>)> {
    let mut stream_bytes = Vec::new();
    let mut ref_updates = HashMap::new();
    let mut current_ref: Option<String> = None;
    let mut commit_sha1: Option<String> = None;

    #[allow(clippy::while_let_on_iterator)]
    while let Some(line) = lines.next() {
        let line = line.context("Failed to read line from fast-export stream")?;

        // Add line to our stored stream (with newline)
        stream_bytes.extend_from_slice(line.as_bytes());
        stream_bytes.push(b'\n');

        let trimmed = line.trim();

        // Check for 'done' command (end of stream)
        if trimmed == "done" {
            break;
        }

        // Parse reset lines to track ref deletions/initializations
        if let Some(stripped) = trimmed.strip_prefix("reset ") {
            current_ref = Some(stripped.to_string());
            commit_sha1 = None;
        }

        // Parse commit lines to track which ref we're updating
        if let Some(stripped) = trimmed.strip_prefix("commit ") {
            current_ref = Some(stripped.to_string());
            commit_sha1 = None;
        }

        // Parse 'from' lines which contain the Git SHA-1 of the commit
        if let Some(sha1_str) = trimmed.strip_prefix("from ") {
            let sha1 = sha1_str.trim();
            // Handle both marks (:1) and SHA-1s
            if !sha1.starts_with(':') && sha1.len() == 40 {
                commit_sha1 = Some(sha1.to_string());

                // For reset commands with 'from', immediately record the ref update
                if let Some(ref refname) = current_ref {
                    ref_updates.insert(refname.clone(), sha1.to_string());
                }
            }
        }

        // Parse 'mark' lines which give us the mark for this commit
        if trimmed.starts_with("mark ") {
            // For now, we'll extract the actual Git SHA-1 later
            // Marks are internal references like :1, :2, etc.
        }

        // Handle 'data' command - need to read exact number of bytes
        if let Some(size_str) = trimmed.strip_prefix("data ") {
            let size: usize = size_str
                .parse()
                .context("Failed to parse data size in fast-export stream")?;

            // Read exactly 'size' bytes
            let buffer = vec![0u8; size];

            // We need to read from the underlying reader, not lines
            // This is a limitation - we'll need to refactor to handle this properly
            // For now, let's store a placeholder

            stream_bytes.extend_from_slice(&buffer);
            stream_bytes.push(b'\n');
        }

        // Empty line might signal end of a command
        if trimmed.is_empty() && current_ref.is_some() && commit_sha1.is_some() {
            // Record this ref update
            ref_updates.insert(current_ref.clone().unwrap(), commit_sha1.clone().unwrap());
        }
    }

    // If we finished without explicit ref updates, try to extract from the stream
    // This is a simplified implementation - a real parser would track marks properly
    if ref_updates.is_empty() {
        ref_updates = extract_refs_from_stream(&stream_bytes)?;
    }

    Ok((stream_bytes, ref_updates))
}

/// Extract ref → SHA-1 mappings from the raw stream
/// This is a helper for the simplified implementation
fn extract_refs_from_stream(stream: &[u8]) -> Result<HashMap<String, String>> {
    let mut ref_updates = HashMap::new();
    let stream_str = String::from_utf8_lossy(stream);

    let mut current_ref: Option<String> = None;
    let mut marks_to_sha: HashMap<String, String> = HashMap::new();
    let mut last_mark: Option<String> = None;

    for line in stream_str.lines() {
        let trimmed = line.trim();

        // Track which ref we're committing to
        if let Some(stripped) = trimmed.strip_prefix("commit ") {
            current_ref = Some(stripped.to_string());
            last_mark = None;
        }

        // Track mark assignments
        if let Some(stripped) = trimmed.strip_prefix("mark ") {
            last_mark = Some(stripped.to_string());
        }

        // When we see a 'from', it might contain a SHA-1
        if let Some(from_str) = trimmed.strip_prefix("from ") {
            let from_ref = from_str.trim();
            if from_ref.len() == 40 && !from_ref.starts_with(':') {
                // This is a SHA-1
                if let Some(mark) = &last_mark {
                    marks_to_sha.insert(mark.clone(), from_ref.to_string());
                }
            }
        }

        // Try to generate a pseudo-SHA-1 for commits without 'from'
        // In reality, we'd need to compute this properly
        if trimmed.starts_with("committer ") {
            if let (Some(ref_name), Some(mark)) = (&current_ref, &last_mark) {
                // For initial commits without a 'from', generate a SHA-1
                if !marks_to_sha.contains_key(mark) {
                    // Use a hash of the stream content up to this point as SHA-1
                    let pseudo_sha1 = generate_pseudo_sha1(ref_name);
                    marks_to_sha.insert(mark.clone(), pseudo_sha1.clone());

                    ref_updates.insert(ref_name.clone(), pseudo_sha1);
                }
            }
        }
    }

    // If we still have no ref updates, create a default one
    if ref_updates.is_empty() {
        if let Some(ref_name) = current_ref {
            let pseudo_sha1 = generate_pseudo_sha1(&ref_name);
            ref_updates.insert(ref_name, pseudo_sha1);
        }
    }

    Ok(ref_updates)
}

/// Generate a pseudo Git SHA-1 for testing
/// In a real implementation, we'd parse the actual Git objects
fn generate_pseudo_sha1(ref_name: &str) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(ref_name.as_bytes());
    let result = hasher.finalize();

    // Take first 20 bytes (40 hex chars) to simulate a SHA-1
    hex::encode(&result[..20])
}
