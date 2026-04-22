//! `nockup patches …` — inspect and release the applied-patches ledger.

use std::env;
use std::path::{Path, PathBuf};

use anyhow::Result;
use colored::Colorize;

use crate::cli::PatchesCommand;
use crate::manifest::{HoonPackage, NockAppLock};

pub async fn run(cmd: PatchesCommand) -> Result<()> {
    match cmd {
        PatchesCommand::Eject { package } => eject(package).await,
        PatchesCommand::List => list().await,
    }
}

/// Release a package's claim on the files it previously patched. Files
/// on disk stay as-they-are; a later install will re-prompt before
/// touching them.
async fn eject(package: String) -> Result<()> {
    let lock_path = lock_path()?;
    let mut lockfile = NockAppLock::load(&lock_path)?;

    let mut ejected = 0usize;
    for entry in lockfile.package.iter_mut() {
        if entry.name == package {
            ejected += entry.applied_patches.len();
            entry.applied_patches.clear();
        }
    }

    if ejected == 0 {
        println!(
            "{} {} had no applied-patches entries",
            "·".dimmed(),
            package.yellow()
        );
        return Ok(());
    }

    lockfile.save(&lock_path)?;
    println!(
        "{} Ejected {} patch ledger entry(ies) for {}",
        "✓".green(),
        ejected,
        package.yellow()
    );
    Ok(())
}

async fn list() -> Result<()> {
    let lockfile = NockAppLock::load(&lock_path()?)?;
    let mut any = false;
    for entry in &lockfile.package {
        if entry.applied_patches.is_empty() {
            continue;
        }
        any = true;
        println!("{}", entry.name.yellow());
        for applied in &entry.applied_patches {
            println!(
                "  [{}] {}  {}",
                applied.index + 1,
                applied.file,
                format!("({}…)", &applied.content_hash[..12]).dimmed()
            );
        }
    }
    if !any {
        println!("  {} No applied patches recorded", "·".dimmed());
    }
    Ok(())
}

/// The install command computes the project dir as `<cwd>/<package-name>`;
/// mirror that here by reading the consumer's `nockapp.toml`.
fn lock_path() -> Result<PathBuf> {
    let cwd = env::current_dir()?;
    Ok(project_dir(&cwd)?.join("nockapp.lock"))
}

fn project_dir(cwd: &Path) -> Result<PathBuf> {
    let manifest_path = cwd.join("nockapp.toml");
    let pkg = HoonPackage::load(&manifest_path)?
        .ok_or_else(|| anyhow::anyhow!("No nockapp.toml found in {}", cwd.display()))?;
    let dir = cwd.join(&pkg.package.name);
    if !dir.exists() {
        anyhow::bail!(
            "Project directory {} not found — run `nockup package install` first",
            dir.display()
        );
    }
    Ok(dir)
}
