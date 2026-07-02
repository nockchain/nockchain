""" Hoon rules for Bazel build system.
This file contains rules for compiling Hoon source files into JAM files
using the hoonc compiler. It provides a rule for compiling Hoon files
and a higher-level macro for creating libraries from Hoon files.
"""

def _jam_resource_set(_os, _inputs_size):
    """Local-scheduler footprint of a kernel jam compile (hoonc or honk).

    Kernel builds peak ~8-10GB RSS; declaring it keeps Bazel from
    co-scheduling both sides of a parity pair on a 16GB CI runner.
    """
    return {"memory": 10240, "cpu": 1}

def _jam_resource_set_arbitrary(_os, _inputs_size):
    """Arbitrary (hoon-138 self-mint) builds peak ~14GB RSS."""
    return {"memory": 14336, "cpu": 1}

def _hoon_jam_impl(ctx):
    """Implementation of the hoon_jam rule without extra copying."""

    # Print present working directory
    # print("DEBUG: Current working directory: " + str(ctx.configuration))
    # Output file
    output = ctx.outputs.out

    # Create only the temp directory for hoonc's internal use
    tmp_dir = ctx.actions.declare_directory("_tmp_{}".format(ctx.label.name))

    # print("DEBUG: Temp dir: " + tmp_dir.path)
    # Collect the main source file and all deps
    src = ctx.file.src
    deps = depset(
        direct = ctx.files.deps,
        transitive = [dep[DefaultInfo].files for dep in ctx.attr.deps],
    ).to_list()

    # print("DEBUG: Deps: " + str([f.path for f in ctx.files.deps]))
    # Create the command to build the JAM file
    cmd = []
    cmd.append("set -e")  # Exit immediately if any command fails

    # Create temp directory
    cmd.append("mkdir -p {}".format(tmp_dir.path))

    # Resolve the root of the Hoon source tree from the source path: hoon/
    # at the repository root, or nested one directory deep (<parent>/hoon).
    src_path = src.path
    src_dir_parts = src_path.split("/")

    if src_dir_parts[0] == "hoon":
        hoon_dir = "hoon"
    else:
        hoon_dir = "/".join(src_dir_parts[:2])

    # Run hoonc directly with the source path
    hoonc_args = ["--new"]
    if ctx.attr.arbitrary:
        hoonc_args.append("--arbitrary")
    if ctx.attr.output:
        # hoonc writes to the output basename in CWD; the existing
        # `mv out.jam` below picks it up.
        hoonc_args.append("--output out.jam")

    # print("Hoon dir: " + hoon_dir)
    cmd.append("env -i HOME={0} XDG_DATA_HOME={0}/.local/share XDG_CONFIG_HOME={0}/.config TMPDIR={1} RUST_LOG=trace {2} {3} {4} {5}".format(
        hoon_dir,  # Use the original location instead of copying
        tmp_dir.path,
        ctx.executable._hoonc.path,
        " ".join(hoonc_args),
        src_path,  # Use original source path
        hoon_dir,  # Pass the hoon directory directly
    ))

    # Copy the output file
    cmd.append("mv out.jam {}".format(output.path))

    # Execute the command
    ctx.actions.run_shell(
        inputs = [src] + deps,
        outputs = [tmp_dir, output],
        tools = [ctx.executable._hoonc],
        command = "\n".join(cmd),
        progress_message = "Building JAM file from %s" % src.path,
        mnemonic = "HoonCompile",
        use_default_shell_env = False,
        resource_set = _jam_resource_set_arbitrary if ctx.attr.arbitrary else _jam_resource_set,
    )

    return [DefaultInfo(files = depset([output]))]

# Define the rule
hoon_jam = rule(
    implementation = _hoon_jam_impl,
    attrs = {
        "src": attr.label(
            allow_single_file = [".hoon"],
            mandatory = True,
            doc = "Main Hoon source file to compile",
        ),
        "deps": attr.label_list(
            allow_files = [".hoon", ".jam"],
            doc = "Hoon dependencies",
        ),
        "out": attr.output(mandatory = True),
        "arbitrary": attr.bool(
            default = False,
            doc = "Whether to use the --arbitrary flag",
        ),
        "output": attr.bool(
            default = False,
            doc = "Whether to use the --output flag",
        ),
        "_hoonc": attr.label(
            default = Label("//crates/hoonc:hoonc_bin"),
            executable = True,
            cfg = "exec",
        ),
    },
    doc = "Compiles a Hoon source file into a JAM file using Choo",
)

