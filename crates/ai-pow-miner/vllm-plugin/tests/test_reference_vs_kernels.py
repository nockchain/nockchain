"""
Test comparing Python reference implementations with CUDA kernel implementations.

This module tests that the Python reference implementations produce the same results
as their corresponding CUDA kernel implementations.
"""

import secrets

import numpy as np
import pearl_gemm
import pytest
import torch
from blake3 import blake3
from miner_base.commitment_hash import CommitmentHasher
from miner_base.inner_hash import hash_tile
from miner_base.matrix_merkle_tree import MatrixMerkleTree
from miner_base.noise_generation import NoiseGenerator
from pearl_gemm.test_components import inner_hash as inner_hash_cuda

# Arbitrary (m, n) for the V3 salted derivation; any positive pair exercises
# the same path.
SALTED_DIMS = (192, 320)


def _to_cuda_u8(data: bytes) -> torch.Tensor:
    return torch.frombuffer(bytearray(data), dtype=torch.uint8).cuda()


def _offsets_hash_cuda(routing_offsets: list[int], key: bytes) -> bytes:
    """Keyed ``tensor_hash`` of the routing offsets.

    Mirrors the miner's GPU offsets leaf so the test exercises the same path.
    ``tensor_hash`` applies the BLAKE3 chunk padding internally. ``top_k`` is no
    longer part of this leaf (it lives in the mining config / job key).
    """
    payload = torch.tensor(routing_offsets, dtype=torch.int32, device="cuda")
    offsets_bytes = payload.view(torch.uint8)
    key_tensor = _to_cuda_u8(key)
    offsets_hash = torch.empty(blake3.digest_size, dtype=torch.uint8, device="cuda")
    scratchpad = torch.empty(
        pearl_gemm.get_required_scratchpad_bytes(offsets_bytes.numel()),
        dtype=torch.uint8,
        device="cuda",
    )
    pearl_gemm.tensor_hash(offsets_bytes.reshape(1, -1), key_tensor, offsets_hash, scratchpad)
    torch.cuda.synchronize()
    return offsets_hash.cpu().numpy().tobytes()


@pytest.fixture
def test_noise_seed_A() -> bytes:
    """Generate a 32-byte random key for BLAKE3 keyed hashing."""
    return secrets.token_bytes(blake3.digest_size)


@pytest.fixture
def test_noise_seed_B() -> bytes:
    """Generate a 32-byte random key for BLAKE3 keyed hashing."""
    return secrets.token_bytes(blake3.digest_size)


