from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any


@dataclass(frozen=True)
class ArtifactRecord:
    relative_path: str
    size_bytes: int


@dataclass
class SweepRun:
    run_id: str
    root: Path
    result: dict[str, Any] | None
    raw_tx_replay: dict[str, Any] = field(default_factory=dict)
    artifacts: list[ArtifactRecord] = field(default_factory=list)


@dataclass
class DockerImageRecord:
    canonical_identity: str | None
    local_image_ref: str | None
    provenance_image_digest: str | None
    local_image_id: str | None = None
    local_image_size_bytes: int | None = None
    ghcr_tag: str | None = None
    ghcr_ref: str | None = None
    ghcr_package_url: str | None = None
    ghcr_digest: str | None = None
    publish_status: str | None = None


@dataclass
class SweepCase:
    case_id: str
    root: Path
    execution_mode: str
    axis_assignments: dict[str, Any]
    requested_case: dict[str, Any] | None
    resolved_case: dict[str, Any] | None
    summary: dict[str, Any] | None
    trusted_plan: dict[str, Any] | None
    verdict: dict[str, Any] | None
    provenance: dict[str, Any] | None
    materialized: bool = True
    completion_state: str = "complete"
    missing_artifacts: list[str] = field(default_factory=list)
    cpu_profile: dict[str, Any] | None = None
    comparison_case: dict[str, Any] | None = None
    validation: dict[str, Any] | None = None
    runs: list[SweepRun] = field(default_factory=list)
    artifacts: list[ArtifactRecord] = field(default_factory=list)


@dataclass
class SweepData:
    root: Path
    execution_mode: str
    schema_version: str | None
    matrix: dict[str, Any]
    matrix_expanded: list[dict[str, Any]]
    schedule: dict[str, Any]
    comparison: dict[str, Any] | None
    verdict: dict[str, Any] | None
    cases: list[SweepCase]
    artifact_inventory: list[ArtifactRecord]
    top_level_artifacts: list[ArtifactRecord]
    completion_state: str = "complete"
    missing_top_level_artifacts: list[str] = field(default_factory=list)
    docker_images: list[DockerImageRecord] = field(default_factory=list)
