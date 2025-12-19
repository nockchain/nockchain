use std::process::Command;

#[allow(deprecated)]
use assert_cmd::cargo::cargo_bin;
use assert_cmd::prelude::*;
use predicates::prelude::*;
use tempfile::TempDir;

// Helper to get the cargo bin path
// Note: cargo_bin function is deprecated but the macro isn't exposed through prelude
fn nockup_bin() -> std::path::PathBuf {
    #[allow(deprecated)]
    cargo_bin("nockup")
}

#[cfg(test)]
mod cli_input_validation_tests {
    use super::*;

    // Test basic command structure
    #[test]
    fn test_no_args_shows_version() {
        let mut cmd = Command::new(nockup_bin());
        cmd.assert()
            .success()
            .stdout(predicate::str::contains("version"));
    }

    #[test]
    fn test_help_command() {
        let mut cmd = Command::new(nockup_bin());
        cmd.arg("help");
        cmd.assert()
            .success()
            .stdout(predicate::str::contains("Initialize nockup cache"));
    }

    #[test]
    fn test_invalid_command() {
        let mut cmd = Command::new(nockup_bin());
        cmd.arg("invalid-command");
        cmd.assert()
            .failure()
            .stderr(predicate::str::contains("error"))
            .stderr(predicate::str::contains("invalid-command"));
    }

    // Test install command validation
    #[test]
    fn test_install_with_invalid_flags() {
        let mut cmd = Command::new(nockup_bin());
        cmd.args(["install", "--invalid-flag"]);
        cmd.assert()
            .failure()
            .stderr(predicate::str::contains("unexpected argument"));
    }

    // Test start command validation
    #[test]
    fn test_init_without_project_name() {
        let mut cmd = Command::new(nockup_bin());
        cmd.arg("init");
        cmd.assert()
            .failure()
            .stderr(predicate::str::contains("required"));
    }

    #[test]
    fn test_init_with_empty_project_name() {
        let mut cmd = Command::new(nockup_bin());
        cmd.args(["init", ""]);
        cmd.assert().failure().stderr(predicate::str::contains(
            "Error: Project configuration file '.toml' not found",
        ));
    }

    #[test]
    fn test_init_with_valid_project_names() {
        let valid_names = vec!["myproject", "my-project", "my_project", "project123"];

        for name in valid_names {
            let temp_dir = TempDir::new().expect("Failed to create temp dir");
            let mut cmd = Command::new(nockup_bin());
            cmd.current_dir(temp_dir.path()).args(["init", name]);

            // This might fail due to missing cache, but shouldn't fail on name validation
            let output = cmd.output().expect("Failed to run command");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(!stderr.contains("invalid project name"));
        }
    }

    // Test build command validation
    #[test]
    fn test_build_without_project_name() {
        let mut cmd = Command::new(nockup_bin());
        cmd.arg("build");
        cmd.assert()
            .failure()
            .stderr(predicate::str::contains("required"));
    }

