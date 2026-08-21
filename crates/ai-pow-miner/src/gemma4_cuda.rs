//! Hopper/Blackwell CUDA session for native Gemma 4 fused gate/up mining.
//!
//! The session owns one immutable source-matrix generation and every
//! candidate-bound CUDA allocation. The no-hit search path performs no allocation.

use std::ffi::{c_int, c_void};

use ai_pow::matmul::TileState;
use ai_pow::pearl_compat::PearlWorkCommitments;
use anyhow::{bail, Result};

use crate::gemma4::GEMMA4_NATIVE_PARAMS;

pub const GEMMA4_K: usize = GEMMA4_NATIVE_PARAMS.k as usize;
pub const GEMMA4_RANK: usize = GEMMA4_NATIVE_PARAMS.noise_rank as usize;
pub const GEMMA4_TILE: usize = GEMMA4_NATIVE_PARAMS.tile as usize;
pub const GEMMA4_M: usize = GEMMA4_NATIVE_PARAMS.m as usize;
pub const GEMMA4_N: usize = GEMMA4_NATIVE_PARAMS.n as usize;
const NO_WINNER: u64 = u64::MAX;

#[repr(C)]
struct FfiSearchResult {
    winner_ordinal: u64,
    jackpot: [u8; 32],
    kernel_ms: f32,
}

#[repr(C)]
struct FfiPrepareResult {
    kappa: [u8; 32],
    h_a: [u8; 32],
    h_b: [u8; 32],
    s_a: [u8; 32],
    s_b: [u8; 32],
    commitment_ms: f32,
    noise_ms: f32,
}

#[repr(C)]
struct FfiKernelInfo {
    sm_count: u32,
    threads_per_cta: u32,
    active_ctas_per_sm: u32,
    registers_per_thread: u32,
    static_shared_bytes: u64,
    dynamic_shared_bytes: u64,
}

