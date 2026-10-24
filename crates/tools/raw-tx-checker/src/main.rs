use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use nockchain_types::tx_engine::common::Hash;
use nockvm::mem::{NockStack, NOCK_STACK_SIZE_TINY};
use nockvm::noun::IndirectAtom;
use nockvm::serialization;
use noun_serde::prelude::*;
use zkvm_jetpack::jets::tip5_jets::hash_hashable;

#[derive(Parser)]
#[command(author, version, about = "Inspect raw transaction hashable jams")]
struct Args {
    /// Path to a jammed hashable noun produced from a raw transaction
    #[arg(value_name = "JAM_PATH")]
    input: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let jam_bytes = fs::read(&args.input)
        .with_context(|| format!("failed to read {}", args.input.display()))?;

    let mut stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
    let space = stack.noun_space();
    let jam_atom = unsafe {
        let mut atom = IndirectAtom::new_raw_bytes(&mut stack, jam_bytes.len(), jam_bytes.as_ptr());
        atom.normalize_as_atom_stack()
    };

    let hashable = serialization::cue(&mut stack, jam_atom)
        .map_err(|err| anyhow!("failed to cue jammed noun: {err:?}"))?;
    let digest_noun = hash_hashable(&mut stack, hashable, &space)
        .map_err(|err| anyhow!("hash_hashable jet failed: {err:?}"))?;

    let tip5_hash = Hash::from_noun(&digest_noun, &space)?;
    println!("Tip5 limbs (hex):");
    for (idx, limb) in tip5_hash.0.iter().enumerate() {
        let belt_u64 = limb.0;
        println!("  belt_u64[{idx}]: 0x{belt_u64:016x}");
    }
    println!("Base58: {}", tip5_hash.to_base58());

    Ok(())
}
