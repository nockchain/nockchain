// src/commands/package/remove.rs
use std::{env, fs};

use anyhow::{anyhow, Context, Result};
use colored::Colorize;

use crate::manifest::HoonPackage;

/// Remove a dependency from nockapp.toml and clean up installed files
pub async fn run(package_name: String) -> Result<()> {
    let cwd = env::current_dir()?;
    let manifest_path = cwd.join("nockapp.toml");

    if !manifest_path.exists() {
        anyhow::bail!("No nockapp.toml found in current directory");
    }

    println!(
        "{} Removing dependency {}...",
        "📦".cyan(),
        package_name.yellow()
    );

    // Load existing manifest
    let mut manifest = match HoonPackage::load(&manifest_path)? {
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

    // Check if dependencies exist
    let deps = manifest
        .dependencies
        .as_mut()
        .ok_or_else(|| anyhow!("No dependencies found in nockapp.toml"))?;

    // Check if package exists
    if !deps.contains_key(&package_name) {
        anyhow::bail!("Package '{}' not found in dependencies", package_name);
    }

    // Remove the dependency
    deps.remove(&package_name);

    // Save the manifest
    manifest.save(&manifest_path)?;

    println!(
        "{} Removed {} from nockapp.toml",
        "✓".green(),
        package_name.yellow()
    );

    // Clean up installed files
    // Note: We don't know the exact version that was installed, so we'll look for any version
    let packages_dir = project_dir.join("hoon").join("packages");
    if packages_dir.exists() {
        if let Ok(entries) = fs::read_dir(&packages_dir) {
            for entry in entries.flatten() {
                let dir_name = entry.file_name();
                let dir_name_str = dir_name.to_string_lossy();

                // Check if directory name starts with "packagename@"
                if dir_name_str.starts_with(&format!("{}@", package_name)) {
                    let package_path = entry.path();
                    println!("  {} Removing {}", "🗑".cyan(), dir_name_str.yellow());
                    fs::remove_dir_all(&package_path)
                        .with_context(|| format!("Failed to remove {}", package_path.display()))?;
                }
            }
        }
    }

    // Clean up symlinks in hoon/lib
    let lib_dir = project_dir.join("hoon").join("lib");
    if lib_dir.exists() {
        println!("  {} Cleaning up symlinks in hoon/lib/", "🧹".cyan());

        if let Ok(entries) = fs::read_dir(&lib_dir) {
            for entry in entries.flatten() {
                let path = entry.path();

                // Check if it's a symlink
                if path.is_symlink() {
                    // Read the symlink target
                    if let Ok(target) = fs::read_link(&path) {
                        let target_str = target.to_string_lossy();

                        // Check if symlink points to removed package
                        if target_str.contains(&format!("{}@", package_name)) {
                            let file_name = path
                                .file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_else(|| path.display().to_string());
                            println!("    {} Removing symlink {}", "→".cyan(), file_name.yellow());
                            fs::remove_file(&path).with_context(|| {
                                format!("Failed to remove symlink {}", path.display())
                            })?;
                        }
                    }
                }
            }
        }
    }

    println!(
        "  Run {} to update dependencies",
        "nockup package install".cyan()
    );

    Ok(())
}
