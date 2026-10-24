// src/commands/cache/mod.rs
pub mod clear;

use anyhow::Result;

use crate::cli::CacheCommand;

pub async fn run(cmd: CacheCommand) -> Result<()> {
    match cmd {
        CacheCommand::Clear {
            git,
            packages,
            registry,
            all,
        } => clear::run(git, packages, registry, all).await,
    }
}
