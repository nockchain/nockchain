//! Permutation-agnostic config and call wrapper over Poseidon1 / Poseidon2.
//!
//! Lets MMCS/FRI code build perm rows without naming a specific hash: dispatch
//! is centralized in [`CircuitBuilder::add_perm`] and [`perm_private_data`].

use alloc::vec::Vec;

use p3_field::Field;

use crate::CircuitBuilderError;
use crate::builder::CircuitBuilder;
use crate::ops::poseidon1_perm::{Poseidon1Config, Poseidon1PermCall, Poseidon1PermPrivateData};
use crate::ops::poseidon2_perm::{Poseidon2Config, Poseidon2PermCall, Poseidon2PermPrivateData};
use crate::ops::tip5_perm::call::Tip5PermCallMmcs;
use crate::ops::tip5_perm::{Tip5Config, Tip5PermPrivateData};
use crate::ops::{NpoPrivateData, NpoTypeId};
use crate::types::{ExprId, NonPrimitiveOpId};

/// Challenger/MMCS permutation config: Poseidon1, Poseidon2, or Tip5.
///
/// Closed enum consumed by the in-circuit challenger AND
/// `recursion/src/pcs/mmcs.rs` (via `.d()/.rate()/.rate_ext()/
/// .width_ext()/.npo_type_id()`). The `Tip5` arm is the deployed
/// ai-pow-zk recursive hash (Goldilocks D=1, width 16, rate 10,
/// capacity 6, digest 5, 5-round); its arms mirror the Poseidon1 D=1
/// arms exactly with Tip5 numbers (rate 10, capacity 6). This is not
/// Nockchain's canonical non-recursive 7-round hash path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PermConfig {
    Poseidon1(Poseidon1Config),
    Poseidon2(Poseidon2Config),
    Tip5(Tip5Config),
}

impl PermConfig {
    pub const fn poseidon1(c: Poseidon1Config) -> Self {
        Self::Poseidon1(c)
    }

    pub const fn poseidon2(c: Poseidon2Config) -> Self {
        Self::Poseidon2(c)
    }

    pub const fn tip5(c: Tip5Config) -> Self {
        Self::Tip5(c)
    }

    pub const fn d(self) -> usize {
        match self {
            Self::Poseidon1(c) => c.d(),
            Self::Poseidon2(c) => c.d(),
            Self::Tip5(c) => c.d(),
        }
    }

    pub const fn rate(self) -> usize {
        match self {
            Self::Poseidon1(c) => c.rate(),
            Self::Poseidon2(c) => c.rate(),
            Self::Tip5(c) => c.rate(),
        }
    }

    pub const fn rate_ext(self) -> usize {
        match self {
            Self::Poseidon1(c) => c.rate_ext(),
            Self::Poseidon2(c) => c.rate_ext(),
            Self::Tip5(c) => c.rate_ext(),
        }
    }

    pub const fn width_ext(self) -> usize {
        match self {
            Self::Poseidon1(c) => c.width_ext(),
            Self::Poseidon2(c) => c.width_ext(),
            Self::Tip5(c) => c.width_ext(),
        }
    }

    /// MMCS digest length in extension elements.
    ///
    /// For Poseidon1/2 this equals `rate_ext()` (the Poseidon
    /// convention: `PaddingFreeSponge<_, _, RATE, RATE>` /
    /// `TruncatedPermutation<_, 2, RATE, WIDTH>`). For Tip5 it is the
    /// deployed digest 5 (native
    /// `PaddingFreeSponge<Tip5Perm,16,10,5>` /
    /// `TruncatedPermutation<Tip5Perm,2,5,16>` — digest ≠ rate). The
    /// `PermConfig`-generic MMCS uses this for every *digest* width
    /// (leaf squeeze length, sibling-compress digest placement, root
    /// length); Poseidon callers are unaffected since `digest_ext ==
    /// rate_ext` there.
    pub const fn digest_ext(self) -> usize {
        match self {
            Self::Poseidon1(c) => c.digest_ext(),
            Self::Poseidon2(c) => c.digest_ext(),
            Self::Tip5(c) => c.digest_ext(),
        }
    }

    pub const fn as_poseidon1(self) -> Option<Poseidon1Config> {
        match self {
            Self::Poseidon1(c) => Some(c),
            Self::Poseidon2(_) | Self::Tip5(_) => None,
        }
    }

