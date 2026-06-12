# Bazel-driven equivalents of the top-level cargo recipes in `justfile`.
# Invoke through the module name, e.g. `just bazel build` or `just bazel honk-parity`.
#
# These mirror the cargo recipes one-for-one but drive Bazel instead:
#   - build / test            -> the whole Bazel graph
#   - build-assets            -> hoonc-built kernel jams (rules_hoon)
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
