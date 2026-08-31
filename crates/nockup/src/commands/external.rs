//! External-subcommand dispatcher.
//!
//! When the user runs `nockup foo bar`, clap routes any unknown
//! subcommand into [`Commands::External`]. We look up `nockup-foo` on
//! `$PATH` (cargo's plugin convention) and exec it with `[bar, ...]`.
//!
//! Trust the full `$PATH`. Restricting to a sanctioned plugin dir
//! breaks `cargo install nockup-foo` and `nix profile install`
//! workflows for downstream tools.

use std::ffi::OsString;
use std::process::Command;

use anyhow::Result;
use colored::Colorize;

pub async fn run(args: Vec<OsString>) -> Result<()> {
    let (head, tail) = args
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("no subcommand provided"))?;

    let name = head
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("subcommand name is not valid UTF-8"))?;
    let bin = format!("nockup-{}", name);

    let path = match which::which(&bin) {
        Ok(p) => p,
        Err(_) => {
            anyhow::bail!(
                "unknown command: {}\n\
                 hint: install `{}` and put it on $PATH (cargo-style plugins)",
                name.yellow(),
                bin.cyan()
            );
        }
    };

    let status = Command::new(path)
        .args(tail)
        .status()
        .map_err(|e| anyhow::anyhow!("failed to exec {}: {}", bin, e))?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}
