use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use colored::Colorize;
use handlebars::{no_escape, Handlebars};

use crate::manifest::NockAppManifest;

const TEMPLATE_COMPATIBILITY_MANIFEST: &str = "nockup-template.toml";
const NOCKCHAIN_REV_CONTEXT_KEY: &str = "nockchain_rev";

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
        // does not populate. Define template-version and dependency
        // compatibility semantics before making this public path reliable.
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

    // Build Handlebars context from the app manifest and template bundle metadata.
    let context = build_template_context(&manifest, &template_src)?;

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

    Ok(ctx)
}

fn build_template_context(
    manifest: &NockAppManifest,
    template_src: &Path,
) -> Result<HashMap<String, String>> {
    let mut ctx = build_handlebars_context(manifest)?;
    apply_template_compatibility_context(&mut ctx, template_src)?;
    Ok(ctx)
}

fn apply_template_compatibility_context(
    ctx: &mut HashMap<String, String>,
    template_src: &Path,
) -> Result<()> {
    let compatibility_path = template_compatibility_manifest_path(template_src)?;
    let compatibility_toml = fs::read_to_string(&compatibility_path)
        .with_context(|| format!("Failed to read {}", compatibility_path.display()))?;
    let compatibility_toml: toml::Value = toml::from_str(&compatibility_toml)
        .with_context(|| format!("Failed to parse {}", compatibility_path.display()))?;
    let nockchain_rev = compatibility_toml
        .get("nockchain")
        .and_then(|nockchain| nockchain.get("rev"))
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|rev| !rev.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{} must define [nockchain].rev",
                compatibility_path.display()
            )
        })?;

    ctx.insert(
        NOCKCHAIN_REV_CONTEXT_KEY.to_string(),
        nockchain_rev.to_string(),
    );
    Ok(())
}

fn template_compatibility_manifest_path(template_src: &Path) -> Result<std::path::PathBuf> {
    let template_root = template_src.parent().ok_or_else(|| {
        anyhow::anyhow!(
            "Could not determine template root for {}",
            template_src.display()
        )
    })?;

    Ok(template_root.join(TEMPLATE_COMPATIBILITY_MANIFEST))
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

    const TEMPLATE_NOCKCHAIN_REV: &str = "5d022ced55040221e8b6fcfd78114189fbae91a0";

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
                NOCKCHAIN_REV_CONTEXT_KEY.to_string(),
                TEMPLATE_NOCKCHAIN_REV.to_string(),
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

    fn bundled_template_compatibility_manifest() -> toml::Value {
        let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("templates")
            .join("nockup-template.toml");
        let manifest = fs::read_to_string(&manifest_path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", manifest_path.display()));
        toml::from_str(&manifest)
            .unwrap_or_else(|err| panic!("failed to parse {}: {err}", manifest_path.display()))
    }

    fn bundled_template_nockchain_rev() -> String {
        bundled_template_compatibility_manifest()
            .get("nockchain")
            .and_then(|nockchain| nockchain.get("rev"))
            .and_then(toml::Value::as_str)
            .expect("nockup-template.toml should contain nockchain.rev")
            .to_string()
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
    fn template_compatibility_manifest_adds_nockchain_rev_to_context() {
        let tmp = tempdir().expect("tempdir should be created");
        let template_root = tmp.path().join("templates");
        let template_src = template_root.join("basic");
        fs::create_dir_all(&template_src).expect("template dir should be created");
        fs::write(
            template_root.join(TEMPLATE_COMPATIBILITY_MANIFEST),
            format!("[nockchain]\nrev = \"{TEMPLATE_NOCKCHAIN_REV}\"\n"),
        )
        .expect("compatibility manifest should be written");
        let manifest = NockAppManifest {
            package: PackageMeta {
                name: "arcadia".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };

        let ctx = build_template_context(&manifest, &template_src).expect("context should build");

        assert_eq!(
            ctx.get(NOCKCHAIN_REV_CONTEXT_KEY),
            Some(&TEMPLATE_NOCKCHAIN_REV.to_string())
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
            assert_nockchain_deps_use_rev(&cargo_toml, TEMPLATE_NOCKCHAIN_REV);
        }
    }

    #[test]
    fn bundled_templates_source_nockchain_rev_from_compatibility_manifest() {
        let nockchain_rev = bundled_template_nockchain_rev();
        assert_eq!(nockchain_rev, TEMPLATE_NOCKCHAIN_REV);

        for template_dir in bundled_template_dirs() {
            let manifest_source = fs::read_to_string(template_dir.join("Cargo.toml.hbs"))
                .expect("template manifest should be readable");

            assert!(
                manifest_source.contains(r#"rev = "{{nockchain_rev}}""#),
                "{} should render Nockchain deps from nockchain_rev",
                template_dir.display()
            );
            assert!(
                !manifest_source.contains(TEMPLATE_NOCKCHAIN_REV),
                "{} should not duplicate the compatibility rev literal",
                template_dir.display()
            );
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
        let basic_template_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates/basic");
        let context =
            build_template_context(&manifest, &basic_template_dir).expect("context should build");

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
}
