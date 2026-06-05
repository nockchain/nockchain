use std::error::Error;

use nockapp::noun::slab::{Jammer, NounSlab};
use nockapp::utils::make_tas;
use nockapp::wire::Wire;
use nockapp::{AtomExt, Bytes, NockApp, NockAppError, ToBytes};
use nockchain_types::BlockchainConstants;
use nockvm::noun::{Atom, D, T};
use nockvm_macros::tas;
use noun_serde::NounEncode;

#[cfg(feature = "bazel_build")]
pub static FAKENET_GENESIS_BLOCK: &[u8] = include_bytes!(env!("FAKENET_GENESIS_PATH"));

#[cfg(not(feature = "bazel_build"))]
pub static FAKENET_GENESIS_BLOCK: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/jams/fakenet-genesis-pow-2-bex-1.jam"
));

// TODO: Necessary for now, but we will delete this parameter from genesis seal
pub const DEFAULT_GENESIS_BLOCK_HEIGHT: u64 = 0;
pub const FAKENET_GENESIS_MESSAGE: &str = "3WNP3WtcQJYtP5PCvFHDQVEeiZEznsULEY5Lc4vUKV64Ge8feBxAkYp";
pub const REALNET_GENESIS_MESSAGE: &str = "2c8Ltbg44dPkEGcNPupcVAtDgD87753M9pG2fg8yC2mTEqg5qAFvvbT";

pub enum SetupCommand {
    PokeFakenetConstants(BlockchainConstants),
    PokeSetGenesisSeal(String),
    PokeSetBtcData,
}

pub async fn poke<J: Jammer + Send + 'static>(
    nockapp: &mut NockApp<J>,
    command: SetupCommand,
) -> Result<(), Box<dyn Error>> {
    let poke: NounSlab = match command {
        SetupCommand::PokeFakenetConstants(constants) => {
            let mut poke_slab = NounSlab::new();
            let tag = make_tas(&mut poke_slab, "set-constants").as_noun();
            let constants_noun = constants.to_noun(&mut poke_slab);
            let poke_noun = T(&mut poke_slab, &[D(tas!(b"command")), tag, constants_noun]);
            poke_slab.set_root(poke_noun);
            poke_slab
        }
        SetupCommand::PokeSetGenesisSeal(seal) => {
            let mut poke_slab = NounSlab::new();
            let block_height_noun =
                Atom::new(&mut poke_slab, DEFAULT_GENESIS_BLOCK_HEIGHT).as_noun();
            let seal_byts = Bytes::from(
                seal.to_bytes()
                    .expect("Failed to convert seal message to bytes"),
            );
            let seal_noun = Atom::from_bytes(&mut poke_slab, &seal_byts).as_noun();
            let tag = Bytes::from(b"set-genesis-seal".to_vec());
            let set_genesis_seal = Atom::from_bytes(&mut poke_slab, &tag).as_noun();
            let poke_noun = T(
                &mut poke_slab,
                &[D(tas!(b"command")), set_genesis_seal, block_height_noun, seal_noun],
            );
            poke_slab.set_root(poke_noun);
            poke_slab
        }
        SetupCommand::PokeSetBtcData => {
            let mut poke_slab = NounSlab::new();
            let poke_noun = T(
                &mut poke_slab,
                &[D(tas!(b"command")), D(tas!(b"btc-data")), D(0)],
            );
            poke_slab.set_root(poke_noun);
            poke_slab
        }
    };

    nockapp
        .poke(nockapp::wire::SystemWire.to_wire(), poke)
        .await?;
    Ok(())
}

pub fn heard_fake_genesis_block(
    fake_genesis_data: Option<Vec<u8>>,
) -> Result<NounSlab, NockAppError> {
    let mut poke_slab = NounSlab::new();
    let tag = make_tas(&mut poke_slab, "heard-block").as_noun();
    let block_bytes = if let Some(data) = fake_genesis_data {
        Bytes::from(data)
    } else {
        Bytes::from(FAKENET_GENESIS_BLOCK)
    };
    let block = poke_slab.cue_into(block_bytes)?;
    let poke_noun = T(&mut poke_slab, &[D(tas!(b"fact")), D(0), tag, block]);
    poke_slab.set_root(poke_noun);
    Ok(poke_slab)
}
