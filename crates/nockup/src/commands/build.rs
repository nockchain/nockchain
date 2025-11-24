use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result};
use colored::Colorize;
use tokio::process::Command;

pub async fn run(project: String) -> Result<()> {
    let project_dir = Path::new(&project);

    // Check if project directory exists
    if !project_dir.exists() {
        return Err(anyhow::anyhow!("Project directory '{}' not found", project));
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
    let binaries: Vec<std::path::PathBuf> = {
        if expected_binaries.is_empty() {
            vec![project_dir.join("src").join("main.rs")]
        } else if expected_binaries.len() == 1 {
            vec![project_dir.join("src").join("main.rs")]
        } else {
            let mut binaries = Vec::new();
            for bin_name in &expected_binaries {
                let bin_path = project_dir.join("src").join(format!("{}.rs", bin_name));
                binaries.push(bin_path);
            }
            binaries
        }
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
        let name = if bin_path.file_name().unwrap() == "main.rs" {
            "app".to_string()
        } else {
            bin_path.file_stem().unwrap().to_string_lossy().to_string()
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
            .arg(hoon_app_path.strip_prefix(project_dir).unwrap())
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
                bin_path.file_stem().unwrap().to_string_lossy()
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
