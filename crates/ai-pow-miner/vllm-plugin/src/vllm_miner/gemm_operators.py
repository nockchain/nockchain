import torch
from miner_base.commitment_hash import CommitmentHasher
from miner_base.gpu_matmul_config import GPUMatmulConfigFactory
from miner_utils import get_logger
from pearl_gemm import (
    commitment_hash_from_merkle_roots,
    gemm,
    get_host_signal_sync_size,
    get_required_scratchpad_bytes,
    make_pow_target_tensor,
    noise_gen,
    noisy_gemm,
    tensor_hash,
)

from .callbacks import (
    StatusCheckCallback,
)
from .config import config
from .mining_state import (
    get_async_manager,
    get_pinned_pool,
)

_LOGGER = get_logger("vllm.pearl_miner")


def pearl_gemm_vanilla(
    A: torch.Tensor,
    B: torch.Tensor,
    scale_a: torch.Tensor,
    scale_b: torch.Tensor,
    out_dtype: torch.dtype,
    *,
    weight_transposed: bool = False,
) -> torch.Tensor:
    """
    Performs standard quantized matrix multiplication without mining operations.

    Computes C = A @ B.T using optimized CUDA kernels for int8 quantized inputs.

    :param A: Input matrix A (int8, quantized)
    :param B: Input matrix B, or its cached transpose when weight_transposed is true
    :param scale_a: Quantization scale factors for matrix A
    :param scale_b: Quantization scale factors for matrix B
    :param out_dtype: Output data type (bfloat16 or float16)
    :param weight_transposed: Whether B already has (K, N) layout
    :return: Result matrix C
    """
    assert out_dtype is torch.bfloat16 or out_dtype is torch.float16
    if torch.cuda.get_device_capability(A.device)[0] == 12:
        logical_m = A.shape[0]
        weight_for_mm = B if weight_transposed else B.T.contiguous()
        if logical_m <= 16:
            padded = torch.zeros((32, A.shape[1]), device=A.device, dtype=A.dtype)
            padded[:logical_m].copy_(A)
            accumulator = torch._int_mm(padded, weight_for_mm)[:logical_m]
        else:
            accumulator = torch._int_mm(A, weight_for_mm)
        return (
            accumulator.to(torch.float32)
            * scale_a.reshape(-1, 1)
            * scale_b.reshape(1, -1)
        ).to(out_dtype)

    A_scales = scale_a
    B_scales = scale_b

    C = torch.empty((A.shape[0], B.shape[0]), dtype=out_dtype, device=A.device)

    gemm(
        A=A,
        B=B,
        A_scales=A_scales,
        B_scales=B_scales,
        C=C,
        tile_size_m=config.settings.tile_size_m,
        tile_size_n=config.settings.tile_size_n,
        tile_size_k=config.settings.tile_size_k,
    )

    return C


