use std::fs;
use std::io::Read;
use std::path::PathBuf;

use anyhow::{Context, Result};
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

    // Create cache directory structure
    create_cache_structure(&cache_dir).await?;

    // Download or update templates
    common::download_templates(&cache_dir).await?;

    // Download toolchain files
    common::download_toolchain_files(&cache_dir).await?;

    // Set default channel to stable and this architecture
    let config_path = cache_dir.join("config.toml");
    let mut config = common::get_or_create_config()?;
    println!("📝 Config installed at: {}", config_path.display());
    config["channel"] = toml::Value::String("stable".into());
    config["architecture"] = toml::Value::String(common::get_target_identifier());
    fs::write(config_path, toml::to_string(&config)?).context("Failed to write config file")?;

    // Write commit details to status file
    common::write_commit_details(&cache_dir).await?;

    // Download binaries for current channel
    common::download_binaries(&config).await?;

    // Prepend cache bin directory to PATH
    prepend_path_to_shell_rc(&cache_dir.join("bin")).await?;

    println!("{} Setup complete!", "✅".green());
    println!(
        "{} Templates are now available in: {}",
        "📂".blue(),
        cache_dir.join("templates").display().to_string().cyan()
    );

    Ok(())
}

async fn create_cache_structure(cache_dir: &PathBuf) -> Result<()> {
    println!("{} Creating cache directory structure...", "📁".green());

    fs::create_dir_all(cache_dir)?;

    let bin_dir = cache_dir.join("bin");
    fs::create_dir_all(&bin_dir)?;

    let templates_dir = cache_dir.join("templates");
    fs::create_dir_all(&templates_dir)?;

    println!("{} Created directory structure", "✓".green());
    Ok(())
}

async fn prepend_path_to_shell_rc(bin_dir: &PathBuf) -> Result<()> {
    let shell = std::env::var("SHELL").unwrap_or_default();
    let rc_file = if shell.contains("zsh") {
        dirs::home_dir().unwrap().join(".zshrc")
    } else if shell.contains("bash") {
        dirs::home_dir().unwrap().join(".bashrc")
    } else {
        return Ok(());
    };

    let mut contents = String::new();
    if rc_file.exists() {
        let mut file = fs::File::open(&rc_file)?;
        file.read_to_string(&mut contents)?;
    }

    let path_entry = format!("export PATH=\"{}:$PATH\"", bin_dir.display());
    println!("{}", path_entry);
    if !contents.contains(&path_entry) {
        let new_contents = format!("{}\n{}", contents, path_entry);
        fs::write(&rc_file, new_contents)?;
        println!("{} Updated {}", "📝".green(), rc_file.display());
    }

    Ok(())
}
