use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use colored::Colorize;
use handlebars::Handlebars;

use crate::git_fetcher::{GitFetcher, GitSpec};
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

    // Resolve template directory. Three sources, in priority order:
    //   1. `template_git` — fetch from any git URL (incl. `file://`)
    //   2. `template_commit` pin — channel cache, commit-suffixed
    //   3. plain channel cache (~/.nockup/templates/<name>/)
    let cache_dir = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?
        .join(".nockup/templates");

    let template_src = if let Some(git_url) = manifest.package.template_git.as_deref() {
        resolve_git_template(
            &cache_dir,
            git_url,
            template_commit,
            manifest.package.template_path.as_deref(),
            template_name,
        )
        .await?
    } else if let Some(commit) = template_commit {
        cache_dir.join(format!("{}-{}", template_name, commit))
    } else {
        cache_dir.join(template_name)
    };

    if !template_src.exists() {
        anyhow::bail!(
            "Template '{}' not found at {}.\n\
             {}",
            template_name,
            template_src.display(),
            if manifest.package.template_git.is_some() {
                "Verify `template_git`, `template_path`, and that `template` names a directory under that path."
            } else {
                "Run `nockup channel update` or check your template-commit hash."
            }
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

/// Fetch a template from a git URL via [`GitFetcher`] and resolve the
/// concrete template directory under it.
///
/// Layout:
///   - When `template_path` is set, the template lives at
///     `<repo>/<template_path>/<template_name>/`.
///   - When unset, it lives at `<repo>/<template_name>/`.
///
/// `template_commit` pins the fetch to a specific revision; when unset
/// `GitFetcher` resolves whatever the URL's HEAD points at. The cache
/// directory is shared with the channel cache (`~/.nockup/templates/git/`)
/// and keyed by URL hash + commit, so repeat fetches are no-ops.
async fn resolve_git_template(
    cache_dir: &Path,
    git_url: &str,
    template_commit: Option<&str>,
    template_path: Option<&str>,
    template_name: &str,
) -> Result<PathBuf> {
    let fetcher = GitFetcher::new(cache_dir.join("git"));
    let spec = GitSpec {
        url: git_url.to_string(),
        commit: template_commit.map(str::to_string),
        tag: None,
        branch: None,
        path: None,
        install_path: None,
        file: None,
    };
    let repo_root = fetcher
        .fetch(&spec)
        .await
        .with_context(|| format!("Failed to fetch template repo {}", git_url))?;

    let sub = template_path.unwrap_or("").trim_matches('/');
    let resolved = if sub.is_empty() {
        repo_root.join(template_name)
    } else {
        repo_root.join(sub).join(template_name)
    };
    Ok(resolved)
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
