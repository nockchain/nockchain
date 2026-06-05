use std::error::Error;

use nockapp::noun::slab::NounSlab;
use nockapp::{AtomExt, Bytes};
use nockvm::noun::Atom;

pub fn string_to_atom(slab: &mut NounSlab, s: &str) -> Result<Atom, Box<dyn Error>> {
    let bytes = Bytes::from(s.as_bytes().to_vec());
    Ok(Atom::from_bytes(slab, &bytes))
}

pub const GRPC_PORT: &str = "5555";
