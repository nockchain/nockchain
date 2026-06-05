use nockchain_math::noun_ext::NounMathExtHandle;
use nockchain_math::structs::HoonMapIter;
use nockchain_math::zoon::zmap::ZMap;
use nockchain_math::zoon::zset::ZSet;
use nockvm::noun::{Noun, NounAllocator, NounSpace};
use noun_serde::{NounDecode, NounDecodeError, NounEncode};

use super::note::{Lock, NoteV0, TimelockIntent};
use crate::tx_engine::common::{Hash, Name, Nicks, Signature, Source, TimelockRangeAbsolute, TxId};

//  +$  form
//    $:  id=tx-id  :: hash of +.raw-tx
//        =inputs
//        ::    the "union" of the ranges of valid page-numbers
//        ::    in which all inputs of the tx are able to spend,
//        ::    as enforced by their timelocks
//        =timelock-range
//        ::    the sum of all fees paid by all inputs
//        total-fees=coins
//    ==
//  ++  inputs  (z-map nname input)
//  ++  input   [note=nnote =spend]
//  ++  signature  (z-map schnorr-pubkey schnorr-signature)
//  ++  spend   $:  signature=(unit signature)
//                ::  everything below here is what is hashed for the signature
//                  =seeds
//                  fee=coins
//              ==
//
//  ++  seeds  (z-set seed)
//  ++  seed
//     $:  ::    if non-null, enforces that output note must have precisely
//         ::    this source
//         output-source=(unit source)
//         ::    the .lock of the output note
//         recipient=lock
//         ::    if non-null, enforces that output note must have precisely
//         ::    this timelock (though [~ ~ ~] means ~). null means there
//         ::    is no intent.
//         =timelock-intent
//         ::    quantity of assets gifted to output note
//         gift=coins
//         ::   check that parent hash of every seed is the hash of the
//         ::   parent note
//         parent-hash=^hash
//     ==
//
//

#[derive(Debug, Clone, PartialEq, NounDecode, NounEncode)]
pub struct Input {
    pub note: NoteV0,
    pub spend: Spend,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spend {
    pub signature: Option<Signature>,
    pub seeds: Seeds,
    pub fee: Nicks,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Inputs(pub Vec<(Name, Input)>);

impl NounEncode for Inputs {
    fn to_noun<A: NounAllocator>(&self, stack: &mut A) -> Noun {
        ZMap::try_from_entries(self.0.clone())
            .expect("inputs z-map should encode")
            .to_noun(stack)
    }
}

impl NounDecode for Inputs {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        let entries = HoonMapIter::new(&noun.in_space(space))
            .filter(|entry| entry.is_cell())
            .map(|entry| {
                let [key, value] = entry
                    .uncell()
                    .map_err(|_| NounDecodeError::Custom("input entry not a pair".into()))?;
                let name = Name::from_noun_handle(&key)?;
                let input = Input::from_noun_handle(&value)?;
                Ok((name, input))
            })
            .collect::<Result<Vec<_>, NounDecodeError>>()?;
        Ok(Self(entries))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawTx {
    pub id: TxId,
    pub inputs: Inputs,
    pub timelock_range: TimelockRangeAbsolute,
    pub total_fees: Nicks,
}

impl NounEncode for RawTx {
    fn to_noun<A: NounAllocator>(&self, stack: &mut A) -> Noun {
        let id = self.id.to_noun(stack);
        let inputs = self.inputs.to_noun(stack);
        let range = self.timelock_range.to_noun(stack);
        let fees = self.total_fees.to_noun(stack);
        nockvm::noun::T(stack, &[id, inputs, range, fees])
    }
}

impl NounDecode for RawTx {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        let cell = noun.in_space(space).as_cell()?;
        let id_noun = cell.head().noun();
        let id = TxId::from_noun(&id_noun, space)?;

        let tail = cell.tail();
        let cell = tail.as_cell()?;
        let inputs_noun = cell.head().noun();
        let inputs = Inputs::from_noun(&inputs_noun, space)?;

        let tail = cell.tail();
        let cell = tail.as_cell()?;
        let range_noun = cell.head().noun();
        let timelock_range = TimelockRangeAbsolute::from_noun(&range_noun, space)?;

        let fees_noun = cell.tail().noun();
        let total_fees = Nicks::from_noun(&fees_noun, space)?;

        Ok(Self {
            id,
            inputs,
            timelock_range,
            total_fees,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seeds {
    pub seeds: Vec<Seed>,
}

#[derive(Debug, Clone, PartialEq, Eq, NounDecode, NounEncode)]
pub struct Seed {
    pub output_source: Option<Source>,
    pub recipient: Lock,
    pub timelock_intent: Option<TimelockIntent>,
    pub gift: Nicks,
    pub parent_hash: Hash,
}

impl NounEncode for Seeds {
    fn to_noun<A: NounAllocator>(&self, stack: &mut A) -> Noun {
        ZSet::try_from_items(self.seeds.clone())
            .expect("seed z-set should encode")
            .to_noun(stack)
    }
}

impl NounDecode for Seeds {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        fn traverse(
            node: &Noun,
            space: &NounSpace,
            acc: &mut Vec<Seed>,
        ) -> Result<(), NounDecodeError> {
            if let Ok(atom) = node.in_space(space).as_atom() {
                if atom.as_u64()? == 0 {
                    return Ok(());
                }
                return Err(NounDecodeError::ExpectedCell);
            }

            let cell = node
                .in_space(space)
                .as_cell()
                .map_err(|_| NounDecodeError::Custom("seed node not a cell".into()))?;
            let seed_noun = cell.head().noun();
            let seed = Seed::from_noun(&seed_noun, space)?;
            acc.push(seed);

            let branches = cell
                .tail()
                .as_cell()
                .map_err(|_| NounDecodeError::Custom("seed branches not a cell".into()))?;
            let left = branches.head().noun();
            let right = branches.tail().noun();
            traverse(&left, space, acc)?;
            traverse(&right, space, acc)?;
            Ok(())
        }

        let mut seeds = Vec::new();
        traverse(noun, space, &mut seeds)?;
        Ok(Seeds { seeds })
    }
}

impl NounEncode for Spend {
    fn to_noun<A: NounAllocator>(&self, stack: &mut A) -> Noun {
        let signature = self.signature.to_noun(stack);
        let seeds = self.seeds.to_noun(stack);
        let fee = self.fee.to_noun(stack);
        let inner = nockvm::noun::T(stack, &[seeds, fee]);
        nockvm::noun::T(stack, &[signature, inner])
    }
}

impl NounDecode for Spend {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        let cell = noun.in_space(space).as_cell()?;
        let sig_noun = cell.head().noun();
        let signature = Option::<Signature>::from_noun(&sig_noun, space)?;
        let inner = cell.tail().as_cell()?;
        let seeds_noun = inner.head().noun();
        let fee_noun = inner.tail().noun();
        let seeds = Seeds::from_noun(&seeds_noun, space)?;
        let fee = Nicks::from_noun(&fee_noun, space)?;

        Ok(Spend {
            signature,
            seeds,
            fee,
        })
    }
}