    #[test]
    fn test_build_nonexistent_project() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut cmd = Command::new(nockup_bin());
        cmd.current_dir(temp_dir.path())
            .args(["build", "nonexistent-project"]);
        cmd.assert().failure().stderr(
            predicate::str::contains("Project directory")
                .and(predicate::str::contains("not found")),
        );
    }

    // Test run command validation
    #[test]
    fn test_run_without_project_name() {
        let mut cmd = Command::new(nockup_bin());
        cmd.arg("run");
        cmd.assert()
            .failure()
            .stderr(predicate::str::contains("required"));
    }

    // Test channel command validation
    #[test]
    fn test_channel_without_subcommand() {
        let mut cmd = Command::new(nockup_bin());
        cmd.arg("channel");
        cmd.assert()
            .failure()
            .stderr(predicate::str::contains("nockup channel <COMMAND>"));
    }

    #[test]
    fn test_channel_show_with_extra_args() {
        let mut cmd = Command::new(nockup_bin());
        cmd.args(["channel", "show", "extra-arg"]);
        cmd.assert()
            .failure()
            .stderr(predicate::str::contains("unexpected argument"));
    }

    #[test]
    fn test_channel_set_without_channel_name() {
        let mut cmd = Command::new(nockup_bin());
        cmd.args(["channel", "set"]);
        cmd.assert()
            .failure()
            .stderr(predicate::str::contains("required"));
    }

    #[test]
    fn test_channel_set_invalid_channel() {
        let mut cmd = Command::new(nockup_bin());
        cmd.args(["channel", "set", "invalid-channel"]);
        cmd.assert()
            .failure()
            .stderr(predicate::str::contains("Invalid channel"));
    }

    #[test]
    fn test_channel_set_valid_channels() {
        let channels = vec!["stable", "nightly"];

        for channel in channels {
            let mut cmd = Command::new(nockup_bin());
            cmd.args(["channel", "set", channel]);

            // This might fail due to missing cache, but shouldn't fail on channel validation
            let output = cmd.output().expect("Failed to run command");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(!stderr.contains("invalid channel"));
        }
    }

    // Test configuration file validation (if manifest is required)
    #[test]
    fn test_build_without_manifest() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let project_dir = temp_dir.path().join("test-project");
        std::fs::create_dir_all(&project_dir).expect("Failed to create project dir");

        let mut cmd = Command::new(nockup_bin());
        cmd.current_dir(&project_dir).args(["build", "."]);
        cmd.assert().failure().stderr(predicate::str::contains(
            "Error: Not a NockApp project: '.' missing manifest.toml",
        ));
    }
}

// Unit tests for argument parsing (if you have a separate args module)
#[cfg(test)]
mod unit_input_validation_tests {
    // use super::*;

    // These would test your argument parsing functions directly
    // Example assuming you have a validate_project_name function:

    /*
    #[test]
    fn test_validate_project_name_valid() {
        assert!(validate_project_name("valid-project").is_ok());
        assert!(validate_project_name("valid_project").is_ok());
        assert!(validate_project_name("project123").is_ok());
    }

    #[test]
    fn test_validate_project_name_invalid() {
        assert!(validate_project_name("").is_err());
        assert!(validate_project_name("project with spaces").is_err());
        assert!(validate_project_name("project/with/slashes").is_err());
        assert!(validate_project_name("project@with@symbols").is_err());
    }

    #[test]
    fn test_validate_channel_name() {
        assert!(validate_channel_name("stable").is_ok());
        assert!(validate_channel_name("nightly").is_ok());
        assert!(validate_channel_name("invalid").is_err());
        assert!(validate_channel_name("").is_err());
    }
    */
}

// Property-based testing example (add proptest = "1.0" to dev-dependencies)
// #[cfg(feature = "proptest")]
// mod property_tests {
//     use proptest::prelude::*;

//     proptest! {
//         #[test]
//         fn test_project_name_chars(s in "[a-zA-Z0-9_-]{1,50}") {
//             // Valid project names should only contain alphanumeric, underscore, hyphen
//             let mut cmd = Command::cargo_bin("nockup").unwrap();
//             cmd.args(&["start", &s]);
//             let output = cmd.output().unwrap();
//             let stderr = String::from_utf8_lossy(&output.stderr);
//             // Should not fail on name validation (might fail for other reasons)
//             assert!(!stderr.contains("invalid project name"));
//         }

//         #[test]
//         fn test_invalid_project_name_chars(s in "[^a-zA-Z0-9_-]+") {
//             // Invalid characters should be rejected
//             let mut cmd = Command::cargo_bin("nockup").unwrap();
//             cmd.args(&["start", &s]);
//             cmd.assert().failure();
//         }
//     }
// }

// Helper functions for test setup
#[cfg(test)]
mod test_helpers {
    // use super::*;
    // use std::fs;
}
