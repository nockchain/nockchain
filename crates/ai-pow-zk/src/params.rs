//! Public puzzle parameters, mirrored locally so this crate doesn't
//! depend back on `ai-pow`.
//!
//! The caller in `ai-pow` constructs a [`ZkParams`] from its own
//! [`MatmulParams`](`ai_pow::params::MatmulParams`) at the call site:
//!
//! ```ignore
//! let zk_params = ZkParams {
//!     m: p.m,
//!     k: p.k,
//!     n: p.n,
//!     noise_rank: p.noise_rank,
//!     tile: p.tile,
//!     difficulty_bits: p.difficulty_bits,
//! };
//! ```
//!
//! Keeping these fields in lock-step with `MatmulParams` is the caller's
//! responsibility.

/// Subset of `ai_pow::params::MatmulParams` the SNARK's AIR cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZkParams {
    pub m: u32,
    pub k: u32,
    pub n: u32,
    /// `r`: Pearl noise rank, also the accumulator stripe width. Must be
    /// a power of two and divide `k`.
    pub noise_rank: u32,
    /// `t`: native square tile size. Native tile-derived schedules require it
    /// to divide both `m` and `n`; explicit Pearl strip schedules only use it
    /// as legacy metadata and validate their row/column sets directly.
    pub tile: u32,
    /// `b`: log-2 difficulty (Pearl §4.5).
    pub difficulty_bits: u32,
}

impl ZkParams {
    /// Base preconditions for any explicit strip schedule. This deliberately
    /// does not require `tile | m,n`; Pearl schedules bind concrete public row
    /// and column sets instead of deriving them from a native square grid.
    pub fn validate_base(&self) -> Result<(), String> {
        if self.m == 0 || self.n == 0 {
            return Err("m and n must be > 0".into());
        }
        if self.k == 0 || self.k > (1u32 << 16) {
            return Err("k must be in 1..=2^16".into());
        }
        if self.noise_rank < 2 || !self.noise_rank.is_power_of_two() {
            return Err("noise_rank must be a power of two >= 2".into());
        }
        if self.k % self.noise_rank != 0 {
            return Err("noise_rank must divide k".into());
        }
        Ok(())
    }

    /// Same native square-grid precondition set as
    /// `ai_pow::params::MatmulParams::validate`. Re-validated here
    /// defensively because the cross-crate boundary could be misused.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_base()?;
        if self.tile == 0 || self.m % self.tile != 0 || self.n % self.tile != 0 {
            return Err("tile must divide m and n".into());
        }
        Ok(())
    }
}
