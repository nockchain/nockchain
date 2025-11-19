use std::collections::{HashMap, HashSet};

use nockvm::ext::{make_tas, AtomExt};
use nockvm::noun::{Noun, NounAllocator, D, T};

use crate::{NounDecode, NounDecodeError, NounEncode};

/// A public or private key
#[derive(Debug, Clone, PartialEq)]
pub enum Key {
    Pub(u64), // @ux in hoon
    Prv(u64), // @ux in hoon
}

impl NounEncode for Key {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        match self {
            Key::Pub(x) => {
                let tag = make_tas(allocator, "pub").as_noun();
                let value = D(*x);
                T(allocator, &[tag, value])
            }
            Key::Prv(x) => {
                let tag = make_tas(allocator, "prv").as_noun();
                let value = D(*x);
                T(allocator, &[tag, value])
            }
        }
    }
}

impl NounDecode for Key {
    #[allow(unused_variables)]
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let tag = cell.head().as_atom()?.into_string()?;
        let value = cell.tail().as_atom()?.as_u64()?;

        match tag.as_str() {
            "pub" => Ok(Key::Pub(value)),
            "prv" => Ok(Key::Prv(value)),
            _ => Err(NounDecodeError::InvalidEnumVariant),
        }
    }
}

/// A chain code (knot in hoon)
pub type Knot = u64; // @ux in hoon

/// A key and its associated chain code
#[derive(Debug, Clone, PartialEq)]
pub struct Coil {
    pub key: Key,
    pub knot: Knot,
}

impl NounEncode for Coil {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let tag = make_tas(allocator, "coil").as_noun();
        let key_noun = self.key.to_noun(allocator);
        let knot_noun = D(self.knot);
        let data = T(allocator, &[key_noun, knot_noun]);
        T(allocator, &[tag, data])
    }
}

impl NounDecode for Coil {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let tag = cell.head().as_atom()?.into_string()?;
        if tag != "coil" {
            return Err(NounDecodeError::InvalidTag);
        }

        let data = cell.tail().as_cell()?;
        let key = Key::from_noun(&data.head())?;
        let knot = data.tail().as_atom()?.as_u64()?;

        Ok(Coil { key, knot })
    }
}

/// Metadata stored for a key
#[derive(Debug, Clone, PartialEq)]
pub enum Meta {
    Coil(Coil),
    Label(String),
    Address(u64), // @ux in hoon
}

impl NounEncode for Meta {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        match self {
            Meta::Coil(coil) => coil.to_noun(allocator),
            Meta::Label(label) => {
                let tag = make_tas(allocator, "label").as_noun();
                let value = make_tas(allocator, label).as_noun();
                T(allocator, &[tag, value])
            }
            Meta::Address(addr) => {
                let tag = make_tas(allocator, "address").as_noun();
                let value = D(*addr);
                T(allocator, &[tag, value])
            }
        }
    }
}

impl NounDecode for Meta {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let tag = cell.head().as_atom()?.into_string()?;

        match tag.as_str() {
            "coil" => Ok(Meta::Coil(Coil::from_noun(noun)?)),
            "label" => {
                let value = cell.tail().as_atom()?.into_string()?;
                Ok(Meta::Label(value))
            }
            "address" => {
                let value = cell.tail().as_atom()?.as_u64()?;
                Ok(Meta::Address(value))
            }
            _ => Err(NounDecodeError::InvalidEnumVariant),
        }
    }
}

/// A transaction in the wallet
#[derive(Debug, Clone, PartialEq)]
pub struct Transaction {
    pub recipient: u64, // @ux in hoon
    pub amount: u64,    // @ud in hoon
    pub status: TransactionStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TransactionStatus {
    Unsigned,
    Signed,
    Sent,
}

impl NounEncode for Transaction {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let recipient_noun = D(self.recipient);
        let amount_noun = D(self.amount);
        let status_noun = self.status.to_noun(allocator);
        let data = T(allocator, &[amount_noun, status_noun]);
        T(allocator, &[recipient_noun, data])
    }
}

impl NounDecode for Transaction {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let recipient = cell.head().as_atom()?.as_u64()?;

        let tail = cell.tail().as_cell()?;
        let amount = tail.head().as_atom()?.as_u64()?;
        let status = TransactionStatus::from_noun(&tail.tail())?;

        Ok(Transaction {
            recipient,
            amount,
            status,
        })
    }
}

