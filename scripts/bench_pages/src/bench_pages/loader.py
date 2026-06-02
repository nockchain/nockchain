from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from bench_pages.artifacts import is_publish_artifact_path
from bench_pages.errors import ValidationError
from bench_pages.models import ArtifactRecord, SweepCase, SweepData, SweepRun
from bench_pages.raw_tx_replay import load_raw_tx_replay_summary


REQUIRED_TOP_LEVEL_FILES = (
    "matrix.json",
    "matrix_expanded.json",
    "schedule.json",
)
OPTIONAL_TOP_LEVEL_FILES = (
    "comparison.json",
    "verdict.json",
)
TRACKED_CASE_FILES = (
    "provenance.json",
    "requested_case.json",
    "resolved_case.json",
    "summary.json",
    "verdict.json",
)


def load_sweep(root: Path) -> SweepData:
    sweep_root = root.resolve()
    if not sweep_root.is_dir():
        raise ValidationError(f"sweep root does not exist: {sweep_root}")

    sweep_artifacts, missing_top_level_artifacts = _load_json_artifacts(
        sweep_root,
        required_files=REQUIRED_TOP_LEVEL_FILES,
        tracked_files=REQUIRED_TOP_LEVEL_FILES + OPTIONAL_TOP_LEVEL_FILES,
    )
    matrix = sweep_artifacts["matrix"]
    matrix_expanded = sweep_artifacts["matrix_expanded"]
    schedule = sweep_artifacts["schedule"]
    comparison = sweep_artifacts.get("comparison")
    verdict = sweep_artifacts.get("verdict")
    schema_version = _read_text_if_present(sweep_root / "schema_version.txt")

    artifact_inventory = _walk_artifacts(sweep_root)
    top_level_artifacts = [
        artifact
        for artifact in artifact_inventory
        if not artifact.relative_path.startswith("cases/")
    ]

    expanded_by_id = {
        case_entry.get("case_id"): case_entry
        for case_entry in matrix_expanded
        if isinstance(case_entry, dict) and case_entry.get("case_id")
    }
    comparison_by_id = {
        case_entry.get("case_id"): case_entry
        for case_entry in _mapping(comparison).get("cases", [])
        if isinstance(case_entry, dict) and case_entry.get("case_id")
    }

    cases_root = sweep_root / "cases"
    case_dirs_by_id = {
        case_root.name: case_root
        for case_root in _sorted_child_dirs(cases_root)
    }
    cases: list[SweepCase] = []
    for case_entry in matrix_expanded:
        if not isinstance(case_entry, dict):
            continue
        case_id = case_entry.get("case_id")
        if not case_id:
            continue
        case_root = case_dirs_by_id.pop(case_id, cases_root / case_id)
        cases.append(
            _load_case(
                sweep_root=sweep_root,
                case_root=case_root,
                expanded_case=case_entry,
                comparison_case=comparison_by_id.get(case_id),
            )
        )

    for case_root in sorted(case_dirs_by_id.values()):
        cases.append(
            _load_case(
                sweep_root=sweep_root,
                case_root=case_root,
                expanded_case=expanded_by_id.get(case_root.name),
                comparison_case=comparison_by_id.get(case_root.name),
            )
        )

    sweep_execution_mode = _normalize_execution_mode(
        [
            _normalize_execution_signal(matrix.get("base", {}).get("mode"), "matrix.base.mode"),
            *[
                _normalize_execution_signal(case.execution_mode, f"case {case.case_id}")
                for case in cases
                if case.execution_mode != "unknown"
            ],
        ]
    )
    completion_state = _sweep_completion_state(missing_top_level_artifacts, cases)

    return SweepData(
        root=sweep_root,
        execution_mode=sweep_execution_mode,
        schema_version=schema_version,
        matrix=matrix,
        matrix_expanded=matrix_expanded,
        schedule=schedule,
        comparison=comparison,
        verdict=verdict,
        completion_state=completion_state,
        missing_top_level_artifacts=missing_top_level_artifacts,
        cases=cases,
        artifact_inventory=artifact_inventory,
        top_level_artifacts=top_level_artifacts,
    )


