# *Nockup*: the NockApp channel installer

*Nockup* is a command-line tool to produce [NockApps](https://github.com/nockchain/nockchain) and manage project builds and dependencies.

[The NockApp platform](https://github.com/nockchain/nockchain) is a general-purpose framework for building apps that run using the Nock instruction set architecture.  It is particularly well-suited for use with [Nockchain](https://nockchain.org) and the Nock ZKVM.

![](./img/hero.jpg)

## Installation

### From Script

Prerequisites: Rust toolchain (`rustup`, `cargo`, &c.), Git.

```sh
curl -fsSL https://raw.githubusercontent.com/nockchain/nockchain/refs/heads/master/crates/nockup/install.sh | bash
```

This checks for dependencies and then installs the Nockup binary and its requirements, including the GPG key used to verify binaries on Linux.  (This is from the `stable` channel by default; see [Channels](#channels) for more information.)

### From Source

Prerequisites: Rust toolchain, Git

0. Before building, switch your `rustup` to `nightly` to satisfy `nockapp`/`nockvm` dependencies.

    ```sh
    rustup update
    rustup install nightly
    rustup override set nightly
    ```

1. Install Nockchain and build `hoon` and `hoonc`.

    ```sh
    $ git clone https://github.com/nockchain/nockchain.git
    $ cd nockchain
    $ make install-hoonc
    $ cargo install --locked --force --path crates/hoon --bin hoon
    ```

2. Install Nockup.

    ```sh
    $ git clone https://github.com/nockchain/nockchain.git
    $ cd crates/nockup/
    $ cargo build --release
    ```

    `nockup` builds by default in `./target/release`, so further commands to `nockup` refer to it in whatever location you have it.  `nockup install` will provide it in your `$PATH`.

    Alternatively, you may install it globally using Cargo:

    ```sh
    $ cargo install --path . --locked
    ```

3. Install the GPG public key (on Linux).  Nockup **will not work** if you do not provide the public key.

    ```sh
    $ gpg --keyserver keyserver.ubuntu.com --recv-keys A6FFD2DB7D4C9710
    ```

4. Install `nockup` and dependencies.

    ```sh
    nockup install
    ```

5. Check for updates.

    ```sh
    $ nockup update
    ```

### On Replit

A [Replit template is available](https://replit.com/@neal50/NockApp?v=1) which demonstrates Nockup functionality in the cloud.  Due to Replit's memory limitations, its current functionality is not extensive.

## Tutorial

Nockup provides a command-line interface for managing NockApp projects.  It uses binaries to process manifest files to create NockApp projects from templates then build and run them.

```sh
# Show basic program information.  (On some systems, like Docker 
# containers, the hoon and hoonc binaries are not identified.)
$ nockup
nockup version 0.0.1
hoon   version 0.1.0
hoonc  version 0.2.0
current channel stable
current architecture aarch64

# Start the nockup environment.
$ nockup install
🚀 Setting up nockup cache directory...
📁 Cache location: /Users/myuser/.nockup
📁 Creating cache directory structure...
✓ Created directory structure
⬇️ Downloading templates from GitHub...
Cloning into '/Users/myuser/.nockup/temp_repo'...
remote: Enumerating objects: 36, done.
remote: Counting objects: 100% (36/36), done.
remote: Compressing objects: 100% (30/30), done.
remote: Total 36 (delta 1), reused 18 (delta 0), pack-reused 0 (from 0)
Receiving objects: 100% (36/36), 45.18 KiB | 1.56 MiB/s, done.
Resolving deltas: 100% (1/1), done.
✓ Templates downloaded successfully
✅ Setup complete!
📂 Templates are now available in: /Users/myuser/.nockup/templates

# Initialize a default project.
$ cp ~/.nockup/manifests/example-manifest.toml arcadia.toml
$ nockup init arcadia
Initializing new NockApp project 'arcadia'...
  create Cargo.toml
  create manifest.toml
  create Cargo.lock
  create hoon/app/app.hoon
  create hoon/common/wrapper.hoon
  create hoon/lib/lib.hoon
  create hoon/lib/http.hoon
  create README.md
  create src/main.rs
📚 Processing library dependencies...
  ⬇️ Fetching library 'sequent'...
    ⬇️ Cloning repository...
      copy sys.kelvin
      copy lib/seq.hoon
      copy lib/test.hoon
    ✓ Installed library 'sequent'
✓ All libraries processed successfully!
✓ New project created in ./arcadia//
To get started:
  nockup build arcadia
  nockup run arcadia

# Show project settings.
$ cd arcadia
$ ls
Cargo.lock    Cargo.toml    hoon          manifest.toml README.md     src

$ cd ..

# Build the project (wraps hoonc).
$ nockup build arcadia
🔨 Building project 'arcadia'...
    Updating crates.io index
    Updating git repository `https://github.com/nockchain/nockchain.git`
     Locking 486 packages to latest compatible versions
      Adding matchit v0.8.4 (available: v0.8.6)
      Adding toml v0.8.23 (available: v0.9.5)
   Compiling proc-macro2 v1.0.101
* * *
I (11:53:08) "hoonc: build succeeded, sending out write effect"
I (11:53:08) "hoonc: output written successfully to '/Users/myuser/nockchain/nockup/arcadia/out.jam'"
no panic!
✓ Hoon compilation completed successfully!

# Run the project (wraps hoon).
$ nockup run arcadia
🔨 Running project 'arcadia'...
    Finished `release` profile [optimized] target(s) in 0.31s
     Running `target/release/arcadia`
I (11:53:14) [no] kernel::boot: Tracy tracing is enabled
I (11:53:14) [no] kernel::boot: kernel: starting
W (11:53:15) poked: cause
I (11:53:15) Pokes awaiting implementation

✓ Run completed successfully!
```

The final product is, of course, a binary which you may run either via `nockup run` (as demonstrated here) or directly (from `./target/release`).

### Project Templates and Manifests

A NockApp consists of a Rust wrapper and a Nock ISA kernel.  The wrapper handles command-line arguments, filesystem I/O, etc.  The kernel is the business logic.

One of the design goals of Nockup is to avoid the need to write much, if any, Rust code to successfully deploy a NockApp.  To that end, we provide templates which by and large only expect the developer to write in Hoon or another language which targets the Nock ISA.

A project is specified by its manifest file, which includes details like the project name and the template to use.  Many projects will prefer the `basic` template, but other options are available in `/templates`.

#### Basic Templates

*Basic templates demonstrate simple NockApps without Nockchain interaction.*

- `basic`:  simplest NockApp template.
- `grpc`:  gRPC listener and broadcaster.
- `http-static`:  static HTTP file server.
- `http-server`:  stateful HTTP server.
- `repl`:  read-eval-print loop.

#### Nockchain Templates

*Nockchain templates demonstrate NockApps which interact with a Nockchain instance.  They use the `nockchain-wallet` crate as a library.  We recommend using a [fakenet](https://docs.nockchain.org/nockapp/what-is-nockapp/development-and-testing) to avoid needing to spend $NOCK on the livenet during development.  At the current time, this only means running a local fakenet node since wallet credentials are compatible with the livenet format, granting other caveats.*

<!-- - `chain`:  Nockchain listener, built using `nockchain-wallet`.  Demonstrates poking and peeking the chain state.
- `oracle`:  Nockchain attestation poster, built using `nockchain-wallet`.  Demonstrates signing a message using a private key.
- `remote`:  Nockchain remote instance gRPC interaction.  Demonstrates interacting with a Nockchain public instance via remote gRPC.
- `rollup`:  Nockchain rollup bundler for NockApps.  Demonstrates producing a consistent rollup and pushing it to the chain. -->

#### Manifests

A project manifest is a file containing sufficient information to produce a basic NockApp from a template with specified imports.

```toml
[project]
name = "Et In Arcadia Ego"
project_name = "arcadia"
version = "1.0.0"
description = "I too was in Arcadia."
author_name = "Nicolas Poussin"
author_email = "nicolas@poussin.edu"
github_username = "arcadia"
license = "MIT"
keywords = ["nockapp", "nockchain", "hoon"]
nockapp_commit_hash = "336f744b6b83448ec2b86473a3dec29b15858999"
template = "basic"
```

Manifests let you set several project parameters and specify the template to use.  This information will also be used to populate a README file.  (By default we supply the [MIT License](https://opensource.org/licenses/MIT) and we specify the version as [0.1.0](https://0ver.org/).)

#### Multiple Targets

A NockApp project can produce more than one binary target.  This is scenario is demonstrated by the `grpc` template.

The default expectation for a single-binary project is to supply the following two files:

1. `src/main.rs` - the main Rust driver.
2. `hoon/app/app.hoon` - the Hoon kernel.

However, if you want to produce multiple binaries and kernels, you should supply the programs in this pattern:

1. `src/main1.rs` - the first Rust driver.  (This may have any name.)
2. `src/main2.rs` - the second Rust driver.  (This may have any name.)
3. `hoon/app/main1.hoon` - the first Hoon kernel.  (This should have the same name as the Rust driver `main1.rs`.)
4. `hoon/app/main2.hoon` - the second Hoon kernel.  (This should have the same name as the Rust driver `main2.rs`.)

In the `Cargo.toml` file, include both targets explicitly:

```toml
[[bin]]
name = "main1"
path = "src/bin/main1.rs"

[[bin]]
name = "main2"
path = "src/bin/main2.rs"
```

Nockup is opinionated here, and will match `hoon/app/main1.hoon`, etc., as kernels; that is,

```sh
nockup build myproject
```

will produce both `target/release/main1` and `target/release/main2`.

Projects which produce more than one binary cannot be used directly with `nockup run` since more than one process must be started.  This should be kept in mind when using templates which produce more than one binary (like `grpc`).

#### Nockchain Interactions

A Nockchain must be running locally in order to obtain chain state data.

For instance, with a NockApp based on the template `chain`, you need to connect to a running NockApp instance at port 5555:

```
nockup run chain -- --nockchain-socket=5555 get-heaviest-block
# - or -
./chain/target/release/chain --nockchain-socket=5555 get-heaviest-block
```

### Libraries

A project manifest may optionally include a `[libraries]` section.  Conventionally, Hoon libraries have been manually supplied within a desk or repository by manually copying them in.  While this solves the linked library problem by using shared nouns ([~rovnys-ricfer & ~wicdev-wisryt 2024](https://urbitsystems.tech/article/v01-i01/a-solution-to-static-vs-dynamic-linking)), no universal versioning system exists and cross-repository dependencies are difficult to automate.

To that end, Nockup supports three patterns for importing libraries:

1. Single file imports.
2. Repository imports, simple structure.
3. Nested repository imports.

Examples of each are provided in [`example-manifest-with-libraries.toml`](https://github.com/sigilante/nockup/blob/master/manifests/example-manifest-with-libraries.toml).

#### Single Libraries

A single file may be plucked out of context from a public repo for inclusion.

- [`urbit/urbit`:  `bits.hoon`](https://github.com/urbit/urbit/blob/develop/pkg/arvo/lib/bits.hoon) bitwise aliases for Hoon stdlib

```toml
[libraries.bits]
url = "https://github.com/urbit/urbit"
branch = "develop"
file = "pkg/arvo/lib/bits.hoon"
```

This supplies `bits.hoon` at `/hoon/lib/bits.hoon`.  (The developer is responsible for managing dependencies such as `/sur` structure files.)

#### Top-Level Libraries

A simple Hoon library repo should supply a `/desk`, `/hoon`, or `/src` directory at the top level.  (While Rust typically reserves `/src` for `.rs` files, Hoon repositories are not generally configured to expect a Rust runtime and may use the `/src` directory for Hoon source files.)  The `/app`, `/lib` and `/sur` contents are copied directly into `/hoon`.

Sequent is a good example of the simplest possible structure:

- [`jackfoxy/sequent`](https://github.com/jackfoxy/sequent) list functions

This is imported via the `configuration.toml` manifest:

```toml
[libraries.sequent]
url = "https://github.com/jackfoxy/sequent"
commit = "0f6e6777482447d4464948896b763c080dc9e559"
```

which supplies `/desk/lib/seq.hoon` at `/hoon/lib/seq.hoon` and ignores `/mar` and `/tests` (which are both Urbit-specific affordances).

Other Hoon libraries of note include:

- [`lynko/re.hoon`](https://github.com/lynko/re.hoon)
- [`mikolajpp/bytestream`](https://github.com/mikolajpp/bytestream)

#### Nested Libraries

A more complex structure features top-level nesting before the Hoon source library (`/desk`, `/hoon`, or `/src`), such as with the Urbit numerical computing suite.

- [`urbit/numerics`](https://github.com/urbit/numerics)

```toml
[libraries.math]
url = "https://github.com/urbit/numerics"
branch = "main"
directory = "libmath"
commit = "7c11c48ab3f21135caa5a4e8744a9c3f828f2607"

[libraries.lagoon]
url = "https://github.com/urbit/numerics"
branch = "main"
directory = "lagoon"
commit = "7c11c48ab3f21135caa5a4e8744a9c3f828f2607"
```

which supplies these files (among others) in the following pattern:

* `/libmath/desk/lib/math.hoon` at `/hoon/lib/math.hoon`.
* `/lagoon/desk/lib/lagoon.hoon` at `/hoon/lib/lagoon.hoon`.
* `/lagoon/desk/sur/lagoon.hoon` at `/hoon/sur/lagoon.hoon`.

These are simply copied over from the source directory in the repository, so care should be taken to ensure that files with the same name do not conflict (such as `types.hoon`).

### Channels

Nockup can use the `stable` build of `hoon` and `hoonc`.  (As of this release, there is not yet a `nightly` build, but we demonstrate its support here.)

```sh
$ nockup channel show
Default channel: "stable"
Architecture: "aarch64"

$ nockup channel set nightly
Set default channel to 'nightly'.

$ nockup channel show
Default channel: "nightly"
Architecture: "aarch64"
```

## Uninstallation

To uninstall Nockup delete the binary and remove the installation cache:

```sh
$ rm -rf ~/.nockup
```

## Command Reference

Nockup supports the following `nockup` commands.

### Operations

- `nockup install`:  Initialize Nockup cache and download binaries and templates.
- `nockup update`:  Check for updates to binaries and templates.
- `nockup help`:  Print this message or the help of the given subcommand(s).

### Project

- `nockup init`:  Initialize a new NockApp project from a `.toml` config file.
- `nockup build`:  Build a NockApp project using Cargo.
- `nockup run`:  Run a NockApp project.

### channel

- `nockup channel show`: Show currently active channel.
- `nockup channel set`: Set the active channel, from `stable` and `nightly`.  (Most users will prefer `stable`.)

## Security

*Nockup is entirely experimental and many parts are unaudited.  We make no representations or guarantees as to the behavior of this software.*

Nockup uses HTTPS for binary downloads (overriding HTTP in the channel manifests).  The commands `nockup install` and  `nockup update` have the following security measures in place:

1. Check the Blake3 and SHA-1 checksums of the downloaded binaries against the expected index.

    You can do this manually by running:

    ```sh
    b3sum nockup
    sha1sum --check <file>
    ```

    and compare the answers to the expected values from the appropriate toolchain file in `~/.nockup/toolchain`.

2. Check that the binaries are appropriately signed.  Binaries are signed using the [`zorp-gpg-key`](./zorp-gpg-key.pub) for Linux.  (Apple binaries are not currently signed.)

    You can do this manually by running:

    ```sh
    gpg --verify nockup.asc nockup
    ```

    using the `asc` signature listed in the appropriate toolchain file in `~/.nockup/toolchain`.

Code building is a general-purpose computing process, like `eval`.  You should not do it on the same machine on which you store your wallet private keys [0] [1].

- [0]: https://semgrep.dev/blog/2025/security-alert-nx-compromised-to-steal-wallets-and-credentials/
- [1]: https://jdstaerk.substack.com/p/we-just-found-malicious-code-in-the

## Roadmap

### Release Roadmap

* [ ] Replit instance (needs better memory swap management)
* [ ] add Apple code signing support
* [x] update manifest files (and install/update strings) to `nockchain/nockchain`
* [ ] unify batch/continuous kernels via `exit` event:  `[%exit code=@]`

### Later

* `nockup test` to run unit tests
* expand repertoire of templates
  * list and ship appropriate Hoon libraries
* `nockup publish`/`nockup clone` (awaiting PKI/namespace)

## Contributor's Guide

### Release Checklist

Each time [Nockchain](https://github.com/nockchain/nockchain) or Nockup updates:

- [x] Update checksums and code signatures (automatic).
- [x] Update versions and commit hashes in toolchain channels (automatic).
- [x] Update versions and commit hashes in install scripts (automatic).
- [ ] Check and update downstream clients like Replit if necessary (manual per instance, but `nockup update` works).

### Unit Testing

Some CLI unit tests have been implemented and are accessible via `cargo test`.  These can, of course, always be improved.

### Replit Instance

There is a Replit instance available at https://replit.com/@neal50/NockApp?v=1 which demonstrates Nockup functionality in the cloud.  Nockup should automatically track the latest release of `hoonc` and `hoon` on each `nockup install` or `nockup update`.

Pending a public fakenet, it's necessary to run a local fakenet Nockchain node in the Replit container to test Nockchain interactions.  There are also difficulties due to Replit's memory limitations with using the Rust `nightly` toolchain.
