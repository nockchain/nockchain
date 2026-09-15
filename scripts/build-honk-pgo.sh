#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repo_root}"

output_dir="${HONK_PGO_OUTPUT_DIR:-${repo_root}/target/honk-pgo}"
mkdir -p "${output_dir}"

work_dir="$(mktemp -d "${TMPDIR:-/tmp}/honk-pgo.XXXXXX")"
trap 'rm -rf "${work_dir}"' EXIT

host_target="$(rustc -vV | sed -n 's/^host: //p')"
sysroot="$(rustc --print sysroot)"
llvm_profdata="${sysroot}/lib/rustlib/${host_target}/bin/llvm-profdata"
if [[ ! -x "${llvm_profdata}" ]]; then
  echo "missing ${llvm_profdata}" >&2
  echo "install the matching Rust LLVM tools with: rustup component add llvm-tools-preview" >&2
  exit 1
fi

instrument_target="${work_dir}/instrumented-target"
pgo_target="${work_dir}/pgo-target"
raw_profiles="${work_dir}/raw-profiles"
training_dir="${work_dir}/training"
mkdir -p "${raw_profiles}" "${training_dir}"

echo "==> building instrumented honk (${host_target})"
CARGO_TARGET_DIR="${instrument_target}" \
  RUSTFLAGS="-Cprofile-generate=${raw_profiles}" \
  cargo build \
    --locked \
    --release \
    --target "${host_target}" \
    -p honk \
    --bin honk

instrumented_honk="${instrument_target}/${host_target}/release/honk"

train() {
  local name="$1"
  local entry="$2"
  local run_dir="${training_dir}/${name}"
  mkdir -p "${run_dir}"
  echo "==> training on ${entry}"
  (
    cd "${run_dir}"
    LLVM_PROFILE_FILE="${raw_profiles}/${name}-%m-%p.profraw" \
      "${instrumented_honk}" \
        --new \
        --output out.jam \
        --prelude "${repo_root}/hoon/common/hoon.hoon" \
        "${repo_root}/${entry}" \
        "${repo_root}/hoon"
  )
}

train wallet hoon/apps/wallet/wallet.hoon
train dumb hoon/apps/dumbnet/outer.hoon

shopt -s nullglob
profile_inputs=("${raw_profiles}"/*.profraw)
if [[ "${#profile_inputs[@]}" -eq 0 ]]; then
  echo "instrumented training produced no .profraw files" >&2
  exit 1
fi

merged_profile="${work_dir}/honk.profdata"
echo "==> merging ${#profile_inputs[@]} raw profile(s)"
"${llvm_profdata}" merge -o "${merged_profile}" "${profile_inputs[@]}"

echo "==> building profile-optimized honk"
CARGO_TARGET_DIR="${pgo_target}" \
  RUSTFLAGS="-Cprofile-use=${merged_profile}" \
  cargo build \
    --locked \
    --release \
    --target "${host_target}" \
    -p honk \
    --bin honk

pgo_honk="${pgo_target}/${host_target}/release/honk"
verification_dir="${work_dir}/verification"
mkdir -p "${verification_dir}"
echo "==> verifying byte-identical Dumbnet output"
(
  cd "${verification_dir}"
  "${pgo_honk}" \
    --new \
    --output out.jam \
    --prelude "${repo_root}/hoon/common/hoon.hoon" \
    "${repo_root}/hoon/apps/dumbnet/outer.hoon" \
    "${repo_root}/hoon"
)
cmp "${training_dir}/dumb/out.jam" "${verification_dir}/out.jam"

hash_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

binary_hash="$(hash_file "${pgo_honk}")"
profile_hash="$(hash_file "${merged_profile}")"
source_identity="$(git describe --always --dirty)"

install -m 755 "${pgo_honk}" "${output_dir}/honk.tmp"
mv "${output_dir}/honk.tmp" "${output_dir}/honk"
cp "${merged_profile}" "${output_dir}/honk.profdata.tmp"
mv "${output_dir}/honk.profdata.tmp" "${output_dir}/honk.profdata"
{
  printf 'source=%s\n' "${source_identity}"
  printf 'host=%s\n' "${host_target}"
  printf 'rustc=%s\n' "$(rustc --version)"
  printf 'training=hoon/apps/wallet/wallet.hoon,hoon/apps/dumbnet/outer.hoon\n'
  printf 'binary_sha256=%s\n' "${binary_hash}"
  printf 'profile_sha256=%s\n' "${profile_hash}"
} >"${output_dir}/IDENTITY.txt"

echo "==> wrote ${output_dir}/honk"
echo "    binary sha256: ${binary_hash}"
echo "    profile sha256: ${profile_hash}"