impl NounEncode for TransactionStatus {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let tag = match self {
            TransactionStatus::Unsigned => "unsigned",
            TransactionStatus::Signed => "signed",
            TransactionStatus::Sent => "sent",
        };
        make_tas(allocator, tag).as_noun()
    }
}

impl NounDecode for TransactionStatus {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let tag = noun.as_atom()?.into_string()?;
        match tag.as_str() {
            "unsigned" => Ok(TransactionStatus::Unsigned),
            "signed" => Ok(TransactionStatus::Signed),
            "sent" => Ok(TransactionStatus::Sent),
            _ => Err(NounDecodeError::InvalidEnumVariant),
        }
    }
}

/// A file effect for reading or writing files
#[derive(Debug, Clone, PartialEq)]
pub enum FileEffect {
    Read { path: String },
    Write { path: String, contents: u64 }, // @t for path, @ for contents
}

impl NounEncode for FileEffect {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        match self {
            FileEffect::Read { path } => {
                let file_tag = make_tas(allocator, "file").as_noun();
                let op_tag = make_tas(allocator, "read").as_noun();
                let path_noun = make_tas(allocator, path).as_noun();
                let data = T(allocator, &[op_tag, path_noun]);
                T(allocator, &[file_tag, data])
            }
            FileEffect::Write { path, contents } => {
                let file_tag = make_tas(allocator, "file").as_noun();
                let op_tag = make_tas(allocator, "write").as_noun();
                let path_noun = make_tas(allocator, path).as_noun();
                let contents_noun = D(*contents);
                let data = T(allocator, &[op_tag, path_noun, contents_noun]);
                T(allocator, &[file_tag, data])
            }
        }
    }
}

impl NounDecode for FileEffect {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let file_tag = cell.head().as_atom()?.into_string()?;
        if file_tag != "file" {
            return Err(NounDecodeError::InvalidTag);
        }

        let op_cell = cell.tail().as_cell()?;
        let op_tag = op_cell.head().as_atom()?.into_string()?;

        match op_tag.as_str() {
            "read" => {
                let path = op_cell.tail().as_atom()?.into_string()?;
                Ok(FileEffect::Read { path })
            }
            "write" => {
                let data = op_cell.tail().as_cell()?;
                let path = data.head().as_atom()?.into_string()?;
                let contents = data.tail().as_atom()?.as_u64()?;
                Ok(FileEffect::Write { path, contents })
            }
            _ => Err(NounDecodeError::InvalidEnumVariant),
        }
    }
}

/// An effect that can be produced by the wallet
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    File(FileEffect),
    Markdown(String),
    Exit { code: u64 },
}

impl NounEncode for Effect {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        match self {
            Effect::File(file_effect) => file_effect.to_noun(allocator),
            Effect::Markdown(text) => {
                let tag = make_tas(allocator, "markdown").as_noun();
                let text_noun = make_tas(allocator, text).as_noun();
                T(allocator, &[tag, text_noun])
            }
            Effect::Exit { code } => {
                let tag = make_tas(allocator, "exit").as_noun();
                T(allocator, &[tag, D(*code)])
            }
        }
    }
}

impl NounDecode for Effect {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let tag = cell.head().as_atom()?.into_string()?;

        match tag.as_str() {
            "file" => Ok(Effect::File(FileEffect::from_noun(noun)?)),
            "markdown" => {
                let text = cell.tail().as_atom()?.into_string()?;
                Ok(Effect::Markdown(text))
            }
            "exit" => {
                let code = cell.tail().as_atom()?.as_u64()?;
                Ok(Effect::Exit { code })
            }
            _ => Err(NounDecodeError::InvalidEnumVariant),
        }
    }
}

/// A mask tracking which fields of a spend have been set
#[derive(Debug, Clone, PartialEq)]
pub struct SpendMask {
    pub signature: bool,
    pub seeds: bool,
    pub fee: bool,
}

impl Default for SpendMask {
    fn default() -> Self {
        SpendMask {
            signature: false,
            seeds: false,
            fee: false,
        }
    }
}

impl NounEncode for SpendMask {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let mut current = D(self.fee as u64);
        for field in [self.seeds, self.signature].iter().rev() {
            current = T(allocator, &[D(*field as u64), current]);
        }
        current
    }
}

impl NounDecode for SpendMask {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let signature = cell.head().as_atom()?.as_u64()? != 0;

        let rest = cell.tail().as_cell()?;
        let seeds = rest.head().as_atom()?.as_u64()? != 0;
        let fee = rest.tail().as_atom()?.as_u64()? != 0;

