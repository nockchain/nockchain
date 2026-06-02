from __future__ import annotations

import hashlib
import json
from collections.abc import Callable
from dataclasses import asdict
from pathlib import Path
from typing import Any

from bench_pages.docker_metadata import case_docker_image_metadata
from bench_pages.models import DockerImageRecord, SweepCase, SweepData, SweepRun
from bench_pages.raw_tx_replay import combine_raw_tx_replay_summaries


CaseContextExtractor = Callable[[SweepCase], Any]
CASE_CONTEXT_EXTRACTORS: dict[str, CaseContextExtractor] = {
    "fixture_identity": lambda case: _case_fixture_identity(case),
    "runtime_flavor": lambda case: _case_runtime_flavor(case),
    "boot_source": lambda case: _case_boot_source(case),
    "boot_event_num": lambda case: _case_boot_event_num(case),
    "pma_work_dir_mode": lambda case: _case_pma_work_dir_mode(case),
}


def build_sweep_id(sweep: SweepData) -> str:
    case_contexts = {
        case.case_id: _extract_case_context(case)
        for case in sweep.cases
    }
    identity_inputs = _build_sweep_identity_inputs(sweep, case_contexts)
    axis_part = _slug("-".join(identity_inputs["axis_names"]) or "no-axes")
    commit_part = _short_commit(_git_commit(sweep) or "unknown")
    fixture_part = _slug(
        _summarize_fixture_values(identity_inputs["fixture_identities"])
        or "unknown-fixture"
    )
    runtime_part = _slug(
        _summarize_distinct_values(identity_inputs["runtime_flavors"])
        or "unknown-runtime"
    )
    context_hash = _identity_hash(identity_inputs)
    return (
        f"{sweep.execution_mode}-{axis_part}-{fixture_part}-"
        f"{runtime_part}-{commit_part}-{context_hash}"
    )


def build_manifest(
    sweep: SweepData,
    docker_images: list[DockerImageRecord] | None = None,
) -> dict[str, Any]:
    sweep_id = build_sweep_id(sweep)
    docker_image_records = docker_images or _collect_docker_images(sweep)
    case_contexts = {
        case.case_id: _extract_case_context(case)
        for case in sweep.cases
    }
    sweep_context = _derive_sweep_context(sweep, case_contexts)
    case_failure_reasons = {
        case.case_id: _case_failure_reasons(sweep.verdict, case.case_id)
        for case in sweep.cases
    }
    complete_case_count = sum(1 for case in sweep.cases if case.completion_state == "complete")
    partial_case_count = sum(1 for case in sweep.cases if case.completion_state == "partial")
    missing_case_count = sum(1 for case in sweep.cases if case.completion_state == "missing")

    manifest = {
        "sweep": {
            "id": sweep_id,
            "source_sweep_path": str(sweep.root),
            "execution_mode": sweep.execution_mode,
            "git_commit": _git_commit(sweep),
            "build_profile": _build_profile(sweep),
            "axis_names": _axis_names(sweep),
            "verdict": sweep.verdict,
            "schema_version": sweep.schema_version,
            "completion_state": sweep.completion_state,
            "missing_top_level_artifacts": sweep.missing_top_level_artifacts,
            "scheduled_case_count": len(sweep.matrix_expanded),
            "materialized_case_count": sum(1 for case in sweep.cases if case.materialized),
            "complete_case_count": complete_case_count,
            "partial_case_count": partial_case_count,
            "missing_case_count": missing_case_count,
            **sweep_context,
        },
        "source_artifacts": {
            "matrix": sweep.matrix,
            "matrix_expanded": sweep.matrix_expanded,
            "schedule": sweep.schedule,
            "comparison": sweep.comparison,
            "verdict": sweep.verdict,
        },
        "top_level_artifacts": [
            _artifact_dict(record, sweep_id=sweep_id)
            for record in sweep.top_level_artifacts
        ],
        "artifact_bundle": _artifact_bundle_dict(sweep_id),
        "cases": [
            _case_manifest(
                case,
                sweep_id=sweep_id,
                context=case_contexts[case.case_id],
                failure_reasons=case_failure_reasons[case.case_id],
            )
            for case in sweep.cases
        ],
        "docker_images": [asdict(record) for record in docker_image_records],
        "artifact_inventory": [
            _artifact_dict(record, sweep_id=sweep_id)
            for record in sweep.artifact_inventory
        ],
    }
    return manifest