def pearl_gemm_noisy(
    a: torch.Tensor,
    b: torch.Tensor,
    scale_a: torch.Tensor,
    scale_b: torch.Tensor,
    out_dtype: torch.dtype,
    layer: torch.nn.Module | None = None,
    submit_block: bool = True,
) -> torch.Tensor:
    """
    Performs quantized matrix multiplication with cryptographic noise for blockchain mining.

    Computes C = A @ B.T while generating proof-of-work hashes from intermediate computations
    using optimized CUDA kernels for noise generation and matrix operations.

    :param a: Input matrix A (int8, quantized)
    :param b: Input matrix B (int8, quantized)
    :param scale_a: Quantization scale factors for matrix A
    :param scale_b: Quantization scale factors for matrix B
    :param out_dtype: Output data type (bfloat16 or float16)
    :param layer: Layer containing weight tensors
    :param submit_block: Whether to submit mining results
    :return: Result matrix C
    """
    assert out_dtype is torch.bfloat16 or out_dtype is torch.float16

    m = a.shape[0]
    n = b.shape[0]
    k = a.shape[1]
    r = config.settings.noise_rank
    A = a
    B = b
    A_scales = scale_a
    B_scales = scale_b
    C = torch.empty((m, n), dtype=out_dtype, device=a.device)

    matrix_bytes = max(m * k, n * k)
    tensor_hash_scratchpad = torch.empty(
        get_required_scratchpad_bytes(matrix_bytes),
        dtype=torch.uint8,
        device=a.device,
    )
    matmul_config = GPUMatmulConfigFactory.create(k=k, noise_rank=r)

    # Get current mining job from shared state
    mining_job = get_async_manager().get_mining_job()

    # Calculate adjusted pow_target
    adjusted_target = mining_job.adjust_target(
        mining_config=matmul_config.mining_config
    )

    hash_key = CommitmentHasher.get_key(
        mining_job.incomplete_header_bytes, matmul_config.mining_config
    )

    key_tensor = torch.frombuffer(bytearray(hash_key), dtype=torch.uint8).to("cuda")

    A_tensor_hash = torch.empty(32, device="cuda", dtype=torch.uint8)
    tensor_hash(
        A.to(torch.uint8),
        key_tensor,
        A_tensor_hash,
        tensor_hash_scratchpad,
    )

    B_tensor_hash = torch.empty(32, device="cuda", dtype=torch.uint8)
    tensor_hash(
        B.to(torch.uint8),
        key_tensor,
        B_tensor_hash,
        tensor_hash_scratchpad,
    )

    # Generate commitment hashes for noise generation.
    commitment_hash_A_tensor = torch.empty(32, device="cuda", dtype=torch.uint8)
    commitment_hash_B_tensor = torch.empty(32, device="cuda", dtype=torch.uint8)
    commitment_hash_from_merkle_roots(
        A_tensor_hash,
        B_tensor_hash,
        key_tensor,
        commitment_hash_A_tensor,
        commitment_hash_B_tensor,
        salted_dims=(m, n) if mining_job.cert_version.uses_salted_seeds else None,
    )

    # Generate noise factors from commitment hashes
    (
        EAL,
        EAR_R_major,
        EBL_R_major,
        EAR_K_major,
        EBL_K_major,
        EBR,
        EAL_fp16,
        EBR_fp16,
    ) = generate_noise_factors(
        m,
        n,
        k,
        r,
        commitment_hash_A_tensor,
        commitment_hash_B_tensor,
        a.device,
    )

    # Always compute B noising (depends on A through EAR)
    BpEB = torch.empty((n, k), dtype=torch.int8, device=a.device)
    EARxBpEB = torch.empty((n, r), dtype=torch.float16, device=a.device)

    # Allocate A noising tensors (input-dependent)
    ApEA = torch.empty((m, k), dtype=torch.int8, device=a.device)
    A_E_BL = torch.empty((m, r), dtype=torch.float16, device=a.device)

    host_signal_sync_size = get_host_signal_sync_size()
    host_signal_sync = torch.zeros(
        (host_signal_sync_size,), dtype=torch.int8, device="cuda"
    )
    host_signal_header_pinned = get_pinned_pool().acquire()

    # Create pow_target tensor from adjusted_target
    pow_target_tensor = make_pow_target_tensor(adjusted_target)

    # Run noisy GEMM with default kernel configurations
    noisy_gemm(
        A=A,  # Input matrix A (m x k)
        B=B,  # Input matrix B (n x k)
        EAL=EAL,  # Noise factor E_AL (m x r)
        EAL_fp16=EAL_fp16,  # fp16 version
        EBR=EBR,  # Noise factor E_BR (n x r)
        EBR_fp16=EBR_fp16,  # fp16 version
        EAR_R_major=EAR_R_major,
        EBL_R_major=EBL_R_major,
        EAR_K_major=EAR_K_major,
        EBL_K_major=EBL_K_major,
        AxEBL_fp16=A_E_BL,  # Intermediate tensor A * E_BL (m x r)
        EARxBpEB_fp16=EARxBpEB,  # Output tensor for EAR * BpEB (n x r)
        ApEA=ApEA,  # Output tensor for A + EA (m x k)
        BpEB=BpEB,  # Output tensor for B + EB (n x k)
        A_scales=A_scales,  # Scale factors for A
        B_scales=B_scales,  # Scale factors for B
        C=C,  # Output matrix C (m x n)
        host_signal_header_pinned=host_signal_header_pinned,
        host_signal_sync=host_signal_sync,
        pow_target=pow_target_tensor,
        pow_key=commitment_hash_A_tensor.view(torch.uint32),
        tile_size_m=config.settings.tile_size_m,
        tile_size_n=config.settings.tile_size_n,
        tile_size_k=config.settings.tile_size_k,
        run_noising_A=True,  # run_noising_A
        run_noising_B=True,  # run_noising_B
        skip_reduction=False,  # skip_reduction
        skip_denoising=False,  # skip_denoising
    )

    if submit_block:
        # Record a CUDA event after the kernel launch - will complete when kernel finishes
        cuda_event = torch.cuda.Event()
        cuda_event.record()

        # Create callback for processing the status check
        callback = StatusCheckCallback(
            host_signal_header_pinned=host_signal_header_pinned,
            commitment_hash_A_tensor=commitment_hash_A_tensor,
            commitment_hash_B_tensor=commitment_hash_B_tensor,
            A=A,
            B=B,
            mining_job=mining_job,
        )

        get_async_manager().schedule_status_check(cuda_event, callback)

        # Callback owns these tensors
        host_signal_header_pinned = None
        commitment_hash_A_tensor = None
        commitment_hash_B_tensor = None
    else:
        get_pinned_pool().release(host_signal_header_pinned)
        del host_signal_header_pinned
        del commitment_hash_A_tensor
        del commitment_hash_B_tensor

    del pow_target_tensor
    del ApEA
    del BpEB
    del A_E_BL
    del EAL
    del EBR
    del EAR_R_major
    del EBL_R_major
    del EAR_K_major
    del EBL_K_major
    del EAL_fp16
    del EBR_fp16
    del key_tensor
    del A_tensor_hash
    del B_tensor_hash
    del tensor_hash_scratchpad
    del host_signal_sync
    del EARxBpEB
    return C


