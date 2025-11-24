use std::path::Path;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("Project name cannot be empty")]
    EmptyProjectName,
    #[error("Project name contains invalid characters: {0}")]
    InvalidProjectNameChars(String),
    #[error("Project name is too long (max 50 characters)")]
    ProjectNameTooLong,
    #[error("Invalid channel name: {0}. Must be 'stable' or 'nightly'")]
    InvalidChannelName(String),
    #[error("Directory already exists: {0}")]
    DirectoryExists(String),
    #[error("Path does not exist: {0}")]
    PathNotFound(String),
}

pub type ValidationResult<T> = Result<T, ValidationError>;

pub fn validate_project_name(name: &str) -> ValidationResult<()> {
    if name.is_empty() {
        return Err(ValidationError::EmptyProjectName);
    }

    if name.len() > 50 {
        return Err(ValidationError::ProjectNameTooLong);
    }

    // Only allow alphanumeric characters, hyphens, and underscores
    let invalid_chars: Vec<char> = name
        .chars()
        .filter(|&c| !c.is_alphanumeric() && c != '-' && c != '_')
        .collect();

    if !invalid_chars.is_empty() {
        let invalid_str: String = invalid_chars.iter().collect();
        return Err(ValidationError::InvalidProjectNameChars(invalid_str));
    }

    Ok(())
}

pub fn validate_channel_name(channel: &str) -> ValidationResult<()> {
    match channel {
        "stable" | "nightly" => Ok(()),
        _ => Err(ValidationError::InvalidChannelName(channel.to_string())),
    }
}

pub fn validate_project_path(path: &Path) -> ValidationResult<()> {
    if path.exists() {
        return Err(ValidationError::DirectoryExists(
            path.display().to_string(),
        ));
    }
    Ok(())
}

pub fn validate_existing_project(path: &Path) -> ValidationResult<()> {
    if !path.exists() {
        return Err(ValidationError::PathNotFound(path.display().to_string()));
    }

    // Check for manifest.toml
    let manifest_path = path.join("manifest.toml");
    if !manifest_path.exists() {
        return Err(ValidationError::PathNotFound(
            "manifest.toml not found in project directory".to_string(),
        ));
    }

    Ok(())
}

// src/cli.rs - CLI argument structure with validation
use clap::{Parser, Subcommand};
use crate::validation::{validate_project_name, validate_channel_name, ValidationResult};

#[derive(Parser)]
#[command(name = "nockup")]
#[command(about = "NockApp template app installer and manager")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize Nockup cache and download templates
    Install,
    /// Check for updates to nockup, hoon, and hoonc
    Update,
    /// Initialize a new NockApp project
    Start {
        /// Name of the project to create
        #[arg(value_parser = validate_project_name_arg)]
        project_name: String,
    },
    /// Build a NockApp project
    Build {
        /// Path to the project to build
        project_path: String,
    },
    /// Run a NockApp project
    Run {
        /// Path to the project to run
        project_path: String,
    },
    /// Manage channels
    #[command(subcommand)]
    Channel(ChannelCommands),
}

#[derive(Subcommand)]
pub enum ChannelCommands {
    /// List available channels
    List,
    /// Set the active channel
    Set {
        /// Channel name ('stable' or 'nightly')
        #[arg(value_parser = validate_channel_name_arg)]
        channel: String,
    },
}

// Custom validators for clap
fn validate_project_name_arg(s: &str) -> Result<String, String> {
    validate_project_name(s)
        .map(|_| s.to_string())
        .map_err(|e| e.to_string())
}

fn validate_channel_name_arg(s: &str) -> Result<String, String> {
    validate_channel_name(s)
        .map(|_| s.to_string())
        .map_err(|e| e.to_string())
}

// src/lib.rs - Expose modules for testing
pub mod validation;
pub mod cli;

