use p3_circuit::ops::PermConfig;

/// FRI verifier parameters (subset needed for verification).
///
/// These parameters are extracted from the full `FriParameters` and contain
/// only the information needed during verification (not proving).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FriVerifierParams {
    /// Log₂ of the blowup factor (rate = 1/blowup)
    pub log_blowup: usize,
    /// Log₂ of the final polynomial length (after all folding rounds)
    pub log_final_poly_len: usize,
    /// Number of commit-phase proof-of-work bits required
    pub commit_pow_bits: usize,
    /// Number of query proof-of-work bits required
    pub query_pow_bits: usize,
    /// Permutation configuration for MMCS verification (Poseidon1 or Poseidon2).
    /// When `Some`, recursive MMCS verification is performed.
    /// When `None`, only arithmetic verification is performed — this is
    /// **unsound** and only reachable via
    /// [`Self::unsafe_arithmetic_only_for_tests`].
    pub permutation_config: Option<PermConfig>,
}

impl FriVerifierParams {
    /// Create params with MMCS verification enabled.
    pub fn with_mmcs(
        log_blowup: usize,
        log_final_poly_len: usize,
        commit_pow_bits: usize,
        query_pow_bits: usize,
        permutation_config: impl Into<PermConfig>,
    ) -> Self {
        Self {
            log_blowup,
            log_final_poly_len,
            commit_pow_bits,
            query_pow_bits,
            permutation_config: Some(permutation_config.into()),
        }
    }

    /// Create params **without MMCS verification** (arithmetic-only).
    ///
    /// # Safety / soundness
    ///
    /// A verifier built from these params checks the FRI arithmetic fold chain
    /// but does **not** verify Merkle/MMCS commitment openings. This is
    /// **unsound for production use**: a prover can open commitments to
    /// arbitrary values without detection.
    ///
    /// This constructor exists only for tests that exercise the arithmetic path
    /// in isolation. Production verifier builders must use [`Self::with_mmcs`],
    /// which is the only safe constructor and the only way to obtain a
    /// `permutation_config`. There is intentionally no `From<&FriParameters>`
    /// (or other implicit) conversion, so MMCS verification cannot be disabled
    /// accidentally.
    pub const fn unsafe_arithmetic_only_for_tests(
        log_blowup: usize,
        log_final_poly_len: usize,
        commit_pow_bits: usize,
        query_pow_bits: usize,
    ) -> Self {
        Self {
            log_blowup,
            log_final_poly_len,
            commit_pow_bits,
            query_pow_bits,
            permutation_config: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use p3_circuit::ops::Poseidon2Config;

    use super::*;

    fn p2() -> PermConfig {
        PermConfig::poseidon2(Poseidon2Config::KOALA_BEAR_D4_W16)
    }

    /// The only safe constructor must always produce MMCS-enabled params, so a
    /// production verifier builder cannot accidentally skip commitment opening
    /// checks.
    #[test]
    fn with_mmcs_always_enables_mmcs_verification() {
        let params = FriVerifierParams::with_mmcs(1, 0, 0, 0, p2());
        assert!(
            params.permutation_config.is_some(),
            "with_mmcs must enable MMCS verification"
        );
    }

    /// Disabling MMCS verification must require the explicitly unsafe,
    /// test-only constructor — there is no implicit (`From`/`into`) path.
    #[test]
    fn arithmetic_only_is_the_only_way_to_disable_mmcs() {
        let params = FriVerifierParams::unsafe_arithmetic_only_for_tests(1, 0, 0, 0);
        assert!(
            params.permutation_config.is_none(),
            "arithmetic-only params must not perform MMCS verification"
        );
    }
}