def _case_manifest(
    case: SweepCase,
    sweep_id: str,
    context: dict[str, Any],
    failure_reasons: list[str],
) -> dict[str, Any]:
    return {
        "case_id": case.case_id,
        "execution_mode": case.execution_mode,
        "axis_assignments": case.axis_assignments,
        "materialized": case.materialized,
        "completion_state": case.completion_state,
        "missing_artifacts": case.missing_artifacts,
        "requested_case": case.requested_case,
        "resolved_case": case.resolved_case,
        "summary": case.summary,
        "trusted_plan": case.trusted_plan,
        "verdict": case.verdict,
        "provenance": case.provenance,
        "cpu_profile": _cpu_profile_manifest(case, sweep_id=sweep_id),
        "validation": case.validation,
        "comparison_case": case.comparison_case,
        "failure_reasons": failure_reasons,
        "artifacts": [_artifact_dict(record, sweep_id=sweep_id) for record in case.artifacts],
        "runs": [_run_manifest(run, sweep_id=sweep_id) for run in case.runs],
        "raw_tx_replay": _case_raw_tx_replay(case),
        **context,
    }


def _run_manifest(run: SweepRun, sweep_id: str) -> dict[str, Any]:
    return {
        "run_id": run.run_id,
        "result": run.result,
        "raw_tx_replay": run.raw_tx_replay,
        "artifacts": [_artifact_dict(record, sweep_id=sweep_id) for record in run.artifacts],
    }


def _case_raw_tx_replay(case: SweepCase) -> dict[str, Any]:
    return combine_raw_tx_replay_summaries(
        [run.raw_tx_replay for run in case.runs]
    )


def _artifact_dict(record: Any, sweep_id: str) -> dict[str, Any]:
    return {
        "relative_path": record.relative_path,
        "size_bytes": record.size_bytes,
        "href": _artifact_href(sweep_id, record.relative_path),
    }


def _artifact_bundle_dict(sweep_id: str) -> dict[str, Any]:
    filename = f"{sweep_id}-artifacts.tar.gz"
    return {
        "filename": filename,
        "href": f"sweeps/{sweep_id}/{filename}",
        "size_bytes": None,
    }


def _cpu_profile_manifest(case: SweepCase, sweep_id: str) -> dict[str, Any] | None:
    if not case.cpu_profile:
        return None

    output_relative_path = str(case.cpu_profile["output_relative_path"])
    symbol_dir_relative_path = str(case.cpu_profile["symbol_dir_relative_path"])
    symbol_binary_relative_path = str(case.cpu_profile["symbol_binary_relative_path"])
    published_profile_path = _case_relative_path(case.case_id, output_relative_path)
    published_symbol_dir = _case_relative_path(case.case_id, symbol_dir_relative_path)
    published_symbol_binary = _case_relative_path(case.case_id, symbol_binary_relative_path)
    artifact_sizes = {
        record.relative_path: record.size_bytes
        for record in case.artifacts
    }

    return {
        "profiler_kind": case.cpu_profile.get("profiler_kind"),
        "sample_rate_hz": case.cpu_profile.get("sample_rate_hz"),
        "execution_kind": case.cpu_profile.get("execution_kind"),
        "profile_artifact": {
            "relative_path": published_profile_path,
            "size_bytes": artifact_sizes.get(published_profile_path),
            "href": _artifact_href(sweep_id, published_profile_path),
        },
        "symbol_dir": {
            "relative_path": published_symbol_dir,
        },
        "symbol_binary": {
            "relative_path": published_symbol_binary,
            "size_bytes": artifact_sizes.get(published_symbol_binary),
            "href": _artifact_href(sweep_id, published_symbol_binary),
        },
        "load_command": (
            "samply load --symbol-dir "
            f"artifacts/{published_symbol_dir} "
            f"artifacts/{published_profile_path}"
        ),
    }