        Ok(SpendMask {
            signature,
            seeds,
            fee,
        })
    }
}

/// A mask tracking which fields of an input have been set
#[derive(Debug, Clone, PartialEq)]
pub struct InputMask {
    pub note: bool,
    pub spend: SpendMask,
}

impl Default for InputMask {
    fn default() -> Self {
        InputMask {
            note: false,
            spend: SpendMask::default(),
        }
    }
}

impl NounEncode for InputMask {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let note_noun = D(self.note as u64);
        let spend_noun = self.spend.to_noun(allocator);
        T(allocator, &[note_noun, spend_noun])
    }
}

impl NounDecode for InputMask {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let note = cell.head().as_atom()?.as_u64()? != 0;
        let spend = SpendMask::from_noun(&cell.tail())?;
        Ok(InputMask { note, spend })
    }
}

/// A seed in preparation with its mask
#[derive(Debug, Clone, PartialEq)]
pub struct PreSeed {
    pub name: String,
    pub seed: u64, // Simplified for now, actual seed type needed
    pub mask: SeedMask,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SeedMask {
    pub output_source: bool,
    pub recipient: bool,
    pub timelock_intent: bool,
    pub gift: bool,
    pub parent_hash: bool,
}

impl Default for SeedMask {
    fn default() -> Self {
        SeedMask {
            output_source: false,
            recipient: false,
            timelock_intent: false,
            gift: false,
            parent_hash: false,
        }
    }
}

impl NounEncode for SeedMask {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let mut current = D(self.parent_hash as u64);
        for field in [self.gift, self.timelock_intent, self.recipient, self.output_source]
            .iter()
            .rev()
        {
            current = T(allocator, &[D(*field as u64), current]);
        }
        current
    }
}

impl NounDecode for SeedMask {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let mut current = noun;
        let next_cell = |n: &Noun| -> Result<(Noun, Noun), NounDecodeError> {
            let cell = n.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
            Ok((cell.head(), cell.tail()))
        };

        let (output_source_noun, current_) = next_cell(current)?;
        let output_source = output_source_noun.as_atom()?.as_u64()? != 0;
        current = &current_;

        let (recipient_noun, current_) = next_cell(current)?;
        let recipient = recipient_noun.as_atom()?.as_u64()? != 0;
        current = &current_;

        let (timelock_intent_noun, current_) = next_cell(current)?;
        let timelock_intent = timelock_intent_noun.as_atom()?.as_u64()? != 0;
        current = &current_;

        let (gift_noun, current_) = next_cell(current)?;
        let gift = gift_noun.as_atom()?.as_u64()? != 0;
        current = &current_;

        let parent_hash = current.as_atom()?.as_u64()? != 0;

        Ok(SeedMask {
            output_source,
            recipient,
            timelock_intent,
            gift,
            parent_hash,
        })
    }
}

impl NounEncode for PreSeed {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let name_noun = make_tas(allocator, &self.name).as_noun();
        let seed_noun = D(self.seed);
        let mask_noun = self.mask.to_noun(allocator);
        let inner = T(allocator, &[seed_noun, mask_noun]);
        T(allocator, &[name_noun, inner])
    }
}

impl NounDecode for PreSeed {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let name = cell.head().as_atom()?.into_string()?;

        let data = cell.tail().as_cell()?;
        let seed = data.head().as_atom()?.as_u64()?;
        let mask = SeedMask::from_noun(&data.tail())?;

        Ok(PreSeed { name, seed, mask })
    }
}

/// A input in preparation with its mask
#[derive(Debug, Clone, PartialEq)]
pub struct PreInput {
    pub name: String,
    pub input: u64, // Simplified for now, actual input type needed
    pub mask: InputMask,
}

impl NounEncode for PreInput {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let name_noun = make_tas(allocator, &self.name).as_noun();
        let input_noun = D(self.input);
        let mask_noun = self.mask.to_noun(allocator);
        let inner = T(allocator, &[input_noun, mask_noun]);
        T(allocator, &[name_noun, inner])
    }
}

impl NounDecode for PreInput {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let name = cell.head().as_atom()?.into_string()?;

        let data = cell.tail().as_cell()?;
        let input = data.head().as_atom()?.as_u64()?;
        let mask = InputMask::from_noun(&data.tail())?;

        Ok(PreInput { name, input, mask })
    }
}

/// A draft with a name and inputs
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub name: String,
    pub inputs: u64, // Simplified for now, actual inputs type needed
}

