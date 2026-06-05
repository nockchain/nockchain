use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use colored::Colorize;
use handlebars::{no_escape, Handlebars};

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
        // TODO: template_commit currently relies on a pre-existing
        // `<template>-<commit>` cache directory that `nockup channel update`
        // does not populate. Commit metadata is also read from the root
        // template cache, so pinned template revs need a dedicated fix.
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
    let mut context = build_handlebars_context(&manifest)?;
    apply_template_source_context(&mut context, &template_src)?;

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
    let authors = p.authors.clone().unwrap_or_default();
    let author = authors.join(", ");
    let (author_name, author_email) = authors
        .first()
        .map(|author| split_author_name_email(author))
        .unwrap_or_default();

    ctx.insert("name".to_string(), p.name.clone());
    ctx.insert("project_name".to_string(), p.name.clone());
    ctx.insert("rust_crate_name".to_string(), rust_crate_name(&p.name));
    ctx.insert(
        "version".to_string(),
        p.version.clone().unwrap_or_else(|| "0.1.0".to_string()),
    );
    let description = p.description.clone().unwrap_or_default();
    let toml_description = toml::Value::String(description.clone()).to_string();
    ctx.insert("description".to_string(), description.clone());
    ctx.insert("project_description".to_string(), description);
    ctx.insert("toml_description".to_string(), toml_description);
    ctx.insert("author".to_string(), author);
    ctx.insert("author_name".to_string(), author_name);
    ctx.insert("author_email".to_string(), author_email);
    ctx.insert(
        "toml_authors".to_string(),
        toml::Value::Array(authors.into_iter().map(toml::Value::String).collect()).to_string(),
    );
    ctx.insert("license".to_string(), p.license.clone().unwrap_or_default());
    ctx.insert(
        "nockapp_commit_hash".to_string(),
        env!("GIT_HASH").to_string(),
    );

    Ok(ctx)
}

fn apply_template_source_context(
    ctx: &mut HashMap<String, String>,
    template_src: &Path,
) -> Result<()> {
    if let Some(commit_hash) = template_source_commit_hash(template_src)? {
        ctx.insert("nockapp_commit_hash".to_string(), commit_hash);
    }

    Ok(())
}

fn template_source_commit_hash(template_src: &Path) -> Result<Option<String>> {
    let Some(template_cache_dir) = template_src.parent() else {
        return Ok(None);
    };
    let commit_path = template_cache_dir.join("commit.toml");

    if !commit_path.exists() {
        return Ok(None);
    }

    let commit_toml = fs::read_to_string(&commit_path)
        .with_context(|| format!("Failed to read {}", commit_path.display()))?;
    let commit_toml: toml::Value = toml::from_str(&commit_toml)
        .with_context(|| format!("Failed to parse {}", commit_path.display()))?;
    let commit_hash = commit_toml
        .get("commit")
        .and_then(|commit| commit.get("id"))
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|commit| !commit.is_empty());

    Ok(commit_hash.map(ToOwned::to_owned))
}

fn split_author_name_email(author: &str) -> (String, String) {
    let author = author.trim();

    if let Some((name, email)) = author.rsplit_once('<') {
        if let Some(email) = email.trim().strip_suffix('>') {
            return (name.trim().to_string(), email.trim().to_string());
        }
    }

    (author.to_string(), String::new())
}

fn rust_crate_name(project_name: &str) -> String {
    project_name.replace('-', "_")
}

