from __future__ import annotations

import html
import json
import statistics
from importlib.resources import files
from pathlib import Path
from typing import Any

from bench_pages.file_ops import copy_directory_contents, write_json_file
from bench_pages.operation_view import (
    build_case_operation_rows,
    build_operation_health_rows,
    missing_peek_status_reasons,
    step_missing_count,
    summarize_plan_operations,
)
from bench_pages.value_stats import (
    is_number as _is_number,
    is_value_stats as _is_value_stats,
    stats_scalar as _stats_scalar,
)
from jinja2 import Environment, FileSystemLoader, select_autoescape
from markupsafe import Markup


# -- Metric classification for UI tiers --

# Ordered list of summary metrics for the primary comparison table.
_COMPARISON_METRICS = [
    "steps_per_second",
    "pokes_per_second",
    "raw_tx_pokes_per_second",
    "peeks_per_second",
    "cold_peeks_per_second",
    "cold_cache_peeks_per_second",
    "warm_cache_peeks_per_second",
    "ambient_cache_peeks_per_second",
    "unknown_cache_peeks_per_second",
    "total_step_time_secs",
    "steps",
    "throughput_blocks_per_second",
    "total_replay_time_secs",
    "init_time_secs",
    "average_block_time_ms",
    "peak_process_rss_bytes",
    "minor_faults_total",
    "major_faults_total",
    "measured_runs_requested",
    "measured_runs_succeeded",
]

# Ordered list of run-result keys for per-case run tables.
_RUN_KEY_ORDER = [
    "success",
    "steps_per_second",
    "pokes_per_second",
    "raw_tx_pokes_per_second",
    "peeks_per_second",
    "cold_peeks_per_second",
    "total_step_time_secs",
    "steps",
    "throughput_blocks_per_second",
    "init_time_secs",
    "total_replay_time_secs",
    "average_block_time_ms",
    "peak_process_rss_bytes",
    "minor_faults_total",
    "major_faults_total",
    "blocks_poked",
    "failed_pokes",
]

# Keys excluded from run tables (structural, not metric).
_RUN_EXCLUDE_KEYS = {"run_id", "error"}

# Keys that always appear in tables even if absent or null in data.
_ALWAYS_SHOW_KEYS = {"minor_faults_total", "major_faults_total"}

# Metrics that get strip charts showing per-case run spread.
_STRIP_CHART_METRICS = [
    ("steps_per_second", "Steps/s"),
    ("pokes_per_second", "Pokes/s"),
    ("raw_tx_pokes_per_second", "Raw tx/s"),
    ("peeks_per_second", "Peeks/s"),
    ("cold_peeks_per_second", "Cold peeks/s"),
    ("throughput_blocks_per_second", "Throughput (blk/s)"),
    ("total_replay_time_secs", "Replay Time (s)"),
    ("minor_faults_total", "Minor Faults"),
]

# Human-readable labels with units for known metric keys.
_METRIC_LABELS: dict[str, str] = {
    "steps_per_second": "Steps/s",
    "pokes_per_second": "Pokes/s",
    "raw_tx_pokes_per_second": "Raw tx/s",
    "peeks_per_second": "Peeks/s",
    "cold_peeks_per_second": "Cold peeks/s",
    "cold_cache_peeks_per_second": "Cold peeks/s",
    "warm_cache_peeks_per_second": "Warm peeks/s",
    "ambient_cache_peeks_per_second": "Ambient peeks/s",
    "unknown_cache_peeks_per_second": "Unknown peeks/s",
    "total_step_time_secs": "Total Step (s)",
    "steps": "Steps",
    "throughput_blocks_per_second": "Throughput (blk/s)",
    "total_replay_time_secs": "Replay (s)",
    "init_time_secs": "Init (s)",
    "average_block_time_ms": "Avg Block (ms)",
    "peak_process_rss_bytes": "Peak RSS",
    "minor_faults_total": "Minor Fault",
    "major_faults_total": "Major Fault",
    "measured_runs_requested": "Runs Req",
    "measured_runs_succeeded": "Runs OK",
    "failed_pokes": "Fld Pokes",
    "blocks_poked": "Blocks",
    "success": "OK",
    "raw_tx_pokes_completed": "Raw Tx Pokes",
    "raw_tx_slabs_prebuilt": "Raw Tx Slabs",
    "raw_tx_payload_bytes_prebuilt": "Raw Tx Bytes",
    "slab_prebuild_start_rss_bytes": "Prebuild RSS Start",
    "slab_prebuild_peak_rss_bytes": "Prebuild RSS Peak",
}

# Hover tooltip descriptions for metric fields.
_FIELD_TOOLTIPS: dict[str, str] = {
    # Summary / comparison metrics
    "steps_per_second": "Trusted orchestrate plan steps completed per second. Higher is better.",
    "pokes_per_second": "Poke operations completed per second. Higher is better.",
    "raw_tx_pokes_per_second": "Raw transaction poke operations completed per second. Higher is better.",
    "peeks_per_second": "Peek operations completed per second. Higher is better.",
    "cold_peeks_per_second": "Cold-forced peek operations completed per second. Higher is better.",
    "cold_cache_peeks_per_second": "Peek operations expected to hit cold cache per second. Higher is better.",
    "warm_cache_peeks_per_second": "Peek operations expected to hit warm cache per second. Higher is better.",
    "ambient_cache_peeks_per_second": "Peek operations with ambient cache expectation per second. Higher is better.",
    "unknown_cache_peeks_per_second": "Peek operations with unknown cache expectation per second. Higher is better.",
    "total_step_time_secs": "Total wall-clock time spent executing trusted plan steps.",
    "steps": "Trusted orchestrate plan steps executed per measured run.",
    "by_step_type": "Per-step-type aggregate metrics from the trusted orchestrate workflow.",
    "normalized_plan_sha256_hex": "SHA-256 hash of the normalized trusted plan.",
    "step_signature_sha256_hex": "SHA-256 hash of the ordered trusted plan step signature.",
    "throughput_blocks_per_second": (
        "Blocks replayed per second. Higher is better."
    ),
    "total_replay_time_secs": (
        "Total wall-clock time for the block replay phase (seconds). Lower is better."
    ),
    "init_time_secs": (
        "Time to initialize the replay environment (seconds)."
    ),
    "average_block_time_ms": (
        "Average wall-clock time to replay one block (milliseconds). Lower is better."
    ),
    "peak_process_rss_bytes": (
        "Peak resident set size (physical memory) of the benchmark process."
    ),
    "minor_faults_total": (
        "Minor (soft) page faults: resolved from page cache without disk I/O."
    ),
    "major_faults_total": (
        "Major (hard) page faults: required disk I/O to resolve."
    ),
    "measured_runs_requested": (
        "Number of measured benchmark runs requested by the sweep matrix."
    ),
    "measured_runs_succeeded": (
        "Number of measured runs that completed successfully."
    ),
    "failed_runs": "List of run identifiers that failed.",
    "raw_tx_pokes_completed": "Raw transaction poke operations completed by an archive replay step.",
    "block_poke_duration_ms": "Time spent poking the archive block, excluding raw transaction pokes.",
    "raw_tx_poke_duration_ms": "Time spent poking raw transaction facts for an archive block.",
    "slab_prebuild_duration_ms": "Total slab prebuild time before archive poke execution.",
    "block_slab_prebuild_duration_ms": "Slab prebuild time for the block poke.",
    "raw_tx_slab_prebuild_duration_ms": "Slab prebuild time for raw transaction pokes.",
    "raw_tx_slabs_prebuilt": "Raw transaction poke slabs successfully prebuilt.",
    "raw_tx_payload_bytes_prebuilt": "Raw transaction payload bytes successfully prebuilt.",
    "slab_prebuild_start_rss_bytes": "Resident set size at the start of slab prebuild.",
    "slab_prebuild_peak_rss_bytes": "Peak resident set size observed during slab prebuild.",
    # Run-level fields
    "success": "Whether this individual run completed successfully.",
    "blocks_poked": "Total number of blocks replayed in this run.",
    "failed_pokes": "Block replay operations that failed within a run.",
    # Provenance / evidence fields
    "validity": (
        "Validity assessment. Valid = all runs completed within acceptable parameters."
    ),
    "fixture_sha256_hex": "SHA-256 hash of the test fixture file.",
    "capture_timestamp_ms": "Unix timestamp (ms) when provenance was captured.",
    "schema_version": "Artifact schema version.",
    "build_profile": "Cargo build profile (e.g. release, debug).",
    "realized_memory_max": "Maximum memory limit for the container (bytes).",
    "realized_memory_current": "Current cgroup memory usage of the container (bytes).",
    "total_memory_bytes": "Total physical memory on the host system.",
    "realized_cpuset": "CPUs available to the container.",
    "realized_cpu_max": "CPU bandwidth limit (max period).",
    "allocation_request_bytes": "Memory allocation requested for the benchmark.",
    "memory_limit_matches": "Whether the realized memory limit matches the requested limit.",
    "runtime_flavor": "Runtime flavor used by the benchmark execution.",
    "boot_source": "How the PMA runtime was bootstrapped for this case.",
    "boot_event_num": "Fixture boot event number used for per-case PMA display context.",
    "pma_work_dir_mode": "Normalized PMA work directory mode recorded in provenance.",
}

