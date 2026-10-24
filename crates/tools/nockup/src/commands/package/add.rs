// src/commands/package/add.rs
use std::env;

use anyhow::Result;
use colored::Colorize;

use crate::manifest::HoonPackage;

/// Add a dependency to nockapp.toml
pub async fn run(package_name: String, version: Option<String>) -> Result<()> {
    let cwd = env::current_dir()?;
    let manifest_path = cwd.join("nockapp.toml");

    if !manifest_path.exists() {
        anyhow::bail!("No nockapp.toml found in current directory");
    }

    println!(
        "{} Adding dependency {}...",
        "📦".cyan(),
        package_name.yellow()
    );

    // Load existing manifest
    let mut manifest = match HoonPackage::load(&manifest_path)? {
        Some(m) => m,
        None => anyhow::bail!("Failed to load nockapp.toml"),
    };

    // Determine the version spec to use
    let version_spec = if let Some(v) = version {
        v
    } else {
        // For registry packages, we could fetch latest version
        // For now, prompt user or use a sensible default
        println!(
            "  {} No version specified, using latest available",
            "→".cyan()
        );
        // For kelvin packages, we might want to determine latest kelvin
        // For now, let's default to requiring explicit version
        anyhow::bail!(
            "Please specify a version for '{}'. \
            Examples: @k409, ^1.2.3, @tag:v1.0.0, @branch:main",
            package_name
        );
    };

    // Initialize dependencies map if it doesn't exist
    let deps = manifest
        .dependencies
        .get_or_insert_with(std::collections::BTreeMap::new);

    // Check if package already exists
    if deps.contains_key(&package_name) {
        anyhow::bail!(
            "Package '{}' is already in dependencies. \
            Use 'nockup package remove {}' first if you want to change the version.",
            package_name,
            package_name
        );
    }

    // Add the dependency
    use crate::manifest::DependencySpec;
    deps.insert(package_name.clone(), DependencySpec::Simple(version_spec));

    // Save the manifest
    manifest.save(&manifest_path)?;

    println!(
        "{} Added {} to nockapp.toml",
        "✓".green(),
        package_name.yellow()
    );
    println!(
        "  Run {} to install the dependency",
        "nockup package install".cyan()
    );

    Ok(())
}