impl NounEncode for Draft {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let name_noun = make_tas(allocator, &self.name).as_noun();
        let inputs_noun = D(self.inputs);
        T(allocator, &[name_noun, inputs_noun])
    }
}

impl NounDecode for Draft {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let name = cell.head().as_atom()?.into_string()?;
        let inputs = cell.tail().as_atom()?.as_u64()?;

        Ok(Draft { name, inputs })
    }
}

/// A tree structure for managing drafts, inputs, and seeds
#[derive(Debug, Clone, PartialEq)]
pub struct DraftEntity {
    pub kind: DraftEntityKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DraftEntityKind {
    Draft { name: String, draft: Draft },
    Input { name: String, input: PreInput },
    Seed { name: String, seed: PreSeed },
}

impl NounEncode for DraftEntity {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        match &self.kind {
            DraftEntityKind::Draft { name, draft } => {
                let tag = make_tas(allocator, "draft").as_noun();
                let name_noun = make_tas(allocator, name).as_noun();
                let draft_noun = draft.to_noun(allocator);
                let inner = T(allocator, &[name_noun, draft_noun]);
                T(allocator, &[tag, inner])
            }
            DraftEntityKind::Input { name, input } => {
                let tag = make_tas(allocator, "input").as_noun();
                let name_noun = make_tas(allocator, name).as_noun();
                let input_noun = input.to_noun(allocator);
                let inner = T(allocator, &[name_noun, input_noun]);
                T(allocator, &[tag, inner])
            }
            DraftEntityKind::Seed { name, seed } => {
                let tag = make_tas(allocator, "seed").as_noun();
                let name_noun = make_tas(allocator, name).as_noun();
                let seed_noun = seed.to_noun(allocator);
                let inner = T(allocator, &[name_noun, seed_noun]);
                T(allocator, &[tag, inner])
            }
        }
    }
}

impl NounDecode for DraftEntity {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let tag = cell.head().as_atom()?.into_string()?;

        let data = cell.tail().as_cell()?;
        let name = data.head().as_atom()?.into_string()?;

        let kind = match tag.as_str() {
            "draft" => {
                let draft = Draft::from_noun(&data.tail())?;
                DraftEntityKind::Draft { name, draft }
            }
            "input" => {
                let input = PreInput::from_noun(&data.tail())?;
                DraftEntityKind::Input { name, input }
            }
            "seed" => {
                let seed = PreSeed::from_noun(&data.tail())?;
                DraftEntityKind::Seed { name, seed }
            }
            _ => return Err(NounDecodeError::InvalidEnumVariant),
        };

        Ok(DraftEntity { kind })
    }
}

/// A master key pair containing public and private keys
#[derive(Debug, Clone, PartialEq)]
pub struct Master {
    pub pub_key: Coil,
    pub prv_key: Coil,
}

impl NounEncode for Master {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let pub_noun = self.pub_key.to_noun(allocator);
        let prv_noun = self.prv_key.to_noun(allocator);
        T(allocator, &[pub_noun, prv_noun])
    }
}

impl NounDecode for Master {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let pub_key = Coil::from_noun(&cell.head())?;
        let prv_key = Coil::from_noun(&cell.tail())?;

        Ok(Master { pub_key, prv_key })
    }
}

/// A note name (simplified for now)
pub type NoteName = String;

/// A note hash (simplified for now)
pub type NoteHash = u64;

/// A note (simplified for now)
pub type Note = u64;

/// A balance mapping note names to notes
#[derive(Debug, Clone, PartialEq)]
pub struct Balance {
    pub notes: HashMap<NoteName, Note>,
}

impl NounEncode for Balance {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        self.notes.to_noun(allocator)
    }
}

impl NounDecode for Balance {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let notes = HashMap::from_noun(noun)?;
        Ok(Balance { notes })
    }
}

/// The network type (mainnet or testnet)
#[derive(Debug, Clone, PartialEq)]
pub enum Network {
    Mainnet,
    Testnet,
}

impl NounEncode for Network {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let tag = match self {
            Network::Mainnet => "mainnet",
            Network::Testnet => "testnet",
        };
        make_tas(allocator, tag).as_noun()
    }
}

impl NounDecode for Network {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let tag = noun.as_atom()?.into_string()?;
        match tag.as_str() {
            "mainnet" => Ok(Network::Mainnet),
            "testnet" => Ok(Network::Testnet),
            _ => Err(NounDecodeError::InvalidEnumVariant),
        }
    }
}

