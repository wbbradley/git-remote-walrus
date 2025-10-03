use anyhow::Result;
use std::io::Write;

/// Handle the capabilities command
/// Output the capabilities this remote helper supports
pub fn handle<W: Write>(output: &mut W) -> Result<()> {
    // Use fetch capability for native pack format (no fast-export/import)
    // Export is still used for push operations
    writeln!(output, "fetch")?;
    writeln!(output, "export")?;
    writeln!(output, "refspec refs/heads/*:refs/heads/*")?;
    writeln!(output, "refspec refs/tags/*:refs/tags/*")?;
    writeln!(output)?; // Empty line signals completion

    Ok(())
}