// Unit tests for validation functions
#[cfg(test)]
mod validation_tests {
    use super::validation::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn test_validate_project_name_valid() {
        assert!(validate_project_name("valid-project").is_ok());
        assert!(validate_project_name("valid_project").is_ok());
        assert!(validate_project_name("project123").is_ok());
        assert!(validate_project_name("a").is_ok());
        assert!(validate_project_name("my-awesome-project_v2").is_ok());
    }

    #[test]
    fn test_validate_project_name_empty() {
        assert!(matches!(
            validate_project_name(""),
            Err(ValidationError::EmptyProjectName)
        ));
    }

    #[test]
    fn test_validate_project_name_too_long() {
        let long_name = "a".repeat(51);
        assert!(matches!(
            validate_project_name(&long_name),
            Err(ValidationError::ProjectNameTooLong)
        ));
    }

    #[test]
    fn test_validate_project_name_invalid_chars() {
        let invalid_names = vec![
            "project with spaces",
            "project/with/slashes",
            "project@with@symbols",
            "project!",
            "project.dot",
        ];

        for name in invalid_names {
            assert!(matches!(
                validate_project_name(name),
                Err(ValidationError::InvalidProjectNameChars(_))
            ));
        }
    }

    #[test]
    fn test_validate_channel_name() {
        assert!(validate_channel_name("stable").is_ok());
        assert!(validate_channel_name("nightly").is_ok());
        
        assert!(matches!(
            validate_channel_name("invalid"),
            Err(ValidationError::InvalidChannelName(_))
        ));
        assert!(matches!(
            validate_channel_name(""),
            Err(ValidationError::InvalidChannelName(_))
        ));
    }

    #[test]
    fn test_validate_project_path() {
        let temp_dir = TempDir::new().unwrap();
        let non_existing = temp_dir.path().join("non-existing");
        let existing = temp_dir.path().join("existing");
        std::fs::create_dir(&existing).unwrap();

        assert!(validate_project_path(&non_existing).is_ok());
        assert!(matches!(
            validate_project_path(&existing),
            Err(ValidationError::DirectoryExists(_))
        ));
    }

    #[test]
    fn test_validate_existing_project() {
        let temp_dir = TempDir::new().unwrap();
        let project_dir = temp_dir.path().join("project");
        let non_existing = temp_dir.path().join("non-existing");
        
        // Test non-existing project
        assert!(matches!(
            validate_existing_project(&non_existing),
            Err(ValidationError::PathNotFound(_))
        ));

        // Test project without manifest
        std::fs::create_dir(&project_dir).unwrap();
        assert!(matches!(
            validate_existing_project(&project_dir),
            Err(ValidationError::PathNotFound(_))
        ));

        // Test valid project
        std::fs::write(project_dir.join("manifest.toml"), "").unwrap();
        assert!(validate_existing_project(&project_dir).is_ok());
    }
}

// Property-based testing with proptest (optional)
#[cfg(feature = "proptest")]
mod proptest_validation {
    use super::validation::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn test_valid_project_names(s in "[a-zA-Z0-9_-]{1,50}") {
            prop_assert!(validate_project_name(&s).is_ok());
        }

        #[test]
        fn test_invalid_project_names_with_spaces(s in ".*[ ].*") {
            if !s.is_empty() && s.len() <= 50 {
                prop_assert!(validate_project_name(&s).is_err());
            }
        }

        #[test]
        fn test_project_names_too_long(s in "[a-zA-Z0-9_-]{51,100}") {
            prop_assert!(matches!(
                validate_project_name(&s),
                Err(ValidationError::ProjectNameTooLong)
            ));
        }
    }
}

// Example of how to run validation in your main application
pub fn handle_start_command(project_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Validation is already done by clap, but you can add additional checks
    let project_path = std::path::Path::new(project_name);
    
    validate_project_path(project_path)?;
    
    // Proceed with project creation...
    println!("Creating project: {}", project_name);
    Ok(())
}

pub fn handle_build_command(project_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = std::path::Path::new(project_path);
    
    validate_existing_project(path)?;
    
    // Proceed with build...
    println!("Building project at: {}", project_path);
    Ok(())
}