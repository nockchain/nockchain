"""Shared cargo_lint_sources filegroup macro."""

def cargo_lint_sources(name = "cargo_lint_sources"):
    native.filegroup(
        name = name,
        srcs = native.glob(
            [
                "**/*.rs",
                "Cargo.toml",
                "BUILD",
                "BUILD.bazel",
            ],
            allow_empty = True,
            exclude = [
                "bazel-*",
                "bazel-*/*",
                "bazel-*/*/**",
                "bazel",
                "bazel/*",
                "bazel/*/**",
                "target",
                "target/*",
                "target/*/**",
                ".git",
                ".git/*",
                ".git/*/**",
            ],
        ),
        visibility = ["//visibility:public"],
    )
