#!/usr/bin/env bash
set -euo pipefail
cd /source
export CARGO_HOME=/usr/local/cargo CARGO_TARGET_DIR=/build-target CARGO_BUILD_JOBS=4
export RUSTFLAGS='-C force-frame-pointers=yes'
for variant in 256-stream 256-oneshot 128-oneshot; do
 case "$variant" in
  256-stream) flags=();;
  256-oneshot) flags=(--features one-shot-cell);;
  128-oneshot) flags=(--features compact-hash,one-shot-cell);;
 esac
 cargo build --release --locked --bins "${flags[@]}" > "/source/artifacts/build-$variant.log" 2>&1
 cp /build-target/release/nockchain-noun-delta-bench "/source/artifacts/noun-$variant-linux"
 cp /build-target/release/sequence "/source/artifacts/sequence-$variant-linux"
 sha256sum "/source/artifacts/noun-$variant-linux" "/source/artifacts/sequence-$variant-linux" > "/source/artifacts/binary-$variant.sha256"
done
