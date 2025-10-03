use anyhow::Result;
use std::io::{self, BufRead, Write};

use crate::commands;
use crate::storage::StorageBackend;

/// Main protocol handler - reads commands from stdin and dispatches them
pub fn handle_commands<S: StorageBackend>(storage: S) -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let reader = stdin.lock();

    let mut lines = reader.lines();

    while let Some(line) = lines.next() {
        let line = line?;
        let line = line.trim();

        // Log commands to stderr for debugging
        eprintln!("git-remote-gitwal: Received command: {}", line);

        if line.is_empty() {
            continue;
        }

        // Parse and dispatch command
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        match parts[0] {
            "capabilities" => {
                commands::capabilities::handle(&mut stdout)?;
            }
            "list" => {
                let for_push = parts.get(1) == Some(&"for-push");
                commands::list::handle(&storage, &mut stdout, for_push)?;
            }
            "import" => {
                let refs = read_import_refs(&mut lines)?;
                commands::import::handle(&storage, &mut stdout, &refs)?;
            }
            "export" => {
                commands::export::handle(&storage, &mut stdout, &mut lines)?;
            }
            "" => {
                // Empty line signals end of command batch
                break;
            }
            cmd => {
                eprintln!("git-remote-gitwal: Unknown command: {}", cmd);
            }
        }

        stdout.flush()?;
    }

    Ok(())
}

/// Read import ref list until empty line
fn read_import_refs<R: BufRead>(lines: &mut std::io::Lines<R>) -> Result<Vec<String>> {
    let mut refs = Vec::new();

    while let Some(line) = lines.next() {
        let line = line?;
        let line = line.trim();

        if line.is_empty() {
            break;
        }

        if let Some(refname) = line.strip_prefix("import ") {
            refs.push(refname.to_string());
        }
    }

    Ok(refs)
}