class TestMatrixMerkleTreeVsTensorHash:
    """
    Test that MatrixMerkleTree root hash matches tensor_hash CUDA implementation.
    """

    @pytest.mark.parametrize(
        "m, n",
        [
            (512, 1024),
            (1024, 512),
            (512, 512),
            (1024, 1024),
            (1500, 2048),
            (1600, 2048),
        ],
    )
    def test_matrix_merkle_tree_vs_tensor_hash(self, test_noise_seed_A, m, n):
        """Test that MatrixMerkleTree root matches tensor_hash for various shapes."""
        # Create random int8 tensor for MatrixMerkleTree
        matrix_int8 = torch.randint(-128, 127, (m, n), dtype=torch.int8)

        # Convert to uint8 for tensor_hash (CUDA implementation expects uint8)
        matrix_uint8 = matrix_int8.to(torch.uint8).cuda()

        # Create key tensor for CUDA implementation
        key_tensor = _to_cuda_u8(test_noise_seed_A)

        # Compute hash using MatrixMerkleTree (Python reference)
        merkle_tree = MatrixMerkleTree(matrix_int8, test_noise_seed_A)
        python_root = merkle_tree.root

        # Compute hash using tensor_hash (CUDA implementation)
        cuda_result = torch.empty(blake3.digest_size, device="cuda", dtype=torch.uint8)
        tensor_hash_scratchpad = torch.empty(
            pearl_gemm.get_required_scratchpad_bytes(matrix_uint8.numel()),
            device="cuda",
            dtype=torch.uint8,
        )
        pearl_gemm.tensor_hash(matrix_uint8, key_tensor, cuda_result, tensor_hash_scratchpad)
        torch.cuda.synchronize()

        # Convert CUDA result back to bytes for comparison
        cuda_root = cuda_result.cpu().numpy().tobytes()

        blake3_result = blake3(matrix_int8.cpu().numpy().tobytes(), key=test_noise_seed_A).digest()

        assert python_root == blake3_result, (
            f"Hash mismatch for shape {m, n}: MatrixMerkleTree root doesn't match blake3 result"
        )

        # Compare results
        assert python_root == cuda_root, (
            f"Hash mismatch for shape {m, n}: MatrixMerkleTree root doesn't match tensor_hash result"
        )

    @pytest.mark.parametrize("salted_dims", [None, SALTED_DIMS])
    def test_commitment_hash_reference_vs_cuda(self, test_noise_seed_A, salted_dims):
        """Test that Python reference commitment hash matches CUDA implementation."""
        # Generate random 32-byte merkle roots directly
        A_merkle_root = secrets.token_bytes(blake3.digest_size)
        B_merkle_root = secrets.token_bytes(blake3.digest_size)

        # Convert merkle roots to CUDA tensors
        A_merkle_root_tensor = _to_cuda_u8(A_merkle_root)
        B_merkle_root_tensor = _to_cuda_u8(B_merkle_root)
        key_tensor = _to_cuda_u8(test_noise_seed_A)

        # Compute commitment hash using Python reference
        python_commitment_hash = CommitmentHasher.commitment_hash_from_merkle_roots(
            A_merkle_root,
            B_merkle_root,
            test_noise_seed_A,
            salted_dims=salted_dims,
        )

        # Compute commitment hash using CUDA implementation
        cuda_commitment_A_tensor = torch.empty(blake3.digest_size, device="cuda", dtype=torch.uint8)
        cuda_commitment_B_tensor = torch.empty(blake3.digest_size, device="cuda", dtype=torch.uint8)
        pearl_gemm.commitment_hash_from_merkle_roots(
            A_merkle_root_tensor,
            B_merkle_root_tensor,
            key_tensor,
            cuda_commitment_A_tensor,
            cuda_commitment_B_tensor,
            salted_dims=salted_dims,
        )
        torch.cuda.synchronize()

        # Convert CUDA result to bytes for comparison
        cuda_commitment_A = cuda_commitment_A_tensor.cpu().numpy().tobytes()
        cuda_commitment_B = cuda_commitment_B_tensor.cpu().numpy().tobytes()

        # Compare results
        assert python_commitment_hash.noise_seed_A == cuda_commitment_A, (
            "Commitment hash mismatch: Python reference doesn't match CUDA implementation"
        )
        assert python_commitment_hash.noise_seed_B == cuda_commitment_B, (
            "Commitment hash mismatch: Python reference doesn't match CUDA implementation"
        )

    @pytest.mark.parametrize("salted_dims", [None, SALTED_DIMS])
    def test_commitment_hash_moe_reference_vs_cuda(self, test_noise_seed_A, salted_dims):
        """MoE folding: kernel routing_root/offsets_hash path matches CommitmentHasher."""
        # Cumulative exclusive ends per expert; last == m * top_k.
        routing_offsets = [3, 5, 9, 12]

        A_merkle_root = secrets.token_bytes(blake3.digest_size)
        B_merkle_root = secrets.token_bytes(blake3.digest_size)
        routing_root = secrets.token_bytes(blake3.digest_size)

        # Full GPU path: the offsets leaf is a keyed tensor_hash, like the miner.
        # The same offsets root feeds both the reference and the kernel.
        offsets_root = _offsets_hash_cuda(routing_offsets, test_noise_seed_A)

        # Python reference folds routing into A's seed.
        python_commitment_hash = CommitmentHasher.commitment_hash_from_merkle_roots(
            A_merkle_root,
            B_merkle_root,
            test_noise_seed_A,
            routing_root=routing_root,
            offsets_root=offsets_root,
            salted_dims=salted_dims,
        )

        cuda_commitment_A_tensor = torch.empty(blake3.digest_size, device="cuda", dtype=torch.uint8)
        cuda_commitment_B_tensor = torch.empty(blake3.digest_size, device="cuda", dtype=torch.uint8)
        pearl_gemm.commitment_hash_from_merkle_roots(
            _to_cuda_u8(A_merkle_root),
            _to_cuda_u8(B_merkle_root),
            _to_cuda_u8(test_noise_seed_A),
            cuda_commitment_A_tensor,
            cuda_commitment_B_tensor,
            routing_root=_to_cuda_u8(routing_root),
            offsets_hash=_to_cuda_u8(offsets_root),
            salted_dims=salted_dims,
        )
        torch.cuda.synchronize()

        cuda_commitment_A = cuda_commitment_A_tensor.cpu().numpy().tobytes()
        cuda_commitment_B = cuda_commitment_B_tensor.cpu().numpy().tobytes()

        assert python_commitment_hash.noise_seed_A == cuda_commitment_A, (
            "MoE commitment hash mismatch: Python reference doesn't match CUDA implementation"
        )
        assert python_commitment_hash.noise_seed_B == cuda_commitment_B, (
            "MoE commitment hash mismatch: Python reference doesn't match CUDA implementation"
        )

    def test_offsets_hash_via_tensor_hash(self, test_noise_seed_A):
        """GPU keyed tensor_hash of the offsets buffer matches the CommitmentHasher reference."""
        # Cumulative exclusive ends per expert; last == m * top_k.
        routing_offsets = [7, 7, 19, 32]

        gpu_offsets_hash = _offsets_hash_cuda(routing_offsets, test_noise_seed_A)
        reference = CommitmentHasher.get_offsets_hash(routing_offsets, test_noise_seed_A)

        assert gpu_offsets_hash == reference, (
            "Offsets hash mismatch: GPU tensor_hash doesn't match CommitmentHasher reference"
        )


