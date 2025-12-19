use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result};
use colored::Colorize;
use tokio::process::Command;

pub async fn run(project: &str) -> Result<()> {
    let project_dir = Path::new(&project);

    // Check if project directory exists
    if !project_dir.exists() {
        return Err(anyhow::anyhow!("Project directory '{}' not found", project));
    }

    // Auto-install dependencies if nockapp.toml exists
    let nockapp_manifest = project_dir.join("nockapp.toml");
    if nockapp_manifest.exists() {
        // Check if dependencies need to be installed
        if should_install_dependencies(project_dir).await? {
            println!("{} Installing dependencies...", "📦".cyan());
            // Change to project directory to run install
            let original_dir = std::env::current_dir()?;
            std::env::set_current_dir(project_dir)?;

            // Run package install
            let install_result = crate::commands::package::install::run().await;

            // Change back to original directory
            std::env::set_current_dir(original_dir)?;

            install_result?;
            println!();
        }
    }

    // Check if it's a valid NockApp project (has manifest.toml)
    let manifest_path = project_dir.join("manifest.toml");
    if !manifest_path.exists() {
        return Err(anyhow::anyhow!(
            "Not a NockApp project: '{}' missing manifest.toml", project
        ));
    }

    // Check if Cargo.toml exists
    let cargo_toml = project_dir.join("Cargo.toml");
    if !cargo_toml.exists() {
        return Err(anyhow::anyhow!("No Cargo.toml found in '{}'", project));
    }

    println!("{} Building project '{}'...", "🔨".green(), project.cyan());

    // Extract expected binary names from Cargo.toml
    let cargo_toml_content = tokio::fs::read_to_string(&cargo_toml)
        .await
        .context("Failed to read Cargo.toml")?;

    let cargo_toml_parsed: toml::Value =
        toml::from_str(&cargo_toml_content).context("Failed to parse Cargo.toml")?;

    let expected_binaries = if let Some(bins) = cargo_toml_parsed.get("bin") {
        bins.as_array()
            .context("Invalid format for [[bin]] in Cargo.toml")?
            .iter()
            .filter_map(|bin| bin.get("name").and_then(|n| n.as_str()))
            .map(String::from)
            .collect::<Vec<String>>()
    } else {
        Vec::new()
    };

    // Check number of expected binaries; if more than one, check primary source files.
    let binaries: Vec<std::path::PathBuf> = if expected_binaries.len() > 1 {
        expected_binaries
            .iter()
            .map(|bin_name| project_dir.join("src").join(format!("{}.rs", bin_name)))
            .collect()
    } else {
        vec![project_dir.join("src").join("main.rs")]
    };

    // Run cargo build in the project directory
    let mut cargo_command = Command::new("cargo");
    cargo_command
        .arg("build")
        .arg("--release") // Build in release mode by default
        .current_dir(project_dir)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let status = cargo_command
        .status()
        .await
        .context("Failed to execute cargo build")?;

    if !status.success() {
        return Err(anyhow::anyhow!(
            "Cargo build failed with exit code: {}",
            status.code().unwrap_or(-1)
        ));
    }

    println!("{} Cargo build completed successfully!", "✓".green());

    // Check if hoon app file exists
    //  If there is only one binary, then check in the normal spot.
    //  If there are multiple binaries, then check at each location by name.
    for bin_path in &binaries {
        // if this is main.rs, then load app.hoon
        let name = if bin_path
            .file_name()
            .expect("bin_path should have a file name")
            == "main.rs"
        {
            "app".to_string()
        } else {
            bin_path
                .file_stem()
                .expect("bin_path should have a file stem")
                .to_string_lossy()
                .to_string()
        };
        let hoon_app_path = project_dir.join(format!("hoon/app/{}.hoon", name));
        println!("Compiling Hoon app file at: {}", hoon_app_path.display());

        if !hoon_app_path.exists() {
            return Err(anyhow::anyhow!(
                "Hoon app file not found: '{}'",
                hoon_app_path.display()
            ));
        }

        println!("{} Compiling Hoon app...", "📦".green());

        // Run hoonc command from project directory
        let mut hoonc_command = Command::new("hoonc");
        hoonc_command
            .arg(
                hoon_app_path
                    .strip_prefix(project_dir)
                    .expect("hoon_app_path should be under project_dir"),
            )
            .current_dir(project_dir) // Run in project directory
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        let hoonc_status = hoonc_command.status().await.context(
            "Failed to execute hoonc command - make sure hoonc is installed and in PATH",
        )?;

        if !hoonc_status.success() {
            return Err(anyhow::anyhow!(
                "hoonc compilation failed with exit code: {}",
                hoonc_status.code().unwrap_or(-1)
            ));
        }

        // move out.jam to {bin_name}.jam if the program has multiple names
        if binaries.len() > 1 {
            let target_jam = project_dir.join(format!(
                "{}.jam",
                bin_path
                    .file_stem()
                    .expect("bin_path should have a file stem")
                    .to_string_lossy()
            ));
            tokio::fs::rename(project_dir.join("out.jam"), &target_jam)
                .await
                .context(format!(
                    "Failed to rename out.jam to {}",
                    target_jam.display()
                ))?;
            println!(
                "{} Renamed out.jam to {}",
                "🔀".green(),
                target_jam.display().to_string().cyan()
            );
        }
    }

    println!("{} Hoon compilation completed successfully!", "✓".green());

    Ok(())
}

/// Check if dependencies need to be installed
async fn should_install_dependencies(project_dir: &Path) -> Result<bool> {
    use crate::manifest::{HoonPackage, NockAppLock};

    // Load the manifest
    let manifest_path = project_dir.join("nockapp.toml");
    let manifest = match HoonPackage::load(&manifest_path)? {
        Some(m) => m,
        None => return Ok(false), // No manifest, no dependencies needed
    };

    // Check if there are any dependencies
    let has_deps = manifest
        .dependencies
        .as_ref()
        .map(|deps| !deps.is_empty())
        .unwrap_or(false);

    if !has_deps {
        return Ok(false); // No dependencies to install
    }

    // Check if lockfile exists
    let lock_path = project_dir.join("nockapp.lock");
    if !lock_path.exists() {
        return Ok(true); // Lockfile missing, need to install
    }

    // Load lockfile
    let lockfile = NockAppLock::load(&lock_path)?;

    // Check if all dependencies in manifest are in lockfile
    let manifest_deps: std::collections::HashSet<&String> = manifest
        .dependencies
        .as_ref()
        .map(|deps| deps.keys().collect())
        .unwrap_or_default();

    let lockfile_deps: std::collections::HashSet<String> = lockfile
        .package
        .iter()
        .map(|pkg| pkg.name.clone())
        .collect();

    // If any manifest dependency is missing from lockfile, need to install
    for dep_name in manifest_deps {
        if !lockfile_deps.contains(dep_name) {
            return Ok(true);
        }
    }

    // Check if hoon/packages directories exist for all locked packages
    let packages_dir = project_dir.join("hoon").join("packages");
    if !packages_dir.exists() {
        return Ok(true); // Packages directory missing, need to install
    }

    for pkg in &lockfile.package {
        let pkg_dir = packages_dir.join(format!("{}@{}", pkg.name, pkg.version));
        if !pkg_dir.exists() {
            return Ok(true); // Package directory missing, need to install
        }
    }

    Ok(false) // Everything looks good, no install needed
}
