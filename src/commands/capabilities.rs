use anyhow::Result;
use std::io::Write;

/// Handle the capabilities command
/// Output the capabilities this remote helper supports
pub fn handle<W: Write>(output: &mut W) -> Result<()> {
    // Use fetch/push capabilities instead of import/export
    // This enables native Git pack protocol
    writeln!(output, "fetch")?;
    writeln!(output, "push")?;
    writeln!(output, "refspec refs/heads/*:refs/heads/*")?;
    writeln!(output, "refspec refs/tags/*:refs/tags/*")?;
    writeln!(output)?; // Empty line signals completion

    Ok(())
}
