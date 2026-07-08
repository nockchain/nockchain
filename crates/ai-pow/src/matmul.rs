//! Pearl-style tiled INT8 matmul with low-rank noise and iterative tile state.
//!
//! Computes `(A + E) * (B + F)` where the noise factors are low-rank:
//!   `E = E_L · E_R`,  `F = F_L · F_R`
//! with `E_L ∈ i6^{m×r}`, `E_R ∈ {-1,0,1}^{r×k}` choice matrix,
//! `F_L ∈ {-1,0,1}^{k×r}` choice matrix, `F_R ∈ i6^{r×n}` (Pearl §4.4).
//!
//! The product is computed tile-by-tile in stripes of width `r` along the
//! `k`-axis (Pearl §4.5 Alg. 4). At each stripe boundary the 16 × `int32`
//! state `M_{i,j}` is folded with the XOR of all accumulator entries:
//!
//!   M[ℓ mod 16] ← (M[ℓ mod 16] ≪ 13) ⊕ X_ℓ
//!
//! and the final per-tile state is keyed-BLAKE3-hashed to produce both the
//! Merkle leaf and the hardness check value (Pearl §4.5 lines 14-16).

use blake3::Hasher;

use crate::params::MatmulParams;
use crate::prng;

#[cfg(test)]
pub(crate) static BLOCK_NOISE_EXPAND_CALLS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Per-attempt low-rank noise factors. In production these seeds are derived
/// after the nonce is folded into the Pearl attempt state, so reusing expanded
/// noise across nonce values would be a PoW soundness bug.
pub struct BlockNoise {
    pub m: u32,
    pub k: u32,
    pub n: u32,
    pub r: u32,
    /// `E_L` row-major: `m × r`. `E_L[i, p] = e_l[i*r + p]`.
    pub e_l: Vec<i8>,
    /// Per-column choice positions of `E_R`. `e_r_pos[l] = (p_plus, p_minus)`
    /// means column `l` of `E_R` has a `+1` at `p_plus` and a `-1` at `p_minus`.
    pub e_r_pos: Vec<(u32, u32)>,
    /// `F_R` stored col-major: `n × r` with column `j` at `j*r..(j+1)*r`,
    /// so `F_R[p, j] = f_r[j*r + p]`.
    pub f_r: Vec<i8>,
    /// Per-row choice positions of `F_L`. `f_l_pos[l] = (p_plus, p_minus)`.
    pub f_l_pos: Vec<(u32, u32)>,
}

impl BlockNoise {
    /// Build the noise factors per Pearl Alg. 1: `(E_L, E_R)` are keyed by
    /// `s_a`, `(F_R, F_L)` are keyed by `s_b`. The two seeds are derived in
    /// `fiat_shamir.rs` from the nonce-bound attempt transcript and matrix
    /// commitments.
    pub fn expand(s_a: &[u8; 32], s_b: &[u8; 32], params: &MatmulParams) -> Self {
        #[cfg(test)]
        BLOCK_NOISE_EXPAND_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let m = params.m as usize;
        let k = params.k as usize;
        let n = params.n as usize;
        let r = params.noise_rank as usize;

        let mut e_l = vec![0i8; m * r];
        for i in 0..params.m {
            let off = (i as usize) * r;
            prng::expand_e_l_row(s_a, i, params.noise_rank, &mut e_l[off..off + r]);
        }
        let mut e_r_pos = Vec::with_capacity(k);
        for l in 0..params.k {
            e_r_pos.push(prng::e_r_col_positions(s_a, l, params.noise_rank));
        }
        let mut f_r = vec![0i8; n * r];
        for j in 0..params.n {
            let off = (j as usize) * r;
            prng::expand_f_r_col(s_b, j, params.noise_rank, &mut f_r[off..off + r]);
        }
        let mut f_l_pos = Vec::with_capacity(k);
        for k_idx in 0..params.k {
            f_l_pos.push(prng::f_l_row_positions(s_b, k_idx, params.noise_rank));
        }

        Self {
            m: params.m,
            k: params.k,
            n: params.n,
            r: params.noise_rank,
            e_l,
            e_r_pos,
            f_r,
            f_l_pos,
        }
    }

    /// Reconstruct row `i` of `E` (length `k`) into `out`.
    pub fn e_row_into(&self, i: u32, out: &mut [i8]) {
        let r = self.r as usize;
        let k = self.k as usize;
        debug_assert_eq!(out.len(), k);
        let row_off = (i as usize) * r;
        let e_l_row = &self.e_l[row_off..row_off + r];
        for (l, slot) in out.iter_mut().enumerate() {
            let (pp, pm) = self.e_r_pos[l];
            *slot = e_l_row[pp as usize] - e_l_row[pm as usize];
        }
    }

