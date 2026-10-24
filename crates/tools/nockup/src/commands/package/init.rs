// src/commands/package/init.rs
use std::env;

use anyhow::Result;

use crate::manifest::{HoonPackage, PackageMeta};

pub async fn run(name: Option<String>) -> Result<()> {
    let cwd = env::current_dir()?;
    let dir_name = name
        .as_deref()
        .or_else(|| cwd.file_name().and_then(|s| s.to_str()))
        .unwrap_or("my-hoon-lib");

    let project_dir = if name.is_some() {
        let dir = cwd.join(dir_name);
        tokio::fs::create_dir_all(&dir).await?;
        dir
    } else {
        cwd.clone()
    };

    let manifest_path = project_dir.join("hoon.toml");

    if manifest_path.exists() {
        anyhow::bail!("hoon.toml already exists in {}", project_dir.display());
    }

    let pkg = HoonPackage {
        package: PackageMeta {
            name: dir_name.to_string(),
            version: None,
            description: None,
            authors: None,
            license: None,
            template: None,
            template_commit: None,
        },
        dependencies: Some(Default::default()),
    };

    pkg.save(&manifest_path)?;

    // Create minimal src dir
    tokio::fs::create_dir_all(project_dir.join("src")).await?;
    tokio::fs::write(
        project_dir.join("src/lib.hoon"),
        "|=  *@  ^-(^  +<-)".as_bytes(),
    )
    .await?;

    println!("Created library package: {}", dir_name);
    println!("   {}", manifest_path.display());

    Ok(())
}
