use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result};
use colored::Colorize;
use tokio::process::Command;

pub async fn run(project: String, args: Vec<String>) -> Result<()> {
    let project_dir = Path::new(&project);

    // Check if project directory exists
    if !project_dir.exists() {
        return Err(anyhow::anyhow!("Project directory '{}' not found", project));
    }

    // Check if Cargo.toml exists
    let cargo_toml = project_dir.join("Cargo.toml");
    if !cargo_toml.exists() {
        return Err(anyhow::anyhow!("No Cargo.toml found in '{}'", project));
    }

    println!("{} Running project '{}'...", "🔨".green(), project.cyan());

    // Run cargo run in the project directory
    let mut command = Command::new("cargo");
    command
        .arg("run")
        .arg("--release") // Run in release mode by default
        .current_dir(project_dir)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    // Add separator and pass through additional arguments to the program
    if !args.is_empty() {
        command.arg("--").args(&args);
    }

    let status = command
        .status()
        .await
        .context("Failed to execute cargo run")?;

    if status.success() {
        println!("{} Run completed successfully!", "✓".green());
    } else {
        return Err(anyhow::anyhow!(
            "Run failed with exit code: {}",
            status.code().unwrap_or(-1)
        ));
    }

    Ok(())
}
