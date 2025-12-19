// src/commands/package/update.rs
use std::collections::HashMap;
use std::env;

use anyhow::Result;
use colored::Colorize;

use crate::manifest::{DependencySpec, HoonPackage, LockSource, NockAppLock};
use crate::resolver::Resolver;

/// Update dependencies to their latest compatible versions
pub async fn run() -> Result<()> {
    let cwd = env::current_dir()?;
    let manifest_path = cwd.join("nockapp.toml");

    // Load manifest
    let manifest = match HoonPackage::load(&manifest_path)? {
        Some(m) => m,
        None => anyhow::bail!("No nockapp.toml found in {}", cwd.display()),
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

    println!(
        "{} Checking for updates to dependencies for {}",
        "🔄".cyan(),
        manifest.package.name.yellow()
    );
    println!();

    // Check if there are any dependencies
    let deps = match &manifest.dependencies {
        Some(deps) if !deps.is_empty() => deps,
        _ => {
            println!("{} No dependencies to update", "✓".green());
            return Ok(());
        }
    };

    // Load existing lockfile if it exists
    let lock_path = project_dir.join("nockapp.lock");
    let old_lockfile = NockAppLock::load(&lock_path)?;
    let old_versions: HashMap<String, String> = old_lockfile
        .package
        .iter()
        .map(|pkg| (pkg.name.clone(), pkg.version.clone()))
        .collect();

    // Check each dependency to see if it can/should be updated
    let mut updates_available = Vec::new();

    for (name, spec) in deps {
        // Determine if this dependency should be updated
        let should_update = match spec {
            DependencySpec::Simple(v) => {
                // Check if it's a minimum version spec (starts with ^) or "latest"
                v.starts_with('^') || v == "*" || v == "latest"
            }
            DependencySpec::Version { version } => {
                version.starts_with('^') || version == "*" || version == "latest"
            }
            DependencySpec::Full {
                branch,
                commit,
                tag,
                version,
                ..
            } => {
                // Only update if using a branch (not a fixed commit or tag)
                if branch.is_some() && commit.is_none() && tag.is_none() {
                    true
                } else if let Some(v) = version {
                    v.starts_with('^') || v == "*" || v == "latest"
                } else {
                    false
                }
            }
        };

        if should_update {
            if let Some(old_version) = old_versions.get(name) {
                updates_available.push((name.clone(), old_version.clone()));
            }
        }
    }

    if updates_available.is_empty() {
        println!(
            "{} All dependencies are up to date (or pinned to fixed versions)",
            "✓".green()
        );
        return Ok(());
    }

    println!("{} Checking for updates...", "🔍".cyan());
    println!();

    // Re-resolve dependencies (this will fetch latest commits for branches, etc.)
    let resolver = Resolver::new()?;
    let new_graph = resolver.resolve(&manifest).await?;

    // Compare old and new versions
    let mut has_updates = false;
    for (name, old_version) in &updates_available {
        if let Some(new_pkg) = new_graph.packages.get(name) {
            let new_version = new_pkg.version_spec.to_canonical_string();

            // For git-based dependencies, compare commits
            let old_commit =
                if let Some(old_pkg) = old_lockfile.package.iter().find(|p| &p.name == name) {
                    match &old_pkg.source {
                        LockSource::Git { commit, .. } => Some(commit.as_str()),
                        LockSource::Path { .. } => None,
                    }
                } else {
                    None
                };
            let new_commit = Some(new_pkg.commit.as_str());

            // Compare commits to detect updates
            // Note: For Kelvin versions, lower numbers are newer (k408 > k409)
            // But we compare commits, not kelvin numbers, so this works correctly
            if old_commit != new_commit {
                has_updates = true;

                // Check if this is a kelvin version update
                let is_kelvin_update =
                    old_version.starts_with("@k") || old_version.starts_with("k");

                println!(
                    "  {} {} {} → {}{}",
                    "↑".green(),
                    name.yellow(),
                    old_version.cyan(),
                    new_version.cyan(),
                    if is_kelvin_update {
                        " (kelvin ↓ = newer)"
                    } else {
                        ""
                    }
                );
                if let (Some(old_c), Some(new_c)) = (old_commit, new_commit) {
                    if old_c != new_c {
                        println!(
                            "    commit: {} → {}",
                            &old_c[..8.min(old_c.len())],
                            &new_c[..8.min(new_c.len())]
                        );
                    }
                }
            } else {
                println!(
                    "  {} {} {} (no update available)",
                    "→".blue(),
                    name.yellow(),
                    old_version.cyan()
                );
            }
        }
    }

    if !has_updates {
        println!();
        println!(
            "{} All updateable dependencies are already at their latest versions",
            "✓".green()
        );
        return Ok(());
    }

    println!();
    println!(
        "{} Running package install to apply updates...",
        "📦".cyan()
    );
    println!();

    // Run package install to actually install the updates
    crate::commands::package::install::run().await?;

    println!();
    println!("{} Updates applied successfully!", "✓".green());

    Ok(())
}
