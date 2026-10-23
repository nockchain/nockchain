#!/usr/bin/env bash
set -euo pipefail
cd /source
export CARGO_HOME=/usr/local/cargo
export CARGO_TARGET_DIR=/build-target
export RUSTFLAGS='-C force-frame-pointers=yes'
export CARGO_BUILD_JOBS=4
rustc --version > /source/artifacts/linux-toolchain.txt
cargo build --release --bins --locked > /source/artifacts/linux-build.log 2>&1
cp /build-target/release/nockchain-noun-delta-bench /source/artifacts/noun-delta-bench-linux
sha256sum /source/artifacts/noun-delta-bench-linux > /source/artifacts/noun-delta-bench-linux.sha256