class TestNoiseGeneration:
    """
    Test that noise generation produces the same results as the Python reference implementation.
    """

    @pytest.mark.parametrize(
        "m, n, k",
        [
            (512, 512, 256),  # Medium square matrix
            (128, 128, 128),  # Small square matrix
            (1024, 2048, 4096),  # Small square matrix
            (1500, 2000, 2048),  # Non-power-of-2 dimensions
        ],
    )
    def test_noise_generation(self, test_noise_seed_A, test_noise_seed_B, m, n, k):
        """Test that noise generation produces the same results as the Python reference implementation."""
        noise_rank = 128
        noise_range = 128
        noise_generator = NoiseGenerator(noise_rank=noise_rank, noise_range=noise_range)
        ref_AL, ref_AR, ref_BL, ref_BR = noise_generator.generate_noise_metrices(
            key_A=test_noise_seed_A, key_B=test_noise_seed_B, A_rows=m, common_dim=k, B_cols=n
        )
        noise_seed_A_tensor = torch.frombuffer(
            bytearray(test_noise_seed_A), dtype=torch.uint8
        ).cuda()
        noise_seed_B_tensor = torch.frombuffer(
            bytearray(test_noise_seed_B), dtype=torch.uint8
        ).cuda()
        EAR_R_major = torch.empty(size=(k, noise_rank), dtype=torch.int8, device="cuda")
        EBL_R_major = torch.empty(size=(k, noise_rank), dtype=torch.int8, device="cuda")
        EAR_K_major = torch.empty(size=(noise_rank, k), dtype=torch.int8, device="cuda")
        EBL_K_major = torch.empty(size=(noise_rank, k), dtype=torch.int8, device="cuda")
        EBR = torch.empty(size=(n, noise_rank), dtype=torch.int8, device="cuda")
        EAL = torch.empty(size=(m, noise_rank), dtype=torch.int8, device="cuda")
        pearl_gemm.noise_gen(
            R=noise_rank,
            key_A=noise_seed_A_tensor,
            key_B=noise_seed_B_tensor,
            EAL=EAL,
            EAR_R_major=EAR_R_major,
            EAR_K_major=EAR_K_major,
            EBL_R_major=EBL_R_major,
            EBL_K_major=EBL_K_major,
            EBR=EBR,
        )
        torch.cuda.synchronize()

        EAR = EAR_K_major.cpu()  # (R, k) = transposed
        EBL = EBL_R_major.cpu()  # (k, R) = not transposed

        assert torch.all(ref_AL == EAL.cpu())
        assert torch.all(ref_AR == EAR.cpu())
        assert torch.all(ref_BL == EBL.cpu())
        assert torch.all(ref_BR == EBR.cpu().T)


