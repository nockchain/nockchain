use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use colored::Colorize;
use handlebars::Handlebars;

use crate::lib_manager::{process_libraries, ProjectManifest};

pub async fn run(project_name: String) -> Result<()> {
    // Load the project-specific manifest configuration
    let manifest = load_project_config(&project_name)?;
    let project_name = &manifest.project.project_name;

    println!(
        "Initializing new NockApp project '{}'...",
        project_name.green()
    );

    let target_dir = Path::new(project_name);
    // Use cache dir ~/.nockup/templates/{{manifest.template}}
    let template_dir = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?
        .join(format!(".nockup/templates/{}", manifest.project.template));

    // Check if target directory already exists
    if target_dir.exists() {
        return Err(anyhow::anyhow!(
            "Directory '{}' already exists. Please choose a different name or remove the existing directory.", project_name
        ));
    }

    // Check if template directory exists
    if !template_dir.exists() {
        return Err(anyhow::anyhow!(
            "Template directory '{}' not found. Make sure nockup is run from the correct directory.",
            template_dir.display()
        ));
    }

    // Create template context from the project config
    let context = create_template_context(&manifest)?;

    // Copy template directory to new project location
    copy_template_directory(template_dir.as_path(), target_dir, &context)?;

    // Process library dependencies from manifest
    process_libraries(target_dir, &manifest)
        .await
        .context("Failed to process library dependencies")?;

    println!(
        "{} New project created in {}/",
        "✓".green(),
        format!("./{}/", project_name).cyan()
    );
    println!("To get started:");
    println!("  nockup build {}", project_name.cyan());
    println!("  nockup run {}", project_name.cyan());

    Ok(())
}

fn load_project_config(project_name: &str) -> Result<ProjectManifest> {
    let config_filename = format!("{}.toml", project_name);
    let config_path = Path::new(&config_filename);

    if !config_path.exists() {
        return Err(anyhow::anyhow!(
            "Project configuration file '{}.toml' not found", project_name
        ));
    }

    let config_content = fs::read_to_string(config_path)
        .with_context(|| format!("Failed to read {}.toml", project_name))?;

    println!(
        "{} Loaded project configuration from '{}'",
        "✓".green(),
        config_filename.cyan()
    );
    println!("Config content:\n{}", config_content);
    toml::from_str(&config_content)
        .with_context(|| format!("Failed to parse {}.toml", project_name))
}

fn create_template_context(manifest: &ProjectManifest) -> Result<HashMap<String, String>> {
    let mut context = HashMap::new();

    // Add all values directly from manifest
    context.insert("name".to_string(), manifest.project.name.clone());
    context.insert(
        "project_name".to_string(),
        manifest.project.project_name.clone(),
    );
    context.insert("version".to_string(), manifest.project.version.clone());
    context.insert(
        "project_description".to_string(),
        manifest.project.description.clone(),
    );
    context.insert(
        "description".to_string(),
        manifest.project.description.clone(),
    );
    context.insert(
        "author_name".to_string(),
        manifest.project.author_name.clone(),
    );
    context.insert(
        "author_email".to_string(),
        manifest.project.author_email.clone(),
    );
    context.insert(
        "github_username".to_string(),
        manifest.project.github_username.clone(),
    );
    context.insert("license".to_string(), manifest.project.license.clone());
    context.insert(
        "keywords".to_string(),
        manifest.project.keywords.join("\", \""),
    );
    context.insert(
        "nockapp_commit_hash".to_string(),
        manifest.project.nockapp_commit_hash.clone(),
    );
    context.insert("template".to_string(), manifest.project.template.clone());

    Ok(context)
}

fn copy_template_directory(
    src_dir: &Path,
    dest_dir: &Path,
    context: &HashMap<String, String>,
) -> Result<()> {
    let handlebars = Handlebars::new();

    // Create the destination directory
    fs::create_dir_all(dest_dir)
        .with_context(|| format!("Failed to create directory '{}'", dest_dir.display()))?;

    // Recursively copy and process template directory
    copy_dir_recursive(src_dir, dest_dir, &handlebars, context, dest_dir)?;

    Ok(())
}

fn copy_dir_recursive(
    src_dir: &Path,
    dest_dir: &Path,
    handlebars: &Handlebars,
    context: &HashMap<String, String>,
    project_root: &Path,
) -> Result<()> {
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
            copy_dir_recursive(&src_path, &dest_path, handlebars, context, project_root)?;
        } else {
            // Copy and process file
            let content = fs::read_to_string(&src_path)
                .with_context(|| format!("Failed to read file '{}'", src_path.display()))?;

            // Process template variables in file content
            let processed_content =
                handlebars
                    .render_template(&content, context)
                    .with_context(|| {
                        format!("Failed to process template for '{}'", src_path.display())
                    })?;

            fs::write(&dest_path, processed_content)
                .with_context(|| format!("Failed to write file '{}'", dest_path.display()))?;

            // Show relative path from project root for cleaner output
            let relative_path = dest_path.strip_prefix(project_root).unwrap_or(&dest_path);
            println!("  {} {}", "create".green(), relative_path.display());
        }
    }

    Ok(())
}
