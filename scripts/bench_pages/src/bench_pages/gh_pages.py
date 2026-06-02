from __future__ import annotations

import copy
import json
import shutil
import subprocess
import tarfile
from collections.abc import Callable
from pathlib import Path
from typing import Any

from bench_pages.artifacts import is_publish_artifact_path
from bench_pages.errors import ExternalCommandError, ValidationError
from bench_pages.file_ops import copy_directory_contents, write_json_file


Runner = Callable[[list[str]], subprocess.CompletedProcess[str]]
GITHUB_PAGES_MAX_ARTIFACT_BYTES = 95 * 1024 * 1024


def bootstrap_pages_checkout(
    repo_root: Path,
    pages_root: Path,
    branch: str = "gh-pages",
    runner: Runner | None = None,
) -> Path:
    run = runner or _run_command

    branch_exists = _local_branch_exists(repo_root, branch, run)
    remote_branch_exists = _remote_branch_exists(repo_root, branch, run)
    _run_checked(run, ["git", "-C", str(repo_root), "worktree", "add", "--detach", str(pages_root)])
    if remote_branch_exists:
        _run_checked(
            run,
            [
                "git",
                "-C",
                str(repo_root),
                "fetch",
                "origin",
                f"{branch}:refs/remotes/origin/{branch}",
            ],
        )
        _run_checked(run, ["git", "-C", str(pages_root), "checkout", "-B", branch, f"origin/{branch}"])
        _validate_existing_pages_layout(pages_root)
    elif branch_exists:
        _run_checked(run, ["git", "-C", str(pages_root), "checkout", branch])
        _validate_existing_pages_layout(pages_root)
    else:
        _run_checked(run, ["git", "-C", str(pages_root), "checkout", "--orphan", branch])
        _run_checked(run, ["git", "-C", str(pages_root), "rm", "-rf", "--ignore-unmatch", "."])

    ensure_pages_layout(pages_root)
    return pages_root


def ensure_pages_layout(pages_root: Path) -> None:
    pages_root.mkdir(parents=True, exist_ok=True)
    (pages_root / ".nojekyll").write_text("")
    if not (pages_root / "index.json").exists():
        (pages_root / "index.json").write_text("[]\n")
    if not (pages_root / "index.html").exists():
        (pages_root / "index.html").write_text("<!doctype html><title>Bench Pages</title>\n")


def publish_sweep_to_pages(
    pages_root: Path,
    sweep_root: Path,
    manifest: dict[str, Any],
    sweep_html: str,
    index_html: str,
    assets_dir: Path,
    replace: bool = False,
    entries: list[dict[str, Any]] | None = None,
    max_artifact_size_bytes: int | None = None,
) -> list[dict[str, Any]]:
    ensure_pages_layout(pages_root)
    copy_directory_contents(assets_dir, pages_root / "assets")

    sweep_id = manifest["sweep"]["id"]
    sweep_dir = pages_root / "sweeps" / sweep_id
    if replace and sweep_dir.exists():
        shutil.rmtree(sweep_dir)
    (sweep_dir / "artifacts").mkdir(parents=True, exist_ok=True)

    _copy_sweep_artifacts(
        sweep_root=sweep_root,
        artifacts_root=sweep_dir / "artifacts",
        max_artifact_size_bytes=max_artifact_size_bytes,
    )
    if max_artifact_size_bytes is None:
        _write_artifact_bundle(
            sweep_root=sweep_root,
            sweep_dir=sweep_dir,
            artifact_bundle=manifest.get("artifact_bundle"),
        )
    write_json_file(sweep_dir / "manifest.json", manifest)
    (sweep_dir / "index.html").write_text(sweep_html)

    if entries is None:
        entries = prepare_index_entries(pages_root, manifest, replace=replace)
    write_json_file(pages_root / "index.json", entries)
    (pages_root / "index.html").write_text(index_html)
    return entries


