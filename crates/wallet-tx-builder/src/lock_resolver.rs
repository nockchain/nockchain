use nockchain_types::tx_engine::common::{FirstName, Hash, HashDecodeError};
use nockchain_types::tx_engine::v1::tx::{
    FirstNameFromLockRootError, LockHashError, SpendCondition,
};

use crate::note_data::DecodedNoteData;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockResolutionSource {
    NoteData,
    LockRootFirstName,
    ReconstructedSimplePkh,
    ReconstructedCoinbasePkh,
    Unknown,
}

/// Relative maturity encoded in every 014-aletheia protocol-fund coinbase note.
///
/// This is part of the immutable on-chain lock preimage. It must not follow
/// network-specific coinbase defaults.
pub const PROTOCOL_FUND_NOTE_TIMELOCK_MIN: u64 = 100;

/// Canonical 014-aletheia protocol-fund lock root (`+fund-address` in Hoon).
const PROTOCOL_FUND_ADDRESS_B58: &str = "9EhcJiGhAPcWLYrR9DL4ZPjU2Z9XT6FT2ZFkEEwmSQv7ES2TMC7p6Up";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockResolution {
    pub source: LockResolutionSource,
    pub spend_condition: Option<SpendCondition>,
    pub spend_condition_count: Option<u64>,
    /// Additional relative maturity required by protocol routing but absent
    /// from the effective spend condition used for witness planning.
    pub required_relative_min_blocks: Option<u64>,
}

impl LockResolution {
    /// Builds an unresolved lock result.
    pub fn unknown() -> Self {
        Self {
            source: LockResolutionSource::Unknown,
            spend_condition: None,
            spend_condition_count: None,
            required_relative_min_blocks: None,
        }
    }

    pub fn is_selected(&self) -> bool {
        !matches!(self.source, LockResolutionSource::Unknown)
    }
}

/// Input context used by a matcher while resolving the effective spend lock.
pub struct ResolveLockRequest<'a> {
    pub note_first_name: &'a Hash,
    pub decoded_note_data: &'a DecodedNoteData,
    pub signer_pkh: Option<&'a Hash>,
    pub coinbase_relative_min: Option<u64>,
}

/// Matcher that owns lock-selection policy for spendability.
///
/// The matcher carries any local signing/unlock context and is responsible for
/// deciding whether a candidate note is selectable and, when needed, providing
/// the spend-condition metadata required for planning. Matchers may override
/// `resolve_lock` or `select_v1_candidate` to compose or layer additional
/// strategies.
pub trait LockMatcher {
    /// Returns true when this matcher can satisfy the provided spend condition
    /// for the target note first-name.
    fn matches(&self, note_first_name: &Hash, spend_condition: &SpendCondition) -> bool;

    /// Determines whether one v1 candidate should be admitted by this matcher.
    ///
    /// The default selection policy is identical to `resolve_lock`: a note is
    /// selectable if and only if the matcher can recover a spend condition for
    /// it. Matchers that select notes by other metadata, such as lock-root
    /// first-name ownership, should override this method.
    fn select_v1_candidate(&self, request: ResolveLockRequest<'_>) -> LockResolution {
        self.resolve_lock(request)
    }

    /// Resolves the effective lock for a note using matcher-specific policy.
    ///
    /// The default strategy is:
    /// 1. Use a decoded single-leaf `%lock` note-data payload when it is
    ///    spendable by this matcher for the target first-name.
    /// 2. Otherwise, reconstruct a simple PKH lock from the local signer hash and require an
    ///    exact first-name match via matcher policy.
    /// 3. If simple reconstruction does not match, try reconstructed coinbase-style PKH using
    ///    the provided relative-min constant.
    /// 4. The reconstruction uses exactly the provided relative-min constant.
    fn resolve_lock(&self, request: ResolveLockRequest<'_>) -> LockResolution {
        if let Some(lock_data) = request.decoded_note_data.first_decoded_lock() {
            // TODO(wallet-tx-builder): store the decoded `Lock` tree directly in
            // `LockDataPayload` so multi-leaf lock-root matching does not depend on
            // flattened-leaf conversions.
            if lock_data.spend_conditions.len() == 1 {
                let spend_condition = &lock_data.spend_conditions[0];
                if self.matches(request.note_first_name, spend_condition) {
                    return LockResolution {
                        source: LockResolutionSource::NoteData,
                        spend_condition: Some(spend_condition.clone()),
                        spend_condition_count: None,
                        required_relative_min_blocks: None,
                    };
                }
            }
        }

        if let Some(pkh) = request.signer_pkh {
            let simple = SpendCondition::simple_pkh(pkh.clone());
            if self.matches(request.note_first_name, &simple) {
                return LockResolution {
                    source: LockResolutionSource::ReconstructedSimplePkh,
                    spend_condition: Some(simple),
                    spend_condition_count: None,
                    required_relative_min_blocks: None,
                };
            }
        }

        if let (Some(pkh), Some(relative_min)) = (request.signer_pkh, request.coinbase_relative_min)
        {
            let coinbase = SpendCondition::coinbase_pkh(pkh.clone(), relative_min);
            if self.matches(request.note_first_name, &coinbase) {
                return LockResolution {
                    source: LockResolutionSource::ReconstructedCoinbasePkh,
                    spend_condition: Some(coinbase),
                    spend_condition_count: None,
                    required_relative_min_blocks: None,
                };
            }
        }

        LockResolution::unknown()
    }
}

