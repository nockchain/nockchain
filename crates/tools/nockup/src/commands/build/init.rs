use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use colored::Colorize;
use handlebars::Handlebars;

use crate::manifest::NockAppManifest;

pub async fn run() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let manifest_path = cwd.join("nockapp.toml");

    if !manifest_path.exists() {
        anyhow::bail!(
            "No nockapp.toml found in current directory.\n\
             → Create one with your desired name, template, and dependencies,\n\
             → then run `nockup project init` again."
        );
    }

    let manifest = NockAppManifest::load(&manifest_path).context("Failed to parse nockapp.toml")?;

    let project_name = manifest.package.name.trim();
    if project_name.is_empty() {
        anyhow::bail!("package.name in nockapp.toml cannot be empty");
    }

    let template_name = manifest.package.template.as_deref().unwrap_or("basic");

    let template_commit = manifest.package.template_commit.as_deref();

    println!(
        "Initializing new NockApp project '{}' using template '{}'...",
        project_name.green(),
        template_name.cyan()
    );

    let target_dir = Path::new(project_name);
    if target_dir.exists() {
        anyhow::bail!(
            "Directory '{}' already exists. Remove it or choose a different name.", project_name
        );
    }

    // Resolve template directory (supports pinned commit)
    let cache_dir = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?
        .join(".nockup/templates");

    let template_src = if let Some(commit) = template_commit {
        cache_dir.join(format!("{}-{}", template_name, commit))
    } else {
        cache_dir.join(template_name)
    };

    if !template_src.exists() {
        anyhow::bail!(
            "Template '{}' not found in cache at {}.\n\
             Run `nockup channel update` or check your template-commit hash.",
            template_name,
            template_src.display()
        );
    }

    // Build Handlebars context from manifest (same as your old one, but cleaner)
    let context = build_handlebars_context(&manifest)?;

    // Copy and render the template
    copy_and_render_template(&template_src, target_dir, &context)?;

    // Write the canonical nockapp.toml into the new project (exact copy of source)
    let final_manifest_path = target_dir.join("nockapp.toml");
    manifest.save(&final_manifest_path)?;

    println!("Running dependency installation…");
    // Package install will automatically detect the project directory based on manifest name
    crate::commands::package::install::run()
        .await
        .context("Failed to install dependencies")?;

    println!("\nAll done! Project is ready.");
    println!("   cd {}", project_name.cyan());
    println!("   nockup run");
    Ok(())
}

fn build_handlebars_context(manifest: &NockAppManifest) -> Result<HashMap<String, String>> {
    let mut ctx = HashMap::new();
    let p = &manifest.package;

    ctx.insert("name".to_string(), p.name.clone());
    ctx.insert("project_name".to_string(), p.name.clone());
    ctx.insert("version".to_string(), p.version.clone().unwrap_or_default());
    ctx.insert(
        "description".to_string(),
        p.description.clone().unwrap_or_default(),
    );
    ctx.insert(
        "author".to_string(),
        p.authors.clone().unwrap_or_default().join(", "),
    );

    Ok(ctx)
}

fn copy_and_render_template(
    src_dir: &Path,
    dest_dir: &Path,
    context: &HashMap<String, String>,
) -> Result<()> {
    let handlebars = Handlebars::new();

    fs::create_dir_all(dest_dir)?;

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
    for entry in fs::read_dir(src_dir)? {
        let entry = entry?;
        let src_path = entry.path();
        let file_name = entry.file_name();
        let dest_path = dest_dir.join(&file_name);

        if src_path.is_dir() {
            fs::create_dir_all(&dest_path)?;
            copy_dir_recursive(&src_path, &dest_path, handlebars, context, project_root)?;
        } else {
            let content = fs::read_to_string(&src_path)?;
            let rendered = handlebars
                .render_template(&content, context)
                .with_context(|| format!("Template error in {}", src_path.display()))?;

            fs::write(&dest_path, rendered)?;
            let rel = dest_path.strip_prefix(project_root).unwrap_or(&dest_path);
            println!("  {} {}", "create".green(), rel.display());
        }
    }
    Ok(())
}