# Higher-level macro for common patterns
def hoon_library(
        name,
        src,
        deps = [],
        arbitrary = False,
        output = False,
        visibility = None):
    """Builds a JAM file from a Hoon source file.

    Args:
        name: Name of the target
        src: Main Hoon source file
        deps: Hoon dependencies (typically //hoon:all_hoon_files)
        arbitrary: Whether to use --arbitrary flag
        output: Pass an explicit `--output out.jam` to hoonc (needed for
            arbitrary builds, whose default output name is not `out.jam`)
        visibility: Target visibility
    """
    jam_name = name + ".jam"

    hoon_jam(
        name = name + "_compile",
        src = src,
        deps = deps,
        out = jam_name,
        arbitrary = arbitrary,
        output = output,
        visibility = ["//visibility:private"],
    )

    native.filegroup(
        name = name,
        srcs = [":" + name + "_compile"],
        visibility = visibility,
    )


def _honk_jam_impl(ctx):
    """Compile a Hoon source with honk (native compiler) inside the sandbox.

    Mirrors `_hoon_jam_impl`'s environment exactly (same workspace-relative
    source paths, same pruned `hoon/` dep tree, scrubbed env) so a honk-built
    jam is byte-comparable to its hoonc-built counterpart — including the
    directory-hash leaf, which hashes the deps tree both compilers see.
    """
    output = ctx.outputs.out
    tmp_dir = ctx.actions.declare_directory("_tmp_{}".format(ctx.label.name))

    src = ctx.file.src
    prelude = ctx.file.prelude
    deps = depset(
        direct = ctx.files.deps,
        transitive = [dep[DefaultInfo].files for dep in ctx.attr.deps],
    ).to_list()

    src_path = src.path
    src_dir_parts = src_path.split("/")
    if src_dir_parts[0] == "hoon":
        hoon_dir = "hoon"
    else:
        hoon_dir = "/".join(src_dir_parts[:2])

    honk_args = []
    env_vars = ""
    if ctx.attr.arbitrary:
        honk_args.append("--arbitrary")
    else:
        honk_args.append("--new")
    if ctx.attr.native_parity:
        # Disable the embedded hoonc prelude substitution: honk mints the
        # prelude natively (the hoon-138 self-mint parity configuration).
        env_vars = " HONK_NATIVE_PARITY=1"

    cmd = []
    cmd.append("set -e")
    cmd.append("mkdir -p {}".format(tmp_dir.path))
    cmd.append("env -i HOME={0} TMPDIR={1}{2} {3} {4} --output {5} --prelude {6} {7} {8}".format(
        hoon_dir,
        tmp_dir.path,
        env_vars,
        ctx.executable._honk.path,
        " ".join(honk_args),
        output.path,
        prelude.path,
        src_path,
        hoon_dir,
    ))

    ctx.actions.run_shell(
        inputs = [src, prelude] + deps,
        outputs = [tmp_dir, output],
        tools = [ctx.executable._honk],
        command = "\n".join(cmd),
        progress_message = "Building native JAM file from %s" % src.path,
        mnemonic = "HonkCompile",
        use_default_shell_env = False,
        resource_set = _jam_resource_set_arbitrary if ctx.attr.arbitrary else _jam_resource_set,
    )

    return [DefaultInfo(files = depset([output]))]

honk_jam = rule(
    implementation = _honk_jam_impl,
    attrs = {
        "src": attr.label(
            allow_single_file = [".hoon"],
            mandatory = True,
            doc = "Main Hoon source file to compile",
        ),
        "deps": attr.label_list(
            allow_files = [".hoon", ".jam"],
            doc = "Hoon dependencies (must match the paired hoon_jam's deps " +
                  "so both compilers hash the same directory tree)",
        ),
        "out": attr.output(mandatory = True),
        "arbitrary": attr.bool(
            default = False,
            doc = "Use --arbitrary (vase build) instead of --new (kernel build)",
        ),
        "native_parity": attr.bool(
            default = False,
            doc = "Set HONK_NATIVE_PARITY=1 (native prelude self-mint)",
        ),
        "prelude": attr.label(
            allow_single_file = [".hoon"],
            default = Label("//hoon/common:hoon.hoon"),
            doc = "Prelude source passed to --prelude",
        ),
        "_honk": attr.label(
            default = Label("//crates/honk:honk"),
            executable = True,
            cfg = "exec",
        ),
    },
    doc = "Compiles a Hoon source file into a JAM file using honk",
)

def honk_library(
        name,
        src,
        deps = [],
        arbitrary = False,
        native_parity = False,
        visibility = None):
    """honk twin of `hoon_library`: builds `<name>.jam` natively with honk."""
    jam_name = name + ".jam"

    honk_jam(
        name = name + "_compile",
        src = src,
        deps = deps,
        out = jam_name,
        arbitrary = arbitrary,
        native_parity = native_parity,
        visibility = ["//visibility:private"],
    )

    native.filegroup(
        name = name,
        srcs = [":" + name + "_compile"],
        visibility = visibility,
    )
