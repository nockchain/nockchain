// src/commands/package/install.rs
use std::path::{Path, PathBuf};
use std::{env, fs};

use anyhow::{anyhow, Context, Result};
use colored::Colorize;

use crate::cache::PackageCache;
use crate::manifest::{HoonPackage, LockSource, LockedPackage, NockAppLock};
use crate::resolver::Resolver;

pub async fn run() -> Result<()> {
    let cwd = env::current_dir()?;
    let manifest_path = cwd.join("nockapp.toml");

    // Load manifest
    let manifest = match HoonPackage::load(&manifest_path)? {
        Some(m) => m,
        None => anyhow::bail!("No nockapp.toml found in {}", cwd.display()),
    };

    println!(
        "{} Installing dependencies for {}",
        "📦".cyan(),
        manifest.package.name.yellow()
    );
    println!();

    // Determine the project directory based on the package name
    let project_dir = cwd.join(&manifest.package.name);

    // Check if project directory exists
    if !project_dir.exists() {
        anyhow::bail!(
            "Project directory '{}' not found. Run `nockup project init` first.",
            manifest.package.name
        );
    }

    // Initialize resolver
    let resolver = Resolver::new()?;
    let cache = PackageCache::new()?;

    // Resolve dependency graph
    let graph = resolver.resolve(&manifest).await?;

    if graph.packages.is_empty() {
        println!("{} No dependencies to install", "✓".green());

        // Create empty lockfile if needed
        let lock_path = project_dir.join("nockapp.lock");
        if !lock_path.exists() {
            let lockfile = NockAppLock {
                package: Vec::new(),
            };
            lockfile.save(&lock_path)?;
            println!("  Created empty nockapp.lock");
        }

        return Ok(());
    }

    println!();
    println!("{} Installing packages...", "📥".cyan());
    println!();

    // Create hoon/packages, hoon/lib, and hoon/sur directories if they don't exist
    let hoon_dir = project_dir.join("hoon");
    let packages_dir = hoon_dir.join("packages");
    let lib_dir = hoon_dir.join("lib");
    let sur_dir = hoon_dir.join("sur");
    fs::create_dir_all(&packages_dir).context("Failed to create hoon/packages directory")?;
    fs::create_dir_all(&lib_dir).context("Failed to create hoon/lib directory")?;
    fs::create_dir_all(&sur_dir).context("Failed to create hoon/sur directory")?;

    // Install packages in topological order
    let mut locked_packages = Vec::new();

    for pkg_name in &graph.install_order {
        let pkg = graph
            .packages
            .get(pkg_name)
            .ok_or_else(|| anyhow!("Missing package '{}' in resolved graph", pkg_name))?;

        let version_str = pkg.version_spec.to_canonical_string();

        // For wildcard/latest versions ("*"), display as "latest" and use commit for cache
        let (display_version, cache_version) = if version_str == "*" {
            ("latest".to_string(), format!("commit:{}", pkg.commit))
        } else {
            (version_str.clone(), version_str.clone())
        };

        println!(
            "  {} Installing {}@{}...",
            "→".cyan(),
            pkg.name.yellow(),
            display_version.cyan()
        );

        // Check if already in cache using the cache version
        let cached_path = cache.package_path(&pkg.name, &cache_version);

        if !cached_path.exists() {
            // This shouldn't happen since resolver already cached it,
            // but handle it gracefully
            println!(
                "    {} Package not in cache (this is unexpected)",
                "⚠".yellow()
            );
            continue;
        }

        // Install to hoon/packages/<name>--<version>/
        // Sanitize package name (replace / with -) and version (replace : with -) for use in directory names
        let safe_name = sanitize_package_name(&pkg.name);
        let safe_version = sanitize_version(&display_version);
        let install_dir = packages_dir.join(format!("{}--{}", safe_name, safe_version));

        if install_dir.exists() {
            println!("    {} Already installed, skipping", "✓".green());
        } else {
            // Copy from cache to hoon/packages/
            copy_dir_recursive(cached_path.as_path(), install_dir.as_path()).with_context(
                || format!("Failed to install package to {}", install_dir.display()),
            )?;

            println!(
                "    {} Installed to {}",
                "✓".green(),
                format!("hoon/packages/{}--{}", safe_name, safe_version).cyan()
            );
        }

        // Create symlinks for .hoon files
        // If install_path is specified (from registry), preserve directory structure
        // Otherwise, link to hoon/lib/ and hoon/sur/
        if let (Some(ref install_path), Some(ref files)) = (&pkg.install_path, &pkg.source_files) {
            println!("install_path: {:?}", install_path);
            link_registry_package(
                install_dir.as_path(),
                hoon_dir.as_path(),
                install_path,
                &pkg.name,
                files,
            )?;
        } else {
            println!("No install_path specified, linking to hoon/lib/ and hoon/sur/");
            link_package_files(
                install_dir.as_path(),
                lib_dir.as_path(),
                sur_dir.as_path(),
                &pkg.name,
                pkg.source_path.as_deref(),
                pkg.source_files.as_ref(),
            )?;
        }

        // Add to lockfile
        locked_packages.push(LockedPackage {
            name: pkg.name.clone(),
            version: display_version.clone(),
            source: LockSource::Git {
                url: pkg.source_url.clone(),
                commit: pkg.commit.clone(),
                path: pkg.source_path.clone(),
            },
        });
    }

    println!();
    println!(
        "{} Installed {} packages",
        "✓".green(),
        graph.packages.len()
    );

    // Generate/update lockfile
    let lock_path = project_dir.join("nockapp.lock");
    let lockfile = NockAppLock {
        package: locked_packages,
    };

    lockfile.save(&lock_path)?;
    println!("  Updated nockapp.lock");

    Ok(())
}

