use std::fs;
use std::path::PathBuf;

use anyhow::Result;

use crate::commands::common::get_cache_dir;

/// Clear the package cache
pub async fn purge(dry_run: bool) -> Result<()> {
    let cache_dir = get_cache_dir()?;
    let packages_cache = cache_dir.join("cache").join("packages");

    if !packages_cache.exists() {
        println!("No package cache found at {}", packages_cache.display());
        return Ok(());
    }

    println!("Package cache location: {}", packages_cache.display());

    // Collect all cached packages
    let mut total_size = 0u64;
    let mut package_count = 0;

    if let Ok(entries) = fs::read_dir(&packages_cache) {
        for entry in entries.flatten() {
            if let Ok(_metadata) = entry.metadata() {
                total_size += calculate_dir_size(&entry.path())?;
                package_count += 1;

                if dry_run {
                    println!("  Would delete: {}", entry.file_name().to_string_lossy());
                }
            }
        }
    }

    let size_mb = total_size as f64 / 1_048_576.0;

    if dry_run {
        println!(
            "\nDry run: {} packages would be deleted ({:.2} MB)",
            package_count, size_mb
        );
        println!("Run without --dry-run to actually delete the cache");
    } else {
        if package_count == 0 {
            println!("Cache is already empty");
            return Ok(());
        }

        println!("Deleting {} packages ({:.2} MB)...", package_count, size_mb);
        fs::remove_dir_all(&packages_cache)?;
        fs::create_dir_all(&packages_cache)?;

        // Also clear the cache index
        let cache_index = cache_dir.join("cache").join("cache-index.json");
        if cache_index.exists() {
            fs::remove_file(&cache_index)?;
            println!("Cache index cleared");
        }

        println!("Package cache cleared successfully");
    }

    Ok(())
}

/// Calculate the total size of a directory recursively
fn calculate_dir_size(path: &PathBuf) -> Result<u64> {
    let mut size = 0u64;

    if path.is_dir() {
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let entry_path = entry.path();
                if entry_path.is_dir() {
                    size += calculate_dir_size(&entry_path)?;
                } else if let Ok(metadata) = entry.metadata() {
                    size += metadata.len();
                }
            }
        }
    }

    Ok(size)
}
