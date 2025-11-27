use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::Stdio;

use anyhow::{anyhow, Context, Result};
use blake3;
use colored::Colorize;
use flate2::read::GzDecoder;
use sha1::{Digest, Sha1};
use tar::Archive;
use tokio::fs as tokio_fs;
use tokio::process::Command;

const GITHUB_REPO: &str = "sigilante/nockup";
const TEMPLATES_BRANCH: &str = "master";

pub fn get_cache_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?;
    Ok(home.join(".nockup"))
}

pub fn get_target_identifier() -> String {
    let arch = std::env::consts::ARCH;
    let os = std::env::consts::OS;

    match (arch, os) {
        ("x86_64", "linux") => "x86_64-unknown-linux-gnu".to_string(),
        ("x86_64", "windows") => "x86_64-pc-windows-msvc".to_string(),
        ("x86_64", "macos") => "x86_64-apple-darwin".to_string(),
        ("aarch64", "linux") => "aarch64-unknown-linux-gnu".to_string(),
        ("aarch64", "macos") => "aarch64-apple-darwin".to_string(),
        ("aarch64", "windows") => "aarch64-pc-windows-msvc".to_string(),
        _ => format!("{}-unknown-{}", arch, os),
    }
}

pub fn get_config() -> Result<toml::Value> {
    let cache_dir = get_cache_dir()?;
    let config_path = cache_dir.join("config.toml");
    if !config_path.exists() {
        return Err(anyhow::anyhow!(
            "Config file not found. Please run 'nockup install' first."
        ));
    }
    let config_str = std::fs::read_to_string(&config_path).context("Failed to read config file")?;
    let config: toml::Value =
        toml::de::from_str(&config_str).context("Failed to parse config file")?;
    Ok(config)
}

pub fn get_or_create_config() -> Result<toml::Value> {
    let cache_dir = get_cache_dir()?;
    let config_path = cache_dir.join("config.toml");
    if !config_path.exists() {
        write_default_config(&config_path)?;
    }
    let config_str = std::fs::read_to_string(&config_path).context("Failed to read config file")?;
    let config: toml::Value =
        toml::de::from_str(&config_str).context("Failed to parse config file")?;
    Ok(config)
}

fn write_default_config(config_path: &PathBuf) -> Result<()> {
    let default_config = format!(
        r#"channel = "stable"
architecture = "{}"
"#,
        get_target_identifier()
    );
    std::fs::write(config_path, default_config).context("Failed to create default config file")?;
    Ok(())
}

pub async fn download_templates(cache_dir: &PathBuf) -> Result<()> {
    let templates_dir = cache_dir.join("templates");

    if has_existing_templates(&templates_dir).await? {
        println!("{} Existing templates found, updating...", "🔄".yellow());
        update_templates(&templates_dir).await?;
    } else {
        println!("{}  Downloading templates from GitHub...", "⬇️".green());
        clone_templates(&templates_dir).await?;
    }

    Ok(())
}

