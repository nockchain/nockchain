use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use colored::Colorize;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LibrarySpec {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ProjectManifest {
    pub project: ProjectInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub libraries: Option<HashMap<String, LibrarySpec>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ProjectInfo {
    pub name: String,
    pub project_name: String,
    pub version: String,
    pub description: String,
    pub author_name: String,
    pub author_email: String,
    pub github_username: String,
    pub license: String,
    pub keywords: Vec<String>,
    pub nockapp_commit_hash: String,
    pub template: String,
}

pub async fn process_libraries(project_dir: &Path, manifest: &ProjectManifest) -> Result<()> {
    if let Some(libraries) = &manifest.libraries {
        if libraries.is_empty() {
            return Ok(());
        }

        println!("{} Processing library dependencies...", "📚".cyan());

        let cache_dir = get_library_cache_dir()?;
        let project_lib_dir = project_dir.join("hoon").join("lib");

        // Ensure library directory exists
        fs::create_dir_all(&project_lib_dir)
            .context("Failed to create project library directory")?;

        for (lib_name, lib_spec) in libraries {
            println!(
                "  {} Fetching library '{}'...",
                "⬇️".green(),
                lib_name.cyan()
            );

            // Validate library spec
            if let Err(e) = validate_library_spec(lib_spec) {
                println!("    ❌ Validation failed for '{}': {}", lib_name, e);
                return Err(e);
            }

            // Get or clone the repository
            let repo_dir = match fetch_library_repo(&cache_dir, lib_name, lib_spec).await {
                Ok(dir) => dir,
                Err(e) => {
                    println!(
                        "    ❌ Failed to fetch repository for '{}': {}",
                        lib_name, e
                    );
                    return Err(e);
                }
            };

            // Handle single file vs directory/full library
            if let Some(file_path) = &lib_spec.file {
                // Single file import
                copy_single_file(&repo_dir, &project_lib_dir, file_path)?;
            } else {
                // Full library import - find the appropriate source directory (desk or hoon)
                let source_dir = match find_library_source_dir(&repo_dir, lib_spec) {
                    Ok(dir) => dir,
                    Err(e) => {
                        println!(
                            "    ❌ Failed to find source directory for '{}': {}",
                            lib_name, e
                        );
                        return Err(e);
                    }
                };

                // Copy library files to project
                if let Err(e) =
                    copy_library_files(&source_dir, &project_lib_dir, lib_name, lib_spec)
                {
                    println!("    ❌ Failed to copy files for '{}': {}", lib_name, e);
                    return Err(e);
                }
            }

            println!("    ✓ Installed library '{}'", lib_name);
        }

        println!("{} All libraries processed successfully!", "✓".green());
    }

    Ok(())
}

fn validate_library_spec(spec: &LibrarySpec) -> Result<()> {
    // Ensure mutually exclusive branch/commit
    match (&spec.branch, &spec.commit) {
        (Some(_), Some(_)) => {
            return Err(anyhow::anyhow!(
                "Library spec cannot have both 'branch' and 'commit' specified. Please use only one."
            ));
        }
        (None, None) => {
            return Err(anyhow::anyhow!(
                "Library spec must specify either 'branch' or 'commit'"
            ));
        }
        _ => {} // Valid: exactly one of branch or commit is specified
    }

    // Ensure mutually exclusive directory/file
    if let (Some(_), Some(_)) = (&spec.directory, &spec.file) {
        return Err(anyhow::anyhow!(
            "Library spec cannot have both 'directory' and 'file' specified. Please use only one."
        ));
    }

    // Validate URL is GitHub (for now)
    if !spec.url.starts_with("https://github.com/") {
        return Err(anyhow::anyhow!(
            "Only GitHub repositories are currently supported. URL must start with 'https://github.com/'"
        ));
    }

    Ok(())
}

fn get_library_cache_dir() -> Result<PathBuf> {
    let cache_dir = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?
        .join(".nockup")
        .join("library_cache");

    fs::create_dir_all(&cache_dir).context("Failed to create library cache directory")?;

    Ok(cache_dir)
}

async fn fetch_library_repo(
    cache_dir: &Path,
    _lib_name: &str,
    spec: &LibrarySpec,
) -> Result<PathBuf> {
    // Create a unique directory name based on URL and commit/branch
    let repo_name = extract_repo_name(&spec.url)?;
    let unique_id = match (&spec.commit, &spec.branch) {
        (Some(commit), None) => commit.clone(),
        (None, Some(branch)) => branch.clone(),
        _ => unreachable!(), // Already validated
    };

    let repo_cache_dir = cache_dir.join(format!("{}_{}", repo_name, unique_id));

    // If already cached, return it
    if repo_cache_dir.exists() {
        return Ok(repo_cache_dir);
    }

    // Clone the repository
    println!("    ⬇️ Cloning repository...");

    let mut git_cmd = Command::new("git");
    git_cmd.args(["clone", &spec.url]);

    // If branch specified, clone that branch
    if let Some(branch) = &spec.branch {
        git_cmd.args(["--branch", branch]);
    }

    git_cmd.arg(&repo_cache_dir);

    let output = git_cmd.output().context("Failed to execute git clone")?;

    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "Git clone failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    // If commit specified, checkout that commit
    if let Some(commit) = &spec.commit {
        let checkout_output = Command::new("git")
            .args(["checkout", commit])
            .current_dir(&repo_cache_dir)
            .output()
            .context("Failed to checkout commit")?;

        if !checkout_output.status.success() {
            return Err(anyhow::anyhow!(
                "Git checkout failed: {}",
                String::from_utf8_lossy(&checkout_output.stderr)
            ));
        }
    }

    Ok(repo_cache_dir)
}

fn extract_repo_name(url: &str) -> Result<String> {
    // Extract repository name from GitHub URL
    // https://github.com/user/repo -> repo
    let parts: Vec<&str> = url.trim_end_matches('/').split('/').collect();
    if parts.len() < 2 {
        return Err(anyhow::anyhow!("Invalid GitHub URL format"));
    }

    let repo_name = parts[parts.len() - 1];
    let repo_name = repo_name.trim_end_matches(".git");

    Ok(repo_name.to_string())
}

fn find_library_source_dir(repo_dir: &Path, spec: &LibrarySpec) -> Result<PathBuf> {
    let base_dir = if let Some(directory) = &spec.directory {
        repo_dir.join(directory)
    } else {
        repo_dir.to_path_buf()
    };

    // Look for /desk or /hoon directory
    let desk_dir = base_dir.join("desk");
    let hoon_dir = base_dir.join("hoon");
    let src_dir = base_dir.join("src");

    if desk_dir.exists() {
        Ok(desk_dir)
    } else if hoon_dir.exists() {
        Ok(hoon_dir)
    } else if src_dir.exists() {
        Ok(src_dir)
    } else {
        Err(anyhow::anyhow!(
            "No '/desk' or '/hoon' or '/src' directory found in repository. Expected Hoon library structure not found."
        ))
    }
}

fn copy_library_files(
    source_dir: &Path,
    dest_lib_dir: &Path,
    _lib_name: &str,
    _spec: &LibrarySpec,
) -> Result<()> {
    // Always use flattened approach - copy contents directly to appropriate directories
    let project_hoon_dir = dest_lib_dir
        .parent()
        .expect("dest_lib_dir should have a parent directory"); // Get /hoon from /hoon/lib

    copy_top_level_library(source_dir, project_hoon_dir, source_dir)?;

    Ok(())
}

#[allow(dead_code)]
fn ensure_directory_exists(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)
        .with_context(|| format!("Failed to create directory '{}'", dir.display()))
}

fn copy_single_file(repo_dir: &Path, project_lib_dir: &Path, file_path: &str) -> Result<()> {
    let source_file = repo_dir.join(file_path);

    // Check if the source file exists
    if !source_file.exists() {
        return Err(anyhow::anyhow!(
            "File '{}' not found in repository", file_path
        ));
    }

    // Determine destination based on file path structure
    let file_name = source_file
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Invalid file path: {}", file_path))?;

    // Get the project's /hoon directory (parent of /hoon/lib)
    let project_hoon_dir = project_lib_dir
        .parent()
        .expect("project_lib_dir should have a parent directory");

    // Determine destination directory based on file path
    let dest_dir = if file_path.contains("/lib/") {
        project_hoon_dir.join("lib")
    } else if file_path.contains("/sur/") {
        project_hoon_dir.join("sur")
    } else if file_path.contains("/app/") {
        project_hoon_dir.join("app")
    } else {
        // Default to lib if no specific directory is found
        project_hoon_dir.join("lib")
    };

    // Ensure destination directory exists
    fs::create_dir_all(&dest_dir)
        .with_context(|| format!("Failed to create directory '{}'", dest_dir.display()))?;

    // Copy the file
    let dest_file = dest_dir.join(file_name);
    fs::copy(&source_file, &dest_file).with_context(|| {
        format!(
            "Failed to copy file '{}' to '{}'",
            source_file.display(),
            dest_file.display()
        )
    })?;

    println!("      copy {}", file_path);

    Ok(())
}

fn copy_top_level_library(src_dir: &Path, dest_dir: &Path, root_src: &Path) -> Result<()> {
    for entry in fs::read_dir(src_dir)
        .with_context(|| format!("Failed to read directory '{}'", src_dir.display()))?
    {
        let entry = entry?;
        let src_path = entry.path();
        let file_name = entry.file_name();
        let file_name_str = file_name.to_string_lossy();

        // Skip excluded directories
        if src_path.is_dir() && (file_name_str == "mar" || file_name_str == "tests") {
            continue;
        }

        if src_path.is_dir() {
            // Create the corresponding directory in destination and copy its contents
            let dest_subdir = dest_dir.join(&file_name);
            fs::create_dir_all(&dest_subdir).with_context(|| {
                format!("Failed to create directory '{}'", dest_subdir.display())
            })?;
            copy_directory_contents(&src_path, &dest_subdir, root_src)?;
        } else if should_copy_file(&src_path) {
            let dest_path = dest_dir.join(&file_name);
            fs::copy(&src_path, &dest_path)
                .with_context(|| format!("Failed to copy file '{}'", src_path.display()))?;

            let relative_src = src_path.strip_prefix(root_src).unwrap_or(&src_path);
            println!("      copy {}", relative_src.display());
        }
    }

    Ok(())
}

fn copy_directory_contents(src_dir: &Path, dest_dir: &Path, root_src: &Path) -> Result<()> {
    for entry in fs::read_dir(src_dir)
        .with_context(|| format!("Failed to read directory '{}'", src_dir.display()))?
    {
        let entry = entry?;
        let src_path = entry.path();
        let file_name = entry.file_name();

        let dest_path = dest_dir.join(&file_name);

        if src_path.is_dir() {
            // Create subdirectory and recurse
            fs::create_dir_all(&dest_path)
                .with_context(|| format!("Failed to create directory '{}'", dest_path.display()))?;
            copy_directory_contents(&src_path, &dest_path, root_src)?;
        } else {
            // Copy file (only .hoon files and other relevant extensions)
            if should_copy_file(&src_path) {
                fs::copy(&src_path, &dest_path)
                    .with_context(|| format!("Failed to copy file '{}'", src_path.display()))?;

                // Show relative path for cleaner output
                let relative_src = src_path.strip_prefix(root_src).unwrap_or(&src_path);
                println!("      copy {}", relative_src.display());
            }
        }
    }

    Ok(())
}

fn should_copy_file(path: &Path) -> bool {
    if let Some(extension) = path.extension() {
        let ext = extension.to_string_lossy().to_lowercase();
        // Copy .hoon files and other relevant Hoon ecosystem files
        matches!(ext.as_str(), "hoon" | "hoon-mark" | "kelvin")
    } else {
        // Copy files without extensions that might be relevant
        false
    }
}
