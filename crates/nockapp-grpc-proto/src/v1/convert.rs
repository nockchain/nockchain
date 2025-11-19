use anyhow::Result;
use nockchain_math::belt::Belt as DBelt;
use nockchain_math::crypto::cheetah::{CheetahPoint as DCheetahPoint, F6lt as DF6lt};
use nockchain_types::tx_engine::v0;

use crate::common::{ConversionError, RPCErrorStatus, Required};
use crate::pb::common::v1::{
    time_lock_intent, BalanceEntry, Belt, BlockHeight, BlockHeightDelta, CheetahPoint, EightBelt,
    ErrorStatus, Hash, Input as PbInput, Lock, Name, NamedInput as PbNamedInput, Nicks, Note,
    NoteVersion, OutputSource, RawTransaction as PbRawTransaction, SchnorrPubkey,
    SchnorrSignature as PbSchnorrSignature, Seed as PbSeed, Signature as PbSignature,
    SignatureEntry as PbSignatureEntry, Source, Spend as PbSpend, TimeLockIntent,
    TimeLockRangeAbsolute, TimeLockRangeAbsoluteAndRelative, TimeLockRangeNeither,
    TimeLockRangeRelative, WalletBalanceData,
};
use crate::pb::public::v1::{wallet_get_balance_response, WalletGetBalanceResponse};
// =========================
// Helper: wrapper conversions (pb <-> domain)
// =========================

impl From<DBelt> for Belt {
    fn from(b: DBelt) -> Self {
        Belt { value: b.0 }
    }
}

impl From<Belt> for DBelt {
    fn from(b: Belt) -> Self {
        DBelt(b.value)
    }
}

impl From<[DBelt; 8]> for EightBelt {
    fn from(belts: [DBelt; 8]) -> Self {
        EightBelt {
            belt_1: Some(Belt::from(belts[0])),
            belt_2: Some(Belt::from(belts[1])),
            belt_3: Some(Belt::from(belts[2])),
            belt_4: Some(Belt::from(belts[3])),
            belt_5: Some(Belt::from(belts[4])),
            belt_6: Some(Belt::from(belts[5])),
            belt_7: Some(Belt::from(belts[6])),
            belt_8: Some(Belt::from(belts[7])),
        }
    }
}

impl TryFrom<EightBelt> for [DBelt; 8] {
    type Error = ConversionError;

    fn try_from(value: EightBelt) -> Result<Self, Self::Error> {
        Ok([
            value.belt_1.required("EightBelt", "belt_1")?.into(),
            value.belt_2.required("EightBelt", "belt_2")?.into(),
            value.belt_3.required("EightBelt", "belt_3")?.into(),
            value.belt_4.required("EightBelt", "belt_4")?.into(),
            value.belt_5.required("EightBelt", "belt_5")?.into(),
            value.belt_6.required("EightBelt", "belt_6")?.into(),
            value.belt_7.required("EightBelt", "belt_7")?.into(),
            value.belt_8.required("EightBelt", "belt_8")?.into(),
        ])
    }
}

impl From<DF6lt> for crate::pb::common::v1::SixBelt {
    fn from(f: DF6lt) -> Self {
        crate::pb::common::v1::SixBelt {
            belt_1: Some(Belt::from(f.0[0])),
            belt_2: Some(Belt::from(f.0[1])),
            belt_3: Some(Belt::from(f.0[2])),
            belt_4: Some(Belt::from(f.0[3])),
            belt_5: Some(Belt::from(f.0[4])),
            belt_6: Some(Belt::from(f.0[5])),
        }
    }
}

impl TryFrom<crate::pb::common::v1::SixBelt> for DF6lt {
    type Error = ConversionError;
    fn try_from(v: crate::pb::common::v1::SixBelt) -> Result<Self, Self::Error> {
        Ok(DF6lt([
            v.belt_1.required("SixBelt", "belt_1")?.into(),
            v.belt_2.required("SixBelt", "belt_2")?.into(),
            v.belt_3.required("SixBelt", "belt_3")?.into(),
            v.belt_4.required("SixBelt", "belt_4")?.into(),
            v.belt_5.required("SixBelt", "belt_5")?.into(),
            v.belt_6.required("SixBelt", "belt_6")?.into(),
        ]))
    }
}

