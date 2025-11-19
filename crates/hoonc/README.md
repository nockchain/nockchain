# `hoonc`: compile hoon

To self-bootstrap:

```bash
cargo run --release hoon/apps/hoonc/hoonc.hoon hoon
```

This will save the built kernel as `out.jam` in the current directory. This should be moved to the bootstrap directory so the `hoonc` binary can pick it up:

```bash
mv out.jam bootstrap/hoonc.jam
```

Once this is done, you should be able to run

``` bash
cargo build --release
```

and use the resulting binary in `target/release/hoonc` (in the `nockapp` directory) to build NockApp kernels or arbitrary hoon files as detailed in the following section.

## Bootstraps

The repository ships with two bootstrap jams:

- `bootstrap/hoonc-prewarm.jam` – pre-booted and used by default.
- `bootstrap/hoonc.jam` – the minimal bootstrap, kept as a fallback.

Regenerate the prewarmed bootstrap after changing the kernel or bundled Hoon text:

```bash
cargo run --release --bin prewarm -- --output bootstrap/hoonc-prewarm.jam
```

The helper writes to a temporary data directory by default; pass `--data-dir` if you need to inspect the intermediate checkpoint.

## Usage

The following assumes you have the `hoonc` binary in your path, which can be built with `cargo build --release` and found in `target/release/hoonc`.

For `hoonc`, the first argument is the entrypoint to the program, while the second argument is the root directory for source files.

```bash
hoonc main.hoon hoon
```

### Building Arbitrary Hoon

To build arbitrary Hoon files, use the `--arbitrary` flag:

```bash
# Create a directory for your Hoon files
mkdir hoon

# Create a simple Hoon file
echo '%trivial' > hoon/trivial.hoon

# Build the Hoon file (exclude --new if you want to use the build cache)
hoonc --new --arbitrary hoon/trivial.hoon hoon
```

For ad-hoc Hoon experiments without wiring up `hoonc`, you can also run the `hoon` CLI directly (see `crates/hoon/src/main.rs`):

```bash
cargo run --release -p hoon -- <path-to-generator.hoon>
```

## Hoon

`hoonc` supports the Hoon language as defined in `/sys/hoon`.  However, the build system does not replicate Urbit's `+ford`
functionality exactly, as that is closely tied to the Urbit Arvo operating system.  `hoonc` supports the following build
runes:

- `/+` load from `/lib`
- `/-` load from `/sur` (currently discouraged in NockApps)
- `/=` load from specified path (required `%hoon` mark)
- `/*` load from specified path via specified mark (presumptively `%hoon` or `%jock`)
- `/#` load and kick from `/dat`. Used when you have some nock computation you want to precompute.
- `/?` version pinning (ignored)

## Developer Troubleshooting

If you make changes to the `poke` arm in `hoonc.hoon` or in `wrapper.hoon`, you'll need to update the minimal `hoonc.jam` file by running:

```bash
cargo run --release hoon/apps/hoonc/hoonc.hoon hoon
mv out.jam bootstrap/hoonc.jam
```

and committing the changes to `hoonc.jam` so that the CI can properly bootstrap the `hoonc` kernel. Afterwards, re-run the prewarm helper to refresh `bootstrap/hoonc-prewarm.jam`.
