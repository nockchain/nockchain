// src/commands/package/list.rs
use std::env;

use anyhow::Result;
use colored::Colorize;

use crate::manifest::{HoonPackage, NockAppLock};

/// List all dependencies from nockapp.toml and their installation status
pub async fn run() -> Result<()> {
    let cwd = env::current_dir()?;
    let manifest_path = cwd.join("nockapp.toml");

    if !manifest_path.exists() {
        anyhow::bail!("No nockapp.toml found in current directory");
    }

    // Load manifest
    let manifest = match HoonPackage::load(&manifest_path)? {
        Some(m) => m,
        None => anyhow::bail!("Failed to load nockapp.toml"),
    };

    // Determine the project directory based on the package name
    let project_dir = cwd.join(&manifest.package.name);

    // Check if project directory exists
    if !project_dir.exists() {
        anyhow::bail!(
            "Project directory '{}' not found. Run `nockup project init` first.",
            manifest.package.name
        );
    }

    // Load lockfile if it exists
    let lock_path = project_dir.join("nockapp.lock");
    let lockfile = NockAppLock::load(&lock_path)?;

    println!("{} Package dependencies:", "📦".cyan());
    println!();

    // Check if there are any dependencies
    let deps = match manifest.dependencies {
        Some(ref deps) if !deps.is_empty() => deps,
        _ => {
            println!("  No dependencies found");
            return Ok(());
        }
    };

    // Create a map of installed packages from lockfile
    let installed: std::collections::HashMap<String, String> = lockfile
        .package
        .iter()
        .map(|pkg| (pkg.name.clone(), pkg.version.clone()))
        .collect();

    // List each dependency
    for (name, spec) in deps {
        let spec_str = match spec {
            crate::manifest::DependencySpec::Simple(v) => v.clone(),
            crate::manifest::DependencySpec::Version { version } => version.clone(),
            crate::manifest::DependencySpec::Full {
                version,
                tag,
                branch,
                commit,
                ..
            } => {
                // Determine which version identifier to show
                if let Some(v) = version {
                    v.clone()
                } else if let Some(t) = tag {
                    format!("@tag:{}", t)
                } else if let Some(b) = branch {
                    format!("@branch:{}", b)
                } else if let Some(c) = commit {
                    format!("@commit:{}", &c[..8.min(c.len())])
                } else {
                    "?".to_string()
                }
            }
        };

        // Check installation status
        if let Some(installed_version) = installed.get(name) {
            // Verify the package directory exists
            // Package directories must be @tas compatible (lowercase, numbers, hyphens only)
            let package_dir_name = format!(
                "{}--{}",
                name.replace('/', "-"),
                installed_version.replace(['.', ':'], "-")
            );
            let package_dir = project_dir
                .join("hoon")
                .join("packages")
                .join(package_dir_name);

            if package_dir.exists() {
                println!(
                    "  {} {} {} (installed: {})",
                    "✓".green(),
                    name.yellow(),
                    spec_str.cyan(),
                    installed_version.cyan()
                );
            } else {
                println!(
                    "  {} {} {} (in lockfile but missing from disk)",
                    "⚠".yellow(),
                    name.yellow(),
                    spec_str.cyan()
                );
            }
        } else {
            println!(
                "  {} {} {} (not installed)",
                "✗".red(),
                name.yellow(),
                spec_str.cyan()
            );
        }
    }

    println!();

    // Show summary
    let total = deps.len();
    let installed_count = deps
        .keys()
        .filter(|name| installed.contains_key(*name))
        .count();

    if installed_count == total {
        println!("{} All {} packages installed", "✓".green(), total);
    } else {
        println!(
            "{} {}/{} packages installed",
            "→".cyan(),
            installed_count,
            total
        );
        if installed_count < total {
            println!(
                "  Run {} to install missing packages",
                "nockup package install".cyan()
            );
        }
    }

    Ok(())
}