unsafe extern "C" {
    fn ai_pow_cuda_gemma4_kernel_info(device_ordinal: u32, info_out: *mut FfiKernelInfo) -> c_int;
    fn ai_pow_cuda_gemma4_session_create(
        device_ordinal: u32,
        m: u32,
        n: u32,
        k: u32,
        rank: u32,
        tile: u32,
        a_prime: *const i8,
        b_prime: *const i8,
        pow_key: *const u8,
        session_out: *mut *mut c_void,
    ) -> c_int;
    fn ai_pow_cuda_gemma4_source_session_create(
        device_ordinal: u32,
        m: u32,
        n: u32,
        k: u32,
        rank: u32,
        tile: u32,
        a: *const i8,
        b: *const i8,
        session_out: *mut *mut c_void,
    ) -> c_int;
    fn ai_pow_cuda_gemma4_session_prepare(
        session: *mut c_void,
        sigma: *const u8,
        mu: *const u8,
        result_out: *mut FfiPrepareResult,
    ) -> c_int;
    fn ai_pow_cuda_gemma4_session_search(
        session: *mut c_void,
        ordinal_start: u64,
        ordinal_count: u64,
        target: *const u8,
        result_out: *mut FfiSearchResult,
    ) -> c_int;
    fn ai_pow_cuda_gemma4_session_debug(
        session: *mut c_void,
        ordinal: u64,
        state_out: *mut i32,
        jackpot_out: *mut u8,
    ) -> c_int;
    fn ai_pow_cuda_gemma4_session_destroy(session: *mut c_void) -> c_int;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gemma4SearchResult {
    pub winner: Option<u64>,
    pub jackpot: [u8; 32],
    pub kernel_ms: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gemma4DebugResult {
    pub state: TileState,
    pub jackpot: [u8; 32],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gemma4Preparation {
    pub commitments: PearlWorkCommitments,
    pub commitment_ms: f32,
    pub noise_ms: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gemma4KernelInfo {
    pub sm_count: u32,
    pub threads_per_cta: u32,
    pub active_ctas_per_sm: u32,
    pub registers_per_thread: u32,
    pub static_shared_bytes: u64,
    pub dynamic_shared_bytes: u64,
}

pub struct Gemma4CudaSession {
    raw: *mut c_void,
    m: usize,
    n: usize,
    total_tickets: u64,
}

// One worker owns the session, its stream, and all device allocations.
unsafe impl Send for Gemma4CudaSession {}

impl Gemma4CudaSession {
    pub fn kernel_info(device_ordinal: usize) -> Result<Gemma4KernelInfo> {
        let mut info = FfiKernelInfo {
            sm_count: 0,
            threads_per_cta: 0,
            active_ctas_per_sm: 0,
            registers_per_thread: 0,
            static_shared_bytes: 0,
            dynamic_shared_bytes: 0,
        };
        // SAFETY: `info` remains valid for the synchronous query.
        let status =
            unsafe { ai_pow_cuda_gemma4_kernel_info(u32::try_from(device_ordinal)?, &mut info) };
        check_cuda("query Gemma CUDA kernel", status)?;
        Ok(Gemma4KernelInfo {
            sm_count: info.sm_count,
            threads_per_cta: info.threads_per_cta,
            active_ctas_per_sm: info.active_ctas_per_sm,
            registers_per_thread: info.registers_per_thread,
            static_shared_bytes: info.static_shared_bytes,
            dynamic_shared_bytes: info.dynamic_shared_bytes,
        })
    }

    /// Create a diagnostic session from already-noised operands.
    pub fn new(
        device_ordinal: usize,
        m: usize,
        n: usize,
        a_prime: &[i8],
        b_prime: &[i8],
        pow_key: &[u8; 32],
    ) -> Result<Self> {
        validate_shape(m, n, a_prime.len(), b_prime.len())?;
        let total_tickets = total_tickets(m, n)?;
        let mut raw = std::ptr::null_mut();
        // SAFETY: operand lengths and the fixed geometry are validated. CUDA copies
        // the slices before this synchronous call returns.
        let status = unsafe {
            ai_pow_cuda_gemma4_session_create(
                u32::try_from(device_ordinal)?,
                u32::try_from(m)?,
                u32::try_from(n)?,
                GEMMA4_K as u32,
                GEMMA4_RANK as u32,
                GEMMA4_TILE as u32,
                a_prime.as_ptr(),
                b_prime.as_ptr(),
                pow_key.as_ptr(),
                &mut raw,
            )
        };
        check_cuda("Gemma diagnostic session creation", status)?;
        finish_session(raw, m, n, total_tickets)
    }

    /// Create a candidate-prepared session from unnoised INT7 operands.
    pub fn new_source(
        device_ordinal: usize,
        m: usize,
        n: usize,
        a: &[i8],
        b: &[i8],
    ) -> Result<Self> {
        validate_shape(m, n, a.len(), b.len())?;
        let total_tickets = total_tickets(m, n)?;
        let mut raw = std::ptr::null_mut();
        // SAFETY: operand lengths and the fixed geometry are validated. CUDA copies
        // the slices before this synchronous call returns.
        let status = unsafe {
            ai_pow_cuda_gemma4_source_session_create(
                u32::try_from(device_ordinal)?,
                u32::try_from(m)?,
                u32::try_from(n)?,
                GEMMA4_K as u32,
                GEMMA4_RANK as u32,
                GEMMA4_TILE as u32,
                a.as_ptr(),
                b.as_ptr(),
                &mut raw,
            )
        };
        check_cuda("Gemma source session creation", status)?;
        finish_session(raw, m, n, total_tickets)
    }

    pub fn prepare(&mut self, sigma: &[u8], mu: &[u8]) -> Result<Gemma4Preparation> {
        if sigma.len() != 76 || mu.len() != 52 {
            bail!(
                "Gemma transcript lengths must be sigma=76 and mu=52, got {} and {}",
                sigma.len(),
                mu.len()
            );
        }
        let mut result = FfiPrepareResult {
            kappa: [0; 32],
            h_a: [0; 32],
            h_b: [0; 32],
            s_a: [0; 32],
            s_b: [0; 32],
            commitment_ms: 0.0,
            noise_ms: 0.0,
        };
        // SAFETY: transcript slices have their ABI-required lengths. The session is
        // exclusively borrowed for the synchronous preparation.
        let status = unsafe {
            ai_pow_cuda_gemma4_session_prepare(self.raw, sigma.as_ptr(), mu.as_ptr(), &mut result)
        };
        check_cuda("Gemma transcript preparation", status)?;
        Ok(Gemma4Preparation {
            commitments: PearlWorkCommitments {
                kappa: result.kappa,
                h_a: result.h_a,
                h_b: result.h_b,
                s_a: result.s_a,
                s_b: result.s_b,
            },
            commitment_ms: result.commitment_ms,
            noise_ms: result.noise_ms,
        })
    }

    pub const fn m(&self) -> usize {
        self.m
    }

    pub const fn n(&self) -> usize {
        self.n
    }

    pub const fn total_tickets(&self) -> u64 {
        self.total_tickets
    }

    pub fn search(
        &mut self,
        ordinal_start: u64,
        ordinal_count: u64,
        target: &[u8; 32],
    ) -> Result<Gemma4SearchResult> {
        if ordinal_count == 0
            || ordinal_start >= self.total_tickets
            || ordinal_count > self.total_tickets - ordinal_start
        {
            bail!("Gemma search range is outside the prepared ticket space");
        }
        let mut result = FfiSearchResult {
            winner_ordinal: NO_WINNER,
            jackpot: [0; 32],
            kernel_ms: 0.0,
        };
        // SAFETY: the session is exclusively borrowed and output buffers have the
        // ABI-required fixed lengths.
        let status = unsafe {
            ai_pow_cuda_gemma4_session_search(
                self.raw,
                ordinal_start,
                ordinal_count,
                target.as_ptr(),
                &mut result,
            )
        };
        check_cuda("Gemma search", status)?;
        Ok(Gemma4SearchResult {
            winner: (result.winner_ordinal != NO_WINNER).then_some(result.winner_ordinal),
            jackpot: result.jackpot,
            kernel_ms: result.kernel_ms,
        })
    }

    pub fn debug_ticket(&mut self, ordinal: u64) -> Result<Gemma4DebugResult> {
        if ordinal >= self.total_tickets {
            bail!("Gemma debug ordinal is outside the prepared ticket space");
        }
        let mut state = [0i32; 16];
        let mut jackpot = [0u8; 32];
        // SAFETY: the session is exclusively borrowed and output buffers have the
        // ABI-required fixed lengths.
        let status = unsafe {
            ai_pow_cuda_gemma4_session_debug(
                self.raw,
                ordinal,
                state.as_mut_ptr(),
                jackpot.as_mut_ptr(),
            )
        };
        check_cuda("Gemma ticket debug", status)?;
        Ok(Gemma4DebugResult {
            state: TileState(state),
            jackpot,
        })
    }
}

impl Drop for Gemma4CudaSession {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            // SAFETY: `raw` is exclusively owned and destroyed exactly once.
            let _ = unsafe { ai_pow_cuda_gemma4_session_destroy(self.raw) };
            self.raw = std::ptr::null_mut();
        }
    }
}

fn validate_shape(m: usize, n: usize, a_len: usize, b_len: usize) -> Result<()> {
    if m == 0 || n == 0 || m % 256 != 0 || n % 128 != 0 {
        bail!("Gemma CUDA shape requires nonzero m%256==0 and n%128==0");
    }
    let expected_a = m
        .checked_mul(GEMMA4_K)
        .ok_or_else(|| anyhow::anyhow!("Gemma A length overflow"))?;
    let expected_b = n
        .checked_mul(GEMMA4_K)
        .ok_or_else(|| anyhow::anyhow!("Gemma B length overflow"))?;
    if a_len != expected_a || b_len != expected_b {
        bail!(
            "Gemma matrix lengths must be m*k={} and n*k={}, got {} and {}", expected_a,
            expected_b, a_len, b_len
        );
    }
    Ok(())
}

fn total_tickets(m: usize, n: usize) -> Result<u64> {
    u64::try_from(m / GEMMA4_TILE)?
        .checked_mul(u64::try_from(n / GEMMA4_TILE)?)
        .ok_or_else(|| anyhow::anyhow!("Gemma ticket count overflow"))
}

fn finish_session(
    raw: *mut c_void,
    m: usize,
    n: usize,
    total_tickets: u64,
) -> Result<Gemma4CudaSession> {
    if raw.is_null() {
        bail!("Gemma session creation returned a null session");
    }
    Ok(Gemma4CudaSession {
        raw,
        m,
        n,
        total_tickets,
    })
}

fn check_cuda(operation: &str, status: c_int) -> Result<()> {
    if status == 0 {
        Ok(())
    } else {
        bail!("{operation} failed with CUDA status {status}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_shape_constants_match_consensus_profile() {
        assert_eq!(GEMMA4_M, 4_096);
        assert_eq!(GEMMA4_K, 5_376);
        assert_eq!(GEMMA4_N, 43_008);
        assert_eq!(GEMMA4_RANK, 128);
        assert_eq!(GEMMA4_TILE, 16);
        assert_eq!(total_tickets(GEMMA4_M, GEMMA4_N).unwrap(), 688_128);
    }

    #[test]
    fn shape_validation_rejects_wrong_lengths() {
        let error = validate_shape(256, 128, 0, 0).unwrap_err();
        assert!(error.to_string().contains("matrix lengths"));
    }

    fn fixture(m: usize, n: usize) -> (Vec<i8>, Vec<i8>, [u8; 32]) {
        let mut state = 0x0123_4567_89ab_cdefu64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ((state >> 32) as u8 & 0x7f) as i8 - 64
        };
        let mut a = Vec::with_capacity(m * GEMMA4_K);
        a.resize_with(m * GEMMA4_K, &mut next);
        let mut b = Vec::with_capacity(n * GEMMA4_K);
        b.resize_with(n * GEMMA4_K, &mut next);
        let key = std::array::from_fn(|index| (index as u8).wrapping_mul(17).wrapping_add(3));
        (a, b, key)
    }

    fn scalar_ticket(a: &[i8], b: &[i8], n: usize, ordinal: u64) -> TileState {
        let col_tiles = n / GEMMA4_TILE;
        let row_tile = ordinal as usize / col_tiles;
        let col_tile = ordinal as usize % col_tiles;
        let mut cells = [0i32; GEMMA4_TILE * GEMMA4_TILE];
        let mut state = TileState::zero();
        for step in 0..GEMMA4_K / GEMMA4_RANK {
            for row in 0..GEMMA4_TILE {
                let a_base = (row_tile * GEMMA4_TILE + row) * GEMMA4_K + step * GEMMA4_RANK;
                for col in 0..GEMMA4_TILE {
                    let b_base = (col_tile * GEMMA4_TILE + col) * GEMMA4_K + step * GEMMA4_RANK;
                    let mut delta = 0i32;
                    for index in 0..GEMMA4_RANK {
                        delta += i32::from(a[a_base + index]) * i32::from(b[b_base + index]);
                    }
                    let cell = row * GEMMA4_TILE + col;
                    cells[cell] = cells[cell].saturating_add(delta);
                }
            }
            let x = cells
                .iter()
                .fold(0u32, |value, cell| value ^ (*cell as u32)) as i32;
            state.fold(step as u32, x);
        }
        state
    }

    fn little_endian_predecessor(mut value: [u8; 32]) -> [u8; 32] {
        for byte in &mut value {
            if *byte != 0 {
                *byte -= 1;
                return value;
            }
            *byte = 0xff;
        }
        panic!("zero has no unsigned predecessor");
    }

    #[test]
    #[ignore = "requires a supported CUDA device"]
    fn device_matches_one_thousand_deterministic_tickets() {
        const TICKET_COUNT: usize = 1_000;
        let (a, b, key) = fixture(2_048, 256);
        let mut session =
            Gemma4CudaSession::new(0, 2_048, 256, &a, &b, &key).expect("Gemma CUDA session");
        let total_tickets = session.total_tickets();
        let mut ordinals = Vec::with_capacity(TICKET_COUNT);
        ordinals.extend([0, 1, 127, 128, 129, 1_023, 1_024, total_tickets - 1]);
        let mut random = 0xd1b5_4a32_d192_ed03u64;
        while ordinals.len() < TICKET_COUNT {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            ordinals.push(random % total_tickets);
        }
        for ordinal in ordinals {
            let scalar = scalar_ticket(&a, &b, 256, ordinal);
            let jackpot = ai_pow::pearl_compat::pearl_jackpot_hash(&scalar, &key);
            let device = session.debug_ticket(ordinal).expect("device ticket");
            assert_eq!(device.state, scalar, "ordinal {ordinal}");
            assert_eq!(device.jackpot, jackpot, "ordinal {ordinal}");
            let lower_target = little_endian_predecessor(jackpot);
            for repetition in 0..3 {
                let hit = session
                    .search(ordinal, 1, &jackpot)
                    .expect("exact-target search");
                assert_eq!(
                    hit.winner,
                    Some(ordinal),
                    "ordinal {ordinal}, repetition {repetition}"
                );
                assert_eq!(hit.jackpot, jackpot);
                let miss = session
                    .search(ordinal, 1, &lower_target)
                    .expect("predecessor-target search");
                assert_eq!(miss.winner, None);
            }
        }
    }

    #[cfg(feature = "node")]
    #[test]
    #[ignore = "requires a supported CUDA device"]
    fn source_session_matches_complete_scalar_transcript() {
        use std::sync::Arc;

        let params = ai_pow::params::MatmulParams {
            m: 256,
            k: GEMMA4_K as u32,
            n: 128,
            noise_rank: GEMMA4_RANK as u32,
            tile: GEMMA4_TILE as u32,
            spot_checks: 1,
            difficulty_bits: 0,
        };
        let (a, b) = ai_pow::synth::synth_matrices(ai_pow::synth::AI_POW_PROD_SYNTH_SEED, &params);
        let a = Arc::new(a);
        let b = Arc::new(b);
        let template = crate::canonical::PreparedCanonicalDenseTemplate::new(
            &params,
            [0x5a; 32],
            Arc::clone(&a),
            Arc::clone(&b),
        )
        .expect("dense template");
        let mut session =
            Gemma4CudaSession::new_source(0, params.m as usize, params.n as usize, &a, &b)
                .expect("Gemma source session");
        for extranonce in [0, 1, u32::MAX - 1, u32::MAX] {
            let scalar = template.prepare(extranonce).expect("scalar preparation");
            let device = session
                .prepare(scalar.sigma(), scalar.mu())
                .expect("device preparation");
            assert_eq!(
                device.commitments,
                *scalar.commitments(),
                "extranonce {extranonce}"
            );
            for ordinal in [0, scalar.row_offsets().len() as u64 - 1, 127] {
                let (t_rows, t_cols) = scalar
                    .offsets_at_ordinal(ordinal)
                    .expect("scalar ticket offsets");
                let expected = scalar
                    .evaluate(t_rows, t_cols, &mut scalar.scratch())
                    .expect("scalar ticket");
                let actual = session.debug_ticket(ordinal).expect("device ticket");
                assert_eq!(
                    actual.state, expected.tile_state,
                    "extranonce {extranonce}, ordinal {ordinal}"
                );
                assert_eq!(
                    actual.jackpot, expected.jackpot_hash,
                    "extranonce {extranonce}, ordinal {ordinal}"
                );
            }
        }
    }
}
