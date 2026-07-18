//! High-level gRPC client for the node's private `NockAppService`.
//!
//! Wraps [`nockapp_grpc::private_nockapp::client::PrivateNockAppGrpcClient`]
//! with mining-specific helpers: `set_mining_key`, `enable_mining`,
//! `watch_candidates`, and `submit_mined_block`. These are the four
//! operations every external miner binary (ZK-PoW + AI-PoW) needs.
//!
//! Each typed helper takes a [`WireRepr`] argument so the caller can
//! supply its own crate-specific wire vocabulary (e.g.
//! `zk_pow_miner::ZkPowMinerWire` with `SOURCE = "zk-pow-miner"`, or
//! `ai_pow_miner::wire::AiPowMinerWire` with `SOURCE = "ai-pow-miner"`).
//! The shared substrate stays wire-agnostic; the kernel-side dispatcher
//! in `hoon/apps/dumbnet/inner.hoon` routes by source.
//!
//! The candidate watcher decodes `%mine` effects from the server's
//! `WatchEffects` stream into [`MiningCandidate`]. The pokes mirror
//! the noun shapes the kernel-side Hoon expects — historically built
//! in-process in `crates/nockchain/src/mining.rs` (now removed).

use std::pin::Pin;

use futures::{Stream, StreamExt};
use nockapp::nockapp::wire::WireRepr;
use nockapp::noun::slab::NounSlab;
use nockapp::noun::AtomExt;
use nockapp_grpc::private_nockapp::client::PrivateNockAppGrpcClient;
use nockapp_grpc::wire_conversion::nockapp_wire_to_grpc;
use nockvm::noun::{Atom, D, NO, T, YES};
use nockvm_macros::tas;
use thiserror::Error;
use tracing::debug;

use crate::candidate::{CandidateDecodeError, MiningCandidate};
use crate::key_config::{MiningKeyConfig, MiningPkhConfig};

