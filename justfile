# Bazel-driven equivalents of the recipes below. Invoke as `just bazel <recipe>`.
mod bazel 'bazel.just'

# List available recipes, including nested modules (default).
default:
    @just --list --list-submodules

build:
    cargo build --release

test:
    cargo nextest run --release

test-honk:
    cargo nextest run --release -p honk

build-honk-assets: honc-cold-138-asset hoonc-octs-type-138-asset

honc-cold-138-asset:
    mkdir -p assets target/honk-assets
    target/release/honk --new --dump-wrapper-assets target/honk-assets/wrapper-assets --prelude hoon/common/hoon.hoon hoon
    cp target/honk-assets/wrapper-assets/honc-cold-138.jam assets/honc-cold-138.jam

hoonc-octs-type-138-asset:
    mkdir -p crates/honk/assets target/honk-assets
    target/release/hoonc --dynock-typed --output target/honk-assets/data-import-typed-dynock.jam hoon/probes/hoon-compiler/hoonc_octs_type_probe.hoon hoon
    target/release/extract-hoonc-octs-type target/honk-assets/data-import-typed-dynock.jam crates/honk/assets/hoonc-octs-type-138.jam

# Each kernel uses a fresh `--new` data dir (target/hoonc-new) so hoonc never
# reuses a warm cache: a clean cold build, good for timing and repeatable across
# all six (bare `--new` aborts on a non-empty data dir).
# Convenience for non-Bazel users; `just bazel build-assets` is canonical.
build-kernel-assets: build dumb-jam wal-jam miner-jam peek-jam bridge-jam roswell-jam

dumb-jam:
    mkdir -p assets
    rm -rf target/hoonc-new
    time target/release/hoonc --new --data-dir target/hoonc-new --output dumb.jam hoon/apps/dumbnet/outer.hoon hoon
    mv dumb.jam assets/dumb.jam

wal-jam:
    mkdir -p assets
    rm -rf target/hoonc-new
    time target/release/hoonc --new --data-dir target/hoonc-new --output wal.jam hoon/apps/wallet/wallet.hoon hoon
    mv wal.jam assets/wal.jam

miner-jam:
    mkdir -p assets
    rm -rf target/hoonc-new
    time target/release/hoonc --new --data-dir target/hoonc-new --output miner.jam hoon/apps/dumbnet/miner.hoon hoon
    mv miner.jam assets/miner.jam

peek-jam:
    mkdir -p assets
    rm -rf target/hoonc-new
    time target/release/hoonc --new --data-dir target/hoonc-new --output peek.jam hoon/apps/peek/peek.hoon hoon
    mv peek.jam assets/peek.jam

bridge-jam:
    mkdir -p assets
    rm -rf target/hoonc-new
    time target/release/hoonc --new --data-dir target/hoonc-new --output bridge.jam hoon/apps/bridge/bridge.hoon hoon
    mv bridge.jam assets/bridge.jam

roswell-jam:
    mkdir -p assets
    rm -rf target/hoonc-new
    time target/release/hoonc --new --data-dir target/hoonc-new --output roswell.jam hoon/apps/roswell/roswell.hoon hoon
    mv roswell.jam assets/roswell.jam

honk-roswell-kernel:
    mkdir -p assets/native
    cargo run --release -p honk --bin honk -- --new --output assets/native/roswell.jam --prelude hoon/common/hoon.hoon hoon/apps/roswell/roswell.hoon hoon

# Build every kernel in assets/ natively with honk into assets/native/.
# Never touches the hoonc-built reference jams in assets/.
honk-kernel-jams:
    cargo build --release -p honk
    mkdir -p assets/native
    target/release/honk --new --output assets/native/dumb.jam --prelude hoon/common/hoon.hoon hoon/apps/dumbnet/outer.hoon hoon
    target/release/honk --new --output assets/native/wal.jam --prelude hoon/common/hoon.hoon hoon/apps/wallet/wallet.hoon hoon
    target/release/honk --new --output assets/native/miner.jam --prelude hoon/common/hoon.hoon hoon/apps/dumbnet/miner.hoon hoon
    target/release/honk --new --output assets/native/peek.jam --prelude hoon/common/hoon.hoon hoon/apps/peek/peek.hoon hoon
    target/release/honk --new --output assets/native/bridge.jam --prelude hoon/common/hoon.hoon hoon/apps/bridge/bridge.hoon hoon
    target/release/honk --new --output assets/native/roswell.jam --prelude hoon/common/hoon.hoon hoon/apps/roswell/roswell.hoon hoon

# Compare every honk-built kernel against the hoonc-built reference.
# PASS requires byte equality or a dir-hash-only difference (proven by
# substitution + rejam). See jam-diff --kernel-parity.
honk-parity:
    cargo build --release -p honk-tools
    target/release/jam-diff --kernel-parity assets/dumb.jam assets/native/dumb.jam
    target/release/jam-diff --kernel-parity assets/wal.jam assets/native/wal.jam
    target/release/jam-diff --kernel-parity assets/miner.jam assets/native/miner.jam
    target/release/jam-diff --kernel-parity assets/peek.jam assets/native/peek.jam
    target/release/jam-diff --kernel-parity assets/bridge.jam assets/native/bridge.jam
    target/release/jam-diff --kernel-parity assets/roswell.jam assets/native/roswell.jam

# Arbitrary-build parity for the hoon-138 prelude: honk's NATIVE mint
# (HONK_NATIVE_PARITY=1, no embedded prelude) vs hoonc's arbitrary build,
# byte-compared. NOTE: honk's native mint of the full prelude currently
# exhausts memory before completing (~4GB/min, no plateau), so this reports the
# blowup under an RSS guard; it becomes a real parity gate once native mint
# memory is bounded. Build honk + hoonc first (`just build`).
honk-138-parity:
    crates/honk/test-assets/honk_138_native_parity.sh

# Native-types migration (docs/native-compiler/NATIVE-TYPES-MIGRATION.md) Phase-0
# harnesses. native-parity-dual: strict-cmp acceptance gate (§2.2/RT-02) — honk
# vs hoonc reference, dir-hash-only diffs reported WAIVED. Pass kernel name(s) to
# filter, e.g. `just native-parity-dual dumb`.
native-parity-dual *args:
    bash crates/honk/test-assets/native-parity/dual_run.sh {{args}}

# Regenerate ("regen") or verify ("check") the emitted-formula golden corpus.
native-goldens mode="check":
    bash crates/honk/test-assets/native-parity/regen_goldens.sh {{mode}}

# Gate: honk must compile the roswell kernel in under 60 seconds
# (cargo build excluded). Diagnose failures with NATIVE_HOON_TRACE=1 and
# RUST_LOG=honk=info for per-phase timing.
honk-roswell-timed:
    cargo build --release -p honk
    mkdir -p assets/native
    bash -c 'start=$(date +%s); target/release/honk --new --output assets/native/roswell.jam --prelude hoon/common/hoon.hoon hoon/apps/roswell/roswell.hoon hoon; end=$(date +%s); elapsed=$((end-start)); echo "roswell native compile: ${elapsed}s"; test "$elapsed" -lt 60'
