#!/bin/bash
export CARGO_INCREMENTAL=0
export RUSTFLAGS="-Zprofile -Ccodegen-units=1 -Copt-level=0 -Clink-dead-code -Coverflow-checks=off -Zpanic_abort_tests -Cpanic=abort"
export RUSTDOCFLAGS="-Cpanic=abort"
cargo clean
cargo +nightly build
cargo +nightly test
grcov . -s . --binary-path ./target/debug/ -t html --branch --ignore-not-existing -o coverage