    /// Reconstruct column `j` of `F` (length `k`) into `out`.
    pub fn f_col_into(&self, j: u32, out: &mut [i8]) {
        let r = self.r as usize;
        let k = self.k as usize;
        debug_assert_eq!(out.len(), k);
        let col_off = (j as usize) * r;
        let f_r_col = &self.f_r[col_off..col_off + r];
        for (l, slot) in out.iter_mut().enumerate() {
            let (pp, pm) = self.f_l_pos[l];
            *slot = f_r_col[pp as usize] - f_r_col[pm as usize];
        }
    }
}

/// Per-attempt perturbed matrices `A' = A + E` (row-major) and `B' = B + F`
/// (column-major). Each entry fits an `i8`: with `|A| <= 64` and `|E| <= 63`
/// (Pearl §4.1/§4.4), `|A + E| <= 127`. Constructed once per nonce attempt
/// and reused across all output tiles.
pub struct Matrices {
    pub m: u32,
    pub k: u32,
    pub n: u32,
    /// `A' = A + E`, row-major: row `i` at `i*k..(i+1)*k`.
    pub a_prime: Vec<i8>,
    /// `B' = B + F`, column-major: column `j` at `j*k..(j+1)*k`.
    pub b_prime: Vec<i8>,
}

impl Matrices {
    /// Build `A' = A + E` (row-major) and `B' = B + F` (column-major) from
    /// caller-supplied input matrices and per-block noise factors.
    /// `a` is `m * k` row-major; `b` is `n * k` column-major (column `j` at
    /// `j*k..(j+1)*k`).
    pub fn build(a: &[i8], b: &[i8], noise: &BlockNoise, params: &MatmulParams) -> Self {
        let m = params.m as usize;
        let k = params.k as usize;
        let n = params.n as usize;
        assert_eq!(a.len(), m * k, "A length mismatch");
        assert_eq!(b.len(), n * k, "B length mismatch");

        let mut a_prime = vec![0i8; m * k];
        let mut row_buf_e = vec![0i8; k];
        for i in 0..params.m {
            noise.e_row_into(i, &mut row_buf_e);
            let off = (i as usize) * k;
            for l in 0..k {
                // |A + E| <= 64 + 63 = 127, fits i8.
                a_prime[off + l] = (a[off + l] as i16 + row_buf_e[l] as i16) as i8;
            }
        }
        let mut b_prime = vec![0i8; k * n];
        let mut col_buf_f = vec![0i8; k];
        for j in 0..params.n {
            noise.f_col_into(j, &mut col_buf_f);
            let off = (j as usize) * k;
            for l in 0..k {
                b_prime[off + l] = (b[off + l] as i16 + col_buf_f[l] as i16) as i8;
            }
        }
        Self {
            m: params.m,
            k: params.k,
            n: params.n,
            a_prime,
            b_prime,
        }
    }

    pub fn a_prime_row(&self, i: u32) -> &[i8] {
        let k = self.k as usize;
        let off = (i as usize) * k;
        &self.a_prime[off..off + k]
    }
    pub fn b_prime_col(&self, j: u32) -> &[i8] {
        let k = self.k as usize;
        let off = (j as usize) * k;
        &self.b_prime[off..off + k]
    }
}

/// 512-bit per-tile state `M_{i,j}` as 16 `int32` slots (Pearl §4.5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TileState(pub [i32; 16]);

impl TileState {
    pub const fn zero() -> Self {
        Self([0; 16])
    }

    /// Fold stripe `step`'s int32-XOR `x` into the rolling state, matching
    /// Pearl §4.5: `M[step mod 16] ← (M[step mod 16] ≪ 13) ⊕ x`.
    #[inline]
    pub fn fold(&mut self, step: u32, x: i32) {
        let slot = (step as usize) % 16;
        let rotated = (self.0[slot] as u32).rotate_left(13) as i32;
        self.0[slot] = rotated ^ x;
    }