    pub const fn as_poseidon2(self) -> Option<Poseidon2Config> {
        match self {
            Self::Poseidon2(c) => Some(c),
            Self::Poseidon1(_) | Self::Tip5(_) => None,
        }
    }

    pub const fn as_tip5(self) -> Option<Tip5Config> {
        match self {
            Self::Tip5(c) => Some(c),
            Self::Poseidon1(_) | Self::Poseidon2(_) => None,
        }
    }

    /// NPO type id for the perm table backing this config.
    pub fn npo_type_id(self) -> NpoTypeId {
        match self {
            Self::Poseidon1(c) => NpoTypeId::poseidon1_perm(c),
            Self::Poseidon2(c) => NpoTypeId::poseidon2_perm(c),
            Self::Tip5(c) => NpoTypeId::tip5_perm(c),
        }
    }
}

impl From<Poseidon1Config> for PermConfig {
    fn from(c: Poseidon1Config) -> Self {
        Self::Poseidon1(c)
    }
}

impl From<Poseidon2Config> for PermConfig {
    fn from(c: Poseidon2Config) -> Self {
        Self::Poseidon2(c)
    }
}

impl From<Tip5Config> for PermConfig {
    fn from(c: Tip5Config) -> Self {
        Self::Tip5(c)
    }
}

/// Config-less perm-row arguments, shared by Poseidon1 and Poseidon2.
#[derive(Clone)]
pub struct PermCall {
    pub new_start: bool,
    pub merkle_path: bool,
    pub mmcs_bit: Option<ExprId>,
    pub inputs: Vec<Option<ExprId>>,
    pub out_ctl: Vec<bool>,
    pub return_all_outputs: bool,
    pub mmcs_index_sum: Option<ExprId>,
}

/// Private (witness) data for one perm row, typed for the configured hash so the
/// runner downcasts it to the matching executor's expected type.
pub fn perm_private_data<F: 'static + Send + Sync>(
    cfg: impl Into<PermConfig>,
    sibling: Vec<F>,
) -> NpoPrivateData {
    match cfg.into() {
        PermConfig::Poseidon1(_) => NpoPrivateData::new(Poseidon1PermPrivateData { sibling }),
        PermConfig::Poseidon2(_) => NpoPrivateData::new(Poseidon2PermPrivateData { sibling }),
        PermConfig::Tip5(_) => NpoPrivateData::new(Tip5PermPrivateData { sibling }),
    }
}

impl<F: Field> CircuitBuilder<F> {
    /// Add a perm row, dispatching to the Poseidon1 or Poseidon2 op per `cfg`.
    pub fn add_perm(
        &mut self,
        cfg: PermConfig,
        call: &PermCall,
    ) -> Result<(NonPrimitiveOpId, Vec<Option<ExprId>>), CircuitBuilderError> {
        let result = match cfg {
            PermConfig::Poseidon1(config) => self.add_poseidon1_perm(&Poseidon1PermCall {
                config,
                new_start: call.new_start,
                merkle_path: call.merkle_path,
                mmcs_bit: call.mmcs_bit,
                inputs: call.inputs.clone(),
                out_ctl: call.out_ctl.clone(),
                return_all_outputs: call.return_all_outputs,
                mmcs_index_sum: call.mmcs_index_sum,
            }),
            PermConfig::Poseidon2(config) => self.add_poseidon2_perm(&Poseidon2PermCall {
                config,
                new_start: call.new_start,
                merkle_path: call.merkle_path,
                mmcs_bit: call.mmcs_bit,
                inputs: call.inputs.clone(),
                out_ctl: call.out_ctl.clone(),
                return_all_outputs: call.return_all_outputs,
                mmcs_index_sum: call.mmcs_index_sum,
            }),
            PermConfig::Tip5(config) => self.add_tip5_perm_mmcs(&Tip5PermCallMmcs {
                config,
                new_start: call.new_start,
                merkle_path: call.merkle_path,
                mmcs_bit: call.mmcs_bit,
                inputs: call.inputs.clone(),
                out_ctl: call.out_ctl.clone(),
                return_all_outputs: call.return_all_outputs,
                mmcs_index_sum: call.mmcs_index_sum,
            }),
        };
        if let Ok((op_id, _)) = &result {
            self.tag_npo_profile_phase(*op_id);
        }
        result
    }
}
