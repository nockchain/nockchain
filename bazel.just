# Bazel-driven equivalents of the top-level cargo recipes in `justfile`.
# Invoke through the module name, e.g. `just bazel build` or `just bazel honk-parity`.
#
# These mirror the cargo recipes one-for-one but drive Bazel instead:
#   - build / test            -> the whole Bazel graph
#   - build-assets            -> hoonc-built kernel jams (rules_hoon)
#   - build-assets-honk       -> the same jams built by honk inside Bazel
#   - honk-kernel-jams        -> kernels compiled natively by the Bazel-built honk
#   - honk-parity             -> compare the two with the Bazel-built jam-diff
# Note: rules_hoon always invokes hoonc with `--new` in a sandbox, so the
# reference kernels produced by `build-assets` are never contaminated by a warm
# hoonc cache (unlike the cargo `build-kernel-assets` recipe).

# List available recipes, including nested modules (default).
default:
    @just --list bazel --list-submodules

# Build the entire workspace with Bazel.
build:
    bazel build //...

# Test the entire workspace with Bazel.
test:
    bazel test //...

# Build the hoonc reference kernels via Bazel (rules_hoon) and copy into assets/.
build-assets:
    bazel build //assets:kernels
    mkdir -p assets
    cp -f bazel-bin/assets/dumb.jam assets/dumb.jam
    cp -f bazel-bin/assets/wal.jam assets/wal.jam
    cp -f bazel-bin/assets/miner.jam assets/miner.jam
    cp -f bazel-bin/assets/peek.jam assets/peek.jam
    cp -f bazel-bin/assets/bridge.jam assets/bridge.jam
    cp -f bazel-bin/assets/roswell.jam assets/roswell.jam
    chmod u+w assets/dumb.jam assets/wal.jam assets/miner.jam assets/peek.jam assets/bridge.jam assets/roswell.jam

# honk twin of build-assets: build all six kernels with honk INSIDE Bazel
# (parallel, resource-scheduled — the Bazel equivalent of the cargo-side
# `just build-kernel-assets-honk`) and copy into the same assets/<kernel>.jam
# consumer paths. Byte-identical to build-assets output; the per-kernel
# strict-cmp parity gates (//assets/native:kernel_parity_test) guarantee it.
build-assets-honk:
    bazel build //assets/native:kernels
    mkdir -p assets
    cp -f bazel-bin/assets/native/dumb_native.jam assets/dumb.jam
    cp -f bazel-bin/assets/native/wal_native.jam assets/wal.jam
    cp -f bazel-bin/assets/native/miner_native.jam assets/miner.jam
    cp -f bazel-bin/assets/native/peek_native.jam assets/peek.jam
    cp -f bazel-bin/assets/native/bridge_native.jam assets/bridge.jam
    cp -f bazel-bin/assets/native/roswell_native.jam assets/roswell.jam
    chmod u+w assets/dumb.jam assets/wal.jam assets/miner.jam assets/peek.jam assets/bridge.jam assets/roswell.jam

# Prime the incremental honk cache consumed by //assets/native honk targets.
# Must use the Bazel-built honk — cache keys embed a per-build compiler
# fingerprint, so a cargo-built honk's cache never hits under Bazel — and
# must run from the workspace root so key paths match the sandbox layout.
# The jams are throwaway; .honk-cache/ is the product. Reprime after
# compiler or source changes to keep warm starts warm (a stale cache is
# never wrong, just cold).
prime-honk-cache:
    bazel build //crates/honk:honk
    rm -rf /tmp/honk-prime && mkdir -p /tmp/honk-prime
    bazel-bin/crates/honk/honk --cache-dir .honk-cache --output /tmp/honk-prime/dumb.jam --prelude hoon/common/hoon.hoon hoon/apps/dumbnet/outer.hoon hoon
    bazel-bin/crates/honk/honk --cache-dir .honk-cache --output /tmp/honk-prime/wal.jam --prelude hoon/common/hoon.hoon hoon/apps/wallet/wallet.hoon hoon
    bazel-bin/crates/honk/honk --cache-dir .honk-cache --output /tmp/honk-prime/miner.jam --prelude hoon/common/hoon.hoon hoon/apps/dumbnet/miner.hoon hoon
    bazel-bin/crates/honk/honk --cache-dir .honk-cache --output /tmp/honk-prime/peek.jam --prelude hoon/common/hoon.hoon hoon/apps/peek/peek.hoon hoon
    bazel-bin/crates/honk/honk --cache-dir .honk-cache --output /tmp/honk-prime/bridge.jam --prelude hoon/common/hoon.hoon hoon/apps/bridge/bridge.hoon hoon
    bazel-bin/crates/honk/honk --cache-dir .honk-cache --output /tmp/honk-prime/roswell.jam --prelude hoon/common/hoon.hoon hoon/apps/roswell/roswell.hoon hoon
    rm -rf /tmp/honk-prime

# Compile every kernel natively with the Bazel-built honk into assets/native/.
honk-kernel-jams:
    bazel build //crates/honk:honk
    mkdir -p assets/native
    bazel-bin/crates/honk/honk --new --output assets/native/dumb.jam --prelude hoon/common/hoon.hoon hoon/apps/dumbnet/outer.hoon hoon
    bazel-bin/crates/honk/honk --new --output assets/native/wal.jam --prelude hoon/common/hoon.hoon hoon/apps/wallet/wallet.hoon hoon
    bazel-bin/crates/honk/honk --new --output assets/native/miner.jam --prelude hoon/common/hoon.hoon hoon/apps/dumbnet/miner.hoon hoon
    bazel-bin/crates/honk/honk --new --output assets/native/peek.jam --prelude hoon/common/hoon.hoon hoon/apps/peek/peek.hoon hoon
    bazel-bin/crates/honk/honk --new --output assets/native/bridge.jam --prelude hoon/common/hoon.hoon hoon/apps/bridge/bridge.hoon hoon
    bazel-bin/crates/honk/honk --new --output assets/native/roswell.jam --prelude hoon/common/hoon.hoon hoon/apps/roswell/roswell.hoon hoon

# (Run `just bazel build-assets` + `just bazel honk-kernel-jams` first.)
# Compare honk kernels vs hoonc references with the Bazel-built jam-diff.
honk-parity:
    bazel build //crates/honk-tools:jam-diff
    bazel-bin/crates/honk-tools/jam-diff --kernel-parity assets/dumb.jam assets/native/dumb.jam
    bazel-bin/crates/honk-tools/jam-diff --kernel-parity assets/wal.jam assets/native/wal.jam
    bazel-bin/crates/honk-tools/jam-diff --kernel-parity assets/miner.jam assets/native/miner.jam
    bazel-bin/crates/honk-tools/jam-diff --kernel-parity assets/peek.jam assets/native/peek.jam
    bazel-bin/crates/honk-tools/jam-diff --kernel-parity assets/bridge.jam assets/native/bridge.jam
    bazel-bin/crates/honk-tools/jam-diff --kernel-parity assets/roswell.jam assets/native/roswell.jam
