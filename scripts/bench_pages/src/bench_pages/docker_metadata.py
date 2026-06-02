from __future__ import annotations

from typing import Any

from bench_pages.models import SweepCase


def docker_payload(raw_value: Any) -> dict[str, Any]:
    if not isinstance(raw_value, dict):
        return {}

    for key in ("Docker", "docker"):
        candidate = raw_value.get(key)
        if isinstance(candidate, dict):
            return candidate
    return {}


def string_or_none(value: Any) -> str | None:
    if value is None:
        return None
    return str(value)


def case_docker_image_metadata(case: SweepCase) -> tuple[str | None, str | None, str]:
    provenance = docker_payload(_mapping(case.provenance).get("backend"))
    requested = docker_payload(_mapping(case.requested_case).get("execution"))
    resolved = _mapping(case.resolved_case).get("docker", {})
    resolved_image = resolved.get("image", {}) if isinstance(resolved, dict) else {}

    digest = string_or_none(provenance.get("image_digest"))
    local_ref = _first_string(
        provenance.get("requested_image_ref"),
        provenance.get("image_tag"),
        _nested_value(provenance, "image_source", "auto_build", "tag"),
        _nested_value(requested, "image", "auto_build", "tag"),
        requested.get("image_tag"),
        resolved_image.get("requested_ref"),
        _nested_value(resolved_image, "source", "auto_build", "tag"),
        resolved.get("image_tag"),
    )
    canonical_identity = digest or local_ref or case.case_id
    return digest, local_ref, canonical_identity


def _nested_value(raw_value: Any, *path: str) -> Any:
    current = raw_value
    for key in path:
        if not isinstance(current, dict):
            return None
        current = current.get(key)
    return current


def _first_string(*values: Any) -> str | None:
    for value in values:
        normalized = string_or_none(value)
        if normalized:
            return normalized
    return None


def _mapping(value: Any) -> dict[str, Any]:
    if isinstance(value, dict):
        return value
    return {}
