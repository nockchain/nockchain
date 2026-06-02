from __future__ import annotations

from pathlib import Path


def is_publish_artifact_path(relative_path: Path) -> bool:
    parts = relative_path.parts
    return not (
        len(parts) >= 5
        and parts[0] == "cases"
        and parts[2] == "runs"
        and parts[4] == "work"
    )