def _case_relative_path(case_id: str, relative_path: str) -> str:
    return str(Path("cases") / case_id / relative_path)


def _collect_docker_images(sweep: SweepData) -> list[DockerImageRecord]:
    records: list[DockerImageRecord] = []
    seen: set[str] = set()
    for case in sweep.cases:
        if case.execution_mode != "docker":
            continue
        digest, local_ref, identity = case_docker_image_metadata(case)
        if identity in seen:
            continue
        seen.add(identity)
        records.append(
            DockerImageRecord(
                canonical_identity=identity,
                local_image_ref=local_ref,
                provenance_image_digest=digest,
            )
        )
    return records


def _axis_names(sweep: SweepData) -> list[str]:
    comparison_axis_names = _mapping(sweep.comparison).get("axis_names")
    if isinstance(comparison_axis_names, list):
        return [str(name) for name in comparison_axis_names]
    axes = sweep.matrix.get("axes", {})
    if isinstance(axes, dict):
        return [str(name) for name in axes.keys()]
    return []


def _extract_case_context(case: SweepCase) -> dict[str, Any]:
    return {
        key: extractor(case)
        for key, extractor in CASE_CONTEXT_EXTRACTORS.items()
    }


def _derive_sweep_context(
    sweep: SweepData,
    case_contexts: dict[str, dict[str, Any]],
) -> dict[str, Any]:
    fixture_values = _distinct_context_values(case_contexts, "fixture_identity")
    runtime_values = _distinct_context_values(case_contexts, "runtime_flavor")
    boot_source_values = _distinct_context_values(case_contexts, "boot_source")
    pma_work_dir_values = _distinct_context_values(case_contexts, "pma_work_dir_mode")

    return {
        "fixture_identity": _sweep_fixture_identity(sweep, case_contexts),
        "fixture_summary": _summarize_fixture_values(fixture_values),
        "runtime_summary": _summarize_distinct_values(runtime_values),
        "boot_source_summary": _summarize_distinct_values(boot_source_values),
        "pma_work_dir_summary": _summarize_distinct_values(pma_work_dir_values),
        "has_pma_cases": "pma" in runtime_values,
    }


def _build_sweep_identity_inputs(
    sweep: SweepData,
    case_contexts: dict[str, dict[str, Any]],
) -> dict[str, Any]:
    return {
        "execution_mode": sweep.execution_mode,
        "axis_names": _axis_names(sweep),
        "fixture_identities": _distinct_context_values(case_contexts, "fixture_identity"),
        "runtime_flavors": _distinct_context_values(case_contexts, "runtime_flavor"),
        "boot_sources": _distinct_context_values(case_contexts, "boot_source"),
        "pma_work_dir_modes": _distinct_context_values(case_contexts, "pma_work_dir_mode"),
        "git_commit": _git_commit(sweep),
        "matrix_hash": _matrix_hash(sweep.matrix),
    }