impl From<DCheetahPoint> for CheetahPoint {
    fn from(p: DCheetahPoint) -> Self {
        CheetahPoint {
            x: Some(crate::pb::common::v1::SixBelt::from(p.x)),
            y: Some(crate::pb::common::v1::SixBelt::from(p.y)),
            inf: p.inf,
        }
    }
}

impl TryFrom<CheetahPoint> for DCheetahPoint {
    type Error = ConversionError;
    fn try_from(p: CheetahPoint) -> Result<Self, Self::Error> {
        Ok(DCheetahPoint {
            x: DF6lt::try_from(p.x.required("CheetahPoint", "x")?)?,
            y: DF6lt::try_from(p.y.required("CheetahPoint", "y")?)?,
            inf: p.inf,
        })
    }
}

impl From<v0::SchnorrPubkey> for SchnorrPubkey {
    fn from(pk: v0::SchnorrPubkey) -> Self {
        SchnorrPubkey {
            value: Some(CheetahPoint::from(pk.0)),
        }
    }
}

impl TryFrom<SchnorrPubkey> for v0::SchnorrPubkey {
    type Error = ConversionError;
    fn try_from(pk: SchnorrPubkey) -> Result<Self, Self::Error> {
        Ok(v0::SchnorrPubkey(DCheetahPoint::try_from(
            pk.value.required("SchnorrPubkey", "value")?,
        )?))
    }
}

impl From<v0::SchnorrSignature> for PbSchnorrSignature {
    fn from(sig: v0::SchnorrSignature) -> Self {
        PbSchnorrSignature {
            chal: Some(EightBelt::from(sig.chal)),
            sig: Some(EightBelt::from(sig.sig)),
        }
    }
}

impl TryFrom<PbSchnorrSignature> for v0::SchnorrSignature {
    type Error = ConversionError;

    fn try_from(sig: PbSchnorrSignature) -> Result<Self, Self::Error> {
        let chal = sig.chal.required("SchnorrSignature", "chal")?.try_into()?;
        let sig_val = sig.sig.required("SchnorrSignature", "sig")?.try_into()?;
        Ok(v0::SchnorrSignature { chal, sig: sig_val })
    }
}

impl From<v0::Signature> for PbSignature {
    fn from(signature: v0::Signature) -> Self {
        let entries = signature
            .0
            .into_iter()
            .map(|(pk, sig)| PbSignatureEntry {
                schnorr_pubkey: Some(SchnorrPubkey::from(pk)),
                signature: Some(PbSchnorrSignature::from(sig)),
            })
            .collect();
        PbSignature { entries }
    }
}

impl TryFrom<PbSignature> for v0::Signature {
    type Error = ConversionError;

    fn try_from(signature: PbSignature) -> Result<Self, Self::Error> {
        let mut entries = Vec::with_capacity(signature.entries.len());
        for entry in signature.entries {
            let pk: v0::SchnorrPubkey = entry
                .schnorr_pubkey
                .required("SignatureEntry", "schnorr_pubkey")?
                .try_into()?;
            let sig: v0::SchnorrSignature = entry
                .signature
                .required("SignatureEntry", "signature")?
                .try_into()?;
            entries.push((pk, sig));
        }
        Ok(v0::Signature(entries))
    }
}

impl From<v0::Spend> for PbSpend {
    fn from(spend: v0::Spend) -> Self {
        PbSpend {
            signature: spend.signature.map(PbSignature::from),
            seeds: spend.seeds.seeds.into_iter().map(PbSeed::from).collect(),
            miner_fee_nicks: Some(Nicks::from(spend.fee)),
        }
    }
}

impl TryFrom<PbSpend> for v0::Spend {
    type Error = ConversionError;

    fn try_from(spend: PbSpend) -> Result<Self, Self::Error> {
        let signature = spend.signature.map(TryInto::try_into).transpose()?;
        let seeds = spend
            .seeds
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()?;
        let seeds = v0::Seeds { seeds };
        let fee: v0::Nicks = spend
            .miner_fee_nicks
            .required("Spend", "miner_fee_nicks")?
            .into();
        Ok(v0::Spend {
            signature,
            seeds,
            fee,
        })
    }
}

