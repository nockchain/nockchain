from __future__ import annotations

import ctypes
import os
from dataclasses import dataclass
from typing import Self

import torch

_K = 5376
_RANK = 128
_TILE = 16
_NO_WINNER = (1 << 64) - 1
_DEFAULT_LIBRARY = "/usr/local/lib/libai_pow_gemma4.so"
_U8_32 = ctypes.c_uint8 * 32


class _PrepareResult(ctypes.Structure):
    _fields_ = [
        ("kappa", _U8_32),
        ("h_a", _U8_32),
        ("h_b", _U8_32),
        ("s_a", _U8_32),
        ("s_b", _U8_32),
        ("commitment_ms", ctypes.c_float),
        ("noise_ms", ctypes.c_float),
    ]


class _InferenceResult(ctypes.Structure):
    _fields_ = [
        ("winner_ordinal", ctypes.c_uint64),
        ("jackpot", _U8_32),
        ("kernel_ms", ctypes.c_float),
        ("output_ms", ctypes.c_float),
    ]


@dataclass(frozen=True)
class NativePreparation:
    kappa: bytes
    h_a: bytes
    h_b: bytes
    s_a: bytes
    s_b: bytes
    commitment_ms: float
    noise_ms: float


@dataclass(frozen=True)
class NativeInferenceResult:
    winner_ordinal: int | None
    jackpot: bytes
    kernel_ms: float
    output_ms: float


class _Library:
    def __init__(self) -> None:
        path = os.environ.get("NOCKCHAIN_GEMMA4_LIBRARY", _DEFAULT_LIBRARY)
        self.raw = ctypes.CDLL(path)
        self.raw.ai_pow_cuda_gemma4_source_session_create_device.argtypes = [
            ctypes.c_uint32,
            ctypes.c_uint32,
            ctypes.c_uint32,
            ctypes.c_uint32,
            ctypes.c_uint32,
            ctypes.c_uint32,
            ctypes.c_void_p,
            ctypes.c_void_p,
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_void_p),
        ]
        self.raw.ai_pow_cuda_gemma4_source_session_create_device.restype = ctypes.c_int
        self.raw.ai_pow_cuda_gemma4_session_bind_device.argtypes = [
            ctypes.c_void_p,
            ctypes.c_void_p,
            ctypes.c_void_p,
            ctypes.c_void_p,
        ]
        self.raw.ai_pow_cuda_gemma4_session_bind_device.restype = ctypes.c_int
        self.raw.ai_pow_cuda_gemma4_session_prepare.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_uint8),
            ctypes.POINTER(ctypes.c_uint8),
            ctypes.POINTER(_PrepareResult),
        ]
        self.raw.ai_pow_cuda_gemma4_session_prepare.restype = ctypes.c_int
        self.raw.ai_pow_cuda_gemma4_session_infer_device.argtypes = [
            ctypes.c_void_p,
            ctypes.c_uint32,
            ctypes.c_void_p,
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_uint8),
            ctypes.c_void_p,
            ctypes.POINTER(_InferenceResult),
        ]
        self.raw.ai_pow_cuda_gemma4_session_infer_device.restype = ctypes.c_int
        self.raw.ai_pow_cuda_gemma4_session_destroy.argtypes = [ctypes.c_void_p]
        self.raw.ai_pow_cuda_gemma4_session_destroy.restype = ctypes.c_int


_LIBRARY: _Library | None = None


def _library() -> _Library:
    global _LIBRARY
    if _LIBRARY is None:
        _LIBRARY = _Library()
    return _LIBRARY


def _fixed_bytes(value: bytes, size: int, field: str):
    if len(value) != size:
        raise ValueError(f"{field} must be {size} bytes, got {len(value)}")
    return (ctypes.c_uint8 * size).from_buffer_copy(value)


def _check_cuda_tensor(
    tensor: torch.Tensor,
    *,
    dtype: torch.dtype,
    dimensions: int,
    field: str,
) -> None:
    if not tensor.is_cuda:
        raise ValueError(f"{field} must be a CUDA tensor")
    if tensor.dtype is not dtype:
        raise ValueError(f"{field} must have dtype {dtype}, got {tensor.dtype}")
    if tensor.ndim != dimensions:
        raise ValueError(
            f"{field} must have {dimensions} dimensions, got {tensor.ndim}"
        )
    if not tensor.is_contiguous():
        raise ValueError(f"{field} must be contiguous")