def _identity_hash(identity_inputs: dict[str, Any]) -> str:
    encoded = json.dumps(identity_inputs, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()[:10]


def _distinct_context_values(
    case_contexts: dict[str, dict[str, Any]],
    key: str,
) -> list[str]:
    values = {
        _stringify_context_value(context.get(key))
        for context in case_contexts.values()
        if context.get(key) not in (None, "")
    }
    return sorted(values)


def _stringify_context_value(value: Any) -> str:
    if isinstance(value, str):
        return value
    return str(value)


def _summarize_fixture_values(values: list[str]) -> str | None:
    if not values:
        return None
    if len(values) == 1:
        return values[0]
    return f"{len(values)} fixtures"


def _summarize_distinct_values(values: list[str]) -> str | None:
    if not values:
        return None
    if len(values) == 1:
        return values[0]
    return ", ".join(values)


def _sweep_fixture_identity(
    sweep: SweepData,
    case_contexts: dict[str, dict[str, Any]],
) -> str | None:
    first_case = _first_case(sweep)
    if first_case is None:
        return None
    first_context = case_contexts.get(first_case.case_id, {})
    return first_context.get("fixture_identity")


def _case_fixture_identity(case: SweepCase) -> str | None:
    for candidate in (
        _mapping(case.resolved_case).get("fixture_sha256_hex"),
        _mapping(case.provenance).get("fixture_sha256_hex"),
        _mapping(case.requested_case).get("fixture_path"),
    ):
        if candidate:
            if isinstance(candidate, str) and "/" in candidate:
                return Path(candidate).name
            return str(candidate)
    return None


def _case_runtime_flavor(case: SweepCase) -> str | None:
    value = _mapping(case.provenance).get("runtime_flavor")
    return str(value) if value not in (None, "") else None


def _case_boot_source(case: SweepCase) -> str | None:
    value = _mapping(case.provenance).get("boot_source")
    return str(value) if value not in (None, "") else None


def _case_boot_event_num(case: SweepCase) -> int | str | None:
    value = _mapping(case.provenance).get("boot_event_num")
    if value in (None, ""):
        return None
    if isinstance(value, int):
        return value
    if isinstance(value, str):
        return value
    return str(value)


def _case_pma_work_dir_mode(case: SweepCase) -> str | None:
    value = _mapping(case.provenance).get("pma_work_dir_mode")
    return str(value) if value not in (None, "") else None


def _git_commit(sweep: SweepData) -> str | None:
    first_case = _first_case_with_mapping(sweep, "provenance")
    if first_case is None:
        return None

    git = _mapping(first_case.provenance).get("git", {})
    commit = git.get("commit")
    return str(commit) if commit else None


def _build_profile(sweep: SweepData) -> str | None:
    first_case = _first_case_with_mapping(sweep, "provenance")
    if first_case is None:
        return None

    binary = _mapping(first_case.provenance).get("binary", {})
    profile = binary.get("build_profile")
    return str(profile) if profile else None


def _first_case(sweep: SweepData) -> SweepCase | None:
    if not sweep.cases:
        return None
    return sweep.cases[0]


def _first_case_with_mapping(sweep: SweepData, field_name: str) -> SweepCase | None:
    for case in sweep.cases:
        if _mapping(getattr(case, field_name)):
            return case
    return None


def _artifact_href(sweep_id: str, relative_path: str) -> str:
    return f"sweeps/{sweep_id}/artifacts/{relative_path}"


def _short_commit(commit: str) -> str:
    return _slug(commit[:7] if len(commit) >= 7 else commit)


def _matrix_hash(matrix: dict[str, Any]) -> str:
    encoded = json.dumps(matrix, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()[:8]


def _slug(value: str) -> str:
    slug = []
    previous_dash = False
    for char in value.lower():
        if char.isalnum():
            slug.append(char)
            previous_dash = False
            continue
        if previous_dash:
            continue
        slug.append("-")
        previous_dash = True
    return "".join(slug).strip("-") or "unknown"


def _case_failure_reasons(verdict: dict[str, Any] | None, case_id: str) -> list[str]:
    return [
        reason
        for reason in _verdict_reasons(verdict)
        if case_id in reason
    ]


def _verdict_reasons(verdict: dict[str, Any] | None) -> list[str]:
    validity = _mapping(verdict).get("validity")
    if not isinstance(validity, dict):
        return []
    payload = next(iter(validity.values()), None)
    if not isinstance(payload, dict):
        return []
    reasons = payload.get("reasons")
    if not isinstance(reasons, list):
        return []
    return [str(reason) for reason in reasons]


def _mapping(value: Any) -> dict[str, Any]:
    if isinstance(value, dict):
        return value
    return {}
