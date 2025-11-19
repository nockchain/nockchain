use std::collections::{HashMap, HashSet};

#[allow(unused)]
use nockapp::utils::make_tas;
use nockvm::noun::FullDebugCell;
use noun_serde::{NounDecode, NounEncode};

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
enum Key {
    Pub(u64),
    Prv(u64),
}

type Knot = u64;

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
pub struct Coil {
    pub key: Key,
    pub knot: Knot,
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
pub enum Meta {
    Coil(Coil),
    Label(String),
    Address(u64),
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
pub struct Transaction {
    pub recipient: u64,
    pub amount: u64,
    pub status: TransactionStatus,
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
pub enum TransactionStatus {
    Unsigned,
    Signed,
    Sent,
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
pub enum FileEffect {
    Read { path: String },
    Write { path: String, contents: u64 },
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
pub enum Effect {
    File(FileEffect),
    Markdown(String),
    Exit { code: u64 },
}

#[derive(Debug, Clone, PartialEq, Default, NounEncode, NounDecode)]
pub struct SpendMask {
    pub signature: bool,
    pub seeds: bool,
    pub fee: bool,
}

#[derive(Debug, Clone, PartialEq, Default, NounEncode, NounDecode)]
pub struct InputMask {
    pub note: bool,
    pub spend: SpendMask,
}

#[derive(Debug, Clone, PartialEq, Default, NounEncode, NounDecode)]
pub struct SeedMask {
    pub output_source: bool,
    pub recipient: bool,
    pub timelock_intent: bool,
    pub gift: bool,
    pub parent_hash: bool,
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
pub struct PreSeed {
    pub name: String,
    pub seed: u64,
    pub mask: SeedMask,
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
pub struct PreInput {
    pub name: String,
    pub input: u64,
    pub mask: InputMask,
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
pub struct Draft {
    pub name: String,
    pub inputs: u64,
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
pub struct DraftEntity {
    pub kind: DraftEntityKind,
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
#[noun(tagged = true)]
pub enum DraftEntityKind {
    #[noun(tag = "draft")]
    Draft {
        #[noun(tag = "name")]
        name: String,
        #[noun(tag = "draft")]
        draft: Draft,
    },
    #[noun(tag = "input")]
    Input {
        #[noun(tag = "name")]
        name: String,
        #[noun(tag = "input")]
        input: PreInput,
    },
    #[noun(tag = "seed")]
    Seed {
        #[noun(tag = "name")]
        name: String,
        #[noun(tag = "seed")]
        seed: PreSeed,
    },
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
pub struct Master {
    pub pub_key: Coil,
    pub prv_key: Coil,
}

pub type NoteName = String;
pub type NoteHash = u64;
pub type Note = u64;

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
pub struct Balance {
    pub notes: HashMap<NoteName, Note>,
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
pub enum Network {
    Mainnet,
    Testnet,
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
pub enum PeekRequest {
    Balance,
    Block,
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
pub struct SeedTuple(pub u64, pub Option<String>);

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
pub struct WalletState {
    pub version: u64,
    pub balance: Balance,
    pub hash_to_name: HashMap<NoteHash, NoteName>,
    pub name_to_hash: HashMap<NoteName, NoteHash>,
    pub receive_address: u64,
    pub master: Option<Master>,
    pub keys: HashMap<String, Meta>,
    pub seed: SeedTuple,
    pub transactions: HashMap<u64, Transaction>,
    pub last_block: Option<u64>,
    pub lock: u64,
    pub network: Network,
    pub peek_requests: HashMap<u64, PeekRequest>,
    pub active_draft: Option<String>,
    pub active_input: Option<String>,
    pub active_seed: Option<String>,
    pub draft_tree: HashMap<String, DraftEntity>,
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
pub struct Trek(pub Vec<String>);

#[derive(Debug, Clone, PartialEq, Eq, Hash, NounEncode, NounDecode)]
pub enum Source {
    Hash(u64),
    Coinbase,
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct Lock {
    pub m: u64,
    pub pubkeys: HashSet<u64>,
}

impl std::hash::Hash for Lock {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.m.hash(state);
        let mut pubkeys: Vec<_> = self.pubkeys.iter().collect();
        pubkeys.sort();
        for pubkey in pubkeys {
            pubkey.hash(state);
        }
    }
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
struct Timelock {
    block: u64,
    intent: TimelockIntent,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, NounEncode, NounDecode)]
enum TimelockIntent {
    None,
    Before,
    After,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, NounEncode, NounDecode)]
struct Seed {
    output_source: Option<Source>,
    recipient: Lock,
    timelock_intent: TimelockIntent,
    gift: u64,
    parent_hash: u64,
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
struct Spend {
    signature: Option<HashMap<u64, u64>>,
    seeds: HashSet<Seed>,
    fee: u64,
}

#[cfg(test)]
mod tests {
    use nockvm::mem::NockStack;

    use super::*;

    // Expected Atom error, shouldn't this be a cell list?
    // #[test]
    // fn test_trek_encoding() {
    //     let mut stack = NockStack::new(8 << 10 << 10, 0);

