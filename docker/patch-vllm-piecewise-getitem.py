#!/usr/bin/env python3
"""Patch vLLM 0.27.1 piecewise codegen to preserve general tensor indexes."""

from __future__ import annotations

import importlib.util
from pathlib import Path

_OLD = """            if node.target is operator.getitem:
                source = ref(node.args[0])
                index = node.args[1]
                assert isinstance(index, int)
                lines.append(f\"    {node.name} = {source}[{index}]\")
"""
_NEW = """            if node.target is operator.getitem:
                source = ref(node.args[0])
                index = ref(node.args[1])
                lines.append(f\"    {node.name} = {source}[{index}]\")
"""


def main() -> int:
    spec = importlib.util.find_spec("vllm.compilation.codegen")
    if spec is None or spec.origin is None:
        raise RuntimeError("vLLM compilation codegen module was not found")
    source_path = Path(spec.origin)
    source = source_path.read_text()
    if source.count(_OLD) != 1:
        raise RuntimeError(
            f"unexpected vLLM codegen source at {source_path}; pinned patch did not apply"
        )
    source_path.write_text(source.replace(_OLD, _NEW))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
