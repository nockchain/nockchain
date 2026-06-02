#!/bin/bash
set -euo pipefail

BUILD_CONTEXT=""

usage() {
    cat <<'EOF'
Build a tracked Docker image for nockchain-bench.

Usage:
  scripts/build_nockchain_bench_image.sh --variant <standard|profiling> --tag <tag> [options]

Options:
  --tag <tag>                 Docker image tag to build.
  --variant <variant>         Image variant to build: standard or profiling.
  --binary <path>             Path to nockchain-bench binary.
                              Default: target/release/nockchain-bench for standard,
                              target/bytehound/nockchain-bench for profiling.
  --samply-bin <path>         Path to samply for profiling builds.
                              Default: command -v samply
  --skip-cargo-build          Skip cargo build and use the selected binary as-is.
  --dry-run                   Print resolved inputs without invoking Docker.
  --help                      Show this help text.
EOF
}

die() {
    printf 'error: %s\n' "$1" >&2
    exit 1
}

resolve_repo_root() {
    local script_path script_dir
    script_path="${BASH_SOURCE[0]}"
    script_dir="${script_path%/*}"
    if [[ "$script_dir" == "$script_path" ]]; then
        script_dir="."
    fi
    cd -- "$script_dir/.." && pwd
}

resolve_repo_path() {
    local repo_root="$1"
    local path="$2"
    if [[ "$path" = /* ]]; then
        printf '%s\n' "$path"
    else
        printf '%s/%s\n' "$repo_root" "$path"
    fi
}

cleanup() {
    if [[ -n "${BUILD_CONTEXT:-}" && -d "${BUILD_CONTEXT}" ]]; then
        rm -rf -- "${BUILD_CONTEXT}"
    fi
}

main() {
    local repo_root
    local tag=""
    local variant=""
    local binary_arg=""
    local samply_arg=""
    local skip_cargo_build=false
    local dry_run=false
    local binary_path samply_path dockerfile_path
    local -a cargo_args=()

    repo_root="$(resolve_repo_root)"

    while (($# > 0)); do
        case "$1" in
            --tag)
                shift
                (($# > 0)) || die "--tag requires a value"
                tag="$1"
                ;;
            --variant)
                shift
                (($# > 0)) || die "--variant requires a value"
                variant="$1"
                ;;
            --binary)
                shift
                (($# > 0)) || die "--binary requires a value"
                binary_arg="$1"
                ;;
            --samply-bin)
                shift
                (($# > 0)) || die "--samply-bin requires a value"
                samply_arg="$1"
                ;;
            --skip-cargo-build)
                skip_cargo_build=true
                ;;
            --dry-run)
                dry_run=true
                ;;
            --help)
                usage
                exit 0
                ;;
            *)
                die "unknown argument: $1"
                ;;
        esac
        shift
    done

    [[ -n "$tag" ]] || die "--tag is required"
    [[ -n "$variant" ]] || die "--variant is required"

    case "$variant" in
        standard)
            dockerfile_path="$repo_root/docker/nockchain-bench/Dockerfile"
            [[ -n "$binary_arg" ]] || binary_arg="target/release/nockchain-bench"
            cargo_args=(build -p nockchain-bench --release)
            ;;
        profiling)
            dockerfile_path="$repo_root/docker/nockchain-bench/Dockerfile.profiling"
            [[ -n "$binary_arg" ]] || binary_arg="target/bytehound/nockchain-bench"
            cargo_args=(build -p nockchain-bench --profile bytehound)
            ;;
        *)
            die "--variant must be one of: standard, profiling"
            ;;
    esac

    binary_path="$(resolve_repo_path "$repo_root" "$binary_arg")"

    if ! $skip_cargo_build; then
        (
            cd -- "$repo_root"
            "${CARGO:-cargo}" "${cargo_args[@]}"
        )
    fi

    [[ -f "$binary_path" ]] || die "nockchain-bench binary not found at $binary_path"

    if [[ "$variant" == "profiling" ]]; then
        if [[ -n "$samply_arg" ]]; then
            samply_path="$samply_arg"
        else
            samply_path="$(command -v samply || true)"
        fi
        [[ -n "$samply_path" ]] || die "profiling variant requires samply; install it or pass --samply-bin <path>"
        [[ -f "$samply_path" ]] || die "samply binary not found at $samply_path"
    else
        samply_path=""
    fi

    if $dry_run; then
        printf 'repo_root=%s\n' "$repo_root"
        printf 'variant=%s\n' "$variant"
        printf 'tag=%s\n' "$tag"
        printf 'binary=%s\n' "$binary_path"
        printf 'dockerfile=%s\n' "$dockerfile_path"
        printf 'skip_cargo_build=%s\n' "$skip_cargo_build"
        if [[ "$variant" == "profiling" ]]; then
            printf 'samply=%s\n' "$samply_path"
        fi
        exit 0
    fi

    BUILD_CONTEXT="$(mktemp -d /tmp/nockchain-bench-image-build.XXXXXX)"
    trap cleanup EXIT

    cp -- "$binary_path" "$BUILD_CONTEXT/nockchain-bench"
    chmod 755 "$BUILD_CONTEXT/nockchain-bench"

    if [[ "$variant" == "profiling" ]]; then
        cp -- "$samply_path" "$BUILD_CONTEXT/samply"
        chmod 755 "$BUILD_CONTEXT/samply"
    fi

    docker build -t "$tag" -f "$dockerfile_path" "$BUILD_CONTEXT"
}

main "$@"
