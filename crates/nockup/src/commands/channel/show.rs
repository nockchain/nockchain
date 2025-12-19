use std::path::PathBuf;

use anyhow::{Context, Result};

pub fn run() -> Result<()> {
    let config = get_config()?;
    println!("Default channel: {}", config["channel"]);
    println!("Architecture: {}", config["architecture"]);
    Ok(())
}

fn get_cache_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?;
    Ok(home.join(".nockup"))
}

fn get_config() -> Result<toml::Value> {
    let cache_dir = get_cache_dir()?;
    let config_path = cache_dir.join("config.toml");
    let config_str = std::fs::read_to_string(&config_path).context("Failed to read config file")?;
    let config: toml::Value =
        toml::de::from_str(&config_str).context("Failed to parse config file")?;
    Ok(config)
}
