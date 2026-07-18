//! Generic challenger permutation config for the recursion circuit.
//!
//! Allows the verifier and circuit challenger to be parameterised by a permutation
//! config without naming a specific hash (e.g. Poseidon2).

use p3_circuit::ops::{Poseidon1Config, Poseidon2Config, Tip5Config};

/// Config for the permutation used by the in-circuit challenger.
///
/// Implemented by concrete permutation configs (e.g. Poseidon2); the recursion
/// verifier and [`crate::CircuitChallenger`] use this trait so they do not depend
/// on a specific hash by name.
pub trait ChallengerPermConfig: Send + Sync {
    /// Extension degree used by the in-circuit permutation NPO (`Poseidon2Config::d()`).
    ///
    /// This need not match the STARK challenge extension `EF::DIMENSION` (e.g. base
    /// width-16 Poseidon2 with `d() == 1` can pair with a quartic or quintic challenge).
    fn extension_degree(&self) -> usize;

    /// Poseidon2 config if this is a Poseidon2 permutation; `None` otherwise.
    fn as_poseidon2(&self) -> Option<&Poseidon2Config> {
        None
    }

    /// Poseidon1 config if this is a Poseidon1 permutation; `None` otherwise.
    fn as_poseidon1(&self) -> Option<&Poseidon1Config> {
        None
    }

    /// Tip5 config if this is a Tip5 permutation; `None` otherwise.
    ///
    /// C2 / M-S4: the deployed ai-pow-zk Layer-0 hash. The default
    /// `None` keeps Poseidon1/2 (and any other impl) non-breaking.
    fn as_tip5(&self) -> Option<&Tip5Config> {
        None
    }
}

impl ChallengerPermConfig for Poseidon2Config {
    fn extension_degree(&self) -> usize {
        Self::d(*self)
    }

    fn as_poseidon2(&self) -> Option<&Poseidon2Config> {
        Some(self)
    }
}

impl ChallengerPermConfig for Poseidon1Config {
    fn extension_degree(&self) -> usize {
        Self::d(*self)
    }

    fn as_poseidon1(&self) -> Option<&Poseidon1Config> {
        Some(self)
    }
}

impl ChallengerPermConfig for Tip5Config {
    fn extension_degree(&self) -> usize {
        // Tip5 is base-field only ⇒ always 1 (independent of the
        // STARK challenge extension, exactly like base width-16
        // Poseidon with d() == 1).
        Self::d(*self)
    }

    fn as_tip5(&self) -> Option<&Tip5Config> {
        Some(self)
    }
}
