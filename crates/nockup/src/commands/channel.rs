use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::cli::ChannelAction;

fn set_channel(channel: &str) -> Result<()> {
    // validate that is 'nightly' or 'stable', change later when more are supported
    if channel != "nightly" && channel != "stable" {
        return Err(anyhow::anyhow!("Invalid channel: {}", channel));
    }
    let mut config = get_config()?;
    config["channel"] = toml::Value::String(channel.to_string());
    let cache_dir = get_cache_dir()?;
    let config_path = cache_dir.join("config.toml");
    std::fs::write(config_path, toml::to_string(&config)?)
        .context("Failed to write config file")?;
    println!("Set default channel to '{}'.", channel);
    Ok(())
}

fn show_channel() -> Result<()> {
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

pub async fn run(command: ChannelAction) -> Result<()> {
    match command {
        ChannelAction::Set { channel } => set_channel(&channel),
        ChannelAction::Show => show_channel(),
    }
}