async fn has_existing_templates(templates_dir: &PathBuf) -> Result<bool> {
    if !templates_dir.exists() {
        return Ok(false);
    }

    let git_dir = templates_dir.join(".git");
    if !git_dir.exists() {
        return Ok(false);
    }

    let entries = fs::read_dir(templates_dir)?;
    let mut count = 0;
    for entry in entries {
        let entry = entry?;
        let file_name = entry.file_name();
        if file_name != ".git" {
            count += 1;
            if count > 0 {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

async fn clone_templates(templates_dir: &PathBuf) -> Result<()> {
    let commit_id = get_git_commit_id().await?;
    let commit_file = templates_dir.join("commit.toml");

    match tokio_fs::read_to_string(&commit_file).await {
        Ok(commit_content) => {
            let commit: toml::Value =
                toml::de::from_str(&commit_content).context("Failed to parse commit file")?;
            let local_commit_id = commit["commit"]["id"].to_string().replace("\"", "");
            if local_commit_id == commit_id {
                println!("{} Templates are up to date", "✅".green());
                return Ok(());
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("{} No local commit ID found", "🔍".yellow());
        }
        Err(e) => {
            return Err(anyhow::anyhow!("Failed to read commit file: {}", e));
        }
    }

    if templates_dir.exists() {
        fs::remove_dir_all(templates_dir)
            .context("Failed to remove existing templates directory")?;

        if templates_dir.exists() {
            return Err(anyhow::anyhow!(
                "Failed to completely remove templates directory at {}",
                templates_dir.display()
            ));
        }
    }

    let temp_dir = templates_dir.parent().unwrap().join("temp_repo");
    if temp_dir.exists() {
        fs::remove_dir_all(&temp_dir)?;
    }

    let repo_url = format!("https://github.com/{}.git", GITHUB_REPO);

    let mut command = Command::new("git");
    command
        .arg("clone")
        .arg("--depth=1")
        .arg("--branch")
        .arg(TEMPLATES_BRANCH)
        .arg(&repo_url)
        .arg(&temp_dir);

    command.stdout(Stdio::null());
    command.stderr(Stdio::null());
    let status = command.status().await?;

    if !status.success() {
        return Err(anyhow::anyhow!(
            "Failed to clone templates from GitHub. Exit code: {}",
            status.code().unwrap_or(-1)
        ));
    }

    let repo_templates_dir = temp_dir.join("templates");
    if !repo_templates_dir.exists() {
        fs::remove_dir_all(&temp_dir).ok();
        return Err(anyhow::anyhow!(
            "No 'templates' directory found in the repository"
        ));
    }

    match fs::rename(&repo_templates_dir, templates_dir) {
        Ok(_) => {}
        Err(e) if e.raw_os_error() == Some(66) => {
            println!("{} Rename failed, copying instead...", "⚠️".yellow());
            copy_dir_recursive(&repo_templates_dir, templates_dir)?;
        }
        Err(e) => return Err(e.into()),
    }

    let repo_manifests_dir = temp_dir.join("manifests");
    if !repo_manifests_dir.exists() {
        fs::remove_dir_all(&temp_dir).ok();
        return Err(anyhow::anyhow!(
            "No 'manifests' directory found in the repository"
        ));
    }

    let manifests_dir = templates_dir.parent().unwrap().join("manifests");
    if manifests_dir.exists() {
        fs::remove_dir_all(&manifests_dir)?;
    }

    match fs::rename(&repo_manifests_dir, &manifests_dir) {
        Ok(_) => {}
        Err(e) if e.raw_os_error() == Some(66) => {
            println!(
                "{} Rename failed for manifests, copying instead...",
                "⚠️".yellow()
            );
            copy_dir_recursive(&repo_manifests_dir, &manifests_dir)?;
        }
        Err(e) => return Err(e.into()),
    }

    let commit_file = templates_dir.join("commit.toml");
    let commit_data = format!("[commit]\nid = \"{}\"\n", commit_id);
    fs::write(&commit_file, commit_data)?;

    fs::remove_dir_all(&temp_dir)?;
    println!(
        "{} Templates and manifests downloaded successfully",
        "✓".green()
    );
    Ok(())
}

fn copy_dir_recursive(src: &PathBuf, dst: &PathBuf) -> Result<()> {
    fs::create_dir_all(dst)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}

async fn update_templates(templates_dir: &PathBuf) -> Result<()> {
    clone_templates(templates_dir).await
}

pub async fn download_toolchain_files(cache_dir: &PathBuf) -> Result<()> {
    let toolchain_dir = cache_dir.join("toolchains");

    if has_existing_toolchain_files(&toolchain_dir).await? {
        println!(
            "{} Existing toolchain files found, updating...",
            "🔄".yellow()
        );
        update_toolchain_files(&toolchain_dir).await?;
    } else {
        println!(
            "{}  Downloading toolchain files from GitHub...",
            "⬇️".green()
        );
        clone_toolchain_files(&toolchain_dir).await?;
    }

    Ok(())
}

async fn has_existing_toolchain_files(toolchain_dir: &PathBuf) -> Result<bool> {
    if !toolchain_dir.exists() {
        return Ok(false);
    }
    let entries = fs::read_dir(toolchain_dir)?;
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn update_toolchain_files(toolchain_dir: &PathBuf) -> Result<()> {
    clone_toolchain_files(toolchain_dir).await
}

async fn clone_toolchain_files(toolchain_dir: &PathBuf) -> Result<()> {
    if toolchain_dir.exists() {
        fs::remove_dir_all(toolchain_dir)?;
    }
    fs::create_dir_all(toolchain_dir)?;

    println!(
        "{} Fetching latest channel manifests from GitHub releases...",
        "⬇️".green()
    );

    async fn get_latest_manifest(channel: &str, toolchain_dir: &PathBuf) -> Result<()> {
        let manifest_file = format!("nockchain-manifest.toml");  // ← Changed from "{channel}-manifest.toml"
        let output_file = toolchain_dir.join(format!("channel-nockup-{}.toml", channel));

        println!("{} Fetching manifest for {}...", "🔍".yellow(), channel);

        let latest_tag = get_git_commit_id().await?;

        // New URL format: build-{commit}/nockchain-manifest.toml
        let manifest_url = format!(
            "https://github.com/nockchain/nockchain/releases/download/build-{}/{}",
            latest_tag, manifest_file
        );

        println!("{} Downloading from: {}", "⬇️".blue(), manifest_url);

        let client = reqwest::Client::new();
        let response = client
            .get(&manifest_url)
            .header("User-Agent", "nockup")
            .send()
            .await
            .context("Failed to download manifest")?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Failed to download manifest: HTTP {}",
                response.status()
            ));
        }

        let content = response
            .text()
            .await
            .context("Failed to read manifest content")?;

        tokio_fs::write(&output_file, content)
            .await
            .context("Failed to write manifest file")?;

        println!(
            "{} Downloaded: channel-nockup-{}.toml",
            "✅".green(),
            channel
        );

        Ok(())
    }

    // For now, only "stable" channel exists (no more stable/nightly separation in tags)
    let channels = ["stable"];
    let mut errors = Vec::new();

    for channel in &channels {
        if let Err(e) = get_latest_manifest(channel, toolchain_dir).await {
            println!(
                "{} Failed to download {} manifest: {}",
                "⚠️".yellow(),
                channel,
                e
            );
            errors.push(format!("{}: {}", channel, e));
        }
    }

    if errors.len() == channels.len() {
        return Err(anyhow::anyhow!(
            "Failed to download any toolchain manifests: {}",
            errors.join(", ")
        ));
    }

    if !errors.is_empty() {
        println!(
            "{} Some manifests failed to download: {}",
            "⚠️".yellow(),
            errors.join(", ")
        );
    }

    println!("{} Toolchain files setup complete", "✅".green());
    Ok(())
}

pub async fn download_binaries(config: &toml::Value) -> Result<()> {
    let channel = config["channel"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid channel in config"))?;
    let architecture = config["architecture"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid architecture in config"))?;

    let cache_dir = get_cache_dir()?;
    let channel_name = format!("channel-nockup-{}", channel);
    let manifest_path = cache_dir
        .join("toolchains")
        .join(format!("{}.toml", channel_name));
    let manifest = std::fs::read_to_string(&manifest_path).context(format!(
        "Failed to read channel manifest for '{}.toml' at path {}",
        channel_name,
        manifest_path.display()
    ))?;
    let manifest: toml::Value = toml::de::from_str(&manifest).context(format!(
        "Failed to parse channel manifest for '{}'",
        channel_name
    ))?;

    println!(
        "{} Downloading binaries for channel '{}' and architecture '{}'...",
        "⬇️".green(),
        channel_name.cyan(),
        architecture.cyan()
    );

    for index in ["hoon", "hoonc", "nockup"] {
        println!("{} Downloading {} binary...", "⬇️".green(), index.cyan());
        let archive_url = manifest["pkg"][index]["target"][architecture]["url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{} Invalid URL for {} binary", "❌".red(), index))?;
        let archive_url = archive_url.replace("http://", "https://");
        // NOTE: No more separate signature_url - it's inside the tarball!

        let archive_blake3 = manifest["pkg"][index]["target"][architecture]["hash_blake3"]
            .as_str()
            .ok_or_else(|| {
                anyhow::anyhow!("{} Invalid Blake3 hash for {} binary", "❌".red(), index)
            })?;
        let archive_sha1 = manifest["pkg"][index]["target"][architecture]["hash_sha1"]
            .as_str()
            .ok_or_else(|| {
                anyhow::anyhow!("{} Invalid SHA1 hash for {} binary", "❌".red(), index)
            })?;

        let archive_path = download_file(&archive_url).await?;

        // Verify checksums of the tarball FIRST (before extraction)
        verify_checksums(&archive_path, &archive_blake3, &archive_sha1).await?;
        println!("{} Blake3 checksum passed.", "✅".green());
        println!("{} SHA1 checksum passed.", "✅".green());

        let target_dir = get_cache_dir()?;
        let binary_path = target_dir.join("bin");
        fs::create_dir_all(&binary_path)?;

        // Extract binary and signature from archive to temp directory
        let temp_extract_dir = std::env::temp_dir().join(format!("nockup_extract_{}", index));
        if temp_extract_dir.exists() {
            fs::remove_dir_all(&temp_extract_dir)?;
        }
        fs::create_dir_all(&temp_extract_dir)?;
        
        extract_archive_contents(&archive_path, &temp_extract_dir, index).await?;

        // Verify GPG signature if on Linux (signature is NOW inside tarball)
        if std::env::consts::OS == "linux" {
            let binary_temp_path = temp_extract_dir.join(index);
            let signature_temp_path = temp_extract_dir.join(format!("{}.asc", index));
            
            if signature_temp_path.exists() {
                verify_gpg_signature(&binary_temp_path, &signature_temp_path).await?;
            } else {
                println!(
                    "{} Warning: No signature file found in archive for {}",
                    "⚠️".yellow(),
                    index
                );
            }
        } else {
            println!(
                "{} Skipping signature verification on {} (not yet supported)",
                "⚠️".yellow(),
                std::env::consts::OS
            );
        }

        // Move binary to final location
        let final_binary_path = binary_path.join(index);
        let binary_temp_path = temp_extract_dir.join(index);
        
        if final_binary_path.exists() {
            fs::remove_file(&final_binary_path)?;
        }
        
        fs::rename(&binary_temp_path, &final_binary_path)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&final_binary_path)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&final_binary_path, perms)?;
        }

        println!(
            "{} Installed {} to {}",
            "✅".green(),
            index,
            final_binary_path.display()
        );

        // Clean up
        fs::remove_dir_all(&temp_extract_dir)?;
        fs::remove_file(&archive_path)?;
    }

    Ok(())
}

async fn verify_gpg_signature(
    archive_path: &std::path::Path,
    signature_path: &std::path::Path,
) -> Result<()> {
    println!("{} Verifying GPG signature...", "🔐".yellow());

    if !archive_path.exists() {
        return Err(anyhow::anyhow!(
            "Archive file does not exist: {}",
            archive_path.display()
        ));
    }
    if !signature_path.exists() {
        return Err(anyhow::anyhow!(
            "Signature file does not exist: {}",
            signature_path.display()
        ));
    }

    let output = Command::new("gpg")
        .args(["--verify", signature_path.to_str().unwrap(), archive_path.to_str().unwrap()])
        .output()
        .await
        .context("Failed to execute gpg command")?;

    if output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("Good signature") {
            println!("{} GPG signature verified successfully", "✅".green());
            return Ok(());
        }
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("No public key") {
        println!(
            "{} Public key not found, importing from keyserver...",
            "🔑".yellow()
        );

        let import_output = Command::new("gpg")
            .args(["--keyserver", "keyserver.ubuntu.com", "--recv-keys", "A6FFD2DB7D4C9710"])
            .output()
            .await
            .context("Failed to import public key from keyserver")?;

        if !import_output.status.success() {
            let alt_import = Command::new("gpg")
                .args(["--keyserver", "keys.openpgp.org", "--recv-keys", "A6FFD2DB7D4C9710"])
                .output()
                .await;

            if let Ok(alt_output) = alt_import {
                if !alt_output.status.success() {
                    return Err(anyhow::anyhow!(
                        "Failed to import public key from keyservers. Please import manually:\n  gpg --keyserver keyserver.ubuntu.com --recv-keys A6FFD2DB7D4C9710"
                    ));
                }
            } else {
                return Err(anyhow::anyhow!(
                    "Failed to import public key. Please import manually:\n  gpg --keyserver keyserver.ubuntu.com --recv-keys A6FFD2DB7D4C9710"
                ));
            }
        }

        println!("{} Public key imported successfully", "✅".green());

        let retry_output = Command::new("gpg")
            .args([
                "--verify",
                "--verbose",
                signature_path.to_str().unwrap(),
                archive_path.to_str().unwrap(),
            ])
            .output()
            .await
            .context("Failed to execute gpg verification after key import")?;

        if !retry_output.status.success() {
            let retry_stderr = String::from_utf8_lossy(&retry_output.stderr);
            return Err(anyhow::anyhow!(
                "GPG signature verification failed after key import: {}", retry_stderr
            ));
        }

        let retry_stderr = String::from_utf8_lossy(&retry_output.stderr);
        if retry_stderr.contains("Good signature") {
            println!("{} GPG signature verified successfully", "✅".green());
        } else {
            return Err(anyhow::anyhow!(
                "GPG signature verification failed: {}", retry_stderr
            ));
        }
    } else {
        return Err(anyhow::anyhow!(
            "GPG signature verification failed: {}", stderr
        ));
    }

    Ok(())
}

async fn extract_archive_contents(
    archive_path: &std::path::Path,
    target_dir: &std::path::Path,
    binary_name: &str,
) -> Result<()> {
    println!(
        "{} Extracting {} and signature from archive...",
        "📦".yellow(),
        binary_name
    );

    let file = std::fs::File::open(archive_path).context("Failed to open archive file")?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);

    for entry in archive
        .entries()
        .context("Failed to read archive entries")?
    {
        let mut entry = entry.context("Failed to read archive entry")?;
        let entry_path = entry.path().context("Failed to get entry path")?;
        let file_name = entry_path
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("Invalid file name in archive"))?;

        // Extract both the binary and its .asc signature
        if file_name == std::ffi::OsStr::new(binary_name)
            || file_name == std::ffi::OsStr::new(&format!("{}.asc", binary_name))
        {
            let target_path = target_dir.join(file_name);

            let mut buffer = Vec::new();
            entry
                .read_to_end(&mut buffer)
                .context("Failed to read file from archive")?;

            std::fs::write(&target_path, buffer).context(format!(
                "Failed to write extracted file: {}",
                target_path.display()
            ))?;

            println!(
                "{} Extracted {}",
                "✅".green(),
                target_path.display()
            );
        }
    }

    // Verify binary was extracted
    if !target_dir.join(binary_name).exists() {
        return Err(anyhow::anyhow!(
            "Binary '{}' not found in archive",
            binary_name
        ));
    }

    Ok(())
}

