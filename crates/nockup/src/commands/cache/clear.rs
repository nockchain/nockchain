// src/commands/cache/clear.rs
use std::fs;

use anyhow::{Context, Result};
use colored::Colorize;

use crate::commands::common::get_cache_dir;

/// Clear nockup cache directories
pub async fn run(git: bool, packages: bool, registry: bool, all: bool) -> Result<()> {
    let cache_dir = get_cache_dir()?.join("cache");

    // Determine what to clear
    let clear_git = all || git;
    let clear_packages = all || packages;
    let clear_registry = all || registry;

    // If no flags specified, show help
    if !clear_git && !clear_packages && !clear_registry {
        println!("{}", "Please specify what to clear:".yellow());
        println!("  --git       Clear git repository cache");
        println!("  --packages  Clear processed packages cache");
        println!("  --registry  Clear registry cache");
        println!("  --all       Clear all caches");
        println!();
        println!("Example: nockup cache clear --all");
        return Ok(());
    }

    println!("{} Clearing nockup cache...", "🗑️".cyan());
    println!();

    let mut cleared_any = false;

    // Clear git cache
    if clear_git {
        let git_dir = cache_dir.join("git");
        if git_dir.exists() {
            let size = calculate_dir_size(&git_dir)?;
            fs::remove_dir_all(&git_dir)
                .with_context(|| format!("Failed to remove git cache at {}", git_dir.display()))?;
            println!(
                "  {} Cleared git cache (freed {})",
                "✓".green(),
                format_size(size).cyan()
            );
            cleared_any = true;
        } else {
            println!("  {} Git cache already empty", "→".cyan());
        }
    }

    // Clear packages cache
    if clear_packages {
        let packages_dir = cache_dir.join("packages");
        if packages_dir.exists() {
            let size = calculate_dir_size(&packages_dir)?;
            fs::remove_dir_all(&packages_dir).with_context(|| {
                format!(
                    "Failed to remove packages cache at {}",
                    packages_dir.display()
                )
            })?;
            println!(
                "  {} Cleared packages cache (freed {})",
                "✓".green(),
                format_size(size).cyan()
            );
            cleared_any = true;
        } else {
            println!("  {} Packages cache already empty", "→".cyan());
        }
    }

    // Clear registry cache
    if clear_registry {
        let registry_dir = cache_dir.join("registry");
        if registry_dir.exists() {
            let size = calculate_dir_size(&registry_dir)?;
            fs::remove_dir_all(&registry_dir).with_context(|| {
                format!(
                    "Failed to remove registry cache at {}",
                    registry_dir.display()
                )
            })?;
            println!(
                "  {} Cleared registry cache (freed {})",
                "✓".green(),
                format_size(size).cyan()
            );
            cleared_any = true;
        } else {
            println!("  {} Registry cache already empty", "→".cyan());
        }
    }

    println!();
    if cleared_any {
        println!("{} Cache cleared successfully", "✓".green());
        println!(
            "  Run {} to re-download dependencies",
            "nockup package install".cyan()
        );
    } else {
        println!("{} No cache to clear", "→".cyan());
    }

    Ok(())
}

/// Calculate the total size of a directory recursively
fn calculate_dir_size(path: &std::path::Path) -> Result<u64> {
    let mut total = 0u64;

    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let metadata = entry.metadata()?;

            if metadata.is_dir() {
                total += calculate_dir_size(&entry.path())?;
            } else {
                total += metadata.len();
            }
        }
    }

    Ok(total)
}

/// Format bytes as human-readable size
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}