def prepare_manifest_for_hosted_pages(
    manifest: dict[str, Any],
    max_artifact_size_bytes: int = GITHUB_PAGES_MAX_ARTIFACT_BYTES,
) -> dict[str, Any]:
    published = copy.deepcopy(manifest)
    omitted_artifacts: dict[str, int | None] = {}

    def track_omitted(record: dict[str, Any]) -> None:
        key = str(record.get("relative_path") or record.get("href") or "unknown")
        omitted_artifacts[key] = record.get("size_bytes")

    def allow(record: dict[str, Any]) -> bool:
        size = record.get("size_bytes")
        if isinstance(size, int) and size > max_artifact_size_bytes:
            track_omitted(record)
            return False
        return True

    published["top_level_artifacts"] = [
        record for record in published.get("top_level_artifacts", []) if allow(record)
    ]
    published["artifact_inventory"] = [
        record for record in published.get("artifact_inventory", []) if allow(record)
    ]

    for case in published.get("cases", []):
        case["artifacts"] = [record for record in case.get("artifacts", []) if allow(record)]
        for run in case.get("runs", []):
            run["artifacts"] = [record for record in run.get("artifacts", []) if allow(record)]

        cpu_profile = case.get("cpu_profile")
        if cpu_profile is None:
            continue
        profile_artifact = cpu_profile.get("profile_artifact")
        symbol_binary = cpu_profile.get("symbol_binary")
        if not isinstance(profile_artifact, dict) or not isinstance(symbol_binary, dict):
            case["cpu_profile"] = None
            continue
        if not allow(profile_artifact) or not allow(symbol_binary):
            case["cpu_profile"] = None

    bundle_present = published.get("artifact_bundle") is not None
    published["artifact_bundle"] = None
    published["publish_limits"] = {
        "max_artifact_size_bytes": max_artifact_size_bytes,
        "max_artifact_size_label": _humanize_bytes(max_artifact_size_bytes),
        "artifact_bundle_omitted": bundle_present,
        "omitted_artifact_count": len(omitted_artifacts),
        "omitted_artifact_total_bytes": sum(
            size for size in omitted_artifacts.values() if isinstance(size, int)
        ),
    }
    return published


def _write_artifact_bundle(
    sweep_root: Path,
    sweep_dir: Path,
    artifact_bundle: dict[str, Any] | None,
) -> None:
    if not artifact_bundle:
        return

    bundle_name = artifact_bundle.get("filename")
    if not isinstance(bundle_name, str) or not bundle_name:
        raise ValidationError("artifact bundle filename missing from manifest")

    bundle_path = sweep_dir / bundle_name
    archive_root = bundle_name.removesuffix(".tar.gz")
    with tarfile.open(bundle_path, mode="w:gz") as bundle:
        for source_path in sorted(path for path in sweep_root.rglob("*") if path.is_file()):
            relative_path = source_path.relative_to(sweep_root)
            if not is_publish_artifact_path(relative_path):
                continue
            bundle.add(source_path, arcname=Path(archive_root) / relative_path)

    artifact_bundle["size_bytes"] = bundle_path.stat().st_size


def _copy_sweep_artifacts(
    sweep_root: Path,
    artifacts_root: Path,
    max_artifact_size_bytes: int | None,
) -> None:
    for source_path in sweep_root.rglob("*"):
        if not source_path.is_file():
            continue
        relative_path = source_path.relative_to(sweep_root)
        if not is_publish_artifact_path(relative_path):
            continue
        size_bytes = source_path.stat().st_size
        if max_artifact_size_bytes is not None and size_bytes > max_artifact_size_bytes:
            continue
        destination = artifacts_root / relative_path
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source_path, destination)


def _humanize_bytes(value: int) -> str:
    amount = float(value)
    for unit in ("B", "KiB", "MiB", "GiB", "TiB"):
        if amount < 1024:
            if amount == int(amount):
                return f"{int(amount)} {unit}"
            return f"{amount:.1f} {unit}"
        amount /= 1024
    return f"{amount:.1f} PiB"