async fn download_file(url: &str) -> Result<PathBuf> {
    let response = reqwest::get(url)
        .await
        .context(format!("Failed to download file from '{}'", url))?;
    if !response.status().is_success() {
        return Err(anyhow::anyhow!(
            "Failed to download file from '{}': HTTP {}",
            url,
            response.status()
        ));
    }

    let url_filename = url.split('/').last().unwrap_or("download");
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let filename = format!("nockup_{}_{}", timestamp, url_filename);
    let temp_file = std::env::temp_dir().join(filename);

    let mut file = std::fs::File::create(&temp_file).context("Failed to create temporary file")?;
    let content = response.bytes().await?;
    std::io::copy(&mut content.as_ref(), &mut file).context("Failed to write to temporary file")?;
    Ok(temp_file)
}

async fn verify_checksums(
    file_path: &PathBuf,
    expected_blake3: &str,
    expected_sha1: &str,
) -> Result<()> {
    let bytes =
        std::fs::read(file_path).context("Failed to read file for checksum verification")?;

    let computed_blake3 = blake3::hash(&bytes);
    if computed_blake3.to_string() != expected_blake3 {
        return Err(anyhow::anyhow!(
            "Checksum verification failed: expected {}, got {}", expected_blake3, computed_blake3
        ));
    }

    let mut hasher = Sha1::new();
    hasher.update(&bytes);
    let computed_sha1 = hasher.finalize();
    let expected_sha1: [u8; 20] = hex::decode(expected_sha1)
        .map_err(|e| anyhow::anyhow!("Invalid hex SHA-1: {}", e))?
        .try_into()
        .map_err(|_| anyhow!("Failed to convert to fixed array (length mismatch)"))?;
    if computed_sha1.as_slice() != &expected_sha1 {
        let expected_hex = hex::encode(&expected_sha1);
        let computed_hex = hex::encode(computed_sha1.as_slice());
        return Err(anyhow::anyhow!(
            "Checksum verification failed: expected {}, got {}", expected_hex, computed_hex
        ));
    }
    Ok(())
}

