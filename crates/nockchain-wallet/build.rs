use anyhow::Result;
use vergen::EmitBuilder;

fn main() -> Result<()> {
    EmitBuilder::builder()
        .git_sha(true)
        .emit()?;
    Ok(())
}
