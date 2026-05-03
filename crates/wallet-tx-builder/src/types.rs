use bytes::Bytes;
use nockchain_types::tx_engine::common::{BlockHeight, Hash, Name, Nicks, SchnorrPubkey, Version};
use nockchain_types::tx_engine::v0::{Lock as V0Lock, TimelockIntent as V0TimelockIntent};
use nockchain_types::tx_engine::v1::note::Note;
use nockchain_types::tx_engine::v1::tx::SpendCondition;

use crate::note_data::DecodedNoteData;

/// Candidate-selection mode for planner input admission and ordering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionMode {
    /// Manual candidate mode with an explicit candidate set provided by name.
    /// Ordering is still governed by `SelectionOrder`.
    Manual { note_names: Vec<Name> },
    /// Automatic candidate mode that orders from the normalized candidate set.
    Auto,
}

/// Refers to the order of the `assets` in each note when selecting candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionOrder {
    Ascending,
    Descending,
}

/// Controls which note versions the selector is allowed to consume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateVersionPolicy {
    /// Default mode: selector chooses only v1 notes.
    V1Only,
    /// Special v0 fan-in mode: selector chooses only v0 notes.
    V0Only,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Chain-level fee and height context used for planner decisions.
pub struct ChainContext {
    /// Current chain height used for fee rules and timelock checks.
    pub height: BlockHeight,
    /// Activation height for Bythos-era fee/seed accounting behavior.
    pub bythos_phase: BlockHeight,
    /// Per-word fee unit.
    pub base_fee: u64,
    /// Witness fee discount divisor applied post-Bythos.
    pub input_fee_divisor: u64,
    /// Global minimum fee floor.
    pub min_fee: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Stable identity for a note candidate independent of decode/render details.
pub struct CandidateIdentity {
    /// Full note name `[first last]`.
    pub name: Name,
    /// Block/page where this note originated.
    pub origin_page: BlockHeight,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Raw key/value note-data entry preserved for round-tripping and fee accounting.
pub struct RawNoteDataEntry {
    /// Note-data key (e.g. `lock`, `bridge`, `bridge-w`).
    pub key: String,
    /// Jammed payload bytes for this key.
    pub blob: Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Candidate payload for a legacy v0 note.
pub struct CandidateV0Note {
    /// Stable identity metadata for this candidate note.
    pub identity: CandidateIdentity,
    /// Spendable asset amount carried by this note.
    pub assets: Nicks,
    /// Legacy v0 signing lock used for migration spendability checks.
    pub lock: V0Lock,
    /// Legacy v0 timelock constraints applied to this note.
    pub timelock: Option<V0TimelockIntent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Candidate payload for a v1 note including decoded note-data views.
pub struct CandidateV1Note {
    /// Stable identity metadata for this candidate note.
    pub identity: CandidateIdentity,
    /// Spendable asset amount carried by this note.
    pub assets: Nicks,
    /// Raw note-data entries preserved from wallet snapshot.
    pub raw_note_data: Vec<RawNoteDataEntry>,
    /// Best-effort decoded note-data view used by lock/timelock logic.
    pub decoded_note_data: DecodedNoteData,
}

/// Shared empty decoded note-data view for v0 candidates.
static EMPTY_DECODED_NOTE_DATA: DecodedNoteData = DecodedNoteData(Vec::new());

#[derive(Debug, Clone, PartialEq, Eq)]
/// Version-dispatched candidate note used by planner selection and filtering.
pub enum CandidateNote {
    V0(CandidateV0Note),
    V1(CandidateV1Note),
}

impl CandidateNote {
    /// Builds a candidate note from a decoded tx-engine note value.
    pub fn from_note(name: &Name, note: &Note) -> Self {
        match note {
            Note::V0(note_v0) => CandidateNote::V0(CandidateV0Note {
                identity: CandidateIdentity {
                    name: name.clone(),
                    origin_page: note_v0.head.origin_page.clone(),
                },
                assets: note_v0.tail.assets.clone(),
                lock: note_v0.tail.lock.clone(),
                timelock: note_v0.head.timelock.0.clone(),
            }),
            Note::V1(note_v1) => {
                let raw_note_data = note_v1
                    .note_data
                    .iter()
                    .map(|entry| RawNoteDataEntry {
                        key: entry.key.clone(),
                        blob: entry.blob.clone(),
                    })
                    .collect::<Vec<_>>();
                let decoded_note_data = DecodedNoteData::from(&note_v1.note_data);
                CandidateNote::V1(CandidateV1Note {
                    identity: CandidateIdentity {
                        name: name.clone(),
                        origin_page: note_v1.origin_page.clone(),
                    },
                    assets: note_v1.assets.clone(),
                    raw_note_data,
                    decoded_note_data,
                })
            }
        }
    }

    /// Returns the stable identity metadata shared across candidate versions.
    pub fn identity(&self) -> &CandidateIdentity {
        match self {
            CandidateNote::V0(note) => &note.identity,
            CandidateNote::V1(note) => &note.identity,
        }
    }

