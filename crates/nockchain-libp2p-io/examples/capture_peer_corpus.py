#!/usr/bin/env python3
"""Capture full consensus nouns using read-only peeks on an operator's node.

The JSONL output is input to the extract_peer_corpus Rust example. It contains
only requested pages/transactions, not event jobs or node configuration. This
requires access to the node's private gRPC listener; do not expose that listener
to obtain fixtures. With --ssh, grpcurl runs on the remote host.
"""

import argparse
import base64
import datetime
import json
from pathlib import Path
import shlex
import subprocess


def text_atom(value):
    return int.from_bytes(value.encode("ascii"), "little")


def jam_path(tag, value):
    """Encode the small path [tag value ~], using literal JAM nodes only.

    Backreferences are optional in JAM. This intentionally limited encoder is
    for operator-created peek paths, never for parsing received peer messages.
    """
    bits = []

    def emit(number, width):
        bits.extend((number >> i) & 1 for i in range(width))

    def visit(noun):
        if isinstance(noun, tuple):
            emit(1, 2)
            visit(noun[0])
            visit(noun[1])
            return
        emit(0, 1)
        if noun == 0:
            emit(1, 1)
            return
        size = noun.bit_length()
        width = size.bit_length()
        emit(0, width)
        emit(1, 1)
        emit(size, width - 1)
        emit(noun, size)

    visit((text_atom(tag), (value, 0)))
    output = bytearray((len(bits) + 7) // 8)
    for index, bit in enumerate(bits):
        output[index // 8] |= bit << (index % 8)
    return bytes(output)


def height_arg(value):
    height = int(value)
    if not 0 <= height < 2**64:
        raise argparse.ArgumentTypeError("height must fit u64")
    return height


def tx_id_arg(value):
    alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
    if not value or len(value) > 64 or any(c not in alphabet for c in value):
        raise argparse.ArgumentTypeError("transaction ID must be base58")
    return value


def peek(args, tag, value):
    command = [
        args.grpcurl,
        "-plaintext",
        "-max-time", "15",
        "-max-msg-sz", "8388608",
        "-d", "@",
        args.address,
        "nockchain.private.v1.NockAppService/Peek",
    ]
    if args.ssh:
        command = [
            "ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10",
            "--", args.ssh, shlex.join(command),
        ]
    request = json.dumps({
        "pid": 3,
        "path": base64.b64encode(jam_path(tag, value)).decode("ascii"),
    })
    result = subprocess.run(
        command, input=request.encode("ascii"), capture_output=True,
        timeout=25, check=True,
    )
    response = json.loads(result.stdout)
    if "data" not in response:
        raise RuntimeError(f"peek failed: {response}")
    return base64.b64decode(response["data"], validate=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ssh", help="SSH host on which to execute grpcurl")
    parser.add_argument("--grpcurl", default="grpcurl", help="grpcurl executable")
    parser.add_argument("--address", default="127.0.0.1:5555")
    parser.add_argument("--height", nargs="+", type=height_arg, default=[])
    parser.add_argument("--tx-id", nargs="+", type=tx_id_arg, default=[])
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--append", action="store_true")
    parser.add_argument(
        "--source-label",
        default=f"accepted-chain-{datetime.datetime.now(datetime.timezone.utc).date()}",
        help="provenance label; omit hostnames and private operational metadata",
    )
    args = parser.parse_args()
    if not args.height and not args.tx_id:
        parser.error("provide at least one --height or --tx-id")
    # Exclusive creation prevents accidentally replacing an earlier capture.
    with args.output.open("a" if args.append else "x", encoding="utf-8") as output:
        for kind, value in [
            *(("page", height) for height in args.height),
            *(("transaction", tx_id) for tx_id in args.tx_id),
        ]:
            tag = "heavy-n" if kind == "page" else "raw-transaction"
            path_value = value if kind == "page" else text_atom(value)
            payload = peek(args, tag, path_value)
            record = {
                "source_label": args.source_label,
                "payload_kind": kind,
                # NockAppService.Peek returns the kernel's double unit, not
                # the already-unwrapped page/transaction noun.
                "peek_result_jam_hex": payload.hex(),
            }
            if kind == "page":
                record["requested_height"] = value
            else:
                record["requested_id"] = value
            output.write(json.dumps(record, sort_keys=True) + "\n")
            output.flush()
            print(f"captured {kind} {value}: {len(payload)} bytes", flush=True)


if __name__ == "__main__":
    main()