def _load_case(
    sweep_root: Path,
    case_root: Path,
    expanded_case: dict[str, Any] | None,
    comparison_case: dict[str, Any] | None,
) -> SweepCase:
    materialized = case_root.is_dir()
    if materialized:
        case_artifacts, missing_artifacts = _load_json_artifacts(
            case_root,
            required_files=(),
            tracked_files=TRACKED_CASE_FILES,
        )
    else:
        case_artifacts = {}
        missing_artifacts = list(TRACKED_CASE_FILES)

    expanded_requested_case = _mapping(expanded_case).get("requested_case")
    requested_case = case_artifacts.get("requested_case") or expanded_requested_case
    resolved_case = case_artifacts.get("resolved_case")
    summary = case_artifacts.get("summary")
    trusted_plan = _load_optional_json(case_root / "trusted_plan.json") if materialized else None
    verdict = case_artifacts.get("verdict")
    provenance = case_artifacts.get("provenance")
    cpu_profile = _load_optional_json(case_root / "cpu_profile.json") if materialized else None
    validation = _load_optional_json(case_root / "validation.json") if materialized else None

    case_execution_mode = _normalize_execution_mode(
        [
            _normalize_execution_signal(
                _mapping(requested_case).get("execution"),
                f"{case_root.name}/requested_case.json:execution",
            ),
            _normalize_execution_signal(
                _mapping(provenance).get("backend"),
                f"{case_root.name}/provenance.json:backend",
            ),
        ]
        or [None],
        default="unknown",
    )

    runs = _load_runs(sweep_root, case_root / "runs") if materialized else []
    artifacts = _artifacts_under(sweep_root, case_root) if materialized else []

    return SweepCase(
        case_id=case_root.name,
        root=case_root,
        execution_mode=case_execution_mode,
        axis_assignments=(expanded_case or {}).get("axis_assignments", {}),
        requested_case=requested_case,
        resolved_case=resolved_case,
        summary=summary,
        trusted_plan=trusted_plan,
        verdict=verdict,
        provenance=provenance,
        materialized=materialized,
        completion_state=_case_completion_state(
            materialized, missing_artifacts, summary, verdict
        ),
        missing_artifacts=missing_artifacts,
        cpu_profile=cpu_profile,
        comparison_case=comparison_case,
        validation=validation,
        runs=runs,
        artifacts=artifacts,
    )


def _validate_required_files(root: Path, required_files: tuple[str, ...]) -> None:
    missing = [name for name in required_files if not (root / name).is_file()]
    if missing:
        raise ValidationError(
            f"missing required files under {root}: {', '.join(sorted(missing))}"
        )


def _load_json_artifacts(
    root: Path,
    *,
    required_files: tuple[str, ...],
    tracked_files: tuple[str, ...],
) -> tuple[dict[str, Any], list[str]]:
    _validate_required_files(root, required_files)
    artifacts: dict[str, Any] = {}
    missing: list[str] = []
    for filename in tracked_files:
        path = root / filename
        if not path.is_file():
            missing.append(filename)
            continue
        artifacts[Path(filename).stem] = _load_json(path)
    return artifacts, missing


def _load_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text())
    except FileNotFoundError as exc:
        raise ValidationError(f"missing JSON artifact: {path}") from exc
    except json.JSONDecodeError as exc:
        raise ValidationError(f"invalid JSON artifact {path}: {exc}") from exc


def _read_text_if_present(path: Path) -> str | None:
    if not path.exists():
        return None
    return path.read_text().strip()


def _load_optional_json(path: Path) -> Any:
    if not path.exists():
        return None
    return _load_json(path)


def _load_runs(sweep_root: Path, runs_root: Path) -> list[SweepRun]:
    if not runs_root.exists():
        return []

    runs: list[SweepRun] = []
    for run_root in _sorted_child_dirs(runs_root):
        runs.append(
            SweepRun(
                run_id=run_root.name,
                root=run_root,
                result=_load_optional_json(run_root / "result.json"),
                raw_tx_replay=load_raw_tx_replay_summary(
                    run_root / "steps.ndjson",
                    run_id=run_root.name,
                ),
                artifacts=_artifacts_under(sweep_root, run_root),
            )
        )
    return runs