/// A peek request type
#[derive(Debug, Clone, PartialEq)]
pub enum PeekRequest {
    Balance,
    Block,
}

impl NounEncode for PeekRequest {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let tag = match self {
            PeekRequest::Balance => "balance",
            PeekRequest::Block => "block",
        };
        make_tas(allocator, tag).as_noun()
    }
}

impl NounDecode for PeekRequest {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let tag = noun.as_atom()?.into_string()?;
        match tag.as_str() {
            "balance" => Ok(PeekRequest::Balance),
            "block" => Ok(PeekRequest::Block),
            _ => Err(NounDecodeError::InvalidEnumVariant),
        }
    }
}

/// The main wallet state
#[derive(Debug, Clone, PartialEq)]
pub struct WalletState {
    pub version: u64, // Always 0 for now
    pub balance: Balance,
    pub hash_to_name: HashMap<NoteHash, NoteName>,
    pub name_to_hash: HashMap<NoteName, NoteHash>,
    pub receive_address: u64, // Simplified for now
    pub master: Option<Master>,
    pub keys: HashMap<String, Meta>, // Simplified for now
    pub seed: (u64, Option<String>), // (index, phrase)
    pub transactions: HashMap<u64, Transaction>,
    pub last_block: Option<u64>,
    pub lock: u64, // Simplified for now
    pub network: Network,
    pub peek_requests: HashMap<u64, PeekRequest>,
    pub active_draft: Option<String>,
    pub active_input: Option<String>,
    pub active_seed: Option<String>,
    pub draft_tree: HashMap<String, DraftEntity>, // Simplified for now
}

impl NounEncode for WalletState {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let version = D(self.version);
        let balance = self.balance.to_noun(allocator);
        let hash_to_name = self.hash_to_name.to_noun(allocator);
        let name_to_hash = self.name_to_hash.to_noun(allocator);
        let receive_address = D(self.receive_address);
        let master = match &self.master {
            Some(m) => m.to_noun(allocator),
            None => D(0),
        };
        let keys = self.keys.to_noun(allocator);
        let seed_noun = match &self.seed.1 {
            Some(s) => make_tas(allocator, s).as_noun(),
            None => D(0),
        };
        let seed = T(allocator, &[D(self.seed.0), seed_noun]);
        let transactions = self.transactions.to_noun(allocator);
        let last_block = match self.last_block {
            Some(b) => T(allocator, &[D(0), D(b)]),
            None => D(0),
        };
        let lock = D(self.lock);
        let network = self.network.to_noun(allocator);
        let peek_requests = self.peek_requests.to_noun(allocator);
        let active_draft = match &self.active_draft {
            Some(d) => {
                let name_noun = make_tas(allocator, d).as_noun();
                T(allocator, &[D(0), name_noun])
            }
            None => D(0),
        };
        let active_input = match &self.active_input {
            Some(i) => {
                let name_noun = make_tas(allocator, i).as_noun();
                T(allocator, &[D(0), name_noun])
            }
            None => D(0),
        };
        let active_seed = match &self.active_seed {
            Some(s) => {
                let name_noun = make_tas(allocator, s).as_noun();
                T(allocator, &[D(0), name_noun])
            }
            None => D(0),
        };
        let draft_tree = self.draft_tree.to_noun(allocator);

        // Build the final noun structure
        let mut current = draft_tree;
        for noun in [
            active_seed, active_input, active_draft, peek_requests, network, lock, last_block,
            transactions, seed, keys, master, receive_address, name_to_hash, hash_to_name, balance,
            version,
        ]
        .iter()
        .rev()
        {
            current = T(allocator, &[noun.clone(), current]);
        }
        current
    }
}

/// A path type used for key derivation
#[derive(Debug, Clone, PartialEq)]
pub struct Trek(pub Vec<String>);

impl NounEncode for Trek {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let mut current = D(0);
        for part in self.0.iter().rev() {
            let part_noun = make_tas(allocator, part).as_noun();
            current = T(allocator, &[part_noun, current]);
        }
        current
    }
}

impl NounDecode for Trek {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let mut current = noun.clone();
        let mut parts = Vec::new();
        while let Ok(cell) = current.as_cell() {
            let part = cell.head().as_atom()?.into_string()?;
            parts.push(part);
            current = cell.tail();
        }
        Ok(Trek(parts))
    }
}

/// A transaction source type
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Source {
    Hash(u64),
    Coinbase,
}