fn copy_and_render_template(
    src_dir: &Path,
    dest_dir: &Path,
    context: &HashMap<String, String>,
) -> Result<()> {
    let mut handlebars = Handlebars::new();
    handlebars.set_strict_mode(true);
    handlebars.register_escape_fn(no_escape);

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

        if src_path.is_dir() {
            let dest_path = dest_dir.join(&file_name);
            fs::create_dir_all(&dest_path)?;
            copy_dir_recursive(&src_path, &dest_path, handlebars, context, project_root)?;
        } else {
            let dest_path = dest_dir.join(rendered_file_name(&file_name));
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

fn rendered_file_name(file_name: &std::ffi::OsStr) -> std::ffi::OsString {
    file_name
        .to_str()
        .and_then(|name| name.strip_suffix(".hbs"))
        .map(Into::into)
        .unwrap_or_else(|| file_name.to_os_string())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use tempfile::tempdir;

    use super::*;
    use crate::manifest::PackageMeta;

    const TEST_NOCKCHAIN_REV: &str = "0123456789abcdef0123456789abcdef01234567";

    fn complete_template_context() -> HashMap<String, String> {
        HashMap::from([
            ("name".to_string(), "arcadia".to_string()),
            ("project_name".to_string(), "arcadia".to_string()),
            ("rust_crate_name".to_string(), "arcadia".to_string()),
            ("version".to_string(), "0.1.0".to_string()),
            ("description".to_string(), "Example app".to_string()),
            ("project_description".to_string(), "Example app".to_string()),
            (
                "toml_description".to_string(),
                r#""Example app""#.to_string(),
            ),
            (
                "author".to_string(),
                "Ada Lovelace <ada@example.com>".to_string(),
            ),
            ("author_name".to_string(), "Ada Lovelace".to_string()),
            ("author_email".to_string(), "ada@example.com".to_string()),
            (
                "toml_authors".to_string(),
                r#"["Ada Lovelace <ada@example.com>"]"#.to_string(),
            ),
            (
                "nockapp_commit_hash".to_string(),
                TEST_NOCKCHAIN_REV.to_string(),
            ),
        ])
    }

    fn complete_template_context_for_project(project_name: &str) -> HashMap<String, String> {
        let mut context = complete_template_context();
        context.insert("name".to_string(), project_name.to_string());
        context.insert("project_name".to_string(), project_name.to_string());
        context.insert(
            "rust_crate_name".to_string(),
            project_name.replace('-', "_"),
        );
        context
    }

    fn bundled_template_dirs() -> Vec<PathBuf> {
        let templates_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
        let mut dirs = fs::read_dir(&templates_dir)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", templates_dir.display()))
            .map(|entry| entry.expect("template dir entry should be readable").path())
            .filter(|path| path.is_dir())
            .collect::<Vec<_>>();
        dirs.sort();
        dirs
    }

    fn bundled_template_dir(template_name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("templates")
            .join(template_name)
    }

    fn render_template_dir(
        template_dir: &Path,
        context: &HashMap<String, String>,
    ) -> (tempfile::TempDir, PathBuf) {
        let tmp = tempdir().expect("tempdir should be created");
        let dest = tmp.path().join("project");

        copy_and_render_template(template_dir, &dest, context)
            .unwrap_or_else(|err| panic!("failed to render {}: {err}", template_dir.display()));

        (tmp, dest)
    }

    fn rendered_cargo_manifest(
        template_dir: &Path,
        context: &HashMap<String, String>,
    ) -> toml::Value {
        let (_tmp, dest) = render_template_dir(template_dir, context);
        let cargo_toml_path = dest.join("Cargo.toml");
        assert!(
            cargo_toml_path.exists(),
            "{} should render Cargo.toml",
            template_dir.display()
        );
        assert!(
            !dest.join("Cargo.toml.hbs").exists(),
            "{} should not render Cargo.toml.hbs",
            template_dir.display()
        );

        let cargo_toml = fs::read_to_string(&cargo_toml_path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", cargo_toml_path.display()));
        assert!(
            !cargo_toml.contains("{{") && !cargo_toml.contains("}}"),
            "{} contains unresolved template placeholders",
            cargo_toml_path.display()
        );
        toml::from_str::<toml::Value>(&cargo_toml)
            .unwrap_or_else(|err| panic!("invalid TOML in {}: {err}", cargo_toml_path.display()))
    }

    fn assert_nockchain_deps_use_rev(cargo_toml: &toml::Value, expected_rev: &str) {
        let deps = cargo_toml
            .get("dependencies")
            .and_then(toml::Value::as_table)
            .expect("Cargo.toml should contain [dependencies]");
        let mut nockchain_dep_count = 0;

        for (dep_name, dep_spec) in deps {
            let Some(dep_table) = dep_spec.as_table() else {
                continue;
            };
            if dep_table.get("git").and_then(toml::Value::as_str)
                == Some("https://github.com/nockchain/nockchain.git")
            {
                nockchain_dep_count += 1;
                assert_eq!(
                    dep_table.get("rev").and_then(toml::Value::as_str),
                    Some(expected_rev),
                    "{dep_name} should use the rendered Nockchain rev"
                );
            }
        }

        assert!(
            nockchain_dep_count > 0,
            "Cargo.toml should include Nockchain git dependencies"
        );
    }

    #[test]
    fn copy_and_render_template_strips_hbs_suffix_from_output_paths() {
        let tmp = tempdir().expect("tempdir should be created");
        let src = tmp.path().join("template");
        let dest = tmp.path().join("project");
        fs::create_dir_all(&src).expect("template dir should be created");
        fs::write(
            src.join("Cargo.toml.hbs"),
            r#"[package]
name = "{{project_name}}"
version = "{{version}}"
edition = "2021"
"#,
        )
        .expect("template manifest should be written");

        copy_and_render_template(&src, &dest, &complete_template_context())
            .expect("template should render");

        assert!(dest.join("Cargo.toml").exists());
        assert!(!dest.join("Cargo.toml.hbs").exists());
        let rendered =
            fs::read_to_string(dest.join("Cargo.toml")).expect("manifest should be readable");
        assert!(rendered.contains(r#"name = "arcadia""#));
    }

    #[test]
    fn build_handlebars_context_includes_legacy_template_keys() {
        let manifest = NockAppManifest {
            package: PackageMeta {
                name: "arcadia".to_string(),
                version: Some("0.2.0".to_string()),
                description: Some("Example app".to_string()),
                authors: Some(vec!["Ada Lovelace <ada@example.com>".to_string()]),
                ..Default::default()
            },
            ..Default::default()
        };

        let ctx = build_handlebars_context(&manifest).expect("context should build");

        assert_eq!(ctx.get("project_name"), Some(&"arcadia".to_string()));
        assert_eq!(ctx.get("rust_crate_name"), Some(&"arcadia".to_string()));
        assert_eq!(ctx.get("version"), Some(&"0.2.0".to_string()));
        assert_eq!(ctx.get("description"), Some(&"Example app".to_string()));
        assert_eq!(
            ctx.get("toml_description"),
            Some(&r#""Example app""#.to_string())
        );
        assert_eq!(
            ctx.get("author"),
            Some(&"Ada Lovelace <ada@example.com>".to_string())
        );
        assert_eq!(ctx.get("author_name"), Some(&"Ada Lovelace".to_string()));
        assert_eq!(
            ctx.get("author_email"),
            Some(&"ada@example.com".to_string())
        );
        assert_eq!(
            ctx.get("toml_authors"),
            Some(&r#"["Ada Lovelace <ada@example.com>"]"#.to_string())
        );
        assert!(ctx
            .get("nockapp_commit_hash")
            .is_some_and(|rev| !rev.trim().is_empty()));
    }

    #[test]
    fn build_handlebars_context_includes_rust_crate_name_for_hyphenated_packages() {
        let manifest = NockAppManifest {
            package: PackageMeta {
                name: "arcadia-app".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };

        let ctx = build_handlebars_context(&manifest).expect("context should build");

        assert_eq!(ctx.get("project_name"), Some(&"arcadia-app".to_string()));
        assert_eq!(ctx.get("rust_crate_name"), Some(&"arcadia_app".to_string()));
    }

    #[test]
    fn template_source_commit_overrides_build_hash_in_context() {
        let tmp = tempdir().expect("tempdir should be created");
        let template_src = tmp.path().join("basic");
        fs::create_dir_all(&template_src).expect("template dir should be created");
        fs::write(
            tmp.path().join("commit.toml"),
            format!("[commit]\nid = \"{TEST_NOCKCHAIN_REV}\"\n"),
        )
        .expect("commit file should be written");
        let mut ctx = HashMap::from([(
            "nockapp_commit_hash".to_string(),
            "fallback-rev".to_string(),
        )]);

        apply_template_source_context(&mut ctx, &template_src).expect("context should update");

        assert_eq!(
            ctx.get("nockapp_commit_hash"),
            Some(&TEST_NOCKCHAIN_REV.to_string())
        );
    }

    #[test]
    fn bundled_templates_store_cargo_manifests_as_hbs_sources() {
        for template_dir in bundled_template_dirs() {
            assert!(
                !template_dir.join("Cargo.toml").exists(),
                "{} should store its manifest as Cargo.toml.hbs",
                template_dir.display()
            );
            assert!(
                template_dir.join("Cargo.toml.hbs").exists(),
                "{} should include Cargo.toml.hbs",
                template_dir.display()
            );
        }
    }

    #[test]
    fn bundled_templates_render_valid_cargo_manifests() {
        for template_dir in bundled_template_dirs() {
            let cargo_toml = rendered_cargo_manifest(&template_dir, &complete_template_context());
            assert_nockchain_deps_use_rev(&cargo_toml, TEST_NOCKCHAIN_REV);
        }
    }

    #[test]
    fn bundled_templates_use_project_name_for_generated_package_names() {
        let project_name = "arcadia-app";
        let context = complete_template_context_for_project(project_name);

        for template_dir in bundled_template_dirs() {
            let cargo_toml = rendered_cargo_manifest(&template_dir, &context);
            let package_name = cargo_toml
                .get("package")
                .and_then(|package| package.get("name"))
                .and_then(toml::Value::as_str);

            assert_eq!(
                package_name,
                Some(project_name),
                "{} should use project_name for [package].name",
                template_dir.display()
            );
        }
    }

    #[test]
    fn single_binary_templates_use_project_name_for_generated_bin_and_runtime_name() {
        let project_name = "arcadia-app";
        let context = complete_template_context_for_project(project_name);

        for template_name in ["basic", "http-server", "http-static", "repl"] {
            let template_dir = bundled_template_dir(template_name);
            let (tmp, dest) = render_template_dir(&template_dir, &context);
            let cargo_toml = rendered_cargo_manifest(&template_dir, &context);
            let bin_name = cargo_toml
                .get("bin")
                .and_then(toml::Value::as_array)
                .and_then(|bins| bins.first())
                .and_then(|bin| bin.get("name"))
                .and_then(toml::Value::as_str);

            assert_eq!(
                bin_name,
                Some(project_name),
                "{template_name} should use project_name for its generated bin name"
            );

            let main_rs = fs::read_to_string(dest.join("src/main.rs"))
                .expect("rendered main.rs should be readable");
            assert!(
                main_rs.contains(&format!(
                    r#"boot::setup(&kernel, cli, &[], "{project_name}", None)"#
                )),
                "{template_name} should use project_name as its runtime app name"
            );

            drop(tmp);
        }
    }

    #[test]
    fn grpc_template_uses_rendered_library_crate_name_in_generated_bins() {
        let context = complete_template_context_for_project("arcadia-app");
        let template_dir = bundled_template_dir("grpc");
        let (_tmp, dest) = render_template_dir(&template_dir, &context);

        let listen_rs = fs::read_to_string(dest.join("src/listen.rs"))
            .expect("rendered listen.rs should be readable");
        let talk_rs = fs::read_to_string(dest.join("src/talk.rs"))
            .expect("rendered talk.rs should be readable");

        assert!(listen_rs.contains("use arcadia_app::GRPC_PORT;"));
        assert!(talk_rs.contains("use arcadia_app::string_to_atom;"));
        assert!(talk_rs.contains("use arcadia_app::GRPC_PORT;"));
        assert!(talk_rs.contains("GRPC_PORT.to_string()"));
    }

    #[test]
    fn bundled_template_manifests_preserve_parsed_manifest_values() {
        let description = "Uses <nouns> & \"quoted\" values";
        let authors = vec!["Ada Lovelace <ada@example.com>".to_string(), "Bob & Carol".to_string()];
        let manifest = NockAppManifest {
            package: PackageMeta {
                name: "arcadia".to_string(),
                version: Some("0.2.0".to_string()),
                description: Some(description.to_string()),
                authors: Some(authors.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut context = build_handlebars_context(&manifest).expect("context should build");
        context.insert(
            "nockapp_commit_hash".to_string(),
            TEST_NOCKCHAIN_REV.to_string(),
        );
        let basic_template_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates/basic");

        let cargo_toml = rendered_cargo_manifest(&basic_template_dir, &context);
        let package = cargo_toml
            .get("package")
            .and_then(toml::Value::as_table)
            .expect("Cargo.toml should contain [package]");

        assert_eq!(
            package.get("description").and_then(toml::Value::as_str),
            Some(description)
        );
        let rendered_authors = package
            .get("authors")
            .and_then(toml::Value::as_array)
            .expect("authors should be an array")
            .iter()
            .map(|author| {
                author
                    .as_str()
                    .expect("author should be a string")
                    .to_string()
            })
            .collect::<Vec<_>>();
        assert_eq!(rendered_authors, authors);
    }

    #[test]
    fn template_cache_commit_flows_into_generated_cargo_manifest() {
        let tmp = tempdir().expect("tempdir should be created");
        let template_src = tmp.path().join("basic");
        fs::create_dir_all(&template_src).expect("template dir should be created");
        fs::write(
            tmp.path().join("commit.toml"),
            format!("[commit]\nid = \"{TEST_NOCKCHAIN_REV}\"\n"),
        )
        .expect("commit file should be written");
        fs::write(
            template_src.join("Cargo.toml.hbs"),
            r#"[package]
name = "{{project_name}}"
version = "{{version}}"
edition = "2021"

[dependencies]
nockapp = { git = "https://github.com/nockchain/nockchain.git", rev = "{{nockapp_commit_hash}}" }
nockvm = { git = "https://github.com/nockchain/nockchain.git", rev = "{{nockapp_commit_hash}}" }
"#,
        )
        .expect("template manifest should be written");
        let manifest = NockAppManifest {
            package: PackageMeta {
                name: "arcadia".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut context = build_handlebars_context(&manifest).expect("context should build");

        apply_template_source_context(&mut context, &template_src).expect("context should update");
        let cargo_toml = rendered_cargo_manifest(&template_src, &context);

        assert_nockchain_deps_use_rev(&cargo_toml, TEST_NOCKCHAIN_REV);
    }
}
