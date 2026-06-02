from __future__ import annotations

import json
import shutil
from pathlib import Path


FIXTURE_DIR = Path(__file__).parent / "fixtures"


def create_partial_sweep_fixture(temp_root: Path) -> Path:
    source_root = FIXTURE_DIR / "docker_minimal"
    partial_root = temp_root / "partial_sweep"
    shutil.copytree(source_root, partial_root)

    _remove_if_exists(partial_root / "comparison.json")
    _remove_if_exists(partial_root / "verdict.json")

    base_case_id = "case-000-memory_limit_8g"
    partial_case_id = "case-001-memory_limit_4g"
    missing_case_id = "case-002-memory_limit_2g"

    base_case_root = partial_root / "cases" / base_case_id
    partial_case_root = partial_root / "cases" / partial_case_id
    shutil.copytree(base_case_root, partial_case_root)
    _rename_case_contents(partial_case_root, partial_case_id)
    _remove_if_exists(partial_case_root / "summary.json")
    _remove_if_exists(partial_case_root / "verdict.json")
    _remove_if_exists(partial_case_root / "runs" / "run-0" / "result.json")

    _rewrite_matrix_expanded(
        partial_root / "matrix_expanded.json",
        case_ids=[base_case_id, partial_case_id, missing_case_id],
        memory_limits=["8g", "4g", "2g"],
    )
    _rewrite_schedule(
        partial_root / "schedule.json",
        case_ids=[base_case_id, partial_case_id, missing_case_id],
    )
    return partial_root


def _rewrite_matrix_expanded(
    path: Path,
    *,
    case_ids: list[str],
    memory_limits: list[str],
) -> None:
    entries = json.loads(path.read_text())
    template = entries[0]
    rewritten = []
    for index, (case_id, memory_limit) in enumerate(zip(case_ids, memory_limits, strict=True)):
        entry = json.loads(json.dumps(template))
        entry["case_index"] = index
        entry["case_id"] = case_id
        entry["axis_assignments"]["memory_limit"] = memory_limit
        entry["requested_case"]["execution"]["Docker"]["memory_limit"] = memory_limit
        rewritten.append(entry)
    path.write_text(json.dumps(rewritten, indent=2) + "\n")


def _rewrite_schedule(path: Path, *, case_ids: list[str]) -> None:
    schedule = json.loads(path.read_text())
    if isinstance(schedule.get("case_ids"), list):
        schedule["case_ids"] = case_ids
    if isinstance(schedule.get("pending_case_ids"), list):
        schedule["pending_case_ids"] = case_ids
    path.write_text(json.dumps(schedule, indent=2) + "\n")


def _rename_case_contents(case_root: Path, case_id: str) -> None:
    requested_path = case_root / "requested_case.json"
    if requested_path.exists():
        requested_case = json.loads(requested_path.read_text())
        requested_case["label"] = case_id
        requested_path.write_text(json.dumps(requested_case, indent=2) + "\n")


def _remove_if_exists(path: Path) -> None:
    if path.exists():
        path.unlink()