_VERDICT_TOOLTIPS: dict[str, str] = {
    "Valid": "All measured runs completed within acceptable parameters.",
    "Invalid": "One or more runs failed or produced out-of-range results.",
    "Partial": "Some runs completed, but the comparison includes failures or policy exceptions.",
    "Unknown": "Validity could not be determined.",
}


# -- Public API --

def render_sweep_page(manifest: dict[str, Any]) -> str:
    template = _environment().get_template("sweep.html.j2")
    cases = manifest["cases"]
    comparison = _build_comparison_table(cases)
    case_sections = [_case_section(case) for case in cases]
    strip_charts = _build_strip_charts(cases)
    command_summary = _build_command_summary(manifest, case_sections)
    sweep_verdict_label = _verdict_label(manifest["sweep"].get("verdict"))
    sweep_completion_label = _completion_label(manifest["sweep"].get("completion_state"))
    return template.render(
        manifest=manifest,
        sweep=manifest["sweep"],
        header_context_items=_build_header_context_items(manifest["sweep"]),
        sweep_verdict_label=sweep_verdict_label,
        sweep_completion_label=sweep_completion_label,
        source_artifacts=manifest["source_artifacts"],
        top_level_artifacts=manifest.get("top_level_artifacts", []),
        artifact_bundle=manifest.get("artifact_bundle"),
        publish_limits=manifest.get("publish_limits"),
        comparison=comparison,
        case_sections=case_sections,
        command_summary=command_summary,
        strip_charts=strip_charts,
        docker_images=manifest["docker_images"],
        artifact_inventory=manifest["artifact_inventory"],
        render_value=_render_value_markup,
        pretty_json=_pretty_json,
    )


def render_index_page(entries: list[dict[str, Any]]) -> str:
    template = _environment().get_template("index.html.j2")
    return template.render(entries=_build_index_rows(entries))


def write_index_json(entries: list[dict[str, Any]], output_path: Path) -> None:
    write_json_file(output_path, entries)


def copy_assets(output_dir: Path) -> Path:
    return copy_directory_contents(assets_dir(), output_dir / "assets")


def assets_dir() -> Path:
    return Path(str(files("bench_pages").joinpath("assets")))


# -- Template environment --

def _environment() -> Environment:
    return Environment(
        loader=FileSystemLoader(str(files("bench_pages").joinpath("templates"))),
        autoescape=select_autoescape(("html", "xml")),
        trim_blocks=True,
        lstrip_blocks=True,
    )


# -- Primary comparison table --

def _build_comparison_table(cases: list[dict[str, Any]]) -> dict[str, Any]:
    summaries = [_comparison_summary(case) for case in cases]
    typed_peek_keys = {
        key
        for summary in summaries
        for key in summary
        if key.endswith("_cache_peeks_per_second")
    }
    excluded_keys = {"failed_runs", "by_step_type", "aggregate", "schema_version"}
    if typed_peek_keys:
        excluded_keys.update({"peeks_per_second", "cold_peeks_per_second"})
    columns = _metric_columns(
        all_keys=_collect_metric_keys(
            summaries,
            always_show_keys=_ALWAYS_SHOW_KEYS,
        ),
        preferred_order=_COMPARISON_METRICS,
        excluded_keys=excluded_keys,
    )

    rows = []
    for case_index, case in enumerate(cases):
        verdict_label = _verdict_label(case.get("verdict"))
        completion_label = _completion_label(case.get("completion_state"))
        summary = summaries[case_index]
        cells = [
            _table_cell(summary.get(column["key"]), column["key"])
            for column in columns
        ]

        failed_runs = summary.get("failed_runs", [])
        failed_count = len(failed_runs) if isinstance(failed_runs, list) else 0

        rows.append(
            {
                "case_id": case["case_id"],
                "case_index": case_index,
                "axis_summary": _axis_summary(case.get("axis_assignments", {})),
                "verdict_label": verdict_label,
                "verdict_tooltip": _VERDICT_TOOLTIPS.get(verdict_label, ""),
                "completion_label": completion_label,
                "completion_class": _completion_class(case.get("completion_state")),
                "completion_tooltip": "; ".join(_case_status_reasons(case)),
                "failed_count": failed_count,
                "cells": cells,
            }
        )

    return {"columns": columns, "rows": rows}


def _comparison_summary(case: dict[str, Any]) -> dict[str, Any]:
    summary = dict(case.get("summary") or {})
    summary.update(_cache_expectation_peek_rates(case))
    return summary