    //     let trek = Trek(vec![
    //         "path".to_string(),
    //         "to".to_string(),
    //         "key".to_string(),
    //     ]);
    //     let encoded = trek.to_noun(&mut stack);
    //     let decoded = Trek::from_noun(&mut stack, &encoded).unwrap();
    //     assert_eq!(trek, decoded);
    // }

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
        println!(
            "Encoded timelock: {:?}",
            FullDebugCell(&encoded.as_cell().unwrap())
        );
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
            inputs: 0x1234,
        };
        let encoded = draft.to_noun(&mut stack);
        let decoded = Draft::from_noun(&encoded).unwrap();
        assert_eq!(draft, decoded);
    }

    #[test]
    fn test_seed_tuple_encoding() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);

        // Test with Some value
        let seed_tuple = SeedTuple(42, Some("seed-phrase".to_string()));
        let encoded = seed_tuple.to_noun(&mut stack);
        let decoded = SeedTuple::from_noun(&encoded).unwrap();
        assert_eq!(seed_tuple, decoded);

        // Test with None value
        let seed_tuple = SeedTuple(42, None);
        let encoded = seed_tuple.to_noun(&mut stack);
        let decoded = SeedTuple::from_noun(&encoded).unwrap();
        assert_eq!(seed_tuple, decoded);
    }

    #[test]
    fn test_wallet_state_encoding() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);

        let mut hash_to_name = HashMap::new();
        hash_to_name.insert(0x1234, "note1".to_string());

        let mut name_to_hash = HashMap::new();
        name_to_hash.insert("note1".to_string(), 0x1234);

        let mut notes = HashMap::new();
        notes.insert("note1".to_string(), 100);

        let mut keys = HashMap::new();
        keys.insert("key1".to_string(), Meta::Address(0x5678));

        let mut transactions = HashMap::new();
        transactions.insert(
            1,
            Transaction {
                recipient: 0x9abc,
                amount: 50,
                status: TransactionStatus::Signed,
            },
        );

        let mut peek_requests = HashMap::new();
        peek_requests.insert(1, PeekRequest::Balance);

        let mut draft_tree = HashMap::new();
        draft_tree.insert(
            "draft1".to_string(),
            DraftEntity {
                kind: DraftEntityKind::Draft {
                    name: "draft1".to_string(), // Match the key name
                    draft: Draft {
                        name: "draft1".to_string(), // Match the key name
                        inputs: 0x1234,
                    },
                },
            },
        );

        let wallet_state = WalletState {
            version: 1,
            balance: Balance { notes },
            hash_to_name,
            name_to_hash,
            receive_address: 0xdef0,
            master: Some(Master {
                pub_key: Coil {
                    key: Key::Pub(0x1111),
                    knot: 0x2222,
                },
                prv_key: Coil {
                    key: Key::Prv(0x3333),
                    knot: 0x4444,
                },
            }),
            keys,
            seed: SeedTuple(42, Some("seed-phrase".to_string())),
            transactions,
            last_block: Some(1000),
            lock: 0x5555,
            network: Network::Mainnet,
            peek_requests,
            active_draft: Some("draft1".to_string()), // Match the draft name
            active_input: None,
            active_seed: None,
            draft_tree,
        };

        println!("Encoding wallet state...");
        let encoded = wallet_state.to_noun(&mut stack);
        println!("Encoded noun: {:?}", encoded);
        let decoded = WalletState::from_noun(&encoded).unwrap();
        assert_eq!(wallet_state, decoded);
    }

    #[test]
    fn test_draft_entity_kind_encoding() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);

        // Test Draft variant
        let draft_kind = DraftEntityKind::Draft {
            name: "test".to_string(),
            draft: Draft {
                name: "test".to_string(),
                inputs: 0x1234,
            },
        };
        println!("Encoding draft kind...");
        let encoded = draft_kind.to_noun(&mut stack);
        println!("Encoded draft kind: {:?}", encoded);
        println!(
            "Encoded draft kind head: {:?}",
            encoded.as_cell().unwrap().head()
        );
        println!(
            "Encoded draft kind tail: {:?}",
            encoded.as_cell().unwrap().tail()
        );
        if let Ok(tail_cell) = encoded.as_cell().unwrap().tail().as_cell() {
            println!("Tail head: {:?}", tail_cell.head());
            println!("Tail tail: {:?}", tail_cell.tail());
            if let Ok(tail_tail_cell) = tail_cell.tail().as_cell() {
                println!("Tail tail head: {:?}", tail_tail_cell.head());
                println!("Tail tail tail: {:?}", tail_tail_cell.tail());
            }
        }
        let decoded = DraftEntityKind::from_noun(&encoded).unwrap();
        assert_eq!(draft_kind, decoded);

        // Test Input variant
        let input_kind = DraftEntityKind::Input {
            name: "test".to_string(),
            input: PreInput {
                name: "test".to_string(),
                input: 0x1234,
                mask: InputMask::default(),
            },
        };
        println!("Encoding input kind...");
        let encoded = input_kind.to_noun(&mut stack);
        println!("Encoded input kind: {:?}", encoded);
        let decoded = DraftEntityKind::from_noun(&encoded).unwrap();
        assert_eq!(input_kind, decoded);

        // Test Seed variant
        let seed_kind = DraftEntityKind::Seed {
            name: "test".to_string(),
            seed: PreSeed {
                name: "test".to_string(),
                seed: 0x1234,
                mask: SeedMask::default(),
            },
        };
        println!("Encoding seed kind...");
        let encoded = seed_kind.to_noun(&mut stack);
        println!("Encoded seed kind: {:?}", encoded);
        let decoded = DraftEntityKind::from_noun(&encoded).unwrap();
        assert_eq!(seed_kind, decoded);
    }
}