/// Errors raised while deriving the protocol-fund-style coinbase-wrapped
/// first-name that a `LockRootLockMatcher` also accepts.
#[derive(Debug, thiserror::Error)]
pub enum CoinbaseFundFirstNameError {
    #[error(transparent)]
    LockHash(#[from] LockHashError),
    #[error(transparent)]
    FirstName(#[from] FirstNameFromLockRootError),
    #[error(transparent)]
    FundAddress(#[from] HashDecodeError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Lock matcher that matches candidate notes by first-name derived from a lock root.
///
/// This matcher is intended for bridge/multisig flows where spendability is keyed
/// by lock root ownership metadata rather than local single-signer key material.
pub struct LockRootLockMatcher {
    lock_root: Hash,
    expected_note_first_name: Hash,
    /// Additional accepted first-name for protocol-fund-style coinbase notes.
    ///
    /// `+make-name:coinbase` does not take a fund note's first-name directly from
    /// the multisig lock-root: it wraps that lock-root in a single
    /// `[%pkh m=1 {lock_root}]` primitive plus the immutable protocol-fund
    /// timelock and takes the first-name of *that* wrapped lock-root
    /// (`+fund-note-firstname` in tx-engine-1.hoon). The committed lock is
    /// therefore unsatisfiable as written; `+check:check-context` special-cases
    /// this first-name and routes the spend to the real m-of-n multisig
    /// (`+check-multisig-lock`). This field is the Rust mirror of that routing
    /// key.
    coinbase_wrapped_first_name: Option<Hash>,
    planning_spend_condition: Option<SpendCondition>,
}

impl LockRootLockMatcher {
    /// Builds a matcher from a canonical lock-root hash.
    pub fn from_lock_root(lock_root: &Hash) -> Result<Self, FirstNameFromLockRootError> {
        Ok(Self {
            lock_root: lock_root.clone(),
            expected_note_first_name: FirstName::from_lock_root(lock_root)?.into_hash(),
            coinbase_wrapped_first_name: None,
            planning_spend_condition: None,
        })
    }

    /// Supplies the canonical spend condition used for timelock and witness
    /// planning after a note is selected by lock-root first-name.
    pub fn with_spend_condition(mut self, spend_condition: SpendCondition) -> Self {
        self.planning_spend_condition = Some(spend_condition);
        self
    }

    /// Enables the 014-aletheia protocol-fund recovery route when this matcher
    /// owns the canonical `+fund-address` lock root.
    ///
    /// Ordinary multisig lock roots are left unchanged: the on-chain
    /// `+check:check-context` special case recognizes only the canonical
    /// `+fund-note-firstname`, so generating arbitrary wrapped first-names would
    /// produce plans the kernel rejects. For the canonical fund, selection
    /// returns the real multisig condition for witness sizing and separately
    /// preserves the immutable maturity bypassed by `+check-multisig-lock`.
    pub fn with_coinbase_fund_notes(mut self) -> Result<Self, CoinbaseFundFirstNameError> {
        if self.lock_root != Hash::from_base58(PROTOCOL_FUND_ADDRESS_B58)? {
            return Ok(self);
        }
        let wrapped =
            SpendCondition::coinbase_pkh(self.lock_root.clone(), PROTOCOL_FUND_NOTE_TIMELOCK_MIN);
        let note_lock_root = wrapped.hash()?;
        self.coinbase_wrapped_first_name =
            Some(FirstName::from_lock_root(&note_lock_root)?.into_hash());
        Ok(self)
    }

    /// Returns true when `note_first_name` is the protocol-fund coinbase-wrapped
    /// first-name accepted by this matcher.
    fn is_coinbase_wrapped_first_name(&self, note_first_name: &Hash) -> bool {
        self.coinbase_wrapped_first_name
            .as_ref()
            .is_some_and(|wrapped| note_first_name.to_array() == wrapped.to_array())
    }
}

impl LockMatcher for LockRootLockMatcher {
    fn matches(&self, note_first_name: &Hash, _spend_condition: &SpendCondition) -> bool {
        note_first_name.to_array() == self.expected_note_first_name.to_array()
    }

    fn select_v1_candidate(&self, request: ResolveLockRequest<'_>) -> LockResolution {
        let matches_lock_root_first_name =
            request.note_first_name.to_array() == self.expected_note_first_name.to_array();
        let matches_coinbase_wrapped = self.is_coinbase_wrapped_first_name(request.note_first_name);
        if !matches_lock_root_first_name && !matches_coinbase_wrapped {
            return LockResolution::unknown();
        }

        // Protocol-fund coinbase notes carry no canonical lock-data and their
        // committed lock is unsatisfiable as written, so `resolve_lock` (which
        // re-derives the leaf first-name from decoded note-data) cannot and must
        // not resolve them. Only attempt note-data resolution for direct
        // lock-root notes; route wrapped fund notes straight to the carried
        // multisig condition and preserve their bypassed maturity requirement.
        if matches_lock_root_first_name && !matches_coinbase_wrapped {
            let resolved = self.resolve_lock(request);
            if resolved.is_selected() {
                return resolved;
            }
        }

        LockResolution {
            source: LockResolutionSource::LockRootFirstName,
            spend_condition: self.planning_spend_condition.clone(),
            spend_condition_count: None,
            required_relative_min_blocks: matches_coinbase_wrapped
                .then_some(PROTOCOL_FUND_NOTE_TIMELOCK_MIN),
        }
    }

    fn resolve_lock(&self, request: ResolveLockRequest<'_>) -> LockResolution {
        let Some(lock_data) = request.decoded_note_data.first_decoded_lock() else {
            return LockResolution::unknown();
        };
        // TODO(wallet-tx-builder): support multi-leaf `%lock` payload resolution
        // once `LockDataPayload` preserves the canonical `Lock` tree structure.
        if lock_data.spend_conditions.len() != 1 {
            return LockResolution::unknown();
        }
        let spend_condition = &lock_data.spend_conditions[0];
        let Ok(leaf_first_name) = spend_condition.first_name() else {
            return LockResolution::unknown();
        };
        if request.note_first_name.to_array() != leaf_first_name.as_hash().to_array() {
            return LockResolution::unknown();
        }
        if self.matches(request.note_first_name, spend_condition) {
            return LockResolution {
                source: LockResolutionSource::NoteData,
                spend_condition: Some(spend_condition.clone()),
                spend_condition_count: None,
                required_relative_min_blocks: None,
            };
        }
        LockResolution::unknown()
    }
}

#[derive(Debug, Default)]
pub struct NeverMatches;

impl LockMatcher for NeverMatches {
    fn matches(&self, _note_first_name: &Hash, _spend_condition: &SpendCondition) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use nockchain_types::tx_engine::v1::tx::Lock;

    use super::*;
    use crate::note_data::{
        DecodedNoteDataEntry, DecodedNoteDataPayload, LockDataPayload, NormalizedNoteDataKey,
    };

    fn hash(v: u64) -> Hash {
        Hash::from_limbs(&[v, 0, 0, 0, 0])
    }

    struct MatchExpected {
        expected: SpendCondition,
    }

    impl LockMatcher for MatchExpected {
        fn matches(&self, _note_first_name: &Hash, spend_condition: &SpendCondition) -> bool {
            spend_condition == &self.expected
        }
    }

    fn lock_entry(lock: SpendCondition) -> DecodedNoteDataEntry {
        lock_entry_from_conditions(vec![lock])
    }

    fn lock_entry_from_conditions(spend_conditions: Vec<SpendCondition>) -> DecodedNoteDataEntry {
        DecodedNoteDataEntry {
            raw_key: "lock".to_string(),
            normalized_key: NormalizedNoteDataKey::Lock,
            raw_blob: Bytes::new(),
            payload: DecodedNoteDataPayload::Lock(LockDataPayload {
                version: 0,
                spend_conditions,
            }),
            decode_error: None,
        }
    }

    fn decoded_note_data(entries: Vec<DecodedNoteDataEntry>) -> DecodedNoteData {
        DecodedNoteData(entries)
    }

    struct AlwaysMatches;

    impl LockMatcher for AlwaysMatches {
        fn matches(&self, _note_first_name: &Hash, _spend_condition: &SpendCondition) -> bool {
            true
        }
    }

    fn first_name_for_lock(spend_condition: &SpendCondition) -> Hash {
        spend_condition
            .first_name()
            .expect("first-name should compute")
            .into_hash()
    }

    fn lock_root_for_lock(spend_condition: &SpendCondition) -> Hash {
        Lock::SpendCondition(spend_condition.clone())
            .hash()
            .expect("lock root should hash")
    }

    #[test]
    fn note_data_lock_has_priority_over_reconstruction() {
        let note_lock = SpendCondition::simple_pkh(hash(9));
        let matcher = MatchExpected {
            expected: note_lock.clone(),
        };
        let decoded = decoded_note_data(vec![lock_entry(note_lock.clone())]);
        let result = matcher.resolve_lock(ResolveLockRequest {
            note_first_name: &first_name_for_lock(&note_lock),
            decoded_note_data: &decoded,
            signer_pkh: Some(&hash(7)),
            coinbase_relative_min: Some(5),
        });

        assert_eq!(result.source, LockResolutionSource::NoteData);
        assert_eq!(result.spend_condition, Some(note_lock));
        assert_eq!(result.spend_condition_count, None);
    }

    #[test]
    fn note_data_lock_tree_is_not_treated_as_a_single_spend_condition() {
        let first = SpendCondition::simple_pkh(hash(9));
        let second = SpendCondition::simple_pkh(hash(10));
        let matcher = MatchExpected {
            expected: second.clone(),
        };
        let entry = lock_entry_from_conditions(vec![first, second.clone()]);
        let decoded = decoded_note_data(vec![entry]);
        let second_first_name = first_name_for_lock(&second);

        let result = matcher.resolve_lock(ResolveLockRequest {
            note_first_name: &second_first_name,
            decoded_note_data: &decoded,
            signer_pkh: None,
            coinbase_relative_min: None,
        });

        assert_eq!(result.source, LockResolutionSource::Unknown);
        assert_eq!(result.spend_condition, None);
        assert_eq!(result.spend_condition_count, None);
    }

    #[test]
    fn note_data_lock_reports_leaf_count_for_larger_lock_trees() {
        let first = SpendCondition::simple_pkh(hash(9));
        let second = SpendCondition::simple_pkh(hash(10));
        let third = SpendCondition::simple_pkh(hash(11));
        let fourth = SpendCondition::simple_pkh(hash(12));
        let matcher = MatchExpected {
            expected: third.clone(),
        };
        let entry = lock_entry_from_conditions(vec![first, second, third.clone(), fourth]);
        let decoded = decoded_note_data(vec![entry]);
        let third_first_name = first_name_for_lock(&third);

        let result = matcher.resolve_lock(ResolveLockRequest {
            note_first_name: &third_first_name,
            decoded_note_data: &decoded,
            signer_pkh: None,
            coinbase_relative_min: None,
        });

        assert_eq!(result.source, LockResolutionSource::Unknown);
        assert_eq!(result.spend_condition, None);
        assert_eq!(result.spend_condition_count, None);
    }

    // KAT: protocol-fund coinbase notes (014-aletheia) do NOT take their
    // first-name directly from the 3-of-4 multisig lock-root (the fund-address).
    // +make-name:coinbase wraps that lock-root in a single `[%pkh m=1
    // {fund-address}]` primitive plus the coinbase relative timelock, then takes
    // the nname/first of the wrapped lock-root. This pins the Rust derivation
    // (FirstName::from_lock_root(coinbase_pkh(fund_address, 100).hash())) against
    // the +fund-address and +fund-note-firstname constants in
    // hoon/common/tx-engine-1.hoon, the values the on-chain notes actually carry.
    #[test]
    fn fund_note_firstname_derives_through_coinbase_pkh_wrapping() {
        const FUND_ADDRESS_B58: &str = "9EhcJiGhAPcWLYrR9DL4ZPjU2Z9XT6FT2ZFkEEwmSQv7ES2TMC7p6Up";
        const FUND_NOTE_FIRSTNAME_B58: &str =
            "8TvVfU7sbFoY8qV53ffUdBag7Kcqw8LXjsnYgY71nQ1biWE6giRYzkn";
        const COINBASE_TIMELOCK_MIN: u64 = 100;

        let fund_address = Hash::from_base58(FUND_ADDRESS_B58).expect("fund address");
        let wrapped = SpendCondition::coinbase_pkh(fund_address, COINBASE_TIMELOCK_MIN);
        let note_lock_root = wrapped.hash().expect("note lock root");
        let fund_note_firstname = FirstName::from_lock_root(&note_lock_root)
            .expect("fund note first-name")
            .into_hash();

        assert_eq!(fund_note_firstname.to_base58(), FUND_NOTE_FIRSTNAME_B58);
    }

    /// Derives the protocol-fund-style coinbase-wrapped first-name for a lock
    /// root the same way `+make-name:coinbase` does: wrap the lock-root in a
    /// single `[%pkh m=1 {lock_root}]` primitive plus the coinbase relative
    /// timelock, then take the first-name of the wrapped lock-root.
    fn coinbase_wrapped_first_name(lock_root: &Hash, coinbase_timelock_min: u64) -> Hash {
        let wrapped = SpendCondition::coinbase_pkh(lock_root.clone(), coinbase_timelock_min);
        let note_lock_root = wrapped.hash().expect("wrapped lock root");
        FirstName::from_lock_root(&note_lock_root)
            .expect("wrapped first-name")
            .into_hash()
    }

    #[test]
    fn lock_root_matcher_selects_coinbase_wrapped_fund_notes() {
        // A protocol-fund spend: the on-chain notes carry the coinbase-wrapped
        // first-name, not `from_lock_root(lock_root)`, and carry no canonical
        // lock-data. The matcher must still select them and hand the planner the
        // real multisig spend-condition (the preimage `+check-multisig-lock`
        // requires), mirroring the `+fund-note-firstname` routing.
        const FUND_NOTE_FIRSTNAME_B58: &str =
            "8TvVfU7sbFoY8qV53ffUdBag7Kcqw8LXjsnYgY71nQ1biWE6giRYzkn";

        let spend_condition = SpendCondition::simple_pkh(hash(42));
        let lock_root =
            Hash::from_base58(PROTOCOL_FUND_ADDRESS_B58).expect("canonical fund address");
        let matcher = LockRootLockMatcher::from_lock_root(&lock_root)
            .expect("matcher")
            .with_spend_condition(spend_condition.clone())
            .with_coinbase_fund_notes()
            .expect("coinbase fund first-name");

        let wrapped_first_name =
            Hash::from_base58(FUND_NOTE_FIRSTNAME_B58).expect("canonical fund first-name");
        // The wrapped first-name is distinct from the direct lock-root first-name;
        // the pre-fix matcher would have rejected it outright.
        let direct_first_name = FirstName::from_lock_root(&lock_root)
            .expect("first-name")
            .into_hash();
        assert_ne!(wrapped_first_name.to_array(), direct_first_name.to_array());

        let decoded = decoded_note_data(Vec::new());
        let result = matcher.select_v1_candidate(ResolveLockRequest {
            note_first_name: &wrapped_first_name,
            decoded_note_data: &decoded,
            signer_pkh: None,
            coinbase_relative_min: None,
        });

        assert!(result.is_selected());
        assert_eq!(result.source, LockResolutionSource::LockRootFirstName);
        assert_eq!(result.spend_condition, Some(spend_condition));
        assert_eq!(result.spend_condition_count, None);
        assert_eq!(
            result.required_relative_min_blocks,
            Some(PROTOCOL_FUND_NOTE_TIMELOCK_MIN)
        );
    }

    #[test]
    fn lock_root_matcher_still_selects_direct_lock_root_notes_with_fund_support() {
        // Enabling fund-note support must not regress normal multisig notes whose
        // first-name is `from_lock_root(lock_root)` and which carry lock-data.
        let spend_condition = SpendCondition::simple_pkh(hash(42));
        let lock_root = lock_root_for_lock(&spend_condition);
        let matcher = LockRootLockMatcher::from_lock_root(&lock_root)
            .expect("matcher")
            .with_spend_condition(spend_condition.clone())
            .with_coinbase_fund_notes()
            .expect("coinbase fund first-name");
        let direct_first_name = FirstName::from_lock_root(&lock_root)
            .expect("first-name")
            .into_hash();
        let decoded = decoded_note_data(vec![lock_entry(spend_condition.clone())]);

        let result = matcher.select_v1_candidate(ResolveLockRequest {
            note_first_name: &direct_first_name,
            decoded_note_data: &decoded,
            signer_pkh: None,
            coinbase_relative_min: Some(100),
        });

        assert_eq!(result.source, LockResolutionSource::NoteData);
        assert_eq!(result.spend_condition, Some(spend_condition));
        assert_eq!(result.required_relative_min_blocks, None);
    }

    #[test]
    fn lock_root_matcher_fund_support_ignores_arbitrary_wrapped_first_name() {
        let spend_condition = SpendCondition::simple_pkh(hash(42));
        let lock_root = lock_root_for_lock(&spend_condition);
        let wrapped_first_name =
            coinbase_wrapped_first_name(&lock_root, PROTOCOL_FUND_NOTE_TIMELOCK_MIN);
        let matcher = LockRootLockMatcher::from_lock_root(&lock_root)
            .expect("matcher")
            .with_spend_condition(spend_condition)
            .with_coinbase_fund_notes()
            .expect("coinbase fund first-name");
        let decoded = decoded_note_data(Vec::new());

        let result = matcher.select_v1_candidate(ResolveLockRequest {
            note_first_name: &wrapped_first_name,
            decoded_note_data: &decoded,
            signer_pkh: None,
            coinbase_relative_min: None,
        });

        assert_eq!(result.source, LockResolutionSource::Unknown);
        assert!(!result.is_selected());
    }

    #[test]
    fn lock_root_matcher_without_fund_support_ignores_coinbase_wrapped_first_name() {
        // Without `with_coinbase_fund_notes`, the wrapped first-name must not be
        // accepted -- fund support is opt-in so non-fund multisig flows are
        // unchanged.
        let spend_condition = SpendCondition::simple_pkh(hash(42));
        let lock_root = lock_root_for_lock(&spend_condition);
        let matcher = LockRootLockMatcher::from_lock_root(&lock_root)
            .expect("matcher")
            .with_spend_condition(spend_condition);
        let wrapped_first_name = coinbase_wrapped_first_name(&lock_root, 100);
        let decoded = decoded_note_data(Vec::new());

        let result = matcher.select_v1_candidate(ResolveLockRequest {
            note_first_name: &wrapped_first_name,
            decoded_note_data: &decoded,
            signer_pkh: None,
            coinbase_relative_min: Some(100),
        });

        assert_eq!(result.source, LockResolutionSource::Unknown);
        assert!(!result.is_selected());
    }

    #[test]
    fn lock_root_matcher_matches_note_first_name_derived_from_lock_root() {
        let spend_condition = SpendCondition::simple_pkh(hash(42));
        let lock_root = lock_root_for_lock(&spend_condition);
        let matcher = LockRootLockMatcher::from_lock_root(&lock_root).expect("matcher");
        let note_first_name = FirstName::from_lock_root(&lock_root)
            .expect("first-name")
            .into_hash();

        assert!(matcher.matches(&note_first_name, &spend_condition));
        assert!(!matcher.matches(&hash(999), &spend_condition));
    }

    #[test]
    fn lock_root_matcher_resolves_single_leaf_note_data() {
        let spend_condition = SpendCondition::simple_pkh(hash(42));
        let lock_root = lock_root_for_lock(&spend_condition);
        let matcher = LockRootLockMatcher::from_lock_root(&lock_root).expect("matcher");
        let decoded = decoded_note_data(vec![lock_entry(spend_condition.clone())]);
        let note_first_name = FirstName::from_lock_root(&lock_root)
            .expect("first-name")
            .into_hash();

        let result = matcher.resolve_lock(ResolveLockRequest {
            note_first_name: &note_first_name,
            decoded_note_data: &decoded,
            signer_pkh: Some(&hash(7)),
            coinbase_relative_min: Some(5),
        });

        assert_eq!(result.source, LockResolutionSource::NoteData);
        assert_eq!(result.spend_condition, Some(spend_condition));
        assert_eq!(result.spend_condition_count, None);
    }

    #[test]
    fn lock_root_matcher_does_not_use_reconstruction_when_lock_data_is_missing() {
        let spend_condition = SpendCondition::simple_pkh(hash(42));
        let lock_root = lock_root_for_lock(&spend_condition);
        let matcher = LockRootLockMatcher::from_lock_root(&lock_root).expect("matcher");
        let decoded = decoded_note_data(Vec::new());
        let note_first_name = FirstName::from_lock_root(&lock_root)
            .expect("first-name")
            .into_hash();

        let result = matcher.resolve_lock(ResolveLockRequest {
            note_first_name: &note_first_name,
            decoded_note_data: &decoded,
            signer_pkh: Some(&hash(42)),
            coinbase_relative_min: Some(1),
        });

        assert_eq!(result.source, LockResolutionSource::Unknown);
        assert_eq!(result.spend_condition, None);
    }

    #[test]
    fn lock_root_matcher_selects_by_first_name_without_note_data() {
        let spend_condition = SpendCondition::simple_pkh(hash(42));
        let lock_root = lock_root_for_lock(&spend_condition);
        let matcher = LockRootLockMatcher::from_lock_root(&lock_root).expect("matcher");
        let decoded = decoded_note_data(Vec::new());
        let note_first_name = FirstName::from_lock_root(&lock_root)
            .expect("first-name")
            .into_hash();

        let result = matcher.select_v1_candidate(ResolveLockRequest {
            note_first_name: &note_first_name,
            decoded_note_data: &decoded,
            signer_pkh: None,
            coinbase_relative_min: None,
        });

        assert_eq!(result.source, LockResolutionSource::LockRootFirstName);
        assert_eq!(result.spend_condition, None);
        assert_eq!(result.spend_condition_count, None);
    }

    #[test]
    fn lock_root_matcher_can_supply_planning_spend_condition_when_selecting_by_first_name() {
        let spend_condition = SpendCondition::simple_pkh(hash(42));
        let lock_root = lock_root_for_lock(&spend_condition);
        let matcher = LockRootLockMatcher::from_lock_root(&lock_root)
            .expect("matcher")
            .with_spend_condition(spend_condition.clone());
        let decoded = decoded_note_data(Vec::new());
        let note_first_name = FirstName::from_lock_root(&lock_root)
            .expect("first-name")
            .into_hash();

        let result = matcher.select_v1_candidate(ResolveLockRequest {
            note_first_name: &note_first_name,
            decoded_note_data: &decoded,
            signer_pkh: None,
            coinbase_relative_min: None,
        });

        assert_eq!(result.source, LockResolutionSource::LockRootFirstName);
        assert_eq!(result.spend_condition, Some(spend_condition));
        assert_eq!(result.spend_condition_count, None);
    }

    #[test]
    fn lock_root_matcher_rejects_multi_leaf_lock_payload_for_now() {
        let first = SpendCondition::simple_pkh(hash(9));
        let second = SpendCondition::simple_pkh(hash(10));
        let lock_root = hash(77);
        let matcher = LockRootLockMatcher::from_lock_root(&lock_root).expect("matcher");
        let note_first_name = FirstName::from_lock_root(&lock_root)
            .expect("first-name")
            .into_hash();
        let decoded = decoded_note_data(vec![lock_entry_from_conditions(vec![first, second])]);

        let result = matcher.resolve_lock(ResolveLockRequest {
            note_first_name: &note_first_name,
            decoded_note_data: &decoded,
            signer_pkh: None,
            coinbase_relative_min: None,
        });

        assert_eq!(result.source, LockResolutionSource::Unknown);
        assert_eq!(result.spend_condition, None);
    }

    #[test]
    fn note_data_lock_is_ignored_when_matcher_rejects_it() {
        let note_lock = SpendCondition::simple_pkh(hash(3));
        let decoded = decoded_note_data(vec![lock_entry(note_lock)]);
        let result = NeverMatches.resolve_lock(ResolveLockRequest {
            note_first_name: &hash(1),
            decoded_note_data: &decoded,
            signer_pkh: None,
            coinbase_relative_min: None,
        });

        assert_eq!(result.source, LockResolutionSource::Unknown);
        assert_eq!(result.spend_condition, None);
        assert_eq!(result.spend_condition_count, None);
    }

    #[test]
    fn simple_reconstruction_is_attempted() {
        let pkh = hash(42);
        let expected_simple = SpendCondition::simple_pkh(pkh.clone());
        let matcher = MatchExpected {
            expected: expected_simple.clone(),
        };
        let decoded = decoded_note_data(Vec::new());
        let note_first_name = first_name_for_lock(&expected_simple);

        let result = matcher.resolve_lock(ResolveLockRequest {
            note_first_name: &note_first_name,
            decoded_note_data: &decoded,
            signer_pkh: Some(&pkh),
            coinbase_relative_min: Some(20),
        });

        assert_eq!(result.source, LockResolutionSource::ReconstructedSimplePkh);
        assert_eq!(result.spend_condition, Some(expected_simple));
        assert_eq!(result.spend_condition_count, None);
    }

    #[test]
    fn coinbase_reconstruction_is_attempted_after_simple() {
        let pkh = hash(42);
        let expected_coinbase = SpendCondition::coinbase_pkh(pkh.clone(), 20);
        let matcher = MatchExpected {
            expected: expected_coinbase.clone(),
        };
        let decoded = decoded_note_data(Vec::new());
        let note_first_name = first_name_for_lock(&expected_coinbase);

        let result = matcher.resolve_lock(ResolveLockRequest {
            note_first_name: &note_first_name,
            decoded_note_data: &decoded,
            signer_pkh: Some(&pkh),
            coinbase_relative_min: Some(20),
        });

        assert_eq!(
            result.source,
            LockResolutionSource::ReconstructedCoinbasePkh
        );
        assert_eq!(result.spend_condition, Some(expected_coinbase));
        assert_eq!(result.spend_condition_count, None);
    }

    #[test]
    fn unresolved_locks_return_unknown() {
        let decoded = decoded_note_data(Vec::new());
        let result = NeverMatches.resolve_lock(ResolveLockRequest {
            note_first_name: &hash(1),
            decoded_note_data: &decoded,
            signer_pkh: None,
            coinbase_relative_min: None,
        });

        assert_eq!(result.source, LockResolutionSource::Unknown);
        assert_eq!(result.spend_condition, None);
        assert_eq!(result.spend_condition_count, None);
    }

    #[test]
    fn coinbase_first_name_does_not_take_simple_reconstruction_path() {
        let pkh = hash(42);
        let expected_coinbase = SpendCondition::coinbase_pkh(pkh.clone(), 20);
        let note_first_name = first_name_for_lock(&expected_coinbase);
        let decoded = decoded_note_data(Vec::new());
        let matcher = MatchExpected {
            expected: expected_coinbase.clone(),
        };

        let result = matcher.resolve_lock(ResolveLockRequest {
            note_first_name: &note_first_name,
            decoded_note_data: &decoded,
            signer_pkh: Some(&pkh),
            coinbase_relative_min: Some(20),
        });

        assert_eq!(
            result.source,
            LockResolutionSource::ReconstructedCoinbasePkh
        );
        assert_eq!(result.spend_condition, Some(expected_coinbase));
    }

    #[test]
    fn malformed_lock_entry_falls_back_to_reconstruction() {
        let pkh = hash(42);
        let expected_simple = SpendCondition::simple_pkh(pkh.clone());
        let decoded = decoded_note_data(vec![DecodedNoteDataEntry {
            raw_key: "lock".to_string(),
            normalized_key: NormalizedNoteDataKey::Lock,
            raw_blob: Bytes::new(),
            payload: DecodedNoteDataPayload::Raw,
            decode_error: Some("bad lock".to_string()),
        }]);

        let result = AlwaysMatches.resolve_lock(ResolveLockRequest {
            note_first_name: &first_name_for_lock(&expected_simple),
            decoded_note_data: &decoded,
            signer_pkh: Some(&pkh),
            coinbase_relative_min: Some(20),
        });

        assert_eq!(result.source, LockResolutionSource::ReconstructedSimplePkh);
        assert_eq!(result.spend_condition, Some(expected_simple));
    }

    #[test]
    fn decoded_lock_mismatch_falls_back_to_coinbase_reconstruction() {
        let pkh = hash(42);
        let simple = SpendCondition::simple_pkh(pkh.clone());
        let expected_coinbase = SpendCondition::coinbase_pkh(pkh.clone(), 20);
        let decoded = decoded_note_data(vec![lock_entry(simple)]);
        let matcher = MatchExpected {
            expected: expected_coinbase.clone(),
        };

        let result = matcher.resolve_lock(ResolveLockRequest {
            note_first_name: &first_name_for_lock(&expected_coinbase),
            decoded_note_data: &decoded,
            signer_pkh: Some(&pkh),
            coinbase_relative_min: Some(20),
        });

        assert_eq!(
            result.source,
            LockResolutionSource::ReconstructedCoinbasePkh
        );
        assert_eq!(result.spend_condition, Some(expected_coinbase));
    }

    #[test]
    fn coinbase_reconstruction_uses_configured_relative_min_only() {
        let pkh = hash(42);
        let expected_legacy_coinbase = SpendCondition::coinbase_pkh(pkh.clone(), 100);
        let decoded = decoded_note_data(Vec::new());
        let matcher = MatchExpected {
            expected: expected_legacy_coinbase.clone(),
        };

        let result = matcher.resolve_lock(ResolveLockRequest {
            note_first_name: &first_name_for_lock(&expected_legacy_coinbase),
            decoded_note_data: &decoded,
            signer_pkh: Some(&pkh),
            coinbase_relative_min: Some(1),
        });

        assert_eq!(result.source, LockResolutionSource::Unknown);
        assert_eq!(result.spend_condition, None);
    }
}
