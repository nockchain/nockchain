""" Rust release profile flags for Bazel build system.
"""

load("@rules_rust//rust:defs.bzl", "rust_test")

def rust_test_with_miri(name, **kwargs):
    """Creates a rust_test that can be run with Miri."""
    rust_test(
        name = name,
        tags = kwargs.pop("tags", []) + ["miri", "manual"],
        **kwargs
    )

def rustc_opt_flags(enable_lto = True, target_cpu = "native"):
    """Returns rustc optimization flags for different build configurations.

    Args:
        enable_lto: Whether to enable Link Time Optimization in release builds
        target_cpu: Target CPU architecture for optimization

    Usage:
        load("//tools/bazel/rust:defs.bzl", "rustc_opt_flags")

        rust_binary(
            name = "my_binary",
            srcs = ["src/main.rs"],
            rustc_flags = rustc_opt_flags(),
            ...
        )
    Returns:
        A dictionary of rustc optimization flags based on the build configuration.
    """
    release_flags = [
        "-Coverflow-checks=off",
        "-Cincremental=false",
        "-Cdebug-assertions=off",
        "-Ccodegen-units=1",
        "-Cpanic=abort",
        "-Copt-level=3",
        "-Cstrip=none",
        "-Ctarget-cpu=" + target_cpu,
    ]

    if enable_lto:
        release_flags.append("-Clto=thin")

    return select({
        "//:release": release_flags,
        "//conditions:default": [
            "-Copt-level=1",
        ],
    })
