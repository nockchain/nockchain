use anyhow::Result;
use colored::Colorize;

use super::common;

pub async fn run() -> Result<()> {
    let cache_dir = common::get_cache_dir()?;

    println!("{} Setting up nockup cache directory...", "🚀".green());
    println!(
        "{} Cache location: {}",
        "📁".blue(),
        cache_dir.display().to_string().cyan()
    );

    // Download or update templates
    common::download_templates(&cache_dir).await?;

    // Download toolchain files
    common::download_toolchain_files(&cache_dir).await?;

    // Write commit details to status file
    common::write_commit_details(&cache_dir).await?;

    // Get existing config
    let config = common::get_config()?;

    // Download binaries for current channel
    common::download_binaries(&config).await?;

    println!("{} Update complete!", "✅".green());

    Ok(())
}