def _sorted_child_dirs(root: Path) -> list[Path]:
    if not root.exists():
        return []
    return sorted(path for path in root.iterdir() if path.is_dir())


def _walk_artifacts(root: Path) -> list[ArtifactRecord]:
    return _artifact_records(root, relative_to=root)


def _artifacts_under(sweep_root: Path, sub_root: Path) -> list[ArtifactRecord]:
    return _artifact_records(sub_root, relative_to=sweep_root)


def _artifact_records(root: Path, relative_to: Path) -> list[ArtifactRecord]:
    if not root.exists():
        return []
    return [
        ArtifactRecord(
            relative_path=str(path.relative_to(relative_to)),
            size_bytes=path.stat().st_size,
        )
        for path in sorted(candidate for candidate in root.rglob("*") if candidate.is_file())
        if is_publish_artifact_path(path.relative_to(relative_to))
    ]


def _normalize_execution_signal(raw_value: Any, source: str) -> str | None:
    if raw_value is None:
        return None
    if raw_value == "unknown":
        return None
    if raw_value in ("Native", "native"):
        return "native"
    if isinstance(raw_value, str):
        if raw_value.lower() == "docker":
            return "docker"
        raise ValidationError(f"unsupported execution value at {source}: {raw_value!r}")
    if isinstance(raw_value, dict):
        if len(raw_value) != 1:
            raise ValidationError(f"ambiguous execution value at {source}: {raw_value!r}")
        tag = next(iter(raw_value))
        normalized = tag.lower()
        if normalized in {"native", "docker"}:
            return normalized
    raise ValidationError(f"unsupported execution value at {source}: {raw_value!r}")


def _normalize_execution_mode(
    signals: list[str | None],
    *,
    default: str | None = None,
) -> str:
    modes = {signal for signal in signals if signal is not None}
    if not modes:
        if default is not None:
            return default
        raise ValidationError("unable to normalize execution mode from sweep artifacts")
    if len(modes) != 1:
        raise ValidationError(f"conflicting execution modes in sweep artifacts: {sorted(modes)}")
    return next(iter(modes))


def _case_completion_state(
    materialized: bool,
    missing_artifacts: list[str],
    summary: dict[str, Any] | None,
    verdict: dict[str, Any] | None,
) -> str:
    if not materialized:
        return "missing"
    if "summary.json" in missing_artifacts or "verdict.json" in missing_artifacts:
        return "partial"
    if _case_has_missing_peeks(summary):
        return "partial"
    if _verdict_label(verdict) not in {"Valid", "Unknown"}:
        return "partial"
    return "complete"


def _case_has_missing_peeks(summary: dict[str, Any] | None) -> bool:
    by_step_type = _mapping(summary).get("by_step_type")
    if not isinstance(by_step_type, dict):
        return False
    for step_type in ("peek_height", "peek_height_cold"):
        missing_count = _mapping(_mapping(by_step_type).get(step_type)).get("missing_count")
        if _stats_max(missing_count) > 0:
            return True
    return False


def _stats_max(value: Any) -> float:
    if isinstance(value, dict):
        raw = value.get("max")
        return float(raw) if isinstance(raw, (int, float)) else 0.0
    if isinstance(value, (int, float)):
        return float(value)
    return 0.0


def _verdict_label(verdict: dict[str, Any] | None) -> str:
    validity = _mapping(verdict).get("validity")
    if isinstance(validity, dict):
        return str(next(iter(validity.keys()), "Unknown"))
    if validity is None:
        return "Unknown"
    return str(validity)


def _sweep_completion_state(
    missing_top_level_artifacts: list[str],
    cases: list[SweepCase],
) -> str:
    if missing_top_level_artifacts:
        return "incomplete"
    if any(case.completion_state != "complete" for case in cases):
        return "incomplete"
    return "complete"


def _mapping(value: Any) -> dict[str, Any]:
    if isinstance(value, dict):
        return value
    return {}