def _cache_expectation_peek_rates(case: dict[str, Any]) -> dict[str, dict[str, Any]]:
    plan = _find_nested_mapping(case, "trusted_plan")
    steps = (case.get("summary") or {}).get("steps")
    if not plan or not isinstance(plan.get("steps"), list) or not isinstance(steps, list):
        return {}

    plan_steps = plan["steps"]
    duration_ms_by_expectation: dict[str, list[float]] = {}
    count_by_expectation: dict[str, list[int]] = {}
    for idx, plan_step in enumerate(plan_steps):
        if not isinstance(plan_step, dict) or idx >= len(steps):
            continue
        step_type = str(plan_step.get("type", ""))
        if "peek" not in step_type:
            continue
        expectation = _normalize_cache_expectation(plan_step.get("cache_expectation"))
        if expectation is None:
            continue
        summary_step = steps[idx]
        if not isinstance(summary_step, dict):
            continue
        duration = summary_step.get("duration_ms")
        if not _is_value_stats(duration):
            continue
        values = duration.get("values")
        if not isinstance(values, list):
            continue
        for run_index, duration_ms in enumerate(values):
            if not _is_number(duration_ms) or duration_ms <= 0:
                continue
            durations = duration_ms_by_expectation.setdefault(expectation, [])
            counts = count_by_expectation.setdefault(expectation, [])
            while len(durations) <= run_index:
                durations.append(0.0)
                counts.append(0)
            durations[run_index] += float(duration_ms)
            counts[run_index] += 1

    metrics = {}
    for expectation, durations in duration_ms_by_expectation.items():
        counts = count_by_expectation[expectation]
        values = [
            count * 1000.0 / duration_ms
            for count, duration_ms in zip(counts, durations)
            if count > 0 and duration_ms > 0
        ]
        if values:
            metrics[f"{expectation}_cache_peeks_per_second"] = _stats_from_values(values)
    return metrics


def _normalize_cache_expectation(value: Any) -> str | None:
    normalized = str(value or "unknown").strip().lower().replace("-", "_")
    if normalized in {"cold", "warm", "ambient", "unknown"}:
        return normalized
    return "unknown"


def _stats_from_values(values: list[float]) -> dict[str, Any]:
    ordered = sorted(values)
    median = statistics.median(ordered)
    mean = statistics.fmean(ordered)
    stddev = statistics.pstdev(ordered) if len(ordered) > 1 else 0.0
    mad = statistics.median([abs(value - median) for value in ordered])
    return {
        "median": median,
        "min": min(ordered),
        "max": max(ordered),
        "mad": mad,
        "stddev": stddev,
        "cv": stddev / mean if mean else 0.0,
        "values": values,
    }


# -- Strip charts --

