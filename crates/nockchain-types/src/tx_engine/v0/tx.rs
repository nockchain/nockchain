use nockchain_math::zoon::zmap::ZMap;
use nockchain_math::zoon::zset::ZSet;
use nockvm::noun::{Noun, NounAllocator};
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
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        Ok(Self(ZMap::<Name, Input>::from_noun(noun)?.into_entries()))
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
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell()?;
        let id = TxId::from_noun(&cell.head())?;

        let tail = cell.tail();
        let cell = tail.as_cell()?;
        let inputs = Inputs::from_noun(&cell.head())?;

        let tail = cell.tail();
        let cell = tail.as_cell()?;
        let timelock_range = TimelockRangeAbsolute::from_noun(&cell.head())?;

        let total_fees = Nicks::from_noun(&cell.tail())?;

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
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        Ok(Seeds {
            seeds: ZSet::<Seed>::from_noun(noun)?.into_items(),
        })
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
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell()?;
        let signature = Option::<Signature>::from_noun(&cell.head())?;
        let inner = cell.tail().as_cell()?;
        let seeds = Seeds::from_noun(&inner.head())?;
        let fee = Nicks::from_noun(&inner.tail())?;

        Ok(Spend {
            signature,
            seeds,
            fee,
        })
    }
}