impl NounEncode for Source {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        match self {
            Source::Hash(h) => T(allocator, &[D(*h), D(0)]),
            Source::Coinbase => T(allocator, &[D(0), D(1)]),
        }
    }
}

impl NounDecode for Source {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let head = cell.head().as_atom()?.as_u64()?;
        let tail = cell.tail().as_atom()?.as_u64()?;
        match (head, tail) {
            (h, 0) => Ok(Source::Hash(h)),
            (0, 1) => Ok(Source::Coinbase),
            _ => Err(NounDecodeError::InvalidEnumVariant),
        }
    }
}

/// A transaction lock type
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lock {
    pub m: u64,                // Number of required signatures
    pub pubkeys: HashSet<u64>, // Set of public keys that can sign
}

impl std::hash::Hash for Lock {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.m.hash(state);
        // Sort pubkeys for consistent hashing
        let mut pubkeys: Vec<_> = self.pubkeys.iter().collect();
        pubkeys.sort();
        for pubkey in pubkeys {
            pubkey.hash(state);
        }
    }
}

impl NounEncode for Lock {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let pubkeys_noun = self.pubkeys.to_noun(allocator);
        T(allocator, &[D(self.m), pubkeys_noun])
    }
}

impl NounDecode for Lock {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let m = cell.head().as_atom()?.as_u64()?;
        let pubkeys = HashSet::from_noun(&cell.tail())?;
        Ok(Lock { m, pubkeys })
    }
}

/// A transaction timelock type
#[derive(Debug, Clone, PartialEq)]
pub struct Timelock {
    pub block: u64,
    pub intent: TimelockIntent,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TimelockIntent {
    None,
    Before,
    After,
}

impl NounEncode for Timelock {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let intent = match self.intent {
            TimelockIntent::None => D(0),
            TimelockIntent::Before => D(1),
            TimelockIntent::After => D(2),
        };
        T(allocator, &[D(self.block), intent])
    }
}

impl NounDecode for Timelock {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let block = cell.head().as_atom()?.as_u64()?;
        let intent = match cell.tail().as_atom()?.as_u64()? {
            0 => TimelockIntent::None,
            1 => TimelockIntent::Before,
            2 => TimelockIntent::After,
            _ => return Err(NounDecodeError::InvalidEnumVariant),
        };
        Ok(Timelock { block, intent })
    }
}

/// A transaction seed type
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Seed {
    pub output_source: Option<Source>,
    pub recipient: Lock,
    pub timelock_intent: TimelockIntent,
    pub gift: u64,
    pub parent_hash: u64,
}

impl NounEncode for Seed {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        println!("\nEncoding Seed: {:?}", self);

        // Build in the correct order to match decoding
        let mut current = D(self.parent_hash);
        println!("Starting with parent_hash: {:?}", current);

        // Add gift
        current = T(allocator, &[D(self.gift), current]);
        println!("Added gift: {:?}", current);

        // Add timelock_intent
        let intent = match self.timelock_intent {
            TimelockIntent::None => D(0),
            TimelockIntent::Before => D(1),
            TimelockIntent::After => D(2),
        };
        current = T(allocator, &[intent, current]);
        println!("Added timelock_intent: {:?}", current);

        // Add recipient
        let recipient = self.recipient.to_noun(allocator);
        current = T(allocator, &[recipient, current]);
        println!("Added recipient: {:?}", current);

        // Add output_source last
        let source = match &self.output_source {
            Some(s) => {
                let s_noun = s.to_noun(allocator);
                println!("Encoding Some(Source) as [0 {:?}]", s_noun);
                T(allocator, &[D(0), s_noun])
            }
            None => {
                println!("Encoding None as 0");
                D(0)
            }
        };
        current = T(allocator, &[source, current]);
        println!("Final encoded Seed: {:?}", current);

        current
    }
}

impl NounDecode for Seed {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        println!("\nDecoding Seed from noun: {:?}", noun);

        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        println!(
            "Root cell - head: {:?}, tail: {:?}",
            cell.head(),
            cell.tail()
        );

        // Decode output_source
        let source_noun = cell.head();
        println!("Source noun: {:?}", source_noun);