def generate_noise_factors(
    m: int,
    n: int,
    k: int,
    r: int,
    commitment_hash_A: torch.Tensor,
    commitment_hash_B: torch.Tensor,
    device: torch.device,
) -> tuple[
    torch.Tensor,
    torch.Tensor,
    torch.Tensor,
    torch.Tensor,
    torch.Tensor,
    torch.Tensor,
    torch.Tensor,
    torch.Tensor,
]:
    """
    Generates cryptographic noise factors for noisy GEMM operations.

    Creates low-rank noise matrices using commitment hashes as seeds for deterministic
    noise generation in blockchain mining operations using optimized CUDA kernels.

    :param m: Number of rows in matrix A
    :param n: Number of rows in matrix B
    :param k: Number of columns in matrices A and B
    :param r: Rank of the noise matrices
    :param commitment_hash_A: Cryptographic hash for matrix A noise generation
    :param commitment_hash_B: Cryptographic hash for matrix B noise generation
    :param device: CUDA device for tensor allocation
    :return: Tuple of noise tensors (EAL, EAR_R_major, EBL_R_major,
             EAR_K_major, EBL_K_major, EBR, EAL_fp16, EBR_fp16)
    """
    EAL = torch.empty((m, r), dtype=torch.int8, device=device)
    EBR = torch.empty((n, r), dtype=torch.int8, device=device)

    EAR_R_major = torch.empty((k, r), dtype=torch.int8, device=device)
    EBL_R_major = torch.empty((k, r), dtype=torch.int8, device=device)
    EAR_K_major = torch.empty((r, k), dtype=torch.int8, device=device)
    EBL_K_major = torch.empty((r, k), dtype=torch.int8, device=device)
    EAL_fp16 = torch.empty((m, r), dtype=torch.float16, device=device)
    EBR_fp16 = torch.empty((n, r), dtype=torch.float16, device=device)

    noise_gen(
        R=r,
        EAL=EAL,
        EAL_fp16=EAL_fp16,
        EAR_R_major=EAR_R_major,
        EAR_K_major=EAR_K_major,
        EBL_R_major=EBL_R_major,
        EBL_K_major=EBL_K_major,
        EBR=EBR,
        EBR_fp16=EBR_fp16,
        key_A=commitment_hash_A,
        key_B=commitment_hash_B,
    )
    return (
        EAL,
        EAR_R_major,
        EBL_R_major,
        EAR_K_major,
        EBL_K_major,
        EBR,
        EAL_fp16,
        EBR_fp16,
    )