    /// Returns mutable identity metadata used by planner tests and transforms.
    pub fn identity_mut(&mut self) -> &mut CandidateIdentity {
        match self {
            CandidateNote::V0(note) => &mut note.identity,
            CandidateNote::V1(note) => &mut note.identity,
        }
    }

    /// Returns this candidate's spendable asset amount.
    pub fn assets(&self) -> &Nicks {
        match self {
            CandidateNote::V0(note) => &note.assets,
            CandidateNote::V1(note) => &note.assets,
        }
    }

    /// Returns decoded note-data entries; v0 notes expose an empty decoded view.
    pub fn decoded_note_data(&self) -> &DecodedNoteData {
        match self {
            CandidateNote::V0(_) => &EMPTY_DECODED_NOTE_DATA,
            CandidateNote::V1(note) => &note.decoded_note_data,
        }
    }

    /// Returns raw note-data entries; v0 notes expose an empty slice.
    pub fn raw_note_data(&self) -> &[RawNoteDataEntry] {
        match self {
            CandidateNote::V0(_) => &[],
            CandidateNote::V1(note) => &note.raw_note_data,
        }
    }

    /// Returns this candidate note version.
    pub fn version(&self) -> Version {
        match self {
            CandidateNote::V0(_) => Version::V0,
            CandidateNote::V1(_) => Version::V1,
        }
    }

    /// Returns this note name as base58 `(first, last)` for logging and errors.
    pub fn note_name_display(&self) -> (String, String) {
        (
            self.identity().name.first.to_base58(),
            self.identity().name.last.to_base58(),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Planner output target with amount and optional note-data payloads.
pub struct PlannedOutput {
    /// Destination lock root for this output.
    pub lock_root: Hash,
    /// Output amount assigned by planner/allocation.
    pub amount: u64,
    /// Output note-data that contributes to seed-word accounting.
    pub note_data: Vec<RawNoteDataEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Planner mode selects between fixed-recipient create-tx and legacy v0 sweep migration.
pub enum PlanningMode {
    /// Standard create-tx planner behavior with fixed recipient outputs and optional refund.
    Standard,
    /// Full-sweep migration mode that spends all admissible v0 notes into one destination output.
    V0MigrationSweep {
        /// Destination output template used for fee accounting and final emission.
        destination_output: PlannedOutput,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Assembled transaction input pairing selected note identity and spend lock.
pub struct AssembledInput {
    pub note: CandidateIdentity,
    pub lock: SpendCondition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Assembled transaction output containing lock root and assigned amount.
pub struct AssembledOutput {
    pub lock_root: Hash,
    pub amount: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Assembled transaction shape used by downstream tx construction steps.
pub struct AssembledTransaction {
    pub inputs: Vec<AssembledInput>,
    pub outputs: Vec<AssembledOutput>,
    pub fee: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Full planner input bundle for deterministic create-tx planning.
pub struct PlanRequest {
    /// Planner mode controls output/funding semantics.
    pub planning_mode: PlanningMode,
    /// Selection mode: caller-specified manual candidate set (by note names) or automatic candidate search.
    pub selection_mode: SelectionMode,
    /// Order used when sorting candidate notes by their `assets` value.
    pub order_direction: SelectionOrder,
    /// Whether output note-data should be included in assembled outputs.
    pub include_data: bool,
    /// Chain fee/timelock context.
    pub chain_context: ChainContext,
    /// Primary signer PKH used for spendability filtering.
    pub signer_pkh: Option<Hash>,
    /// Candidate note version policy used by selector admission checks.
    pub candidate_version_policy: CandidateVersionPolicy,
    /// Normalized candidate notes from a single wallet snapshot.
    pub candidates: Vec<CandidateNote>,
    /// Recipient outputs requested by the command.
    pub recipient_outputs: Vec<PlannedOutput>,
    /// Refund output template; amount is set by planner and must always exist.
    pub refund_output: PlannedOutput,
    /// Relative timelock minimum used to classify coinbase-style locks.
    pub coinbase_relative_min: Option<u64>,
    /// Signer pubkeys used by v0 migration admission checks.
    pub v0_migration_signer_pubkeys: Vec<SchnorrPubkey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Seed/witness word-count totals used to justify final fee decisions.
pub struct WordCountBreakdown {
    /// Seed-side word count used for fee computation.
    pub seed_words: u64,
    /// Witness-side word count used for fee computation.
    pub witness_words: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Final planner output including selected inputs, outputs, and fee details.
pub struct PlanResult {
    /// Final selected input identities in deterministic spend order.
    pub selected: Vec<CandidateIdentity>,
    /// Total assets represented by selected inputs.
    pub selected_total: u64,
    /// Final output set (recipients plus optional refund).
    pub outputs: Vec<PlannedOutput>,
    /// Planner-computed final fee.
    pub final_fee: u64,
    /// Word-count components backing final fee.
    pub word_counts: WordCountBreakdown,
    /// Human-readable planner decisions for debugging.
    pub debug_trace: Vec<String>,
}