        let output_source = if let Ok(atom) = source_noun.as_atom() {
            if atom.as_u64()? == 0 {
                println!("Found atom 0, decoding as None");
                None
            } else {
                println!("Found non-zero atom, invalid Option encoding");
                return Err(NounDecodeError::InvalidEnumVariant);
            }
        } else {
            let source_cell = source_noun.as_cell()?;
            println!(
                "Source cell - head: {:?}, tail: {:?}",
                source_cell.head(),
                source_cell.tail()
            );

            if source_cell.head().as_atom()?.as_u64()? != 0 {
                println!("Invalid Some tag");
                return Err(NounDecodeError::InvalidEnumVariant);
            }

            println!("Decoding Some(Source)");
            Some(Source::from_noun(&source_cell.tail())?)
        };
        println!("Decoded output_source: {:?}", output_source);

        let rest = cell.tail().as_cell()?;
        println!(
            "First rest cell - head: {:?}, tail: {:?}",
            rest.head(),
            rest.tail()
        );

        let recipient = Lock::from_noun(&rest.head())?;
        println!("Decoded recipient: {:?}", recipient);

        let rest = rest.tail().as_cell()?;
        println!(
            "Second rest cell - head: {:?}, tail: {:?}",
            rest.head(),
            rest.tail()
        );

        let timelock_intent = match rest.head().as_atom()?.as_u64()? {
            0 => TimelockIntent::None,
            1 => TimelockIntent::Before,
            2 => TimelockIntent::After,
            x => {
                println!("Invalid timelock_intent value: {}", x);
                return Err(NounDecodeError::InvalidEnumVariant);
            }
        };
        println!("Decoded timelock_intent: {:?}", timelock_intent);

        let rest = rest.tail().as_cell()?;
        println!(
            "Third rest cell - head: {:?}, tail: {:?}",
            rest.head(),
            rest.tail()
        );

        let gift = rest.head().as_atom()?.as_u64()?;
        println!("Decoded gift: {}", gift);

        let parent_hash = rest.tail().as_atom()?.as_u64()?;
        println!("Decoded parent_hash: {}", parent_hash);

        let result = Seed {
            output_source,
            recipient,
            timelock_intent,
            gift,
            parent_hash,
        };
        println!("Final decoded Seed: {:?}", result);

        Ok(result)
    }
}

/// A spend type
#[derive(Debug, Clone, PartialEq)]
pub struct Spend {
    pub signature: Option<HashMap<u64, u64>>, // Pubkey -> Signature
    pub seeds: HashSet<Seed>,
    pub fee: u64,
}

impl NounEncode for Spend {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        println!("\nEncoding Spend: {:?}", self);

        // Encode signature as Option
        let signature = match &self.signature {
            Some(s) => {
                let s_noun = s.to_noun(allocator);
                println!("Encoding Some(signature) as [0 {:?}]", s_noun);
                T(allocator, &[D(0), s_noun])
            }
            None => {
                println!("Encoding None signature as 0");
                D(0)
            }
        };

        let seeds = self.seeds.to_noun(allocator);
        println!("Encoded seeds: {:?}", seeds);

        let fee = D(self.fee);
        println!("Encoded fee: {:?}", fee);

        // Build the final structure
        let inner = T(allocator, &[seeds, fee]);
        let result = T(allocator, &[signature, inner]);
        println!("Final encoded Spend: {:?}", result);
        result
    }
}

impl NounDecode for Spend {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        println!("\nDecoding Spend from noun: {:?}", noun);

        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        println!(
            "Root cell - head: {:?}, tail: {:?}",
            cell.head(),
            cell.tail()
        );

        // Decode signature Option
        let sig_noun = cell.head();
        println!("Signature noun: {:?}", sig_noun);

        let signature = if let Ok(atom) = sig_noun.as_atom() {
            if atom.as_u64()? == 0 {
                println!("Found atom 0, decoding as None");
                None
            } else {
                println!("Found non-zero atom, invalid Option encoding");
                return Err(NounDecodeError::InvalidEnumVariant);
            }
        } else {
            let sig_cell = sig_noun.as_cell()?;
            println!(
                "Signature cell - head: {:?}, tail: {:?}",
                sig_cell.head(),
                sig_cell.tail()
            );

            if sig_cell.head().as_atom()?.as_u64()? != 0 {
                println!("Invalid Some tag");
                return Err(NounDecodeError::InvalidEnumVariant);
            }

            println!("Decoding Some(HashMap)");
            Some(HashMap::from_noun(&sig_cell.tail())?)
        };
        println!("Decoded signature: {:?}", signature);

        let data = cell.tail().as_cell()?;
        println!(
            "Data cell - head: {:?}, tail: {:?}",
            data.head(),
            data.tail()
        );

        let seeds = HashSet::from_noun(&data.head())?;
        println!("Decoded seeds: {:?}", seeds);