pub async fn write_commit_details(cache_dir: &PathBuf) -> Result<()> {
    let status_file = cache_dir.join("status.toml");
    let mut config = toml::map::Map::new();
    config.insert("commit".into(), toml::Value::Table(toml::map::Map::new()));
    let commit_id = get_git_commit_id().await?;
    let commit_table = config
        .get_mut("commit")
        .and_then(|commit| commit.as_table_mut())
        .ok_or_else(|| anyhow::anyhow!("Failed to insert commit ID into config"))?;
    commit_table.insert("id".into(), toml::Value::String(commit_id));
    fs::write(status_file, toml::to_string(&config)?).context("Failed to write config file")?;
    Ok(())
}

async fn get_git_commit_id() -> Result<String> {
    let repo_url = format!("https://api.github.com/repos/nockchain/nockchain/commits/master");
    let client = reqwest::Client::new();
    let response = client
        .get(&repo_url)
        .header("User-Agent", "nockup")
        .send()
        .await
        .context("Failed to fetch commit ID from GitHub")?;

    if !response.status().is_success() {
        return Err(anyhow::anyhow!(
            "Failed to fetch commit ID: HTTP {}",
            response.status()
        ));
    }

    let json: serde_json::Value = response.json().await.context("Invalid JSON response")?;
    let commit_id = json["sha"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing commit ID in response"))?;
    Ok(commit_id.to_string())
}