#[derive(Debug, Error)]
pub enum NodeClientError {
    #[error("gRPC transport error: {0}")]
    Grpc(#[from] nockapp_grpc::error::NockAppGrpcError),
    #[error("kernel did not acknowledge poke")]
    PokeNotAcked,
    #[error("decoding candidate effect: {0}")]
    Decode(#[from] CandidateDecodeError),
}

/// Default `pid` tag sent on Poke / WatchEffects. The node uses it only
/// for tracking / logging; any nonzero int is fine.
const DEFAULT_PID: i32 = 1;

/// Stream of decoded mining candidates yielded by [`NodeClient::watch_candidates`].
///
/// Boxed + pinned so callers can hold it as a single owned type without
/// naming the underlying combinator stack.
pub type CandidateStream =
    Pin<Box<dyn Stream<Item = Result<MiningCandidate, NodeClientError>> + Send>>;

pub struct NodeClient {
    client: PrivateNockAppGrpcClient,
}

impl NodeClient {
    /// Connect to a node's private gRPC service. `address` is a URL
    /// like `http://127.0.0.1:5555`.
    pub async fn connect<T: AsRef<str>>(address: T) -> Result<Self, NodeClientError> {
        let client = PrivateNockAppGrpcClient::connect(address).await?;
        Ok(Self { client })
    }

    /// Push the mining-reward configuration to the node's kernel.
    /// Same noun shape as the old in-process
    /// `set_mining_key_advanced(...)`: `[%command %set-mining-key-advanced
    /// configs-list pkh-configs-list]`. The caller supplies the wire
    /// (typically `XPowMinerWire::SetPubKey.to_wire()` for their crate).
    pub async fn set_mining_key(
        &mut self,
        wire: WireRepr,
        configs: Vec<MiningKeyConfig>,
        pkh_configs: Vec<MiningPkhConfig>,
    ) -> Result<(), NodeClientError> {
        let mut slab = NounSlab::new();
        let set_mining_key_adv =
            <Atom as AtomExt>::from_value(&mut slab, "set-mining-key-advanced")
                .expect("set-mining-key-advanced atom");

        // v0 (pubkey) configs list — cons-cell list, terminated by 0.
        let mut configs_list = D(0);
        for config in configs {
            let mut keys_noun = D(0);
            for key in config.keys {
                let key_atom = <Atom as AtomExt>::from_value(&mut slab, key).expect("key atom");
                keys_noun = T(&mut slab, &[key_atom.as_noun(), keys_noun]);
            }
            let config_tuple = T(&mut slab, &[D(config.share), D(config.m), keys_noun]);
            configs_list = T(&mut slab, &[config_tuple, configs_list]);
        }

        // v1 (pubkey-hash) configs list — cons-cell list, terminated by 0.
        let mut pkh_configs_list = D(0);
        for config in pkh_configs {
            let pkh_noun = <Atom as AtomExt>::from_value(&mut slab, config.pkh)
                .expect("pkh atom")
                .as_noun();
            let config_tuple = T(&mut slab, &[D(config.share), pkh_noun]);
            pkh_configs_list = T(&mut slab, &[config_tuple, pkh_configs_list]);
        }

        let poke = T(
            &mut slab,
            &[
                D(tas!(b"command")),
                set_mining_key_adv.as_noun(),
                configs_list,
                pkh_configs_list,
            ],
        );
        slab.set_root(poke);

        self.poke_wire(wire, slab).await
    }

    /// Toggle mining on / off. Same noun shape as the old in-process
    /// `enable_mining(...)`: `[%command %enable-mining flag]`. The
    /// caller supplies the wire (typically
    /// `XPowMinerWire::Enable.to_wire()` for their crate).
    pub async fn enable_mining(
        &mut self,
        wire: WireRepr,
        enable: bool,
    ) -> Result<(), NodeClientError> {
        let mut slab = NounSlab::new();
        let enable_mining_atom =
            <Atom as AtomExt>::from_value(&mut slab, "enable-mining").expect("enable-mining atom");
        let poke = T(
            &mut slab,
            &[D(tas!(b"command")), enable_mining_atom.as_noun(), if enable { YES } else { NO }],
        );
        slab.set_root(poke);
        self.poke_wire(wire, slab).await
    }

    /// Submit a mined block. The caller produces the same poke payload
    /// the in-process driver historically poked the main kernel with
    /// (the `poke` tail of a `%mine-result` success effect from the
    /// miner kernel) — this just wraps the gRPC round-trip. The caller
    /// supplies the wire (typically `XPowMinerWire::Mined.to_wire()`
    /// for their crate).
    pub async fn submit_mined_block(
        &mut self,
        wire: WireRepr,
        slab: NounSlab,
    ) -> Result<(), NodeClientError> {
        self.poke_wire(wire, slab).await
    }

    /// Subscribe to the node's mining-candidate effects, decoded into
    /// [`MiningCandidate`]s. The `head_filter` is passed through to the
    /// `WatchEffects` RPC so non-matching traffic is dropped server-side.
    ///
    /// Typical callers:
    /// - ZK-PoW miner: `vec![b"mine-zk".to_vec()]`
    /// - AI-PoW miner: `vec![b"mine-ai".to_vec()]`
    ///
    /// The returned stream is owned; callers can drop it to end the
    /// subscription. Server-side disconnect surfaces as a `None` from
    /// `Stream::next`.
    pub async fn watch_candidates(
        &mut self,
        head_filter: Vec<Vec<u8>>,
    ) -> Result<CandidateStream, NodeClientError> {
        let raw = self.client.watch_effects(DEFAULT_PID, head_filter).await?;
        let mapped = raw.filter_map(|item| async move {
            match item {
                Err(e) => Some(Err(NodeClientError::Grpc(e))),
                Ok(slab) => match MiningCandidate::from_effect_slab(slab) {
                    Ok(Some(c)) => Some(Ok(c)),
                    Ok(None) => {
                        // server-side head_filter should mean this never fires,
                        // but the check is defensive — keep the stream alive.
                        debug!("watch_candidates: dropped non-mine-zk/mine-ai effect");
                        None
                    }
                    Err(e) => Some(Err(NodeClientError::Decode(e))),
                },
            }
        });
        Ok(Box::pin(mapped) as CandidateStream)
    }

    /// Send a poke on an arbitrary [`WireRepr`]. This is the underlying
    /// gRPC operation; the typed helpers
    /// ([`set_mining_key`](Self::set_mining_key),
    /// [`enable_mining`](Self::enable_mining),
    /// [`submit_mined_block`](Self::submit_mined_block)) all delegate
    /// here.
    pub async fn poke_wire(
        &mut self,
        wire: WireRepr,
        slab: NounSlab,
    ) -> Result<(), NodeClientError> {
        let grpc_wire = nockapp_wire_to_grpc(&wire);
        let payload = slab.jam().to_vec();
        let acked = self.client.poke(DEFAULT_PID, grpc_wire, payload).await?;
        if !acked {
            return Err(NodeClientError::PokeNotAcked);
        }
        Ok(())
    }
}
