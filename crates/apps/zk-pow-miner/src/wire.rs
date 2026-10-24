//! Wire vocabulary for the ZK-PoW miner.
//!
//! [`ZkPowMinerWire`] mirrors `ai_pow_miner::wire::AiPowMinerWire`
//! (`SOURCE = "ai-pow-miner"`) but on its own source `"zk-pow-miner"`
//! so the kernel-side `do-pow` handler can route by source as well as
//! by inner pow-variant tag. Both wires share the same tag vocabulary
//! (`enable`, `candidate`, `mined`, `setpubkey`).
//!
//! Submission flow:
//! 1. The miner kernel (`assets/miner.jam`) emits a
//!    `[%mine-result %& dig [%command %pow %dumb-zkpow prf dig header nonce]]`
//!    effect.
//! 2. The run loop harvests the inner poke cell and pokes the node over
//!    gRPC via [`nockchain_mining_common::NodeClient::submit_mined_block`]
//!    with `ZkPowMinerWire::Mined.to_wire()`.
//! 3. The node's kernel matches the wire's source against
//!    `?(%zk-pow-miner %ai-pow-miner)` in `hoon/apps/dumbnet/inner.hoon`
//!    and dispatches to `do-pow`, which then matches the `%dumb-zkpow`
//!    inner tag and runs the puzzle-nock STARK verifier.

use nockapp::nockapp::wire::{Wire, WireRepr};

pub enum ZkPowMinerWire {
    /// Driver → node: enable / disable mining.
    Enable,
    /// Miner-kernel internal: candidate-job poke driving the SerfThread.
    Candidate,
    /// Driver → node: solved block. Payload (v1):
    /// `[%command %pow %dumb-zkpow prf dig header nonce]`.
    Mined,
    /// Driver → node: set mining-payout pubkey(s).
    SetPubKey,
}

impl ZkPowMinerWire {
    pub fn verb(&self) -> &'static str {
        match self {
            ZkPowMinerWire::Enable => "enable",
            ZkPowMinerWire::Candidate => "candidate",
            ZkPowMinerWire::Mined => "mined",
            ZkPowMinerWire::SetPubKey => "setpubkey",
        }
    }
}

impl Wire for ZkPowMinerWire {
    const VERSION: u64 = 1;
    const SOURCE: &'static str = "zk-pow-miner";

    fn to_wire(&self) -> WireRepr {
        let tags = vec![self.verb().into()];
        WireRepr::new(ZkPowMinerWire::SOURCE, ZkPowMinerWire::VERSION, tags)
    }
}
