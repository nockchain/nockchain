# *Nockup*: the NockApp channel installer

Status: Experimental

This tool includes experimental workflows; check the commands against the installed version before production use.

*Nockup* is a CLI for bootstrapping NockApp development.

It can:

- manage the local `~/.nockup` toolchain cache (`hoon`, `hoonc`, templates),
- scaffold a project from a `nockapp.toml` manifest,
- install Hoon dependencies from the manifest,
- build Rust binaries and compile matching Hoon kernels.

## Installation

Prerequisites: `git`, Rust toolchain (`cargo`), and `gpg` on Linux.

### Script install

This installer uses the public upstream layout, not this checkout's grouped
crate paths.

```sh
curl -fsSL https://raw.githubusercontent.com/nockchain/nockchain/refs/heads/master/crates/nockup/install.sh | bash
```

### Build from source

From the root of this checkout, run the command below. The
[public upstream](https://github.com/nockchain/nockchain.git) may use a different
directory layout.

```sh
cargo install --path crates/tools/nockup --locked
```

### Initialize or refresh local toolchain cache

```sh
nockup update
```

This syncs templates/manifests/toolchains into `~/.nockup` and downloads channel binaries.

## Quick Start

`nockup project init` expects a `nockapp.toml` in your current directory and creates a subdirectory named after `[package].name`.

```sh
mkdir arcadia-work
cd arcadia-work

cat > nockapp.toml <<'TOML'
[package]
name = "arcadia"
version = "0.1.0"
description = "Example app"
template = "basic"

[dependencies]
"urbit/bits" = "@k409"
TOML

nockup update
nockup project init
nockup project build arcadia
nockup project run arcadia
```

## Manifest

`nockapp.toml` supports package metadata, template selection, optional template pinning, and dependencies:

```toml
[package]
name = "arcadia"
version = "0.1.0"
description = "My app"
authors = ["you"]
license = "MIT"
template = "basic"
# optional template pin
# template_commit = "<git-commit>"

[dependencies]
"urbit/bits" = "@k409"
"nockchain/zose" = "latest"
```

Supported template names in this crate:

- `basic`
- `grpc`
- `http-server`
- `http-static`
- `repl`

## Dependency Workflow

Run these from the same directory that contains `nockapp.toml`:

```sh
nockup package add urbit/bits --version @k409
nockup package install
nockup package list
nockup package remove urbit/bits
```

Supported version spec formats include:

- `@k414`
- `@commit:abc123`
- `@tag:v1.2.3`
- `@branch:main`
- semver requirements like `^1.2.0`
- `latest`

## Build and Run Notes

- `nockup project build <project>` runs `cargo build --release` then compiles Hoon kernels.
- Single-binary templates use `hoon/app/app.hoon` and produce `out.jam`.
- Multi-binary templates (notably `grpc`) map each `[[bin]]` target to `hoon/app/<bin>.hoon` and produce `<bin>.jam` files.
- `nockup project run <project>` runs `cargo run --release` for one default binary. For multi-binary templates, run binaries explicitly with `cargo run --release --bin <name>`.

## Channels and Cache

```sh
nockup channel show
nockup channel set stable
nockup cache clear --all
```

`channel set` currently accepts `stable` and `nightly`.

## Command Reference

- `nockup`: show nockup/hoon/hoonc versions and current channel/architecture.
- `nockup update`: refresh local toolchain cache and binaries.
- `nockup project init`: scaffold project from `nockapp.toml`.
- `nockup project build [project]`: build Rust + compile Hoon kernels.
- `nockup project run [project] [-- args...]`: run app via Cargo.
- `nockup package init [name]`: initialize a Hoon library package (`hoon.toml`).
- `nockup package add/remove/list/install/update/purge`: dependency management.
- `nockup cache clear [--git --packages --registry --all]`: clear caches.
- `nockup channel show/set`: inspect or change default channel.

## Security

Nockup is experimental. Treat template and dependency execution as untrusted code execution.

Linux signature verification uses GPG and may require importing key `A6FFD2DB7D4C9710`.
