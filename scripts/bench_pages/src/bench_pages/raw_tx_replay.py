from __future__ import annotations

import json
import math
from collections import deque
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from bench_pages.errors import ValidationError


ERROR_ROW_SAMPLE_LIMIT = 20
ERROR_ROW_SAMPLE_EDGE = ERROR_ROW_SAMPLE_LIMIT // 2


@dataclass(frozen=True)
class RawTxMetricSpec:
    aggregation: str
    value_type: str


RAW_TX_METRIC_SPECS: dict[str, RawTxMetricSpec] = {
    "raw_tx_pokes_completed": RawTxMetricSpec("sum", "integer"),
    "block_poke_duration_ms": RawTxMetricSpec("sum", "number"),
    "raw_tx_poke_duration_ms": RawTxMetricSpec("sum", "number"),
    "slab_prebuild_duration_ms": RawTxMetricSpec("sum", "number"),
    "block_slab_prebuild_duration_ms": RawTxMetricSpec("sum", "number"),
    "raw_tx_slab_prebuild_duration_ms": RawTxMetricSpec("sum", "number"),
    "raw_tx_slabs_prebuilt": RawTxMetricSpec("sum", "integer"),
    "raw_tx_payload_bytes_prebuilt": RawTxMetricSpec("sum", "integer"),
    "slab_prebuild_start_rss_bytes": RawTxMetricSpec("range", "integer"),
    "slab_prebuild_peak_rss_bytes": RawTxMetricSpec("range", "integer"),
}


SUM_FIELDS = tuple(
    field
    for field, spec in RAW_TX_METRIC_SPECS.items()
    if spec.aggregation == "sum"
)
RANGE_FIELDS = tuple(
    field
    for field, spec in RAW_TX_METRIC_SPECS.items()
    if spec.aggregation == "range"
)


def load_raw_tx_replay_summary(path: Path, *, run_id: str) -> dict[str, Any]:
    if not path.exists():
        return empty_raw_tx_replay_summary()

    accumulator = RawTxReplayAccumulator()
    try:
        with path.open() as rows:
            for line_number, line in enumerate(rows, start=1):
                if not line.strip():
                    continue
                try:
                    row = json.loads(line)
                except json.JSONDecodeError as exc:
                    raise ValidationError(
                        f"invalid NDJSON artifact {path}:{line_number}: {exc}"
                    ) from exc
                if not isinstance(row, dict):
                    raise ValidationError(
                        f"invalid NDJSON artifact {path}:{line_number}: expected object"
                    )
                accumulator.add_step(
                    row,
                    run_id=run_id,
                    path=path,
                    line_number=line_number,
                )
    except FileNotFoundError:
        return empty_raw_tx_replay_summary()

    return accumulator.to_summary()


def combine_raw_tx_replay_summaries(
    summaries: list[dict[str, Any]],
) -> dict[str, Any]:
    accumulator = RawTxSummaryAccumulator()
    for summary in summaries:
        accumulator.add_summary(summary)
    return accumulator.to_summary()


def empty_raw_tx_replay_summary() -> dict[str, Any]:
    return _summary_template(active=False)


@dataclass
class RawTxReplayAccumulator:
    step_count: int = 0
    error_step_count: int = 0
    known_zero_raw_tx_poke_steps: int = 0
    sums: dict[str, int | float] = field(default_factory=dict)
    ranges: dict[str, dict[str, int | float]] = field(default_factory=dict)
    error_sampler: "ErrorRowSampler" = field(default_factory=lambda: ErrorRowSampler())

    def add_step(
        self,
        step: dict[str, Any],
        *,
        run_id: str,
        path: Path,
        line_number: int,
    ) -> None:
        values = _validated_raw_tx_values(step, path=path, line_number=line_number)
        if not values:
            return

        self.step_count += 1
        if values.get("raw_tx_pokes_completed") == 0:
            self.known_zero_raw_tx_poke_steps += 1
        for field, value in values.items():
            spec = RAW_TX_METRIC_SPECS[field]
            if spec.aggregation == "sum":
                self.sums[field] = self.sums.get(field, 0) + value
            elif spec.aggregation == "range":
                current = self.ranges.get(field)
                if current is None:
                    self.ranges[field] = {"min": value, "max": value}
                else:
                    current["min"] = min(current["min"], value)
                    current["max"] = max(current["max"], value)

        if _is_failure_step(step):
            self.error_step_count += 1
            self.error_sampler.add(_raw_tx_step_row(step, values=values, run_id=run_id))

    def to_summary(self) -> dict[str, Any]:
        summary = _summary_template(active=self.step_count > 0)
        summary["step_count"] = self.step_count
        summary["error_step_count"] = self.error_step_count
        summary["known_zero_raw_tx_poke_steps"] = self.known_zero_raw_tx_poke_steps
        for field in SUM_FIELDS:
            summary[field] = _normalize_sum(self.sums.get(field))
        for field in RANGE_FIELDS:
            summary[field] = self.ranges.get(field)
        summary["error_rows"] = self.error_sampler.rows()
        summary["error_rows_omitted"] = max(
            0, self.error_step_count - len(summary["error_rows"])
        )
        summary["error_row_sample_limit"] = ERROR_ROW_SAMPLE_LIMIT
        return summary


