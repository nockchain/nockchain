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

# Run every honk parity gate in one shot: the cargo gates (compiler_mint +
# native_parity_138 full hoon-138 self-mint byte parity) AND the 6-kernel
# byte/dir-hash parity vs hoonc. Builds hoonc + honk, the hoonc reference kernel
# jams (assets/*.jam), and honk's native kernel jams (assets/native/*.jam) first
# — so this is a long run (hoonc compiles 6 kernels). hatch's parser-oracle
# parity is separate: it needs Bazel fixtures (`make build-hatch-test-assets`),
# then `cargo nextest run --release -p hatch`.
honk-parity-all: build-kernel-assets honk-kernel-jams
    cargo nextest run --release -p honk
    just honk-parity

# Arbitrary-build parity for the hoon-138 prelude: honk's NATIVE mint
# (HONK_NATIVE_PARITY=1, no embedded prelude) vs hoonc's arbitrary build,
# byte-compared. PASSING (2026-06-28): the native mint completes with bounded
# memory (~40s) and is BYTE-IDENTICAL to hoonc (2,286,744 B). The RSS guard is
# now a cheap backstop, not expected to fire. Build honk + hoonc first
# (`just build`). The hoonc-free cargo equivalent is the `native_parity_138`
# test (default-embedded vs HONK_NATIVE_PARITY=1 honk build, byte-compared).
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

# Peak RSS for honk's NATIVE mint of the hoon-138 prelude (HONK_NATIVE_PARITY=1,
# embedded prelude disabled — the memory-heavy self-mint). macOS-only: uses
# /usr/bin/time -l (peak RSS in bytes). The honk binary's own logs go to the
# .log; only the rusage peak + wall time are printed.
honk-138-rss:
    cargo build --release -p honk
    @echo "honk NATIVE hoon-138 mint — measuring peak RSS (~40s)..."
    HONK_NATIVE_PARITY=1 /usr/bin/time -l target/release/honk --arbitrary --output /tmp/honk-138-rss.jam --prelude hoon/common/hoon.hoon hoon/common/hoon.hoon hoon >/dev/null 2>/tmp/honk-138-rss.log
    @awk '/maximum resident set size/{printf "hoon-138 native mint  peak RSS: %.2f GB (%d bytes)\n", $1/1073741824, $1} / real/{printf "                      wall: %s s\n", $1}' /tmp/honk-138-rss.log

# Peak RSS for honk compiling the roswell kernel (default build, embedded
# prelude — the normal kernel compile path). macOS-only: /usr/bin/time -l.
honk-roswell-rss:
    cargo build --release -p honk
    mkdir -p assets/native
    @echo "honk roswell kernel compile — measuring peak RSS..."
    /usr/bin/time -l target/release/honk --new --output assets/native/roswell.jam --prelude hoon/common/hoon.hoon hoon/apps/roswell/roswell.hoon hoon >/dev/null 2>/tmp/honk-roswell-rss.log
    @awk '/maximum resident set size/{printf "roswell kernel  peak RSS: %.2f GB (%d bytes)\n", $1/1073741824, $1} / real/{printf "                wall: %s s\n", $1}' /tmp/honk-roswell-rss.log