    /// Keyed BLAKE3 hash of the state, used as both the Merkle leaf and the
    /// hardness check value (Pearl §4.5 line 16).
    ///
    /// Byte-equivalent to Pearl's `compute_jackpot_hash(jackpot, key)`
    /// (Pearl zk-pow api/proof_utils.rs:1077-1081): hashes exactly
    /// 64 bytes (16 × `u32` little-endian) under the keyed BLAKE3 mode
    /// with `pow_key` as the key. No context prefix.
    pub fn keyed_hash(&self, pow_key: &[u8; 32]) -> [u8; 32] {
        let mut hasher = Hasher::new_keyed(pow_key);
        for v in &self.0 {
            hasher.update(&v.to_le_bytes());
        }
        *hasher.finalize().as_bytes()
    }

    /// Replay a per-stripe `x` sequence into a fresh state.
    ///
    /// This is the *defining property* the ZK FoldChip (HIGH-2.2)
    /// must reproduce: the final `M` is a pure function of the
    /// `x_steps` sequence alone, via Pearl §4.5
    /// `M[step % 16] ← rotl13(M[step % 16]) ⊕ x_steps[step]`.
    /// Nothing else (matrix layout, accumulator geometry) enters
    /// once the sequence is fixed — so the circuit only has to
    /// bind `x_steps`, not re-derive the fold.
    pub fn from_x_steps(x_steps: &[i32]) -> Self {
        let mut s = Self::zero();
        for (step, &x) in x_steps.iter().enumerate() {
            s.fold(step as u32, x);
        }
        s
    }
}

/// Per-stripe trace of one tile's accumulate-and-fold.
///
/// `compute_tile` only returns the final folded `TileState`; the
/// HIGH-2.2 bridge and every FoldChip test need the intermediate
/// `x_ℓ` values (Pearl §4.5) the fold consumes, since those — not
/// the raw `t·t` accumulator — are what the circuit binds. The
/// invariant `TileState::from_x_steps(&trace.x_steps) ==
/// trace.state` holds by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileTrace {
    /// `x_steps[step]` = int32-XOR of the full `t·t` accumulator
    /// after stripe `step` (the value folded into `M`).
    /// `len() == params.num_stripes()`.
    pub x_steps: Vec<i32>,
    /// Final folded state — identical to `compute_tile`'s return.
    pub state: TileState,
}

/// Compute one output tile `(tile_i, tile_j)`, stepping in stripes of width
/// `r` along the `k`-axis and folding each stripe's XOR into the 512-bit
/// state `M` per Pearl §4.5. Returns the final `M` state; the caller derives
/// the keyed hash via `TileState::keyed_hash`.
pub fn compute_tile(
    matrices: &Matrices,
    params: &MatmulParams,
    tile_i: u32,
    tile_j: u32,
) -> TileState {
    compute_tile_trace(matrices, params, tile_i, tile_j).state
}

/// [`compute_tile`] that also returns the per-stripe `x` sequence
/// (Pearl §4.5). The `.state` field is bit-identical to
/// `compute_tile`'s return; `compute_tile` delegates here.
pub fn compute_tile_trace(
    matrices: &Matrices,
    params: &MatmulParams,
    tile_i: u32,
    tile_j: u32,
) -> TileTrace {
    let t = params.tile as usize;
    let r = params.noise_rank as usize;
    let row0 = (tile_i * params.tile) as usize;
    let col0 = (tile_j * params.tile) as usize;
    let steps = params.num_stripes() as usize;

    let mut c_blk = vec![0i32; t * t];
    let mut state = TileState::zero();
    let mut x_steps = Vec::with_capacity(steps);

    for step in 0..steps {
        let lo = step * r;
        // Inner per-stripe r-wide multiply, accumulated into c_blk.
        for di in 0..t {
            let a_row = &matrices.a_prime_row((row0 + di) as u32)[lo..lo + r];
            for dj in 0..t {
                let b_col = &matrices.b_prime_col((col0 + dj) as u32)[lo..lo + r];
                let mut delta: i32 = 0;
                for l in 0..r {
                    delta = delta.wrapping_add((a_row[l] as i32) * (b_col[l] as i32));
                }
                let idx = di * t + dj;
                c_blk[idx] = c_blk[idx].wrapping_add(delta);
            }
        }
        // Fold the int32-XOR of the running accumulator into M (Pearl §4.5).
        let mut x: i32 = 0;
        for &v in &c_blk {
            x ^= v;
        }
        x_steps.push(x);
        state.fold(step as u32, x);
    }
    TileTrace { x_steps, state }
}

/// Verifier-side compute_tile: same iterative accumulator as `compute_tile`
/// but takes already-extracted row-strips of `A'` and column-strips of `B'`.
/// `a_prime_rows` is `t * k` i8s (row-major over the `t` tile rows);
/// `b_prime_cols` is `t * k` i8s (column-major over the `t` tile columns).
pub fn compute_tile_from_slices(
    a_prime_rows: &[i8],
    b_prime_cols: &[i8],
    params: &MatmulParams,
) -> TileState {
    compute_tile_trace_from_slices(a_prime_rows, b_prime_cols, params).state
}