impl From<v0::Hash> for Hash {
    fn from(h: v0::Hash) -> Self {
        Hash {
            belt_1: Some(Belt::from(h.0[0])),
            belt_2: Some(Belt::from(h.0[1])),
            belt_3: Some(Belt::from(h.0[2])),
            belt_4: Some(Belt::from(h.0[3])),
            belt_5: Some(Belt::from(h.0[4])),
        }
    }
}

impl TryFrom<Hash> for v0::Hash {
    type Error = ConversionError;
    fn try_from(h: Hash) -> Result<Self, Self::Error> {
        Ok(v0::Hash([
            h.belt_1.required("Hash", "belt_1")?.into(),
            h.belt_2.required("Hash", "belt_2")?.into(),
            h.belt_3.required("Hash", "belt_3")?.into(),
            h.belt_4.required("Hash", "belt_4")?.into(),
            h.belt_5.required("Hash", "belt_5")?.into(),
        ]))
    }
}

impl From<v0::Name> for Name {
    fn from(name: v0::Name) -> Self {
        Name {
            first: Some(Hash::from(name.first)),
            last: Some(Hash::from(name.last)),
        }
    }
}

impl TryFrom<Name> for v0::Name {
    type Error = ConversionError;
    fn try_from(name: Name) -> Result<Self, Self::Error> {
        let first: v0::Hash = name.first.required("Name", "first")?.try_into()?;
        let last: v0::Hash = name.last.required("Name", "last")?.try_into()?;
        Ok(v0::Name::new(first, last))
    }
}

