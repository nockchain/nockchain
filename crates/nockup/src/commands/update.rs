use std::fs;

use anyhow::{Context, Result};
use colored::Colorize;

use super::common;

pub async fn run() -> Result<()> {
    run_update(false).await
}

/// Run update with optional initial setup
/// If `is_initial_install` is true, also creates cache structure, sets up config, and updates PATH
pub async fn run_update(is_initial_install: bool) -> Result<()> {
    let cache_dir = common::get_cache_dir()?;

    if is_initial_install {
        println!("{} Setting up nockup cache directory...", "🚀".green());
    } else {
        println!("{} Updating nockup...", "🔄".green());
    }

    println!(
        "{} Cache location: {}",
        "📁".blue(),
        cache_dir.display().to_string().cyan()
    );

    // Create cache directory structure (only for initial install)
    if is_initial_install {
        create_cache_structure(&cache_dir).await?;
    }

    // Download or update templates
    common::download_templates(&cache_dir).await?;

    // Download toolchain files
    common::download_toolchain_files(&cache_dir).await?;

    // Set up or get config
    let config = if is_initial_install {
        let config_path = cache_dir.join("config.toml");
        let mut config = common::get_or_create_config()?;
        println!("📝 Config installed at: {}", config_path.display());
        config["channel"] = toml::Value::String("stable".into());
        config["architecture"] = toml::Value::String(common::get_target_identifier());
        fs::write(&config_path, toml::to_string(&config)?)
            .context("Failed to write config file")?;
        config
    } else {
        common::get_config()?
    };

    // Write commit details to status file
    common::write_commit_details(&cache_dir).await?;

    // Download binaries for current channel
    common::download_binaries(&config).await?;

    // Prepend cache bin directory to PATH (only for initial install)
    if is_initial_install {
        prepend_path_to_shell_rc(&cache_dir.join("bin")).await?;
    }

    if is_initial_install {
        println!("{} Setup complete!", "✅".green());
        println!(
            "{} Templates are now available in: {}",
            "📂".blue(),
            cache_dir.join("templates").display().to_string().cyan()
        );
        println!(
            "{} Binaries are now available in: {}",
            "🛠".blue(),
            cache_dir.join("bin").display().to_string().cyan()
        );
    } else {
        println!("{} Update complete!", "✅".green());
    }

    Ok(())
}

async fn create_cache_structure(cache_dir: &std::path::Path) -> Result<()> {
    let templates_dir = cache_dir.join("templates");
    let bin_dir = cache_dir.join("bin");

    tokio::fs::create_dir_all(&templates_dir)
        .await
        .context("Failed to create templates directory")?;

    tokio::fs::create_dir_all(&bin_dir)
        .await
        .context("Failed to create bin directory")?;

    println!(
        "{} Created {}",
        "✓".green(),
        templates_dir.display().to_string().cyan()
    );
    println!(
        "{} Created {}",
        "✓".green(),
        bin_dir.display().to_string().cyan()
    );

    Ok(())
}

async fn prepend_path_to_shell_rc(bin_dir: &std::path::Path) -> Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?;

    let shell_rcs = vec![home.join(".bashrc"), home.join(".zshrc"), home.join(".profile")];

    let path_line = format!("export PATH=\"{}:$PATH\"", bin_dir.display());

    for rc_path in shell_rcs {
        if rc_path.exists() {
            let content = tokio::fs::read_to_string(&rc_path)
                .await
                .context("Failed to read shell RC file")?;

            if !content.contains(&path_line) {
                let mut updated_content = content;
                updated_content.push_str(&format!("\n# Added by nockup\n{}\n", path_line));
                tokio::fs::write(&rc_path, updated_content)
                    .await
                    .context("Failed to write to shell RC file")?;

                println!(
                    "{} Updated {}",
                    "✓".green(),
                    rc_path.display().to_string().cyan()
                );
            }
        }
    }

    Ok(())
}