/// Sanitize package name for use in directory names (replace / with -)
fn sanitize_package_name(name: &str) -> String {
    name.replace('/', "-")
}

/// Sanitize version string for Hoon @tas compatibility
/// Replaces dots and colons with hyphens to ensure valid @tas
/// Examples:
///   "0.1.0" -> "0-1-0"
///   "commit:abc123" -> "commit-abc123"
///   "v1.2.3" -> "v1-2-3"
fn sanitize_version(version: &str) -> String {
    version.replace(['.', ':'], "-")
}

/// Recursively copy a directory
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();
        let dst_path = dst.join(&file_name);

        if path.is_dir() {
            copy_dir_recursive(&path, &dst_path)?;
        } else {
            fs::copy(&path, &dst_path)?;
        }
    }

    Ok(())
}

/// Create symlinks for registry packages that preserve directory structure
/// For example:
/// - nockchain/common/zose with install_path="common" and files=["zose.hoon"]
///   creates arcadia/hoon/common/zose.hoon -> ../packages/nockchain-common-zose@latest/zose.hoon
/// - urbit/zuse with install_path="sys" and files=["zuse.hoon"]
///   creates arcadia/hoon/sys/zuse.hoon -> ../packages/urbit-zuse@latest/zuse.hoon
fn link_registry_package(
    package_dir: &Path,
    hoon_dir: &Path,
    install_path: &str,
    package_name: &str,
    source_files: &Vec<String>,
) -> Result<()> {
    let package_dir_name = package_dir_basename(package_dir)?;

    // Strip "hoon/" prefix from install_path if present (it's already included in hoon_dir)
    println!("install_path before stripping: {:?}", install_path);
    let relative_path = install_path.strip_prefix("hoon/").unwrap_or(install_path);
    println!("relative_path: {:?}", relative_path);

    // Create the target directory structure in hoon/
    let target_dir = hoon_dir.join(relative_path);
    fs::create_dir_all(&target_dir)
        .with_context(|| format!("Failed to create directory {}", target_dir.display()))?;
    println!("  source_files: {:?}", source_files);

    if !source_files.is_empty() {
        // Link each specified file
        for filename in source_files {
            let source_file = package_dir.join(filename);
            if !source_file.exists() {
                anyhow::bail!("Specific file {} not found in package {}", filename, package_name);
            }

            let link_path = target_dir.join(filename);
            println!("  link_path: {:?}", link_path);

            // Remove existing symlink if it exists
            if link_path.exists() || link_path.is_symlink() {
                fs::remove_file(&link_path).with_context(|| {
                    format!("Failed to remove existing symlink {}", link_path.display())
                })?;
            }

            // Create relative symlink
            // Calculate path from target_dir back to packages/
            // For hoon/common/, we need: ../../packages/package@version/file
            let depth = relative_path.split('/').filter(|s| !s.is_empty()).count();
            let mut relative_target = PathBuf::new();
            for _ in 0..depth {
                relative_target.push("..");
            }
            relative_target.push("packages");
            relative_target.push(Path::new(&package_dir_name));
            relative_target.push(filename);
            println!("  relative_target: {:?}", relative_target);

            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&relative_target, &link_path).with_context(|| {
                    format!(
                        "Failed to create symlink {} -> {}",
                        link_path.display(),
                        relative_target.display()
                    )
                })?;
            }

            #[cfg(windows)]
            {
                std::os::windows::fs::symlink_file(&relative_target, &link_path).with_context(
                    || {
                        format!(
                            "Failed to create symlink {} -> {}",
                            link_path.display(),
                            relative_target.display()
                        )
                    },
                )?;
            }

            println!(
                "    {} Linked {} to hoon/{}/",
                "🔗".cyan(),
                filename.yellow(),
                relative_path.cyan()
            );
        }
    } else {
        // No specific file - link all .hoon files from common library/structure paths
        // When there's no specific file, we assume the package follows Urbit desk structure
        // and link lib/ files to hoon/lib/, sur/ files to hoon/sur/, etc.
        let source_paths = vec![
            ("lib", package_dir.join("lib")),
            ("lib", package_dir.join("desk").join("lib")),
            ("sur", package_dir.join("desk").join("sur")),
        ];

        let mut found_files = false;

        for (dest_subdir, source_dir) in source_paths {
            if !source_dir.exists() {
                continue;
            }

            // Determine target directory (hoon/lib or hoon/sur)
            let dest_dir = hoon_dir.join(dest_subdir);
            fs::create_dir_all(&dest_dir)
                .with_context(|| format!("Failed to create directory {}", dest_dir.display()))?;

            // Link .hoon files from this directory
            for entry in fs::read_dir(&source_dir)
                .with_context(|| format!("Failed to read directory {}", source_dir.display()))?
            {
                let entry = entry?;
                let path = entry.path();

                if path.is_file() {
                    if let Some(extension) = path.extension() {
                        if extension == "hoon" {
                            found_files = true;
                            let Some(file_name) = path.file_name() else {
                                continue;
                            };
                            let link_path = dest_dir.join(file_name);

                            // Remove existing symlink if it exists
                            if link_path.exists() || link_path.is_symlink() {
                                fs::remove_file(&link_path).with_context(|| {
                                    format!(
                                        "Failed to remove existing symlink {}",
                                        link_path.display()
                                    )
                                })?;
                            }

                            // Calculate relative path from package_root to the file
                            let relative_from_package =
                                path.strip_prefix(package_dir).unwrap_or(&path);

                            // Build symlink path from hoon/{dest_subdir}/ to packages/
                            // For hoon/lib/, we need: ../packages/package@version/desk/lib/file.hoon
                            let mut relative_target = PathBuf::new();
                            relative_target.push("..");
                            relative_target.push("packages");
                            relative_target.push(Path::new(&package_dir_name));
                            relative_target.push(relative_from_package);

                            #[cfg(unix)]
                            {
                                std::os::unix::fs::symlink(&relative_target, &link_path)
                                    .with_context(|| {
                                        format!(
                                            "Failed to create symlink {} -> {}",
                                            link_path.display(),
                                            relative_target.display()
                                        )
                                    })?;
                            }

                            #[cfg(windows)]
                            {
                                std::os::windows::fs::symlink_file(&relative_target, &link_path)
                                    .with_context(|| {
                                        format!(
                                            "Failed to create symlink {} -> {}",
                                            link_path.display(),
                                            relative_target.display()
                                        )
                                    })?;
                            }

                            println!(
                                "    {} Linked {} to hoon/{}/",
                                "🔗".cyan(),
                                file_name.to_string_lossy().yellow(),
                                dest_subdir.cyan()
                            );
                        }
                    }
                }
            }
        }

        if !found_files {
            println!(
                "    {} No .hoon files found in package {}",
                "⚠".yellow(),
                package_name.yellow()
            );
        }
    }

    Ok(())
}