class TestInnerHash:
    """Test class for inner_hash function comparing CUDA implementation with Python reference."""

    def setUp(self):
        """Set up test fixtures."""
        torch.manual_seed(0)
        np.random.seed(0)

    def convert_for_reference(self, tensor_uint32: torch.Tensor) -> torch.Tensor:
        """Convert uint32 CUDA tensor to int32 CPU tensor for reference implementation."""
        # Convert to int32 for reference (Python reference expects int32)
        return tensor_uint32.cpu().to(torch.int32)

    def convert_for_cuda(self, tensor_int32: torch.Tensor) -> torch.Tensor:
        """Convert int32 CPU tensor to uint32 CUDA tensor for CUDA implementation."""
        # Convert to uint32 for CUDA (CUDA implementation expects uint32)
        return tensor_int32.to(torch.uint32).cuda()

    def create_test_tensors(self):
        """Create 3 different test tensors for testing."""
        # Test tensor 1: Sequential values (0, 1, 2, ..., 255)
        tensor1_int32 = torch.arange(256, dtype=torch.int32)
        tensor1_uint32 = self.convert_for_cuda(tensor1_int32)

        # Test tensor 2: Random values
        torch.manual_seed(42)  # For reproducibility
        tensor2_int32 = torch.randint(
            0, 2**30, (256,), dtype=torch.int32
        )  # Use smaller range to avoid overflow
        tensor2_uint32 = self.convert_for_cuda(tensor2_int32)

        # Test tensor 3: Mixed pattern with some structure
        tensor3_int32 = torch.zeros(256, dtype=torch.int32)
        # Fill with a pattern: alternating values, some zeros, some large values
        for i in range(256):
            if i % 3 == 0:
                tensor3_int32[i] = i * 1000
            elif i % 3 == 1:
                tensor3_int32[i] = 0
            else:
                tensor3_int32[i] = (i * 37) % 65536
        tensor3_uint32 = self.convert_for_cuda(tensor3_int32)

        return [
            (tensor1_uint32, tensor1_int32, "sequential"),
            (tensor2_uint32, tensor2_int32, "random"),
            (tensor3_uint32, tensor3_int32, "mixed_pattern"),
        ]

    @pytest.mark.parametrize(
        "tensor_idx,test_name", [(0, "sequential"), (1, "random"), (2, "mixed_pattern")]
    )
    @pytest.mark.parametrize("size", [64, 96, 128, 192, 256])
    def test_inner_hash_cuda_vs_reference(self, tensor_idx, test_name, size):
        """Test that CUDA implementation matches Python reference implementation."""
        if not torch.cuda.is_available():
            pytest.skip("CUDA not available")

        # Get test tensors
        test_tensors = self.create_test_tensors()
        tensor_uint32, tensor_int32, name = test_tensors[tensor_idx]

        # Run CUDA implementation
        cuda_result = inner_hash_cuda(tensor_uint32[:size])

        # Run Python reference implementation
        # hash_tile returns InnerHashResult with hash as np.uint32
        reference_result = hash_tile(tensor_int32[:size].view(16, -1)).hash

        # Check that results match
        assert cuda_result.cpu().numpy() == reference_result, (
            f"CUDA and reference results don't match for {test_name} tensor"
        )

        # Additional checks on output properties
        assert cuda_result.shape == (1,), (
            f"CUDA result should have shape (1,), got {cuda_result.shape}"
        )
        assert cuda_result.dtype == torch.uint32, (
            f"CUDA result should have dtype uint32, got {cuda_result.dtype}"
        )
        assert cuda_result.device.type == "cuda", (
            f"CUDA result should be on CUDA device, got {cuda_result.device.type}"
        )
