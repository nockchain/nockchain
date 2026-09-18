#!/usr/bin/env python3
"""Generate Python inference-mining bindings from the canonical Rust schema."""

from __future__ import annotations

import argparse
from pathlib import Path

from grpc_tools import protoc


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("schema", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    schema = args.schema.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    status = protoc.main(
        [
            "grpc_tools.protoc",
            f"-I{schema.parent}",
            f"--python_out={output}",
            f"--grpc_python_out={output}",
            str(schema),
        ]
    )
    if status != 0:
        raise SystemExit(status)

    grpc_output = output / f"{schema.stem}_pb2_grpc.py"
    generated = grpc_output.read_text()
    absolute_import = (
        f"import {schema.stem}_pb2 as {schema.stem.replace('_', '__')}__pb2"
    )
    relative_import = (
        f"from . import {schema.stem}_pb2 as {schema.stem.replace('_', '__')}__pb2"
    )
    if absolute_import not in generated:
        raise RuntimeError("generated gRPC binding has an unexpected protobuf import")
    grpc_output.write_text(generated.replace(absolute_import, relative_import, 1))


if __name__ == "__main__":
    main()