@dataclass
class RawTxSummaryAccumulator:
    step_count: int = 0
    error_step_count: int = 0
    known_zero_raw_tx_poke_steps: int = 0
    sums: dict[str, int | float] = field(default_factory=dict)
    ranges: dict[str, dict[str, int | float]] = field(default_factory=dict)
    error_sampler: "ErrorRowSampler" = field(default_factory=lambda: ErrorRowSampler())

    def add_summary(self, summary: dict[str, Any]) -> None:
        if not isinstance(summary, dict) or not summary.get("active"):
            return
        self.step_count += _int_value(summary.get("step_count"))
        self.error_step_count += _int_value(summary.get("error_step_count"))
        self.known_zero_raw_tx_poke_steps += _int_value(
            summary.get("known_zero_raw_tx_poke_steps")
        )
        for field in SUM_FIELDS:
            value = summary.get(field)
            if _is_number(value):
                self.sums[field] = self.sums.get(field, 0) + value
        for field in RANGE_FIELDS:
            value = summary.get(field)
            if _is_range(value):
                current = self.ranges.get(field)
                if current is None:
                    self.ranges[field] = {"min": value["min"], "max": value["max"]}
                else:
                    current["min"] = min(current["min"], value["min"])
                    current["max"] = max(current["max"], value["max"])
        for row in summary.get("error_rows", []):
            if isinstance(row, dict):
                self.error_sampler.add(row)

    def to_summary(self) -> dict[str, Any]:
        summary = _summary_template(active=self.step_count > 0)
        summary["step_count"] = self.step_count
        summary["error_step_count"] = self.error_step_count
        summary["known_zero_raw_tx_poke_steps"] = self.known_zero_raw_tx_poke_steps
        for field in SUM_FIELDS:
            summary[field] = _normalize_sum(self.sums.get(field))
        for field in RANGE_FIELDS:
            summary[field] = self.ranges.get(field)
        summary["error_rows"] = self.error_sampler.rows()
        summary["error_rows_omitted"] = max(
            0, self.error_step_count - len(summary["error_rows"])
        )
        summary["error_row_sample_limit"] = ERROR_ROW_SAMPLE_LIMIT
        return summary


@dataclass
class ErrorRowSampler:
    head: list[dict[str, Any]] = field(default_factory=list)
    tail: deque[dict[str, Any]] = field(
        default_factory=lambda: deque(maxlen=ERROR_ROW_SAMPLE_EDGE)
    )

    def add(self, row: dict[str, Any]) -> None:
        if len(self.head) < ERROR_ROW_SAMPLE_EDGE:
            self.head.append(row)
        else:
            self.tail.append(row)

    def rows(self) -> list[dict[str, Any]]:
        return self.head + list(self.tail)


def _summary_template(*, active: bool) -> dict[str, Any]:
    summary: dict[str, Any] = {
        "active": active,
        "step_count": 0,
        "error_step_count": 0,
        "known_zero_raw_tx_poke_steps": 0,
        "error_rows": [],
        "error_rows_omitted": 0,
        "error_row_sample_limit": ERROR_ROW_SAMPLE_LIMIT,
    }
    for field in SUM_FIELDS:
        summary[field] = None
    for field in RANGE_FIELDS:
        summary[field] = None
    return summary


def _validated_raw_tx_values(
    step: dict[str, Any],
    *,
    path: Path,
    line_number: int,
) -> dict[str, int | float]:
    values: dict[str, int | float] = {}
    for field, spec in RAW_TX_METRIC_SPECS.items():
        if field not in step or step[field] is None:
            continue
        values[field] = _validate_raw_tx_value(
            field,
            step[field],
            spec=spec,
            path=path,
            line_number=line_number,
        )
    return values


def _validate_raw_tx_value(
    field: str,
    value: Any,
    *,
    spec: RawTxMetricSpec,
    path: Path,
    line_number: int,
) -> int | float:
    if spec.value_type == "integer":
        if isinstance(value, bool) or not isinstance(value, int):
            raise _invalid_raw_tx_field(path, line_number, field, value, "integer")
        return value
    if not _is_number(value):
        raise _invalid_raw_tx_field(path, line_number, field, value, "number")
    return value


def _invalid_raw_tx_field(
    path: Path,
    line_number: int,
    field: str,
    value: Any,
    expected: str,
) -> ValidationError:
    return ValidationError(
        f"invalid raw transaction metric {path}:{line_number}: "
        f"{field} expected {expected}, got {type(value).__name__}"
    )


def _raw_tx_step_row(
    step: dict[str, Any],
    *,
    values: dict[str, int | float],
    run_id: str,
) -> dict[str, Any]:
    row: dict[str, Any] = {
        "run_id": run_id,
        "step_index": step.get("step_index"),
        "label": step.get("label"),
        "type": step.get("type"),
        "height": step.get("height"),
        "outcome": step.get("outcome"),
        "error": step.get("error"),
    }
    row.update(values)
    return row


def _is_failure_step(step: dict[str, Any]) -> bool:
    outcome = step.get("outcome")
    if isinstance(outcome, str) and outcome.lower() == "error":
        return True
    return _has_non_empty_error(step.get("error"))


def _has_non_empty_error(value: Any) -> bool:
    if value is None:
        return False
    if isinstance(value, str):
        return bool(value.strip())
    return True


def _normalize_sum(value: int | float | None) -> int | float | None:
    if value is None:
        return None
    if isinstance(value, float) and value.is_integer():
        return int(value)
    return value


def _int_value(value: Any) -> int:
    if isinstance(value, int) and not isinstance(value, bool):
        return value
    return 0


def _is_range(value: Any) -> bool:
    if not isinstance(value, dict):
        return False
    return _is_number(value.get("min")) and _is_number(value.get("max"))


def _is_number(value: Any) -> bool:
    return (
        isinstance(value, (int, float))
        and not isinstance(value, bool)
        and math.isfinite(value)
    )
