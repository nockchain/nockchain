#!/usr/bin/env python3
"""Verify the source-controlled Plonky3-recursion patch series."""

from __future__ import annotations

import argparse
import hashlib
import shutil
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
MANIFEST = REPO_ROOT / "third_party" / "patches" / "plonky3-recursion" / "manifest.toml"
PATCH_DIR = MANIFEST.parent


def run(args: list[str], *, cwd: Path | None = None, data: bytes | None = None) -> str:
    result = subprocess.run(
        args,
        cwd=cwd,
        input=data,
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    return result.stdout.decode().strip()


def git(args: list[str], *, cwd: Path) -> str:
    return run(["git", *args], cwd=cwd)


def patch_id(path: Path) -> str:
    return run(["git", "patch-id", "--stable"], data=path.read_bytes()).split()[0]


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def verify_patch_files(manifest: dict[str, object]) -> list[Path]:
    patch_paths: list[Path] = []
    manifest_names: list[str] = []
    for patch in manifest["patches"]:
        manifest_names.append(patch["file"])
        patch_path = PATCH_DIR / patch["file"]
        if not patch_path.is_file():
            raise SystemExit(f"missing patch: {patch_path}")
        actual_sha = sha256(patch_path)
        if actual_sha != patch["sha256"]:
            raise SystemExit(f"sha256 mismatch for {patch_path.name}: {actual_sha}")
        actual_patch_id = patch_id(patch_path)
        if actual_patch_id != patch["patch_id"]:
            raise SystemExit(f"patch-id mismatch for {patch_path.name}: {actual_patch_id}")
        patch_paths.append(patch_path)

    extra_patches = sorted(
        path.name for path in PATCH_DIR.glob("*.patch") if path.name not in manifest_names
    )
    if extra_patches:
        raise SystemExit(f"patch file missing from manifest: {extra_patches[0]}")
    return patch_paths


def replay(manifest: dict[str, object], patch_paths: list[Path], work_dir: Path) -> None:
    work_dir.mkdir(parents=True, exist_ok=True)
    git(["init", "-q"], cwd=work_dir)
    git(["remote", "add", "upstream", manifest["official_url"]], cwd=work_dir)
    git(["fetch", "--quiet", "--depth", "1", "upstream", manifest["official_base_commit"]], cwd=work_dir)
    git(["checkout", "--quiet", "--detach", "FETCH_HEAD"], cwd=work_dir)

    base_tree = git(["rev-parse", "HEAD^{tree}"], cwd=work_dir)
    if base_tree != manifest["official_base_tree"]:
        raise SystemExit(f"official base tree mismatch: {base_tree}")

    git(["remote", "add", "fork", manifest["fork_url"]], cwd=work_dir)
    git(["fetch", "--quiet", "--depth", "1", "fork", manifest["patched_fork_commit"]], cwd=work_dir)
    fork_tree = git(["rev-parse", "FETCH_HEAD^{tree}"], cwd=work_dir)
    if fork_tree != manifest["patched_fork_tree"]:
        raise SystemExit(f"patched fork tree mismatch: {fork_tree}")

    git(["checkout", "--quiet", "--detach", manifest["official_base_commit"]], cwd=work_dir)
    git(["config", "user.name", "Nockchain Patch Replay"], cwd=work_dir)
    git(["config", "user.email", "patch-replay@nockchain.invalid"], cwd=work_dir)
    for patch_path in patch_paths:
        git(["am", "--quiet", str(patch_path)], cwd=work_dir)

    replay_tree = git(["rev-parse", "HEAD^{tree}"], cwd=work_dir)
    if replay_tree != fork_tree:
        raise SystemExit(f"replayed tree {replay_tree} != fork tree {fork_tree}")

    print(f"verified {len(patch_paths)} patches: {replay_tree}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--work-dir",
        type=Path,
        help="existing or new ignored directory for the temporary replay checkout",
    )
    parser.add_argument(
        "--keep-work-dir",
        action="store_true",
        help="do not delete the temporary replay checkout",
    )
    args = parser.parse_args()

    manifest = tomllib.loads(MANIFEST.read_text())
    patch_paths = verify_patch_files(manifest)

    created_temp = args.work_dir is None
    work_dir = args.work_dir or Path(
        tempfile.mkdtemp(prefix="plonky3-recursion-patch-replay.", dir=REPO_ROOT / "target")
    )
    try:
        replay(manifest, patch_paths, work_dir)
    finally:
        if created_temp and not args.keep_work_dir:
            shutil.rmtree(work_dir, ignore_errors=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