/// Create symlinks in hoon/lib/ and hoon/sur/ for .hoon files in the package
/// If `source_files` is Some with files, only link those files. Otherwise, link all .hoon files.
fn link_package_files(
    package_dir: &Path,
    lib_dir: &Path,
    sur_dir: &Path,
    package_name: &str,
    _path_from_root: Option<&str>,
    source_files: Option<&Vec<String>>,
) -> Result<()> {
    let package_dir_name = package_dir_basename(package_dir)?;
    println!("  source_files is {:?}", source_files);

    // Get the parent hoon/ directory from lib_dir
    let hoon_dir = lib_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("lib_dir has no parent directory"))?;

    if let Some(files) = source_files {
        // Link each specified file
        // Files may include subdirectories (e.g., "lib/lagoon.hoon", "sur/lagoon.hoon")
        // The package is cached with contents of source_path, so we don't prepend it
        for filename in files {
            let source_file = package_dir.join(filename);
            println!("  source_file: {:?}", source_file);
            if !source_file.exists() {
                anyhow::bail!("Specific file {} not found in package {}", filename, package_name);
            }

            // Determine destination directory based on path prefix
            // Extract the first path component (e.g., "lib" from "lib/lagoon.hoon")
            let (dest_dir, dest_subdir, file_name) =
                if let Some((prefix, rest)) = filename.split_once('/') {
                    // File has a prefix like "lib/lagoon.hoon" or "sys/zuse.hoon"
                    let dest = hoon_dir.join(prefix);
                    (dest, prefix.to_string(), rest.to_string())
                } else {
                    // No prefix, default to lib for backward compatibility
                    (lib_dir.to_path_buf(), "lib".to_string(), filename.clone())
                };

            // Extract just the filename (last component) for the link path
            let file_name = PathBuf::from(&file_name)
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("Invalid filename: {}", filename))?
                .to_os_string();
            let link_path = dest_dir.join(&file_name);
            println!("  link_path: {:?}", link_path);

            // Ensure destination directory exists
            if !dest_dir.exists() {
                fs::create_dir_all(&dest_dir).with_context(|| {
                    format!("Failed to create directory {}", dest_dir.display())
                })?;
            }

            // Remove existing symlink if it exists
            if link_path.exists() || link_path.is_symlink() {
                fs::remove_file(&link_path).with_context(|| {
                    format!("Failed to remove existing symlink {}", link_path.display())
                })?;
            }

            // Create relative symlink
            // filename may include subdirectories (e.g., "lib/lagoon.hoon")
            let mut relative_target = PathBuf::from("../packages");
            relative_target.push(Path::new(&package_dir_name));
            relative_target.push(Path::new(filename));
            println!("  relative_target: {:?}", relative_target);

            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&relative_target, &link_path).with_context(|| {
                    format!(
                        "Failed to create symlink {} -> {}",
                        link_path.display(),
                        relative_target.display()
                    )
                })?;
            }

            #[cfg(windows)]
            {
                std::os::windows::fs::symlink_file(&relative_target, &link_path).with_context(
                    || {
                        format!(
                            "Failed to create symlink {} -> {}",
                            link_path.display(),
                            relative_target.display()
                        )
                    },
                )?;
            }

            println!(
                "    {} Linked {} to hoon/{}/",
                "🔗".cyan(),
                filename.yellow(),
                dest_subdir.cyan()
            );
        }

        return Ok(());
    }

    // Link all .hoon files - check common library directory patterns
    let lib_source_dirs = vec![
        package_dir.join("lib"),
        package_dir.join("src").join("lib"),
        package_dir.join("desk").join("lib"),
    ];

    let sur_source_dirs = vec![
        package_dir.join("sur"),
        package_dir.join("src").join("sur"),
        package_dir.join("desk").join("sur"),
    ];

    let mut found_files = false;

    // Link lib files
    for source_dir in lib_source_dirs {
        if !source_dir.exists() {
            continue;
        }

        // Link .hoon files from this lib directory (non-recursive - only direct children)
        link_hoon_files_from_dir(source_dir.as_path(), package_dir, lib_dir, &mut found_files)?;
    }

    // Link sur files
    for source_dir in sur_source_dirs {
        if !source_dir.exists() {
            continue;
        }

        // Link .hoon files from this sur directory (non-recursive - only direct children)
        link_hoon_files_from_dir(source_dir.as_path(), package_dir, sur_dir, &mut found_files)?;
    }

    if !found_files {
        println!(
            "    {} No .hoon files found in package {}",
            "⚠".yellow(),
            package_name.yellow()
        );
    }

    Ok(())
}

