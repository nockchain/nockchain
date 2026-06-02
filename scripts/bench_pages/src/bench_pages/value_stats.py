from __future__ import annotations

from typing import Any


def is_value_stats(value: Any) -> bool:
    return isinstance(value, dict) and {
        "median",
        "min",
        "max",
        "mad",
        "stddev",
        "cv",
        "values",
    }.issubset(value.keys())


def is_number(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def stats_scalar(value: Any) -> float | int | None:
    if is_value_stats(value):
        value = value.get("median")
    if is_number(value):
        return value
    return None


def stats_max(value: Any) -> float | int | None:
    if is_value_stats(value):
        value = value.get("max")
    if is_number(value):
        return value
    return None