        let fee = data.tail().as_atom()?.as_u64()?;
        println!("Decoded fee: {}", fee);

        let result = Spend {
            signature,
            seeds,
            fee,
        };
        println!("Final decoded Spend: {:?}", result);

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use nockvm::mem::NockStack;

    use super::*;

    #[test]
    fn test_trek_encoding() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);

        let trek = Trek(vec![
            "path".to_string(),
            "to".to_string(),
            "key".to_string(),
        ]);
        let encoded = trek.to_noun(&mut stack);
        let decoded = Trek::from_noun(&encoded).unwrap();
        assert_eq!(trek, decoded);
    }

    #[test]
    fn test_source_encoding() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);

        let hash = Source::Hash(0x1234);
        let encoded = hash.to_noun(&mut stack);
        let decoded = Source::from_noun(&encoded).unwrap();
        assert_eq!(hash, decoded);

        let coinbase = Source::Coinbase;
        let encoded = coinbase.to_noun(&mut stack);
        let decoded = Source::from_noun(&encoded).unwrap();
        assert_eq!(coinbase, decoded);
    }

    #[test]
    fn test_lock_encoding() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);

        let mut pubkeys = HashSet::new();
        pubkeys.insert(0x1234);
        pubkeys.insert(0x5678);

        let lock = Lock { m: 2, pubkeys };
        let encoded = lock.to_noun(&mut stack);
        let decoded = Lock::from_noun(&encoded).unwrap();
        assert_eq!(lock, decoded);
    }

    #[test]
    fn test_timelock_encoding() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);

        let timelock = Timelock {
            block: 0x1234,
            intent: TimelockIntent::After,
        };
        let encoded = timelock.to_noun(&mut stack);
        let decoded = Timelock::from_noun(&encoded).unwrap();
        assert_eq!(timelock, decoded);
    }

    #[test]
    fn test_seed_encoding() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);

        let mut pubkeys = HashSet::new();
        pubkeys.insert(0x1234);

        let seed = Seed {
            output_source: Some(Source::Hash(0x5678)),
            recipient: Lock { m: 1, pubkeys },
            timelock_intent: TimelockIntent::None,
            gift: 100,
            parent_hash: 0x9abc,
        };
        let encoded = seed.to_noun(&mut stack);
        let decoded = Seed::from_noun(&encoded).unwrap();
        assert_eq!(seed, decoded);
    }

    #[test]
    fn test_preseed_encoding() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);

        let preseed = PreSeed {
            name: "test_seed".to_string(),
            seed: 0x1234,
            mask: SeedMask {
                output_source: true,
                recipient: true,
                timelock_intent: true,
                gift: true,
                parent_hash: true,
            },
        };
        let encoded = preseed.to_noun(&mut stack);
        let decoded = PreSeed::from_noun(&encoded).unwrap();
        assert_eq!(preseed, decoded);
    }

    #[test]
    fn test_spend_encoding() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);

        let mut signatures = HashMap::new();
        signatures.insert(0x1234, 0x5678);

        let mut seeds = HashSet::new();
        let mut pubkeys = HashSet::new();
        pubkeys.insert(0x1234);

        let seed = Seed {
            output_source: Some(Source::Hash(0x5678)),
            recipient: Lock { m: 1, pubkeys },
            timelock_intent: TimelockIntent::None,
            gift: 100,
            parent_hash: 0x9abc,
        };
        seeds.insert(seed);

        let spend = Spend {
            signature: Some(signatures),
            seeds,
            fee: 10,
        };
        let encoded = spend.to_noun(&mut stack);
        let decoded = Spend::from_noun(&encoded).unwrap();
        assert_eq!(spend, decoded);
    }

    #[test]
    fn test_preinput_encoding() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);

        let preinput = PreInput {
            name: "test_input".to_string(),
            input: 0x1234,
            mask: InputMask {
                note: true,
                spend: SpendMask {
                    signature: true,
                    seeds: true,
                    fee: true,
                },
            },
        };
        let encoded = preinput.to_noun(&mut stack);
        let decoded = PreInput::from_noun(&encoded).unwrap();
        assert_eq!(preinput, decoded);
    }

    #[test]
    fn test_draft_encoding() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);

        let draft = Draft {
            name: "test_draft".to_string(),
            inputs: 0x1234, // Using u64 as specified in struct
        };
        let encoded = draft.to_noun(&mut stack);
        let decoded = Draft::from_noun(&encoded).unwrap();
        assert_eq!(draft, decoded);
    }
}