/// Link .hoon files from a lib directory (non-recursive - only direct children)
fn link_hoon_files_from_dir(
    source_dir: &Path,
    package_root: &Path,
    lib_dir: &Path,
    found_files: &mut bool,
) -> Result<()> {
    let package_dir_name = package_dir_basename(package_root)?;
    for entry in fs::read_dir(source_dir)
        .with_context(|| format!("Failed to read directory {}", source_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();

        // Only process files, not subdirectories
        if path.is_file() {
            if let Some(extension) = path.extension() {
                if extension == "hoon" {
                    let Some(file_name) = path.file_name() else {
                        continue;
                    };
                    *found_files = true;
                    let link_path = lib_dir.join(file_name);

                    // Remove existing symlink if it exists
                    if link_path.exists() || link_path.is_symlink() {
                        fs::remove_file(&link_path).with_context(|| {
                            format!("Failed to remove existing symlink {}", link_path.display())
                        })?;
                    }

                    // Create relative path from hoon/lib to the file
                    // Calculate the relative path from package_root to the actual file
                    let relative_from_package = path.strip_prefix(package_root).unwrap_or(&path);

                    let mut relative_target = PathBuf::from("../packages");
                    relative_target.push(Path::new(&package_dir_name));
                    relative_target.push(relative_from_package);

                    #[cfg(unix)]
                    {
                        std::os::unix::fs::symlink(&relative_target, &link_path).with_context(
                            || {
                                format!(
                                    "Failed to create symlink {} -> {}",
                                    link_path.display(),
                                    relative_target.display()
                                )
                            },
                        )?;
                    }

                    #[cfg(windows)]
                    {
                        std::os::windows::fs::symlink_file(&relative_target, &link_path)
                            .with_context(|| {
                                format!(
                                    "Failed to create symlink {} -> {}",
                                    link_path.display(),
                                    relative_target.display()
                                )
                            })?;
                    }

                    println!(
                        "    {} Linked {} to hoon/lib/",
                        "🔗".cyan(),
                        file_name.to_string_lossy().yellow()
                    );
                }
            }
        }
        // Skip subdirectories - we only want files directly in lib/
    }

    Ok(())
}

fn package_dir_basename(package_dir: &Path) -> Result<String> {
    package_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| anyhow!("Package directory '{}' has no name", package_dir.display()))
}
