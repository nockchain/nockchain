"""Profile-guided optimization (PGO) rules for Rust binaries.

This module provides rules for building Rust binaries with profile-guided
optimization, which involves three stages:
1. Instrumented build (adds profiling instrumentation)
2. Training run (executes the instrumented binary and collects profiles)
3. Optimized build (uses the collected profiles for optimization)
"""

load("@rules_rust//rust:defs.bzl", "rust_binary")

_LLVM_PROFDATA = "@llvm_toolchain//:llvm-profdata"

# ---------------------------------------------------------------------------
# 1. instrumented build  (adds -Cprofile-generate)
# ---------------------------------------------------------------------------
def rust_pgo_binary_instrument(
        name,
        srcs,
        deps = [],
        data = [],
        crate_root = None,
        edition = None,
        rustc_flags = [],
        **kwargs):
    rust_binary(
        name = name,
        srcs = srcs,
        deps = deps,
        data = data,
        crate_root = crate_root,
        edition = edition,
        rustc_flags = rustc_flags + [
            "-Clink-arg=-Wl",
            "-Cprofile-generate=gen.profdir",
        ],
        **kwargs
    )

# "--remap-path-prefix=$(pwd)=/src",

# ---------------------------------------------------------------------------
# 2. training run  (executes the instr. binary, merges *.profraw → merged.profdata)
# ---------------------------------------------------------------------------
def rust_pgo_binary_run(
        name,
        instr_bin,  # label pointing at rust_pgo_binary_instrument target
        corpus = [],  # input data your program needs at runtime
        args = [],  # command-line args for your program
        tool_deps = [_LLVM_PROFDATA]):
    native.genrule(
        name = name,
        srcs = [instr_bin] + corpus,
        outs = ["merged.profdata"],
        tools = tool_deps,
        cmd = """
set -euo pipefail
mkdir -p raw
# Provide a writable location for hoonc checkpoints inside the Bazel sandbox
mkdir -p nockapp_home
# 1) run workload and collect raw profiles
NOCKAPP_HOME="$$(pwd)/nockapp_home" LLVM_PROFILE_FILE="$$(pwd)/raw/hoonc.profraw" $(location {bin}) {args}
# 2) merge
ls -alh raw/
$(location {_prof}) merge -o "$@" raw/hoonc.profraw
""".format(
            bin = instr_bin,
            args = " ".join(args),
            _prof = _LLVM_PROFDATA,
        ),
    )

# ---------------------------------------------------------------------------
# 3. optimised build  (adds -Cprofile-use + -Cprofile-correction)
# ---------------------------------------------------------------------------
def rust_pgo_binary_optimize(
        name,
        srcs,
        profdata,
        deps = [],
        data = [],
        crate_root = None,
        edition = None,
        rustc_flags = [],
        **kwargs):
    rust_binary(
        name = name,
        srcs = srcs,
        deps = deps,
        data = data + [profdata],  # so the file is available in the sandbox
        crate_root = crate_root,
        edition = edition,
        rustc_flags = rustc_flags + [
            "-Clink-arg=-Wl",
            "-Cprofile-use=$(location {})".format(profdata),
        ],
        **kwargs
    )

# "-Cremap-path-prefix=$(pwd)=/src",
# "-Cprofile-correction",