def _build_strip_charts(cases: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Build SVG strip charts for key metrics showing per-case run spread."""
    charts = []
    for key, label in _STRIP_CHART_METRICS:
        svg = _render_strip_chart_svg(cases, key, label)
        if svg is not None:
            charts.append({"label": label, "svg": svg})
    return charts


def _render_strip_chart_svg(
    cases: list[dict[str, Any]], key: str, label: str,
) -> Markup | None:
    """Generate an inline SVG strip chart for one metric across all cases."""
    # Collect per-case run values and medians.
    rows: list[dict[str, Any]] = []
    all_values: list[float] = []
    for case in cases:
        summary_val = (case.get("summary") or {}).get(key)
        if not _is_value_stats(summary_val):
            continue
        values = [v for v in summary_val.get("values", []) if v is not None]
        median = summary_val.get("median")
        if not values or median is None:
            continue
        rows.append({
            "case_id": case["case_id"],
            "median": median,
            "values": values,
        })
        all_values.extend(values)

    if not rows or not all_values:
        return None

    # Scale with padding so dots aren't at the very edge.
    vmin = min(all_values)
    vmax = max(all_values)
    pad = (vmax - vmin) * 0.08
    if pad == 0:
        pad = max(abs(vmin) * 0.05, 0.5)
    scale_lo = vmin - pad
    scale_hi = vmax + pad
    scale_range = scale_hi - scale_lo

    # Layout constants.
    label_w = 180
    plot_l = label_w + 10
    chart_w = 700
    plot_r = chart_w - 15
    plot_w = plot_r - plot_l
    row_h = 28
    top_m = 6
    bot_m = 22
    chart_h = top_m + len(rows) * row_h + bot_m

    def xp(v: float) -> float:
        return plot_l + (v - scale_lo) / scale_range * plot_w

    parts: list[str] = []
    parts.append(
        f'<svg class="strip-chart" viewBox="0 0 {chart_w} {chart_h}" '
        f'xmlns="http://www.w3.org/2000/svg">'
    )

    # Axis gridlines and tick labels.
    n_ticks = 5
    plot_bot = chart_h - bot_m
    for i in range(n_ticks):
        t = scale_lo + scale_range * i / (n_ticks - 1)
        tx = xp(t)
        parts.append(
            f'<line class="strip-axis-line" '
            f'x1="{tx:.1f}" y1="{top_m}" x2="{tx:.1f}" y2="{plot_bot}"/>'
        )
        tick_label = _format_metric(t, key)
        parts.append(
            f'<text class="strip-tick-label" '
            f'x="{tx:.1f}" y="{chart_h - 5}">{html.escape(tick_label)}</text>'
        )

    # One row per case.
    for idx, row in enumerate(rows):
        cy = top_m + idx * row_h + row_h // 2

        # Case label.
        clabel = row["case_id"]
        if len(clabel) > 28:
            clabel = clabel[:26] + "\u2026"
        parts.append(
            f'<text class="strip-case-label" '
            f'x="{label_w}" y="{cy + 3.5}">{html.escape(clabel)}</text>'
        )

        # Spread line (min to max of run values).
        vals = row["values"]
        x1 = xp(min(vals))
        x2 = xp(max(vals))
        parts.append(
            f'<line class="strip-spread" '
            f'x1="{x1:.1f}" y1="{cy}" x2="{x2:.1f}" y2="{cy}"/>'
        )

        # Individual run dots.
        for j, v in enumerate(vals):
            dx = xp(v)
            tip = f"run-{j}: {_format_metric(v, key)}"
            parts.append(
                f'<circle class="strip-dot" cx="{dx:.1f}" cy="{cy}" r="3">'
                f'<title>{html.escape(tip)}</title></circle>'
            )

        # Median marker (on top).
        mx = xp(row["median"])
        tip = f"median: {_format_metric(row['median'], key)}"
        parts.append(
            f'<circle class="strip-median" cx="{mx:.1f}" cy="{cy}" r="4.5">'
            f'<title>{html.escape(tip)}</title></circle>'
        )

    parts.append("</svg>")
    return Markup("\n".join(parts))


# -- Per-case sections --

def _case_section(case: dict[str, Any]) -> dict[str, Any]:
    verdict_label = _verdict_label(case.get("verdict"))
    cpu_profile = case.get("cpu_profile")
    completion_state = case.get("completion_state")
    failure_reasons = _case_status_reasons(case)
    return {
        "case": case,
        "verdict_label": verdict_label,
        "completion_label": _completion_label(completion_state),
        "completion_class": _completion_class(completion_state),
        "context_items": _build_case_context_items(case),
        "workload_profile": _case_workload_profile(case),
        "operation_rows": _build_case_operation_rows(case),
        "plan_identity": _build_case_plan_identity(case),
        "input_identity": _build_case_input_identity(case),
        "raw_tx_replay": _build_raw_tx_replay_panel(case),
        "run_tables": _build_run_tables(case["runs"]),
        "samply_profile": _resolve_samply_profile(case),
        "cpu_profile": cpu_profile,
        "summary_markup": _render_object_table(case["summary"]),
        "provenance_markup": _render_object_table(case["provenance"]),
        "requested_case_markup": _render_object_table(case["requested_case"]),
        "resolved_case_markup": _render_object_table(case["resolved_case"]),
        "verdict_markup": _render_object_table(case["verdict"]),
        "validation_markup": (
            _render_object_table(case["validation"])
            if case["validation"]
            else None
        ),
        "materialized": bool(case.get("materialized", True)),
        "missing_artifacts": case.get("missing_artifacts", []),
        "failure_reasons": failure_reasons,
        "summary_missing": "summary.json" in (case.get("missing_artifacts") or []),
        "verdict_missing": "verdict.json" in (case.get("missing_artifacts") or []),
    }


def _case_status_reasons(case: dict[str, Any]) -> list[str]:
    reasons = []
    reasons.extend(str(reason) for reason in case.get("failure_reasons", []))
    reasons.extend(_verdict_reasons(case.get("verdict")))
    missing_reasons = missing_peek_status_reasons(case)
    reasons.extend(missing_reasons)
    if case.get("completion_state") == "partial" and not reasons:
        reasons.append("Case completed partially; inspect summary and verdict artifacts.")
    return list(dict.fromkeys(reasons))


def _build_raw_tx_replay_panel(case: dict[str, Any]) -> dict[str, Any]:
    summary = case.get("raw_tx_replay")
    if not isinstance(summary, dict) or not summary.get("active"):
        return {"active": False, "totals": [], "timings": [], "memory": [], "errors": []}

    totals = [
        _raw_tx_metric("Tx-active steps", summary.get("step_count"), "count"),
        _raw_tx_metric("Raw tx pokes", summary.get("raw_tx_pokes_completed"), "count"),
        _raw_tx_metric("Raw tx slabs", summary.get("raw_tx_slabs_prebuilt"), "count"),
        _raw_tx_metric(
            "Payload bytes",
            summary.get("raw_tx_payload_bytes_prebuilt"),
            "raw_tx_payload_bytes_prebuilt",
        ),
    ]
    timings = [
        _raw_tx_metric(
            "Block poke",
            summary.get("block_poke_duration_ms"),
            "block_poke_duration_ms",
        ),
        _raw_tx_metric(
            "Raw tx poke",
            summary.get("raw_tx_poke_duration_ms"),
            "raw_tx_poke_duration_ms",
        ),
        _raw_tx_metric(
            "Slab prebuild",
            summary.get("slab_prebuild_duration_ms"),
            "slab_prebuild_duration_ms",
        ),
        _raw_tx_metric(
            "Block slab",
            summary.get("block_slab_prebuild_duration_ms"),
            "block_slab_prebuild_duration_ms",
        ),
        _raw_tx_metric(
            "Raw tx slab",
            summary.get("raw_tx_slab_prebuild_duration_ms"),
            "raw_tx_slab_prebuild_duration_ms",
        ),
    ]
    memory = [
        _raw_tx_metric(
            "Prebuild RSS start",
            summary.get("slab_prebuild_start_rss_bytes"),
            "slab_prebuild_start_rss_bytes",
        ),
        _raw_tx_metric(
            "Prebuild RSS peak",
            summary.get("slab_prebuild_peak_rss_bytes"),
            "slab_prebuild_peak_rss_bytes",
        ),
    ]
    return {
        "active": True,
        "sections": [
            {"title": "Metric", "rows": totals},
            {"title": "Timing", "rows": timings},
            {"title": "Memory", "rows": memory},
        ],
        "errors": [
            _raw_tx_error_row(row)
            for row in summary.get("error_rows", [])
            if isinstance(row, dict)
        ],
        "error_rows_omitted": int(summary.get("error_rows_omitted") or 0),
    }


def _raw_tx_metric(label: str, value: Any, key: str) -> dict[str, str]:
    return {
        "label": label,
        "value": _format_raw_tx_value(value, key),
        "tooltip": _FIELD_TOOLTIPS.get(key, ""),
    }


def _format_raw_tx_value(value: Any, key: str) -> str:
    if isinstance(value, dict) and {"min", "max"}.issubset(value.keys()):
        low = value.get("min")
        high = value.get("max")
        if _is_number(low) and _is_number(high):
            if low == high:
                return _format_metric(low, key)
            return f"{_format_metric(low, key)}-{_format_metric(high, key)}"
    return _format_optional(value, key)


def _raw_tx_error_row(row: dict[str, Any]) -> dict[str, str]:
    height = row.get("height")
    label = row.get("label") or row.get("type") or "raw tx step"
    if height not in (None, ""):
        label = f"{label} @ {height}"
    return {
        "run_id": str(row.get("run_id") or ""),
        "label": str(label),
        "outcome": str(row.get("outcome") or "error"),
        "raw_tx_pokes_completed": _format_raw_tx_value(
            row.get("raw_tx_pokes_completed"), "count"
        ),
        "raw_tx_slabs_prebuilt": _format_raw_tx_value(
            row.get("raw_tx_slabs_prebuilt"), "count"
        ),
        "slab_prebuild_duration": _format_raw_tx_value(
            row.get("slab_prebuild_duration_ms"), "slab_prebuild_duration_ms"
        ),
        "error": str(row.get("error") or ""),
    }


# -- Command layout model --

def _build_command_summary(
    manifest: dict[str, Any], case_sections: list[dict[str, Any]]
) -> dict[str, Any]:
    cases = [section["case"] for section in case_sections]
    workload_profile = _sweep_workload_profile(cases)
    return {
        "workload_profile": workload_profile,
        "profile_label": _workload_profile_label(workload_profile),
        "kpis": _build_command_kpis(manifest, cases),
        "readable_plan": _build_readable_plan(manifest, cases),
        "operation_health_rows": build_operation_health_rows(
            case_sections,
            format_metric=_format_metric,
        ),
        "artifact_count": len(manifest.get("artifact_inventory") or []),
        "docker_image_count": len(manifest.get("docker_images") or []),
    }


def _build_command_kpis(
    manifest: dict[str, Any], cases: list[dict[str, Any]]
) -> list[dict[str, str]]:
    sweep = manifest["sweep"]
    requested = sweep.get("scheduled_case_count", len(cases))
    complete = sweep.get("complete_case_count", 0)
    runs_ok = sum(_summary_scalar(case, "measured_runs_succeeded") or 0 for case in cases)
    runs_requested = sum(_summary_scalar(case, "measured_runs_requested") or 0 for case in cases)
    kpis = [
        {
            "label": "Cases",
            "value": f"{complete}/{requested}",
            "detail": _completion_label(sweep.get("completion_state")),
        },
        {
            "label": "Runs",
            "value": f"{int(runs_ok)}/{int(runs_requested)}" if runs_requested else "n/a",
            "detail": "measured runs",
        },
        _metric_kpi(cases, "steps_per_second", "Steps/s"),
        _metric_kpi(cases, "pokes_per_second", "Pokes/s"),
    ]
    if any((case.get("summary") or {}).get("raw_tx_pokes_per_second") is not None for case in cases):
        kpis.append(_metric_kpi(cases, "raw_tx_pokes_per_second", "Raw tx/s"))
    kpis.extend(
        [
            _peek_metric_kpi(cases, "peeks_per_second", "Peeks/s", "peek_height"),
            _peek_metric_kpi(
                cases, "cold_peeks_per_second", "Cold peeks/s", "peek_height_cold"
            ),
            _missing_peek_kpi(cases),
            _metric_kpi(cases, "peak_process_rss_bytes", "Peak RSS"),
            {
                "label": "Artifacts",
                "value": str(len(manifest.get("artifact_inventory") or [])),
                "detail": "published files",
            },
        ]
    )
    return kpis


def _metric_kpi(cases: list[dict[str, Any]], key: str, label: str) -> dict[str, str]:
    values = [_summary_scalar(case, key) for case in cases]
    values = [value for value in values if value is not None]
    if not values:
        return {"label": label, "value": "n/a", "detail": "not reported"}
    if len(values) == 1 or min(values) == max(values):
        value = _format_metric(values[0], key)
    else:
        value = f"{_format_metric(min(values), key)}-{_format_metric(max(values), key)}"
    return {"label": label, "value": value, "detail": f"{len(values)} case(s)"}


def _peek_metric_kpi(
    cases: list[dict[str, Any]], key: str, label: str, step_type: str
) -> dict[str, str]:
    cases_with_missing = sum(1 for case in cases if step_missing_count(case, step_type) > 0)
    if cases_with_missing:
        return {
            "label": label,
            "value": "n/a",
            "detail": f"suppressed; {cases_with_missing} case(s) had missing peeks",
        }
    return _metric_kpi(cases, key, label)


def _missing_peek_kpi(cases: list[dict[str, Any]]) -> dict[str, str]:
    values = []
    for case in cases:
        values.append(
            step_missing_count(case, "peek_height")
            + step_missing_count(case, "peek_height_cold")
        )
    if not values:
        return {"label": "Missing peeks", "value": "n/a", "detail": "not reported"}
    if len(values) == 1 or min(values) == max(values):
        value = _format_metric(values[0], "count")
    else:
        value = f"{_format_metric(min(values), 'count')}-{_format_metric(max(values), 'count')}"
    return {"label": "Missing peeks", "value": value, "detail": "median per run"}


def _build_readable_plan(
    manifest: dict[str, Any], cases: list[dict[str, Any]]
) -> dict[str, Any]:
    if not cases:
        return {
            "headline": "No scheduled cases",
            "lines": ["No plan artifacts were available."],
            "operations": [],
        }

    if len(cases) > 1:
        return _build_multi_case_readable_plan(manifest, cases)

    case = cases[0]
    plan = _find_nested_mapping(case, "trusted_plan")
    requested = case.get("requested_case") or {}
    summary = case.get("summary") or {}
    steps = plan.get("steps") if isinstance(plan, dict) else None
    steps = steps if isinstance(steps, list) else []
    boot = plan.get("boot") if isinstance(plan, dict) else None
    boot = boot if isinstance(boot, dict) else {}
    boot_line = _readable_boot_line(boot)

    measured_runs = _summary_scalar(case, "measured_runs_requested")
    if measured_runs is None:
        measured_runs = _first_mapping_value([requested], "measured_runs")
    warmup_runs = _first_mapping_value([requested], "warmup_runs")
    profile_memory = _first_mapping_value([requested], "profile_memory")
    profile_interval = _first_mapping_value([requested], "profile_interval_ms")
    fsync = _first_mapping_value([requested], "fsync")
    if fsync is None:
        fsync = _first_mapping_value([requested], "fsync_enabled")

    operations = summarize_plan_operations(steps, summary)
    operation_total = sum(row["count_raw"] for row in operations)
    if operation_total == 0:
        operation_total = len(steps)

    lines = [
        boot_line,
        f"Run {operation_total} planned operations",
    ]
    lines.extend(_readable_block_range_lines(operations))
    run_bits = []
    if measured_runs is not None:
        run_bits.append(f"Measured runs: {int(measured_runs)}")
    if warmup_runs is not None:
        run_bits.append(f"warmups: {warmup_runs}")
    if run_bits:
        lines.append(", ".join(run_bits))

    execution_line = _readable_execution_line(requested)
    if execution_line:
        lines.append(execution_line)

    settings = []
    if fsync is not None:
        settings.append(f"fsync {'on' if bool(fsync) else 'off'}")
    if profile_memory is not None:
        if bool(profile_memory):
            interval = f", {profile_interval}ms interval" if profile_interval else ""
            settings.append(f"memory profiling on{interval}")
        else:
            settings.append("memory profiling off")
    if settings:
        lines.append("; ".join(settings))

    return {
        "headline": _readable_workload_sentence(_case_workload_profile(case), operations),
        "lines": lines,
        "operations": [
            {
                "type": row["type"],
                "count": str(row["count_raw"]),
                "range": row["range"],
                "cache_expectation": _cache_expectation_label(
                    row.get("cache_expectation")
                ),
            }
            for row in operations
        ],
    }


def _readable_boot_line(boot: dict[str, Any]) -> str:
    kernel_id = boot.get("kernel_input_id") or "kernel"
    source = boot.get("source")
    if not isinstance(source, dict):
        checkpoint_id = boot.get("checkpoint_input_id")
        if checkpoint_id:
            return f"Boot from {checkpoint_id} using {kernel_id}"
        return f"Boot source unknown using {kernel_id}"

    source_type = source.get("type")
    if source_type == "checkpoint":
        checkpoint_id = source.get("checkpoint_input_id") or "checkpoint"
        return f"Boot from checkpoint {checkpoint_id} using {kernel_id}"
    if source_type == "snapshot":
        pma_id = source.get("pma_input_id") or "snapshot-pma"
        manifest_id = source.get("manifest_input_id") or "snapshot-manifest"
        return f"Boot from snapshot {pma_id} + {manifest_id} using {kernel_id}"
    return f"Boot source unknown using {kernel_id}"


def _readable_block_range_lines(operations: list[dict[str, Any]]) -> list[str]:
    lines = []
    for operation in operations:
        step_type = operation["type"]
        height_range = operation["range"]
        if height_range == "n/a":
            continue
        expectation = str(operation.get("cache_expectation") or "unknown").lower()
        if "poke" in step_type:
            label = "Poke block range"
        elif expectation == "warm":
            label = "Warm peek block range"
        elif expectation == "cold":
            label = "Cold peek block range"
        elif expectation == "ambient":
            label = "Ambient peek block range"
        elif step_type == "peek_height_cold":
            label = "Cold peek block range"
        elif "peek" in step_type:
            label = "Peek block range"
        else:
            continue
        lines.append(f"{label}: {height_range}")
    return lines


def _build_multi_case_readable_plan(
    manifest: dict[str, Any], cases: list[dict[str, Any]]
) -> dict[str, Any]:
    workload = _workload_profile_label(_sweep_workload_profile(cases))
    scheduled = manifest["sweep"].get("scheduled_case_count", len(cases))
    complete = manifest["sweep"].get("complete_case_count", 0)
    operation_counts: dict[str, int] = {}
    for case in cases:
        for operation in summarize_plan_operations(
            (_find_nested_mapping(case, "trusted_plan") or {}).get("steps") or [],
            case.get("summary") or {},
        ):
            operation_counts[operation["type"]] = (
                operation_counts.get(operation["type"], 0) + operation["count_raw"]
            )
    operations = [
        {
            "type": step_type,
            "count": str(count),
            "range": "across cases",
            "cache_expectation": "Mixed",
        }
        for step_type, count in sorted(operation_counts.items())
    ]
    return {
        "headline": f"{workload} sweep across {scheduled} scheduled cases",
        "lines": [
            f"{complete} complete out of {scheduled} scheduled cases",
            f"Execution mode: {manifest['sweep'].get('execution_mode', 'unknown')}",
        ],
        "operations": operations,
    }


def _readable_workload_sentence(
    workload_profile: str, operations: list[dict[str, Any]]
) -> str:
    if not operations:
        return _workload_profile_label(workload_profile)
    counts = {row["type"]: row["count_raw"] for row in operations}
    poke_count = sum(count for step_type, count in counts.items() if "poke" in step_type)
    warm_peek_count = sum(
        row["count_raw"]
        for row in operations
        if "peek" in row["type"] and row.get("cache_expectation") == "warm"
    )
    cold_peek_count = sum(
        row["count_raw"]
        for row in operations
        if "peek" in row["type"] and row.get("cache_expectation") == "cold"
    )
    ambient_peek_count = sum(
        row["count_raw"]
        for row in operations
        if "peek" in row["type"] and row.get("cache_expectation") == "ambient"
    )
    unknown_peek_count = sum(
        row["count_raw"]
        for row in operations
        if "peek" in row["type"] and row.get("cache_expectation") == "unknown"
    )
    parts = []
    if poke_count:
        parts.append(f"{poke_count} pokes")
    if cold_peek_count:
        parts.append(f"{cold_peek_count} cold peeks")
    if warm_peek_count:
        parts.append(f"{warm_peek_count} warm peeks")
    if ambient_peek_count:
        parts.append(f"{ambient_peek_count} ambient peeks")
    if unknown_peek_count:
        parts.append(f"{unknown_peek_count} peeks")
    if parts:
        return ", ".join(parts)
    return _workload_profile_label(workload_profile)


def _cache_expectation_label(value: Any) -> str:
    value = str(value or "unknown").replace("-", "_").lower()
    return {
        "cold": "Cold",
        "warm": "Warm",
        "ambient": "Ambient",
        "unknown": "Unknown",
    }.get(value, "Unknown")


def _readable_execution_line(requested: dict[str, Any]) -> str | None:
    execution = requested.get("execution")
    if not isinstance(execution, dict):
        return None
    docker = execution.get("Docker")
    if isinstance(docker, dict):
        parts = ["Docker"]
        if docker.get("memory_limit"):
            parts.append(str(docker["memory_limit"]))
        if docker.get("work_dir_mode"):
            parts.append(str(docker["work_dir_mode"]))
        return ", ".join(parts)
    if "Native" in execution:
        return "Native execution"
    return None


def _build_case_operation_rows(case: dict[str, Any]) -> list[dict[str, str]]:
    return build_case_operation_rows(
        case,
        find_nested_mapping=_find_nested_mapping,
        format_optional=_format_optional,
    )


def _build_case_plan_identity(case: dict[str, Any]) -> dict[str, str]:
    plan = _find_nested_mapping(case, "trusted_plan")
    if not plan:
        plan = _find_nested_mapping(case, "plan")
    return {
        "plan_hash": _short_identity(plan.get("normalized_plan_sha256_hex") if plan else None),
        "step_signature": _short_identity(plan.get("step_signature_sha256_hex") if plan else None),
        "plan_hash_full": str(plan.get("normalized_plan_sha256_hex") or "") if plan else "",
        "step_signature_full": str(plan.get("step_signature_sha256_hex") or "") if plan else "",
    }


def _build_case_input_identity(case: dict[str, Any]) -> list[dict[str, str]]:
    candidates = []
    for source_key in ("resolved_case", "requested_case", "provenance"):
        source = case.get(source_key)
        if isinstance(source, dict):
            candidates.append(source)
            nested = source.get("input_identity")
            if isinstance(nested, dict):
                candidates.append(nested)
            manifest = source.get("fixture_manifest")
            if isinstance(manifest, dict):
                candidates.append(manifest)
    specs = (
        ("Fixture", "fixture_sha256_hex"),
        ("Checkpoint Height", "derived_checkpoint_height"),
        ("Checkpoint Event", "derived_checkpoint_event_num"),
        ("Archive Start", "archive_start_height"),
        ("Archive End", "archive_end_height"),
        ("Kernel", "kernel_hash_hex"),
    )
    rows = []
    for label, key in specs:
        value = _first_mapping_value(candidates, key)
        if value not in (None, ""):
            rows.append({"label": label, "key": key, "value": _short_identity(value)})
    return rows


def _sweep_workload_profile(cases: list[dict[str, Any]]) -> str:
    profiles = {_case_workload_profile(case) for case in cases}
    if "combined" in profiles:
        return "combined"
    if "cold-warm-peek" in profiles:
        return "cold-warm-peek"
    has_poke = "poke-only" in profiles
    has_peek = "peek-only" in profiles
    if has_poke and has_peek:
        return "combined"
    if has_poke:
        return "poke-only"
    if has_peek:
        return "peek-only"
    return "unknown"


def _case_workload_profile(case: dict[str, Any]) -> str:
    summary = case.get("summary") or {}
    step_names = set()
    by_step_type = summary.get("by_step_type")
    if isinstance(by_step_type, dict):
        step_names.update(str(name) for name in by_step_type.keys())
    plan = _find_nested_mapping(case, "trusted_plan")
    if plan and isinstance(plan.get("steps"), list):
        step_names = {
            str(row["type"]) for row in summarize_plan_operations(plan["steps"], summary)
        }
    has_poke = any("poke" in name for name in step_names)
    has_warm_peek = "peek_height" in step_names
    has_cold_peek = "peek_height_cold" in step_names
    has_peek = any("peek" in name for name in step_names)
    has_poke = has_poke or bool((_summary_scalar(case, "pokes_per_second") or 0) > 0)
    has_peek = has_peek or bool((_summary_scalar(case, "peeks_per_second") or 0) > 0)
    has_cold_peek = has_cold_peek or bool((_summary_scalar(case, "cold_peeks_per_second") or 0) > 0)
    has_peek = has_peek or has_cold_peek
    if has_poke and has_peek:
        return "combined"
    if has_warm_peek and has_cold_peek:
        return "cold-warm-peek"
    if has_poke:
        return "poke-only"
    if has_peek:
        return "peek-only"
    return "unknown"


def _workload_profile_label(profile: str) -> str:
    return {
        "poke-only": "Poke-only",
        "peek-only": "Peek-only",
        "cold-warm-peek": "Cold + warm peeks",
        "combined": "Poke + peek",
        "unknown": "Unknown",
    }.get(profile, str(profile))


def _summary_scalar(case: dict[str, Any], key: str) -> float | int | None:
    value = (case.get("summary") or {}).get(key)
    return _stats_scalar(value)


def _verdict_reasons(verdict: Any) -> list[str]:
    if not isinstance(verdict, dict):
        return []
    validity = verdict.get("validity")
    if not isinstance(validity, dict):
        return []
    payload = next(iter(validity.values()), None)
    if not isinstance(payload, dict):
        return []
    reasons = payload.get("reasons")
    if not isinstance(reasons, list):
        return []
    return [str(reason) for reason in reasons]


def _format_optional(value: Any, key: str = "") -> str:
    if _is_value_stats(value):
        value = value.get("median")
    if value in (None, ""):
        return "n/a"
    if _is_number(value):
        return _format_metric(value, key)
    return str(value)


def _find_nested_mapping(value: Any, key: str) -> dict[str, Any] | None:
    if isinstance(value, dict):
        nested = value.get(key)
        if isinstance(nested, dict):
            return nested
        for child in value.values():
            found = _find_nested_mapping(child, key)
            if found is not None:
                return found
    elif isinstance(value, list):
        for child in value:
            found = _find_nested_mapping(child, key)
            if found is not None:
                return found
    return None


def _first_mapping_value(mappings: list[dict[str, Any]], key: str) -> Any:
    for mapping in mappings:
        if key in mapping:
            return mapping[key]
    return None


def _short_identity(value: Any) -> str:
    if value in (None, ""):
        return "n/a"
    text = str(value)
    if len(text) > 20:
        return text[:12] + "..." + text[-6:]
    return text


def _build_run_tables(runs: list[dict[str, Any]]) -> list[dict[str, Any]]:
    if not runs:
        return []

    columns = _metric_columns(
        all_keys=_collect_metric_keys(
            [run.get("result") or {} for run in runs],
            always_show_keys=_ALWAYS_SHOW_KEYS,
            excluded_keys=_RUN_EXCLUDE_KEYS,
        ),
        preferred_order=_RUN_KEY_ORDER,
    )

    rows = []
    for run in runs:
        result = run.get("result") or {}
        cells = [
            _table_cell(result.get(column["key"]), column["key"])
            for column in columns
        ]
        rows.append(
            {
                "run_id": run["run_id"],
                "cells": cells,
                "artifacts": run["artifacts"],
            }
        )

    return [{"columns": columns, "rows": rows}]


def _build_index_rows(entries: list[dict[str, Any]]) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for entry in entries:
        row = dict(entry)
        row["fixture_display"] = (
            entry.get("fixture_summary")
            or entry.get("fixture_identity")
            or "n/a"
        )
        row["runtime_display"] = entry.get("runtime_summary") or "n/a"
        row["work_dir_display"] = entry.get("pma_work_dir_summary")
        rows.append(row)
    return rows


def _build_header_context_items(sweep: dict[str, Any]) -> list[dict[str, str]]:
    return _context_items(
        [
            ("Fixture", sweep.get("fixture_summary") or sweep.get("fixture_identity"), None),
            ("Runtime", sweep.get("runtime_summary"), "runtime_flavor"),
            ("Boot", sweep.get("boot_source_summary"), "boot_source"),
            ("Work Dir", sweep.get("pma_work_dir_summary"), "pma_work_dir_mode"),
        ]
    )


def _build_case_context_items(case: dict[str, Any]) -> list[dict[str, str]]:
    return _context_items(
        [
            ("Fixture", case.get("fixture_identity"), None),
            ("Runtime", case.get("runtime_flavor"), "runtime_flavor"),
            ("Boot", case.get("boot_source"), "boot_source"),
            ("Boot Event", case.get("boot_event_num"), "boot_event_num"),
            ("Work Dir", case.get("pma_work_dir_mode"), "pma_work_dir_mode"),
        ]
    )


def _context_items(
    specs: list[tuple[str, Any, str | None]],
) -> list[dict[str, str]]:
    items: list[dict[str, str]] = []
    for label, value, tooltip_key in specs:
        if value in (None, ""):
            continue
        items.append(
            {
                "label": label,
                "value": str(value),
                "tooltip": _FIELD_TOOLTIPS.get(tooltip_key or "", ""),
            }
        )
    return items


def _axis_summary(axis_assignments: dict[str, Any]) -> str:
    axis_parts = [f"{key}={value}" for key, value in axis_assignments.items()]
    return ", ".join(axis_parts) if axis_parts else "\u2014"


def _collect_metric_keys(
    mappings: list[dict[str, Any]],
    *,
    always_show_keys: set[str] | None = None,
    excluded_keys: set[str] | None = None,
) -> set[str]:
    keys: set[str] = set()
    for mapping in mappings:
        keys.update(mapping.keys())
    if always_show_keys:
        keys.update(always_show_keys)
    if excluded_keys:
        keys -= excluded_keys
    return keys


def _resolve_samply_profile(case: dict[str, Any]) -> dict[str, Any] | None:
    cpu_profile = case.get("cpu_profile")
    if cpu_profile is not None:
        profile_artifact = cpu_profile.get("profile_artifact")
        if profile_artifact is not None:
            return profile_artifact

    for artifact in case.get("artifacts", []):
        if "samply-profile" in artifact.get("relative_path", ""):
            return artifact
    return None


def _metric_column(key: str) -> dict[str, str]:
    return {
        "key": key,
        "label": _METRIC_LABELS.get(key, key),
        "tooltip": _FIELD_TOOLTIPS.get(key, ""),
    }


def _metric_columns(
    all_keys: set[str],
    preferred_order: list[str],
    excluded_keys: set[str] | None = None,
) -> list[dict[str, str]]:
    excluded = excluded_keys or set()
    ordered_keys: list[str] = []
    seen: set[str] = set()
    for key in preferred_order:
        if key in all_keys and key not in excluded:
            ordered_keys.append(key)
            seen.add(key)
    ordered_keys.extend(sorted(all_keys - seen - excluded))
    return [_metric_column(key) for key in ordered_keys]


def _table_cell(value: Any, key: str) -> dict[str, Any]:
    return {
        "markup": _render_value_compact(value, key),
        "tooltip": _cell_tooltip(value, key),
    }


def _verdict_label(verdict: Any) -> str:
    if isinstance(verdict, dict):
        validity = verdict.get("validity", "Unknown")
        if isinstance(validity, dict):
            return str(next(iter(validity.keys()), "Unknown"))
        if validity is None:
            return "Unknown"
        return str(validity)
    if verdict is None:
        return "Unknown"
    return str(verdict)


def _completion_label(completion_state: Any) -> str:
    if completion_state == "complete":
        return "Complete"
    if completion_state == "partial":
        return "Partial"
    if completion_state == "missing":
        return "Missing"
    if completion_state == "incomplete":
        return "Incomplete / Aborted"
    return "Unknown"


def _completion_class(completion_state: Any) -> str:
    if completion_state in {"complete", "partial", "missing", "incomplete"}:
        return str(completion_state)
    return "unknown"


# -- Tooltips --

def _cell_tooltip(value: Any, key: str = "") -> str:
    """Generate a hover tooltip for a table cell."""
    if value is None:
        return "No data available for this metric."
    if _is_value_stats(value):
        return _valuestats_tooltip(value)
    if isinstance(value, bool):
        return "Run completed successfully." if value else "Run failed."
    return _FIELD_TOOLTIPS.get(key, "")


def _valuestats_tooltip(value: dict[str, Any]) -> str:
    """Tooltip showing full ValueStats breakdown."""
    parts = []
    for field in ("median", "min", "max", "stddev", "mad"):
        v = value.get(field)
        if v is not None:
            parts.append(f"{field}: {_format_number(v)}")
    cv = value.get("cv")
    if cv is not None:
        parts.append(f"cv=stddev/mean: {cv:.4f} (lower = more consistent)")
    n = len(value.get("values", []))
    parts.append(f"samples: {n}")
    return " | ".join(parts)


# -- Value rendering --

def _render_value_compact(value: Any, key: str = "") -> Markup:
    """Compact rendering for table cells.

    ValueStats: median as primary line, min-max range + cv as secondary.
    """
    if value is None:
        return Markup('<span class="na">n/a</span>')
    if _is_value_stats(value):
        median = value.get("median")
        if median is None:
            return Markup('<span class="na">n/a</span>')
        primary = _format_metric(median, key)
        parts = []
        vmin = value.get("min")
        vmax = value.get("max")
        if vmin is not None and vmax is not None:
            parts.append(
                f"{_format_metric(vmin, key)}\u2013{_format_metric(vmax, key)}"
            )
        cv = value.get("cv")
        if cv is not None:
            parts.append(f"cv {cv:.3f}")
        secondary = " ".join(parts)
        return Markup(
            '<span class="vs-primary">{primary}</span>'
            '<span class="vs-detail">{secondary}</span>'.format(
                primary=html.escape(primary),
                secondary=html.escape(secondary),
            )
        )
    if isinstance(value, bool):
        css = "val-ok" if value else "val-fail"
        label = "true" if value else "false"
        return Markup(f'<span class="{css}">{html.escape(label)}</span>')
    if _is_number(value):
        return Markup(html.escape(_format_metric(value, key)))
    if isinstance(value, list):
        return Markup(html.escape(str(len(value))))
    return Markup(html.escape(str(value)))


def _render_object_table(value: Any) -> Markup:
    """Render a dict as a key-value table with tooltips and byte humanization."""
    if not isinstance(value, dict):
        return _render_value_markup(value)
    rows = []
    for key, item in value.items():
        tooltip = _FIELD_TOOLTIPS.get(key, "")
        title_attr = f' title="{html.escape(tooltip)}"' if tooltip else ""
        rows.append(
            "<tr><th{title}>{key}</th><td>{value}</td></tr>".format(
                title=title_attr,
                key=html.escape(str(key)),
                value=_render_value_for_key(key, item),
            )
        )
    return Markup('<table class="kv-table">{rows}</table>'.format(rows="".join(rows)))


def _render_value_for_key(key: str, value: Any) -> Markup:
    """Key-aware rendering: humanizes byte values, falls back to full fidelity."""
    if (
        _is_number(value)
        and not isinstance(value, bool)
        and _key_suggests_bytes(key)
        and abs(value) >= 1024
    ):
        human = _humanize_bytes(value)
        raw = _format_number(value)
        return Markup(
            '{human} <span class="raw-bytes">({raw})</span>'.format(
                human=html.escape(human),
                raw=html.escape(raw),
            )
        )
    return _render_value_markup(value)


def _key_suggests_bytes(key: str) -> bool:
    """Heuristic: does this field key suggest the value is in bytes?"""
    if "_bytes" in key:
        return True
    if key.startswith("realized_memory_") or key.startswith("total_memory"):
        return True
    return False


def _render_value_markup(value: Any) -> Markup:
    """Full-fidelity rendering for evidence drawers and detail views."""
    if value is None:
        return Markup('<span class="na">n/a</span>')
    if _is_value_stats(value):
        labels = ("median", "min", "max", "mad", "stddev", "cv", "values")
        rows = []
        for label in labels:
            rows.append(
                "<tr><th>{label}</th><td>{value}</td></tr>".format(
                    label=html.escape(label),
                    value=_render_value_markup(value.get(label)),
                )
            )
        return Markup(
            '<table class="valuestats">{rows}</table>'.format(rows="".join(rows))
        )
    if isinstance(value, dict):
        return _render_object_table(value)
    if isinstance(value, list):
        items = "".join(
            f"<li>{_render_value_markup(item)}</li>" for item in value
        )
        return Markup(f'<ul class="json-list">{items}</ul>')
    if isinstance(value, bool):
        return Markup(html.escape("true" if value else "false"))
    if _is_number(value):
        return Markup(html.escape(_format_number(value)))
    return Markup(html.escape(str(value)))


# -- Formatting helpers --

def _format_metric(value: int | float, key: str = "") -> str:
    if _key_suggests_bytes(key) and isinstance(value, (int, float)) and abs(value) >= 1024:
        return _humanize_bytes(value)
    return _format_compact(value)


def _format_compact(value: int | float) -> str:
    """Compact number formatting for table cells.

    Adapts decimal places to magnitude for readability without excess precision.
    """
    if isinstance(value, int):
        return str(value)
    if value == 0:
        return "0"
    # Float that is exactly an integer value
    if value == int(value) and abs(value) < 1e15:
        return str(int(value))
    av = abs(value)
    if av >= 1000:
        return f"{value:.0f}"
    if av >= 100:
        return f"{value:.1f}"
    if av >= 1:
        return f"{value:.2f}"
    return f"{value:.3g}"


def _humanize_bytes(value: int | float) -> str:
    v = float(value)
    for unit in ("B", "KiB", "MiB", "GiB", "TiB"):
        if abs(v) < 1024:
            if v == int(v):
                return f"{int(v)} {unit}"
            return f"{v:.1f} {unit}"
        v /= 1024
    return f"{v:.1f} PiB"


def _pretty_json(value: Any) -> str:
    return json.dumps(value, indent=2, sort_keys=True)


def _format_number(value: int | float) -> str:
    """Full-precision formatting for evidence/detail views."""
    if isinstance(value, int):
        return str(value)
    return f"{value:.6f}".rstrip("0").rstrip(".")
