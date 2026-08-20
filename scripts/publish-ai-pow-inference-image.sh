#!/usr/bin/env bash
set -euo pipefail

readonly DEFAULT_IMAGE="ghcr.io/nockchain/nockchain-ai-pow-inference"
readonly DEFAULT_PLATFORM="linux/amd64"

usage() {
  cat <<'EOF'
Publish an AI-PoW inference image directly to a registry.

Usage:
  scripts/publish-ai-pow-inference-image.sh [options]

Options:
  --base IMAGE:TAG   Publish a source-only vLLM plugin overlay on this image.
                     Omit this option to build the complete runtime image.
  --builder NAME     Use this persistent Buildx builder.
  --dry-run          Print the Docker command without running it.
  --image IMAGE      Registry repository. Defaults to
                     ghcr.io/nockchain/nockchain-ai-pow-inference.
  --platform VALUE   Target platform. Defaults to linux/amd64.
  --tag TAG          Published tag. Defaults to dev-<current-branch>.
  -h, --help         Show this help.

Environment variables with the same names provide defaults: BASE_IMAGE,
BUILDX_BUILDER, IMAGE, PLATFORM, and TAG. Command-line options take precedence.
Docker must already be authenticated to the target registry.

Fast source loop:
  1. Publish or select a complete base image.
  2. Run this command with --base BASE_IMAGE and a stable development tag.
  3. Use the printed image reference in the Runpod pod or template.
  4. Recreate the development pod to pull that tag. Keep model weights on the
     Runpod volume so they survive pod replacement.

The complete build uses the selected builder's persistent cache and exports a
registry cache beside the image. A native linux/amd64 builder avoids emulation
on an Apple Silicon workstation.

The source-only overlay replaces only the installed vllm-miner Python package.
Use it for edits under vllm-plugin/src. Use a complete build after changes to
Rust, native kernels, pyproject.toml dependencies, lock files, or either base
Dockerfile.
EOF
}

image="${IMAGE:-$DEFAULT_IMAGE}"
platform="${PLATFORM:-$DEFAULT_PLATFORM}"
tag="${TAG:-}"
base_image="${BASE_IMAGE:-}"
builder="${BUILDX_BUILDER:-}"
dry_run=0

while (($#)); do
  case "$1" in
    --base)
      base_image="${2:?--base requires an image reference}"
      shift 2
      ;;
    --builder)
      builder="${2:?--builder requires a builder name}"
      shift 2
      ;;
    --dry-run)
      dry_run=1
      shift
      ;;
    --image)
      image="${2:?--image requires a registry repository}"
      shift 2
      ;;
    --platform)
      platform="${2:?--platform requires a platform}"
      shift 2
      ;;
    --tag)
      tag="${2:?--tag requires a tag}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf 'error: unknown option: %s\n\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -z "$tag" ]]; then
  branch="$(git symbolic-ref --quiet --short HEAD || git rev-parse --short=12 HEAD)"
  branch="$(printf '%s' "$branch" | LC_ALL=C tr -c 'A-Za-z0-9_.-' '-')"
  tag="dev-${branch:0:123}"
fi

if [[ "$image" == *@* || "${image##*/}" == *:* ]]; then
  printf 'error: --image must not include a tag or digest: %s\n' "$image" >&2
  exit 2
fi

readonly image_ref="${image}:${tag}"
build=(docker buildx build
  --platform "$platform"
  --tag "$image_ref"
  --provenance=false
  --push)

if [[ -n "$builder" ]]; then
  build+=(--builder "$builder")
fi

if [[ -n "$base_image" ]]; then
  build+=(
    --file docker/Dockerfile.ai-pow-inference-plugin
    --build-arg "BASE_IMAGE=${base_image}"
  )
else
  readonly cache_ref="${image}:buildcache"
  build+=(
    --file docker/Dockerfile.ai-pow-inference
    --target runtime
    --cache-from "type=registry,ref=${cache_ref}"
    --cache-to "type=registry,ref=${cache_ref},mode=max"
  )
fi

build+=(.)

if ((dry_run)); then
  printf '%q ' "${build[@]}"
  printf '\n'
  exit 0
fi

"${build[@]}"
printf '\nPublished image: %s\n' "$image_ref"
printf 'Runpod image:    %s\n' "$image_ref"