class NativeGemma4Session:
    def __init__(self, a: torch.Tensor, b: torch.Tensor) -> None:
        _check_cuda_tensor(a, dtype=torch.int8, dimensions=2, field="a")
        _check_cuda_tensor(b, dtype=torch.int8, dimensions=2, field="b")
        if a.device != b.device:
            raise ValueError("a and b must use the same CUDA device")
        m, k = a.shape
        n, b_k = b.shape
        if k != _K or b_k != _K:
            raise ValueError(f"a and b must have common dimension {_K}")
        if m == 0 or m % 256 or n == 0 or n % 128:
            raise ValueError("native Gemma shape requires m%256==0 and n%128==0")
        device_index = a.device.index
        if device_index is None:
            device_index = torch.cuda.current_device()
        self._a = a
        self._b = b
        self._stream = torch.cuda.current_stream(device_index)
        self._raw = ctypes.c_void_p()
        status = _library().raw.ai_pow_cuda_gemma4_source_session_create_device(
            device_index,
            m,
            n,
            _K,
            _RANK,
            _TILE,
            ctypes.c_void_p(a.data_ptr()),
            ctypes.c_void_p(b.data_ptr()),
            ctypes.c_void_p(self._stream.cuda_stream),
            ctypes.byref(self._raw),
        )
        self._check("create native Gemma session", status)
        self.m = m
        self.n = n

    @staticmethod
    def _check(operation: str, status: int) -> None:
        if status:
            raise RuntimeError(f"{operation} failed with CUDA status {status}")

    def bind(self, a: torch.Tensor, b: torch.Tensor) -> None:
        _check_cuda_tensor(a, dtype=torch.int8, dimensions=2, field="a")
        _check_cuda_tensor(b, dtype=torch.int8, dimensions=2, field="b")
        if a.device != self._a.device or b.device != self._a.device:
            raise ValueError("rebound tensors must use the session CUDA device")
        if a.shape != (self.m, _K) or b.shape != (self.n, _K):
            raise ValueError("rebound tensors must preserve the native session shape")
        self._stream = torch.cuda.current_stream(a.device)
        status = _library().raw.ai_pow_cuda_gemma4_session_bind_device(
            self._raw,
            ctypes.c_void_p(a.data_ptr()),
            ctypes.c_void_p(b.data_ptr()),
            ctypes.c_void_p(self._stream.cuda_stream),
        )
        self._check("bind native Gemma operands", status)
        self._a = a
        self._b = b

    def prepare(self, sigma: bytes, mu: bytes) -> NativePreparation:
        sigma_array = _fixed_bytes(sigma, 76, "sigma")
        mu_array = _fixed_bytes(mu, 52, "mu")
        result = _PrepareResult()
        status = _library().raw.ai_pow_cuda_gemma4_session_prepare(
            self._raw, sigma_array, mu_array, ctypes.byref(result)
        )
        self._check("prepare native Gemma session", status)
        return NativePreparation(
            kappa=bytes(result.kappa),
            h_a=bytes(result.h_a),
            h_b=bytes(result.h_b),
            s_a=bytes(result.s_a),
            s_b=bytes(result.s_b),
            commitment_ms=float(result.commitment_ms),
            noise_ms=float(result.noise_ms),
        )

    def infer(
        self,
        *,
        logical_m: int,
        a_scales: torch.Tensor,
        b_scales: torch.Tensor,
        target: bytes,
        output: torch.Tensor,
    ) -> NativeInferenceResult:
        _check_cuda_tensor(
            a_scales, dtype=torch.float32, dimensions=1, field="a_scales"
        )
        _check_cuda_tensor(
            b_scales, dtype=torch.float32, dimensions=1, field="b_scales"
        )
        _check_cuda_tensor(output, dtype=torch.bfloat16, dimensions=2, field="output")
        if not 0 < logical_m <= self.m:
            raise ValueError(f"logical_m must be in 1..={self.m}")
        if a_scales.device != self._a.device or b_scales.device != self._a.device:
            raise ValueError("scale tensors must use the session CUDA device")
        if output.device != self._a.device:
            raise ValueError("output must use the session CUDA device")
        if a_scales.shape != (logical_m,) or b_scales.shape != (self.n,):
            raise ValueError("scale tensor shape does not match native Gemma operands")
        if output.shape != (logical_m, self.n):
            raise ValueError("output shape does not match logical_m and n")
        target_array = _fixed_bytes(target, 32, "target")
        result = _InferenceResult()
        status = _library().raw.ai_pow_cuda_gemma4_session_infer_device(
            self._raw,
            logical_m,
            ctypes.c_void_p(a_scales.data_ptr()),
            ctypes.c_void_p(b_scales.data_ptr()),
            target_array,
            ctypes.c_void_p(output.data_ptr()),
            ctypes.byref(result),
        )
        self._check("run native Gemma inference", status)
        return NativeInferenceResult(
            winner_ordinal=(
                None if result.winner_ordinal == _NO_WINNER else result.winner_ordinal
            ),
            jackpot=bytes(result.jackpot),
            kernel_ms=float(result.kernel_ms),
            output_ms=float(result.output_ms),
        )

    def close(self) -> None:
        if self._raw:
            status = _library().raw.ai_pow_cuda_gemma4_session_destroy(self._raw)
            self._raw = ctypes.c_void_p()
            self._check("destroy native Gemma session", status)

    def __enter__(self) -> Self:
        return self

    def __exit__(self, *_args) -> None:
        self.close()

    def __del__(self) -> None:
        raw = getattr(self, "_raw", None)
        if raw:
            _library().raw.ai_pow_cuda_gemma4_session_destroy(raw)
            self._raw = ctypes.c_void_p()


class NativeGemma4SessionCache:
    """One synchronized caller-owned native session slot."""

    def __init__(self) -> None:
        self._entry: tuple[tuple[int, int, int, int], NativeGemma4Session] | None = None

    def __len__(self) -> int:
        return int(self._entry is not None)

    def get(self, a: torch.Tensor, b: torch.Tensor) -> NativeGemma4Session:
        device_index = a.device.index
        if device_index is None:
            device_index = torch.cuda.current_device()
        key = (device_index, a.shape[0], b.shape[0], b.data_ptr())
        if self._entry is not None:
            cached_key, cached = self._entry
            if cached_key == key:
                cached.bind(a, b)
                return cached
            cached.close()
            self._entry = None
        session = NativeGemma4Session(a, b)
        self._entry = (key, session)
        return session

    def close(self) -> None:
        if self._entry is not None:
            _, session = self._entry
            session.close()
            self._entry = None