/// [`compute_tile_from_slices`] that also returns the per-stripe
/// `x` sequence. `.state` is bit-identical to
/// `compute_tile_from_slices`'s return; that fn delegates here.
pub fn compute_tile_trace_from_slices(
    a_prime_rows: &[i8],
    b_prime_cols: &[i8],
    params: &MatmulParams,
) -> TileTrace {
    let t = params.tile as usize;
    let r = params.noise_rank as usize;
    let k = params.k as usize;
    compute_pattern_tile_trace_from_slices(a_prime_rows, b_prime_cols, t, t, k, r, k)
}

/// Compute a Pearl pattern tile state from already-extracted noised A rows and
/// B columns.
///
/// This is the dimension-general form of [`compute_tile_trace_from_slices`].
/// Pearl tickets are `h × w`, where `h = rows_pattern.size()` and
/// `w = cols_pattern.size()`; the legacy Nockchain tile path is the special
/// case `h == w == params.tile` and `dot_product_len == k`.
pub fn compute_pattern_tile_trace_from_slices(
    a_prime_rows: &[i8],
    b_prime_cols: &[i8],
    h: usize,
    w: usize,
    k: usize,
    r: usize,
    dot_product_len: usize,
) -> TileTrace {
    assert!(h > 0, "Pearl pattern tile height must be nonzero");
    assert!(w > 0, "Pearl pattern tile width must be nonzero");
    assert!(k > 0, "Pearl pattern common dimension must be nonzero");
    assert!(r > 0, "Pearl pattern rank must be nonzero");
    assert!(
        dot_product_len <= k,
        "Pearl pattern dot_product_len must be <= common dimension"
    );
    assert_eq!(
        dot_product_len % r,
        0,
        "Pearl pattern rank must divide dot_product_len"
    );
    assert_eq!(a_prime_rows.len(), h * k, "a_prime_rows must be h*k");
    assert_eq!(b_prime_cols.len(), w * k, "b_prime_cols must be w*k");
    let steps = dot_product_len / r;

    let mut c_blk = vec![0i32; h * w];
    let mut state = TileState::zero();
    let mut x_steps = Vec::with_capacity(steps);

    for step in 0..steps {
        let lo = step * r;
        for u in 0..h {
            let a_row = &a_prime_rows[u * k + lo..u * k + lo + r];
            for v in 0..w {
                let b_col = &b_prime_cols[v * k + lo..v * k + lo + r];
                let mut delta: i32 = 0;
                for l in 0..r {
                    delta = delta.wrapping_add((a_row[l] as i32) * (b_col[l] as i32));
                }
                let idx = u * w + v;
                c_blk[idx] = c_blk[idx].wrapping_add(delta);
            }
        }
        let mut x: i32 = 0;
        for &v in &c_blk {
            x ^= v;
        }
        x_steps.push(x);
        state.fold(step as u32, x);
    }
    TileTrace { x_steps, state }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth_ab(seed: u8, params: &MatmulParams) -> (Vec<i8>, Vec<i8>) {
        let m = params.m as usize;
        let k = params.k as usize;
        let n = params.n as usize;
        let mut a = vec![0i8; m * k];
        let mut b = vec![0i8; n * k];
        let state = [seed; 16];
        for i in 0..params.m {
            let off = (i as usize) * k;
            prng::expand_a_row(&state, i, params.k, &mut a[off..off + k]);
        }
        for j in 0..params.n {
            let off = (j as usize) * k;
            prng::expand_b_col(&state, j, params.k, &mut b[off..off + k]);
        }
        (a, b)
    }

    #[test]
    fn block_noise_e_row_matches_full_product() {
        // E[i, l] = sum_p E_L[i, p] * E_R[p, l]. With E_R sparse (one +1 at
        // pp, one -1 at pm), E[i, l] = E_L[i, pp] - E_L[i, pm]. Confirm.
        let params = MatmulParams::TEST_SMALL;
        params.validate().unwrap();
        let s_a = [7u8; 32];
        let s_b = [8u8; 32];
        let noise = BlockNoise::expand(&s_a, &s_b, &params);
        let k = params.k as usize;
        let r = params.noise_rank as usize;

        let mut e_row = vec![0i8; k];
        for i in 0..params.m {
            noise.e_row_into(i, &mut e_row);
            for l in 0..k {
                let (pp, pm) = noise.e_r_pos[l];
                let row_off = (i as usize) * r;
                let expected = noise.e_l[row_off + pp as usize] - noise.e_l[row_off + pm as usize];
                assert_eq!(e_row[l], expected);
            }
        }
    }

    #[test]
    fn perturbed_matrices_fit_i8() {
        // `|A + E| <= 64 + 63 = 127` should always hold.
        let params = MatmulParams::TEST_SMALL;
        let s_a = [3u8; 32];
        let s_b = [4u8; 32];
        let noise = BlockNoise::expand(&s_a, &s_b, &params);
        let (a, b) = synth_ab(7, &params);
        let m = Matrices::build(&a, &b, &noise, &params);
        // i8 already constrains [-128, 127]; verify the tighter `|v| <= 127`
        // bound that follows from |A| <= 64 and |E| <= 63.
        for &v in &m.a_prime {
            assert!(v >= -127, "a_prime entry below -127: {v}");
        }
        for &v in &m.b_prime {
            assert!(v >= -127, "b_prime entry below -127: {v}");
        }
    }

    fn naive_full_product(
        a: &[i8],
        b: &[i8],
        s_a: &[u8; 32],
        s_b: &[u8; 32],
        params: &MatmulParams,
    ) -> Vec<i32> {
        let m = params.m as usize;
        let n = params.n as usize;
        let k = params.k as usize;
        let noise = BlockNoise::expand(s_a, s_b, params);
        let mats = Matrices::build(a, b, &noise, params);
        let mut out = vec![0i32; m * n];
        for i in 0..m {
            let a_row = mats.a_prime_row(i as u32);
            for j in 0..n {
                let b_col = mats.b_prime_col(j as u32);
                let mut acc: i32 = 0;
                for l in 0..k {
                    acc = acc.wrapping_add((a_row[l] as i32) * (b_col[l] as i32));
                }
                out[i * n + j] = acc;
            }
        }
        out
    }

    /// Recompute the final c_blk from the iterative tile loop, side-channeled
    /// out for cross-checking against the naive full product.
    fn iterative_tile_c_blk(
        matrices: &Matrices,
        params: &MatmulParams,
        tile_i: u32,
        tile_j: u32,
    ) -> Vec<i32> {
        let t = params.tile as usize;
        let r = params.noise_rank as usize;
        let row0 = (tile_i * params.tile) as usize;
        let col0 = (tile_j * params.tile) as usize;
        let mut c_blk = vec![0i32; t * t];
        let steps = params.num_stripes() as usize;
        for step in 0..steps {
            let lo = step * r;
            for di in 0..t {
                let a_row = &matrices.a_prime_row((row0 + di) as u32)[lo..lo + r];
                for dj in 0..t {
                    let b_col = &matrices.b_prime_col((col0 + dj) as u32)[lo..lo + r];
                    let mut delta: i32 = 0;
                    for l in 0..r {
                        delta = delta.wrapping_add((a_row[l] as i32) * (b_col[l] as i32));
                    }
                    let idx = di * t + dj;
                    c_blk[idx] = c_blk[idx].wrapping_add(delta);
                }
            }
        }
        c_blk
    }

    #[test]
    fn iterative_c_blk_matches_naive_full_product() {
        let params = MatmulParams::TEST_SMALL;
        let s_a = [3u8; 32];
        let s_b = [4u8; 32];
        let noise = BlockNoise::expand(&s_a, &s_b, &params);
        let (a, b) = synth_ab(11, &params);
        let mats = Matrices::build(&a, &b, &noise, &params);
        let full = naive_full_product(&a, &b, &s_a, &s_b, &params);
        let t = params.tile as usize;
        let n = params.n as usize;
        for tile_i in 0..params.row_tiles() {
            for tile_j in 0..params.col_tiles() {
                let block = iterative_tile_c_blk(&mats, &params, tile_i, tile_j);
                for di in 0..t {
                    for dj in 0..t {
                        let i = tile_i as usize * t + di;
                        let j = tile_j as usize * t + dj;
                        assert_eq!(
                            block[di * t + dj],
                            full[i * n + j],
                            "mismatch at ({tile_i},{tile_j})[{di},{dj}]"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn slice_path_matches_full_path() {
        let params = MatmulParams::TEST_SMALL;
        let s_a = [3u8; 32];
        let s_b = [4u8; 32];
        let noise = BlockNoise::expand(&s_a, &s_b, &params);
        let (a, b) = synth_ab(11, &params);
        let mats = Matrices::build(&a, &b, &noise, &params);

        let tile_i = 1u32;
        let tile_j = 2u32;
        let t = params.tile as usize;
        let k = params.k as usize;
        let row0 = (tile_i * params.tile) as usize;
        let col0 = (tile_j * params.tile) as usize;

        let mut a_rows = vec![0i8; t * k];
        for di in 0..t {
            a_rows[di * k..(di + 1) * k].copy_from_slice(mats.a_prime_row((row0 + di) as u32));
        }
        let mut b_cols = vec![0i8; t * k];
        for dj in 0..t {
            b_cols[dj * k..(dj + 1) * k].copy_from_slice(mats.b_prime_col((col0 + dj) as u32));
        }

        let from_full = compute_tile(&mats, &params, tile_i, tile_j);
        let from_slices = compute_tile_from_slices(&a_rows, &b_cols, &params);
        assert_eq!(from_full, from_slices);
        let from_pattern = compute_pattern_tile_trace_from_slices(
            &a_rows, &b_cols, t, t, k, params.noise_rank as usize, k,
        )
        .state;
        assert_eq!(
            from_full, from_pattern,
            "dimension-general Pearl helper must preserve square tile behavior"
        );
    }

    #[test]
    fn pattern_slice_path_supports_rectangular_rank_aligned_prefix() {
        let h = 4usize;
        let w = 8usize;
        let k = 80usize;
        let r = 16usize;
        let dot_product_len = 64usize;
        let mut a_rows = vec![0i8; h * k];
        let mut b_cols = vec![0i8; w * k];
        for (idx, cell) in a_rows.iter_mut().enumerate() {
            *cell = ((idx * 17 + 3) % 101) as i8 - 50;
        }
        for (idx, cell) in b_cols.iter_mut().enumerate() {
            *cell = ((idx * 29 + 11) % 97) as i8 - 48;
        }

        let prefix =
            compute_pattern_tile_trace_from_slices(&a_rows, &b_cols, h, w, k, r, dot_product_len);
        assert_eq!(prefix.x_steps.len(), dot_product_len / r);

        let mut suffix_mutated_a = a_rows.clone();
        let mut suffix_mutated_b = b_cols.clone();
        for row in 0..h {
            for l in dot_product_len..k {
                suffix_mutated_a[row * k + l] = suffix_mutated_a[row * k + l].wrapping_add(17);
            }
        }
        for col in 0..w {
            for l in dot_product_len..k {
                suffix_mutated_b[col * k + l] = suffix_mutated_b[col * k + l].wrapping_sub(19);
            }
        }
        let same_prefix = compute_pattern_tile_trace_from_slices(
            &suffix_mutated_a, &suffix_mutated_b, h, w, k, r, dot_product_len,
        );
        assert_eq!(
            prefix.state, same_prefix.state,
            "Pearl dot_product_len prefix must ignore rank-truncated suffix"
        );

        let full = compute_pattern_tile_trace_from_slices(&a_rows, &b_cols, h, w, k, r, k);
        let full_mutated = compute_pattern_tile_trace_from_slices(
            &suffix_mutated_a, &suffix_mutated_b, h, w, k, r, k,
        );
        assert_ne!(
            full.state, full_mutated.state,
            "mutated suffix should affect a full-length rank-aligned computation"
        );
    }

    #[test]
    fn tile_state_fold_depends_on_step_order() {
        // Two folds into the same slot must depend on order: the rotation
        // happens between them, so swapping the inputs changes the result.
        let mut s1 = TileState::zero();
        let mut s2 = TileState::zero();
        s1.fold(0, 0x1234_5678);
        s1.fold(16, 0x0bad_f00d); // back to slot 0 — rotation is applied first
        s2.fold(0, 0x0bad_f00d);
        s2.fold(16, 0x1234_5678);
        assert_ne!(s1, s2);
    }

    #[test]
    fn keyed_hash_depends_on_pow_key() {
        let mut s = TileState::zero();
        s.fold(0, 0x1234_5678);
        let h1 = s.keyed_hash(&[1u8; 32]);
        let h2 = s.keyed_hash(&[2u8; 32]);
        assert_ne!(h1, h2);
    }

    // ---- HIGH-2.2 §4.A: per-stripe x-sequence reference ----

    /// `compute_tile_trace().state` must be bit-identical to
    /// `compute_tile()` for every tile/seed, and `x_steps` must
    /// have exactly `num_stripes` entries. This is the contract
    /// the refactor (compute_tile delegating to the trace
    /// variant) must preserve.
    #[test]
    fn tile_trace_state_matches_compute_tile() {
        let params = MatmulParams::TEST_SMALL;
        let n_stripes = params.num_stripes() as usize;
        for seed in [1u8, 7, 200] {
            let s_a = [seed; 32];
            let s_b = [seed ^ 0x5a; 32];
            let noise = BlockNoise::expand(&s_a, &s_b, &params);
            let (a, b) = synth_ab(seed, &params);
            let mats = Matrices::build(&a, &b, &noise, &params);
            for tile_i in 0..params.row_tiles() {
                for tile_j in 0..params.col_tiles() {
                    let tr = compute_tile_trace(&mats, &params, tile_i, tile_j);
                    let st = compute_tile(&mats, &params, tile_i, tile_j);
                    assert_eq!(
                        tr.state, st,
                        "trace.state vs compute_tile @({tile_i},{tile_j}) seed={seed}"
                    );
                    assert_eq!(tr.x_steps.len(), n_stripes, "x_steps length");
                }
            }
        }
    }

    /// The slice-path trace agrees with the full-path trace
    /// (both `x_steps` and `state`) for the same tile, and each
    /// `.state` matches its non-trace twin.
    #[test]
    fn tile_trace_slice_path_agrees_with_full_path() {
        let params = MatmulParams::TEST_SMALL;
        let noise = BlockNoise::expand(&[3u8; 32], &[4u8; 32], &params);
        let (a, b) = synth_ab(11, &params);
        let mats = Matrices::build(&a, &b, &noise, &params);

        let (tile_i, tile_j) = (1u32, 2u32);
        let t = params.tile as usize;
        let k = params.k as usize;
        let row0 = (tile_i * params.tile) as usize;
        let col0 = (tile_j * params.tile) as usize;
        let mut a_rows = vec![0i8; t * k];
        for di in 0..t {
            a_rows[di * k..(di + 1) * k].copy_from_slice(mats.a_prime_row((row0 + di) as u32));
        }
        let mut b_cols = vec![0i8; t * k];
        for dj in 0..t {
            b_cols[dj * k..(dj + 1) * k].copy_from_slice(mats.b_prime_col((col0 + dj) as u32));
        }

        let full = compute_tile_trace(&mats, &params, tile_i, tile_j);
        let sliced = compute_tile_trace_from_slices(&a_rows, &b_cols, &params);
        assert_eq!(full.x_steps, sliced.x_steps, "x_steps full vs slice");
        assert_eq!(full.state, sliced.state, "state full vs slice");
        assert_eq!(full.state, compute_tile(&mats, &params, tile_i, tile_j));
        assert_eq!(
            sliced.state,
            compute_tile_from_slices(&a_rows, &b_cols, &params)
        );
    }

    /// The defining FoldChip invariant: `state` is a *pure
    /// function of the `x_steps` sequence alone*. Replaying the
    /// sequence through `from_x_steps` must reproduce `state`
    /// exactly, for every tile. If this ever fails, the circuit
    /// cannot bind `x_steps` and skip re-deriving the accumulator.
    #[test]
    fn from_x_steps_is_pure_function_of_sequence() {
        let params = MatmulParams::TEST_SMALL;
        let noise = BlockNoise::expand(&[9u8; 32], &[10u8; 32], &params);
        let (a, b) = synth_ab(42, &params);
        let mats = Matrices::build(&a, &b, &noise, &params);
        for tile_i in 0..params.row_tiles() {
            for tile_j in 0..params.col_tiles() {
                let tr = compute_tile_trace(&mats, &params, tile_i, tile_j);
                assert_eq!(
                    TileState::from_x_steps(&tr.x_steps),
                    tr.state,
                    "from_x_steps must reconstruct state @({tile_i},{tile_j})"
                );
            }
        }
    }

    /// `from_x_steps` must implement exactly Pearl §4.5
    /// `M[step%16] = rotl13(M[step%16]) ^ x` — verified against an
    /// independent manual re-derivation (not via `fold`), so a
    /// bug in `fold` and `from_x_steps` can't mask each other.
    #[test]
    fn from_x_steps_matches_manual_rotl13_fold() {
        let xs: Vec<i32> = (0..40i32)
            .map(|i| i.wrapping_mul(0x9E37_79B1u32 as i32) ^ (i << 7))
            .collect();
        let mut m = [0i32; 16];
        for (step, &x) in xs.iter().enumerate() {
            let slot = step % 16;
            m[slot] = ((m[slot] as u32).rotate_left(13) as i32) ^ x;
        }
        assert_eq!(TileState::from_x_steps(&xs), TileState(m));
    }

    /// Guard against a degenerate reference: a real solve's
    /// `x_steps` must not be all-zero or all-equal, otherwise a
    /// trivial/forged FoldChip could satisfy the binding. (At
    /// `difficulty_bits=0` every tile clears, but the *sequence*
    /// must still be non-trivial.)
    #[test]
    fn x_steps_are_nontrivial_on_a_real_tile() {
        let params = MatmulParams::TEST_SMALL;
        let noise = BlockNoise::expand(&[5u8; 32], &[6u8; 32], &params);
        let (a, b) = synth_ab(123, &params);
        let mats = Matrices::build(&a, &b, &noise, &params);
        let tr = compute_tile_trace(&mats, &params, 0, 0);
        assert!(tr.x_steps.iter().any(|&x| x != 0), "x_steps all zero");
        let first = tr.x_steps[0];
        assert!(
            tr.x_steps.iter().any(|&x| x != first),
            "x_steps all identical — fold would be a fixed permutation"
        );
        assert_ne!(tr.state, TileState::zero(), "folded state is zero");
    }

    /// **§4.C.2 / A3.0 — the decisive cross-crate byte-equivalence
    /// KAT.** `ai_pow_zk::noise_ref` (the off-circuit spec the
    /// in-circuit B2 sub-AIR will reproduce) must equal *this*
    /// crate's `BlockNoise` (itself written Pearl-byte-equivalent)
    /// bit-for-bit, over every `(i,l)`/`(l,j)` — i.e. the value
    /// `a_prime[i,l] − A[i,l]` (and `b_prime`). `ai-pow-zk` cannot
    /// depend on `ai-pow`, so the equivalence is asserted here
    /// (this crate may depend on `ai-pow-zk`, `--features zk`).
    #[cfg(feature = "zk")]
    #[test]
    fn noise_ref_byte_equivalent_to_block_noise() {
        use crate::prng;
        let s_a = [0x11u8; 32];
        let s_b = [0xEEu8; 32];
        // A few real/representative geometries (r a power of two,
        // r|k per §4.8/validate). m,n kept small; r/k are what
        // drive the noise streams.
        for &(m, k, n, r) in &[
            (8u32, 64u32, 8u32, 4u32), // TEST_SMALL-shaped
            (4, 128, 4, 32),           // r=2^5 (§4.8 floor)
            (4, 256, 4, 64),           // r=2^6 (Llama r)
            (2, 512, 2, 128),          // larger r, multi-chunk row
        ] {
            let params = MatmulParams {
                m,
                k,
                n,
                noise_rank: r,
                tile: 2,
                spot_checks: 1,
                difficulty_bits: 0,
            };
            let noise = BlockNoise::expand(&s_a, &s_b, &params);
            let mut e_row = vec![0i8; k as usize];
            for i in 0..m {
                noise.e_row_into(i, &mut e_row);
                for l in 0..k {
                    assert_eq!(
                        e_row[l as usize],
                        ai_pow_zk::noise_ref::e_value(&s_a, i, l, r),
                        "E[{i},{l}] r={r} k={k}: noise_ref != BlockNoise"
                    );
                }
            }
            let mut f_col = vec![0i8; k as usize];
            for j in 0..n {
                noise.f_col_into(j, &mut f_col);
                for l in 0..k {
                    assert_eq!(
                        f_col[l as usize],
                        ai_pow_zk::noise_ref::f_value(&s_b, l, j, r),
                        "F[{l},{j}] r={r}: noise_ref != BlockNoise"
                    );
                }
            }
            // And the composed a_prime / b_prime (A + E):
            let a: Vec<i8> = (0..(m * k) as i32).map(|x| (x % 64) as i8).collect();
            let b: Vec<i8> = (0..(k * n) as i32).map(|x| ((x * 3) % 64) as i8).collect();
            let mats = Matrices::build(&a, &b, &noise, &params);
            for i in 0..m {
                for l in 0..k {
                    let want = (a[(i * k + l) as usize] as i16
                        + ai_pow_zk::noise_ref::e_value(&s_a, i, l, r) as i16)
                        as i8;
                    assert_eq!(
                        mats.a_prime[(i * k + l) as usize],
                        want,
                        "a_prime[{i},{l}] != A + noise_ref::e_value"
                    );
                }
            }
            let _ = prng::SEED_LABEL_A; // (pin the shared label origin)
        }
    }
}
