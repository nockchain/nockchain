//! Peer protocol v3: framed protobuf -> checked peer values -> consensus nouns.
//!
//! Generated DTOs are private to this boundary. The existing driver receives
//! locally constructed, canonically jammed nouns during the migration; no wire
//! field contains jam. Consensus validation remains the kernel's responsibility.
pub(crate) mod codec;
mod common;
#[cfg(test)]
mod corpus_tests;
mod envelope;
mod page;
mod transaction;

mod pb {
    include!(concat!(env!("OUT_DIR"), "/nockchain.peer.v3.rs"));
}

pub(crate) use envelope::{
    decode_request, decode_response, encode_request, encode_response, response_envelope_encoded_len,
};
