# Developing NockVM

Status: Experimental

*Trust posture: this guide includes experimental and historical material, verify commands and pill assumptions against current code and tooling.*

## Rust

### Build

To build NockVM, make sure Rust is installed, then run:

```bash
cd crates/lib/nockvm/rust/nockvm
cargo build
```

This builds the `nockvm` library crate. The crate does not currently define a standalone `target/debug/nockvm` executable.

#### Pills

NockVM development and testing, unlike regular development and ship operation, historically required careful control over what pill is used to launch a ship. Pills currently present in `resources/pills/` include:
- **baby.pill**: an extremely minimal Arvo-shaped core and Hoon standard library (`~wicdev-wisryt` [streamed a
video of its development](https://youtu.be/fOVhCx1a-9A))
- **toddler.pill**: a slightly more complex Arvo and Hoon than `baby`, which runs slow recursive operations for testing jets
- **azimuth.pill**: a pill that processes an Azimuth snapshot
- **full.pill**: the complete Urbit `v2.11` pill
- **slim.pill**: a slimmed down version of the Urbit `v2.11` pill that has had every desk and agent not necessary for booting to dojo removed

More pill background lives in [docs/pills.md](docs/pills.md) (legacy context, not canonical protocol guidance).

### Test

The command to run the NockVM suite of unit tests is:

```bash
cd crates/lib/nockvm/rust/nockvm
cargo test --verbose -- --test-threads=1
```

Historically, many tests in this crate were run with `-- --test-threads=1` to avoid shared-resource interference.

### Style

NockVM uses the default Rust formatting and style. The CI jobs are configured to reject any code which produces linter or style warnings. Therefore, as a final step before uploading code changes to GitHub, it's recommended to run the following commands:

```bash
cd crates/lib/nockvm/rust/nockvm
cargo fmt
cargo clippy --all --benches --tests --examples --all-features
```

This will auto-format your code and check for linter warnings.

### Watch

To watch rust and check for errors, run

```bash
cargo watch --clear
```

Until terminated with ctrl-c, this rebuilds the NockVM library on source changes and reports warnings and errors.

## Hoon

The Nock analysis and lowering for NockVM is written in Hoon, and lives at `hoon/codegen.` Historically this was jammed and embedded into runtime binaries during New Mars/Ares experimentation.

If the hoon source has been synced to a desk, e.g. `sandbox`, on a fakezod, then the build generator can be invoked as:

```
.cg/jam +sandbox!cg-make
```

This builds the Hoon standard library and the NockVM Nock analysis as a "trap" meant to be run by NockVM. The jammed output can be found at `<fakezod-pier>/.urb/put/cg.jam` for manual experiments.

Instructions on testing the analysis in a fakezod are forthcoming.
