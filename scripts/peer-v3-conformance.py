#!/usr/bin/env python3
"""Check peer v3 wire vectors with the independent Google protobuf runtime.

Install scripts/peer-v3-conformance-requirements.txt and protoc 36.2, then run
this script from any directory. Only the repository's fixed schemas are
compiled; the vector manifest cannot supply descriptors or generated code.
Known field values, explicit presence, and selected oneof members are compared
using protobuf's JSON representation. Protobuf serialization is not an oracle.
"""

import argparse
import importlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

import google.protobuf
from google.protobuf.json_format import MessageToDict
from google.protobuf.message import DecodeError


ROOT = Path(__file__).resolve().parents[1]
PROTO_ROOT = ROOT / "crates/nockchain-libp2p-io/proto"
VECTORS = (
    ROOT / "crates/nockchain-libp2p-io/tests/fixtures/peer_v3_wire/vectors.json"
)
PROTOC_VERSION = "libprotoc 36.2"
RUNTIME_VERSION = "7.36.2"
SCHEMAS = tuple(
    f"nockchain/peer/v3/{name}.proto"
    for name in ("common", "transaction", "page", "peer")
)


def json_value(value):
    # JSON distinguishes true from 1, unlike Python's ordinary value equality.
    return json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False)


def check_versions(protoc):
    if google.protobuf.__version__ != RUNTIME_VERSION:
        raise ValueError(
            f"expected protobuf {RUNTIME_VERSION}, got {google.protobuf.__version__}; "
            "install scripts/peer-v3-conformance-requirements.txt"
        )
    actual = subprocess.check_output([protoc, "--version"], text=True).strip()
    if actual != PROTOC_VERSION:
        raise ValueError(f"expected {PROTOC_VERSION}, got {actual}")


def check_vectors(manifest, classes):
    if manifest.get("version") != 1:
        raise ValueError("unsupported wire vector manifest version")
    vectors = manifest.get("vectors")
    if not isinstance(vectors, list) or not vectors:
        raise ValueError("wire vector manifest must contain vectors")
    seen = set()
    failures = []
    for vector in vectors:
        name = vector["id"]
        if not isinstance(name, str) or not name or name in seen:
            raise ValueError(f"invalid or duplicate vector id: {name!r}")
        seen.add(name)
        message_type = vector["message_type"]
        if message_type not in classes:
            raise ValueError(f"{name}: unsupported message type {message_type!r}")
        verdict = vector.get("protobuf_verdict", "accept")
        if verdict not in ("accept", "reject"):
            raise ValueError(f"{name}: unknown protobuf verdict {verdict!r}")
        wire = bytes.fromhex(vector["hex"])
        message = classes[message_type]()
        try:
            message.ParseFromString(wire)
        except DecodeError as error:
            if verdict != "reject":
                failures.append(f"{name}: unexpected protobuf rejection: {error}")
            continue
        if verdict == "reject":
            failures.append(f"{name}: expected protobuf rejection")
            continue
        actual = MessageToDict(message, preserving_proto_field_name=True)
        expected = vector["expected_semantics"]
        if json_value(actual) != json_value(expected):
            failures.append(
                f"{name}: known semantics differ\n"
                f"  expected {json_value(expected)}\n"
                f"  actual   {json_value(actual)}"
            )
    if failures:
        raise ValueError("\n".join(failures))
    return len(vectors)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--protoc", default=os.environ.get("PROTOC", "protoc"))
    parser.add_argument("--vectors", type=Path, default=VECTORS)
    args = parser.parse_args()
    check_versions(args.protoc)
    manifest = json.loads(args.vectors.read_text(encoding="utf-8"))
    with tempfile.TemporaryDirectory(prefix="peer-v3-python-") as output:
        subprocess.run(
            [
                args.protoc,
                f"--proto_path={PROTO_ROOT}",
                f"--python_out={output}",
                *SCHEMAS,
            ],
            check=True,
        )
        sys.path.insert(0, output)
        generated = importlib.import_module("nockchain.peer.v3.peer_pb2")
        classes = {
            "nockchain.peer.v3.PeerRequest": generated.PeerRequest,
            "nockchain.peer.v3.PeerResponse": generated.PeerResponse,
        }
        tested = check_vectors(manifest, classes)
    print(
        f"Google protobuf {RUNTIME_VERSION}, {PROTOC_VERSION}: "
        f"{tested} wire vectors passed (known semantics and presence)"
    )


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as error:
        print(f"peer v3 conformance failed: {error}", file=sys.stderr)
        sys.exit(1)