impl From<v0::Lock> for Lock {
    fn from(lock: v0::Lock) -> Self {
        Lock {
            keys_required: lock.keys_required as u32,
            schnorr_pubkeys: lock.pubkeys.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<Lock> for v0::Lock {
    type Error = ConversionError;
    fn try_from(lock: Lock) -> Result<Self, Self::Error> {
        Ok(v0::Lock {
            keys_required: lock.keys_required as u64,
            pubkeys: lock
                .schnorr_pubkeys
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

impl From<v0::Source> for Source {
    fn from(source: v0::Source) -> Self {
        Source {
            hash: Some(Hash::from(source.hash)),
            coinbase: source.is_coinbase,
        }
    }
}

impl TryFrom<Source> for v0::Source {
    type Error = ConversionError;
    fn try_from(source: Source) -> Result<Self, Self::Error> {
        Ok(v0::Source {
            hash: source.hash.required("Source", "hash")?.try_into()?,
            is_coinbase: source.coinbase,
        })
    }
}

impl From<v0::Seed> for PbSeed {
    fn from(seed: v0::Seed) -> Self {
        PbSeed {
            output_source: seed.output_source.map(|source| OutputSource {
                source: Some(Source::from(source)),
            }),
            recipient: Some(Lock::from(seed.recipient)),
            timelock_intent: seed
                .timelock_intent
                .map(|intent| TimeLockIntent::from(v0::Timelock(Some(intent)))),
            gift: Some(Nicks::from(seed.gift)),
            parent_hash: Some(Hash::from(seed.parent_hash)),
        }
    }
}

impl TryFrom<PbSeed> for v0::Seed {
    type Error = ConversionError;

    fn try_from(seed: PbSeed) -> Result<Self, Self::Error> {
        let output_source = match seed.output_source {
            Some(output) => {
                let source = output
                    .source
                    .required("Seed", "output_source.source")?
                    .try_into()?;
                Some(source)
            }
            None => None,
        };

        let recipient: v0::Lock = seed.recipient.required("Seed", "recipient")?.try_into()?;

        let timelock_intent = seed
            .timelock_intent
            .map(
                |intent| -> Result<Option<v0::TimelockIntent>, ConversionError> {
                    let timelock: v0::Timelock = intent.try_into()?;
                    Ok(timelock.0)
                },
            )
            .transpose()?
            .flatten();

        let gift: v0::Nicks = seed.gift.required("Seed", "gift")?.into();

        let parent_hash: v0::Hash = seed
            .parent_hash
            .required("Seed", "parent_hash")?
            .try_into()?;

        Ok(v0::Seed {
            output_source,
            recipient,
            timelock_intent,
            gift,
            parent_hash,
        })
    }
}

impl From<v0::Input> for PbInput {
    fn from(input: v0::Input) -> Self {
        PbInput {
            note: Some(Note::from(input.note)),
            spend: Some(PbSpend::from(input.spend)),
        }
    }
}

impl TryFrom<PbInput> for v0::Input {
    type Error = ConversionError;

    fn try_from(input: PbInput) -> Result<Self, Self::Error> {
        Ok(v0::Input {
            note: input.note.required("Input", "note")?.try_into()?,
            spend: input.spend.required("Input", "spend")?.try_into()?,
        })
    }
}

impl From<(v0::Name, v0::Input)> for PbNamedInput {
    fn from((name, input): (v0::Name, v0::Input)) -> Self {
        PbNamedInput {
            name: Some(Name::from(name)),
            input: Some(PbInput::from(input)),
        }
    }
}

impl TryFrom<PbNamedInput> for (v0::Name, v0::Input) {
    type Error = ConversionError;

    fn try_from(named: PbNamedInput) -> Result<Self, Self::Error> {
        let name = named.name.required("NamedInput", "name")?.try_into()?;
        let input = named.input.required("NamedInput", "input")?.try_into()?;
        Ok((name, input))
    }
}

impl From<v0::RawTx> for PbRawTransaction {
    fn from(tx: v0::RawTx) -> Self {
        PbRawTransaction {
            named_inputs: tx.inputs.0.into_iter().map(PbNamedInput::from).collect(),
            timelock_range: Some(TimeLockRangeAbsolute::from(tx.timelock_range)),
            total_fees: Some(Nicks::from(tx.total_fees)),
            id: Some(Hash::from(tx.id)),
        }
    }
}

impl TryFrom<PbRawTransaction> for v0::RawTx {
    type Error = ConversionError;

    fn try_from(tx: PbRawTransaction) -> Result<Self, Self::Error> {
        let inputs_vec = tx
            .named_inputs
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()?;
        let inputs = v0::Inputs(inputs_vec);
        let timelock_range: v0::TimelockRangeAbsolute = tx
            .timelock_range
            .required("RawTransaction", "timelock_range")?
            .into();
        let total_fees: v0::Nicks = tx
            .total_fees
            .required("RawTransaction", "total_fees")?
            .into();
        let id: v0::Hash = tx.id.required("RawTransaction", "id")?.try_into()?;

        Ok(v0::RawTx {
            id,
            inputs,
            timelock_range,
            total_fees,
        })
    }
}

impl From<v0::BlockHeight> for BlockHeight {
    fn from(h: v0::BlockHeight) -> Self {
        BlockHeight { value: (h.0).0 }
    }
}

impl From<BlockHeight> for v0::BlockHeight {
    fn from(h: BlockHeight) -> Self {
        v0::BlockHeight(DBelt(h.value))
    }
}

impl From<BlockHeightDelta> for v0::BlockHeightDelta {
    fn from(h: BlockHeightDelta) -> Self {
        v0::BlockHeightDelta(DBelt(h.value))
    }
}

impl From<v0::Nicks> for Nicks {
    fn from(n: v0::Nicks) -> Self {
        Nicks { value: n.0 as u64 }
    }
}

impl From<v0::BlockHeightDelta> for BlockHeightDelta {
    fn from(d: v0::BlockHeightDelta) -> Self {
        BlockHeightDelta { value: d.0 .0 }
    }
}

impl From<Nicks> for v0::Nicks {
    fn from(n: Nicks) -> Self {
        v0::Nicks(n.value as usize)
    }
}

impl From<v0::Version> for NoteVersion {
    fn from(v: v0::Version) -> Self {
        NoteVersion { value: v.into() }
    }
}

impl From<NoteVersion> for v0::Version {
    fn from(v: NoteVersion) -> Self {
        v0::Version::from(v.value)
    }
}

impl From<v0::TimelockRangeAbsolute> for TimeLockRangeAbsolute {
    fn from(range: v0::TimelockRangeAbsolute) -> Self {
        TimeLockRangeAbsolute {
            min: range.min.map(Into::into),
            max: range.max.map(Into::into),
        }
    }
}

impl From<TimeLockRangeAbsolute> for v0::TimelockRangeAbsolute {
    fn from(range: TimeLockRangeAbsolute) -> Self {
        v0::TimelockRangeAbsolute::new(
            range.min.map(|v| v.try_into().unwrap()),
            range.max.map(|v| v.try_into().unwrap()),
        )
    }
}

impl From<v0::TimelockRangeRelative> for TimeLockRangeRelative {
    fn from(range: v0::TimelockRangeRelative) -> Self {
        TimeLockRangeRelative {
            min: range.min.map(Into::into),
            max: range.max.map(Into::into),
        }
    }
}

impl From<TimeLockRangeRelative> for v0::TimelockRangeRelative {
    fn from(range: TimeLockRangeRelative) -> Self {
        v0::TimelockRangeRelative::new(
            range.min.map(|v| v.try_into().unwrap()),
            range.max.map(|v| v.try_into().unwrap()),
        )
    }
}

// Local helpers: None if both ends are None, otherwise Some(mapped range)
fn abs_range_to_opt(range: v0::TimelockRangeAbsolute) -> Option<TimeLockRangeAbsolute> {
    if range.min.is_none() && range.max.is_none() {
        None
    } else {
        Some(TimeLockRangeAbsolute::from(range))
    }
}

fn rel_range_to_opt(range: v0::TimelockRangeRelative) -> Option<TimeLockRangeRelative> {
    if range.min.is_none() && range.max.is_none() {
        None
    } else {
        Some(TimeLockRangeRelative::from(range))
    }
}

impl From<v0::Timelock> for TimeLockIntent {
    fn from(tl: v0::Timelock) -> Self {
        let value = match tl.0 {
            None => None,
            Some(intent) => {
                let abs = abs_range_to_opt(intent.absolute);
                let rel = rel_range_to_opt(intent.relative);
                match (abs, rel) {
                    (None, None) => Some(time_lock_intent::Value::Neither(TimeLockRangeNeither {})),
                    (Some(a), None) => Some(time_lock_intent::Value::Absolute(a)),
                    (None, Some(r)) => Some(time_lock_intent::Value::Relative(r)),
                    (Some(a), Some(r)) => Some(time_lock_intent::Value::AbsoluteAndRelative(
                        TimeLockRangeAbsoluteAndRelative {
                            absolute: Some(a),
                            relative: Some(r),
                        },
                    )),
                }
            }
        };
        TimeLockIntent { value }
    }
}

impl TryFrom<TimeLockIntent> for v0::Timelock {
    type Error = ConversionError;
    fn try_from(intent: TimeLockIntent) -> Result<Self, Self::Error> {
        let tl = match intent.value {
            Some(time_lock_intent::Value::Absolute(abs)) => {
                v0::Timelock(Some(v0::TimelockIntent {
                    absolute: abs.into(),
                    relative: v0::TimelockRangeRelative::none(),
                }))
            }
            Some(time_lock_intent::Value::Relative(rel)) => {
                v0::Timelock(Some(v0::TimelockIntent {
                    absolute: v0::TimelockRangeAbsolute::none(),
                    relative: rel.into(),
                }))
            }
            Some(time_lock_intent::Value::AbsoluteAndRelative(both)) => {
                let abs = both.absolute.required("AbsoluteAndRelative", "absolute")?;
                let rel = both.relative.required("AbsoluteAndRelative", "relative")?;
                v0::Timelock(Some(v0::TimelockIntent {
                    absolute: abs.into(),
                    relative: rel.into(),
                }))
            }
            Some(time_lock_intent::Value::Neither(..)) => {
                v0::Timelock(Some(v0::TimelockIntent::none()))
            }
            None => v0::Timelock(None),
        };
        Ok(tl)
    }
}

impl From<v0::NoteV0> for Note {
    fn from(n: v0::NoteV0) -> Self {
        Note {
            origin_page: Some(BlockHeight::from(n.head.origin_page)),
            timelock: match &n.head.timelock.0 {
                Some(_) => Some(TimeLockIntent::from(n.head.timelock)),
                None => None,
            },
            name: Some(Name::from(n.tail.name)),
            lock: Some(Lock::from(n.tail.lock)),
            source: Some(Source::from(n.tail.source)),
            assets: Some(Nicks::from(n.tail.assets)),
            version: Some(NoteVersion::from(n.head.version)),
        }
    }
}

impl TryFrom<Note> for v0::NoteV0 {
    type Error = ConversionError;
    fn try_from(n: Note) -> Result<Self, Self::Error> {
        Ok(v0::NoteV0 {
            head: v0::NoteHead {
                version: n.version.required("Note", "version")?.into(),
                origin_page: n.origin_page.required("Note", "origin_page")?.into(),
                timelock: n
                    .timelock
                    .map(TryInto::try_into)
                    .transpose()?
                    .unwrap_or(v0::Timelock(None)),
            },
            tail: v0::NoteTail {
                name: n.name.required("Note", "name")?.try_into()?,
                lock: n.lock.required("Note", "lock")?.try_into()?,
                source: n.source.required("Note", "source")?.try_into()?,
                assets: n.assets.required("Note", "assets")?.into(),
            },
        })
    }
}

impl TryFrom<WalletBalanceData> for v0::BalanceUpdate {
    type Error = anyhow::Error;

    fn try_from(update: WalletBalanceData) -> Result<Self> {
        let notes = update
            .notes
            .into_iter()
            .map(|entry| {
                let name: v0::Name = entry
                    .name
                    .required("WalletBalanceData", "name")?
                    .try_into()?;
                let note: v0::NoteV0 = entry
                    .note
                    .required("WalletBalanceData", "note")?
                    .try_into()?;
                Ok((name, note))
            })
            .collect::<Result<Vec<(v0::Name, v0::NoteV0)>, Self::Error>>()?;
        Ok(v0::BalanceUpdate {
            notes: v0::Balance(notes),
            height: update
                .height
                .required("WalletBalanceData", "height")?
                .try_into()?,
            block_id: update
                .block_id
                .required("WalletBalanceData", "block_id")?
                .try_into()?,
        })
    }
}

impl TryFrom<v0::BalanceUpdate> for WalletBalanceData {
    type Error = ErrorStatus;

    fn try_from(update: v0::BalanceUpdate) -> Result<Self, Self::Error> {
        let notes = update
            .notes
            .0
            .into_iter()
            .map(|(name, note)| {
                Ok(BalanceEntry {
                    name: Some(Name::from(name)),
                    note: Some(Note::from(note)),
                })
            })
            .collect::<Result<Vec<_>, ErrorStatus>>()?;

        Ok(WalletBalanceData {
            notes,
            height: Some(BlockHeight::from(update.height)),
            block_id: Some(Hash::from(update.block_id)),
            page: Some(crate::pb::common::v1::PageResponse {
                next_page_token: String::new(),
            }),
        })
    }
}

impl TryFrom<v0::BalanceUpdate> for WalletGetBalanceResponse {
    type Error = ErrorStatus;

    fn try_from(update: v0::BalanceUpdate) -> Result<Self, Self::Error> {
        let balance_data = WalletBalanceData::try_from(update)?;
        Ok(WalletGetBalanceResponse {
            result: Some(wallet_get_balance_response::Result::Balance(balance_data)),
        })
    }
}

impl TryFrom<WalletGetBalanceResponse> for v0::BalanceUpdate {
    type Error = anyhow::Error;
    fn try_from(update: WalletGetBalanceResponse) -> Result<Self> {
        let update = update
            .result
            .required("WalletGetBalanceResponse", "result")?;
        match update {
            wallet_get_balance_response::Result::Balance(update) => {
                Ok(v0::BalanceUpdate::try_from(update)?)
            }
            wallet_get_balance_response::Result::Error(err) => Err(RPCErrorStatus::from(err))?,
        }
    }
}