def prepare_index_entries(
    pages_root: Path,
    manifest: dict[str, Any],
    replace: bool = False,
) -> list[dict[str, Any]]:
    entries = _load_index_entries(pages_root / "index.json")
    return _upsert_index_entry(entries, _index_entry_from_manifest(manifest), replace=replace)


def commit_pages_changes(
    pages_root: Path,
    message: str,
    branch: str = "gh-pages",
    push: bool = False,
    runner: Runner | None = None,
) -> None:
    run = runner or _run_command
    _run_checked(run, ["git", "-C", str(pages_root), "add", "."])
    _run_checked(run, ["git", "-C", str(pages_root), "commit", "-m", message])
    if push:
        result = run(["git", "-C", str(pages_root), "push", "origin", branch])
        if result.returncode != 0:
            raise ExternalCommandError(
                f"git push failed for {branch}; concurrent publish may require retry\n{result.stderr}"
            )


def _validate_existing_pages_layout(pages_root: Path) -> None:
    index_path = pages_root / "index.json"
    if not index_path.exists():
        raise ValidationError(
            "existing gh-pages branch does not contain the publisher index.json layout; "
            "delete or replace the legacy branch before publishing"
        )


def _local_branch_exists(repo_root: Path, branch: str, runner: Runner) -> bool:
    return _command_succeeds(
        runner,
        ["git", "-C", str(repo_root), "show-ref", "--verify", f"refs/heads/{branch}"],
    )


def _remote_branch_exists(repo_root: Path, branch: str, runner: Runner) -> bool:
    return _command_succeeds(
        runner,
        [
            "git",
            "-C",
            str(repo_root),
            "ls-remote",
            "--exit-code",
            "--heads",
            "origin",
            branch,
        ],
    )


def _load_index_entries(path: Path) -> list[dict[str, Any]]:
    if not path.exists():
        return []
    return json.loads(path.read_text())


def _upsert_index_entry(
    entries: list[dict[str, Any]],
    new_entry: dict[str, Any],
    replace: bool,
) -> list[dict[str, Any]]:
    updated: list[dict[str, Any]] = []
    inserted = False
    for entry in entries:
        if entry.get("id") == new_entry["id"]:
            if replace or not inserted:
                updated.append(new_entry)
                inserted = True
            continue
        updated.append(entry)
    if not inserted:
        updated.append(new_entry)
    return sorted(updated, key=lambda entry: entry["id"])


def _index_entry_from_manifest(manifest: dict[str, Any]) -> dict[str, Any]:
    sweep = manifest["sweep"]
    verdict = sweep.get("verdict") or {}
    validity = verdict.get("validity")
    if isinstance(validity, dict):
        validity_value = next(iter(validity.keys()), "unknown")
    else:
        validity_value = validity or "unknown"
    return {
        "id": sweep["id"],
        "path": f"sweeps/{sweep['id']}/index.html",
        "execution_mode": sweep.get("execution_mode"),
        "fixture_identity": sweep.get("fixture_identity"),
        "fixture_summary": sweep.get("fixture_summary"),
        "git_commit": sweep.get("git_commit"),
        "build_profile": sweep.get("build_profile"),
        "axis_names": sweep.get("axis_names", []),
        "runtime_summary": sweep.get("runtime_summary"),
        "boot_source_summary": sweep.get("boot_source_summary"),
        "pma_work_dir_summary": sweep.get("pma_work_dir_summary"),
        "has_pma_cases": sweep.get("has_pma_cases"),
        "verdict": validity_value,
    }


def _run_command(command: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, check=False, capture_output=True, text=True)


def _command_succeeds(
    runner: Runner,
    command: list[str],
) -> bool:
    return runner(command).returncode == 0


def _run_checked(
    runner: Runner,
    command: list[str],
) -> subprocess.CompletedProcess[str]:
    result = runner(command)
    if result.returncode != 0:
        raise ExternalCommandError(
            f"command failed ({result.returncode}): {' '.join(command)}\n{result.stderr}"
        )
    return result
