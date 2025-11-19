use std::error::Error;

use clap::{arg, command, Parser, Subcommand};
use nockapp::driver::Operation;
use nockapp::kernel::boot;
use nockapp::noun::slab::NounSlab;
use nockapp::utils::make_tas;
use nockapp::{file_driver, markdown_driver, AtomExt, NockApp};
use nockapp_grpc::private_nockapp::grpc_listener_driver;
use nockvm::noun::{D, T};
use nockvm_macros::tas;
use tracing::info;
use zkvm_jetpack::hot::produce_prover_hot_state;

#[derive(Parser, Debug, Clone)]
#[command(name = "nockchain-peek")]
pub struct NockchainPeekCli {
    #[command(flatten)]
    nockapp_cli: nockapp::kernel::boot::Cli,
    #[arg(
        long,
        value_name = "GRPC_ADDRESS",
        default_value = "http://localhost:5555",
        help = "Nockchain gRPC server address"
    )]
    grpc_address: String,
    #[command(subcommand)]
    command: PeekCommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum PeekCommand {
    #[command(about = "Peek at the heaviest block ID")]
    Heavy,
    #[command(about = "Peek at a specific block by ID")]
    Block {
        #[arg(help = "Block ID in base58 format")]
        block_id: String,
    },
    #[command(about = "Peek at all blocks (full block data with pow)")]
    Blocks,
    #[command(about = "Peek at the heaviest block page")]
    HeaviestBlock,
    #[command(about = "Peek at a page by height using heavy-n")]
    HeavyN {
        #[arg(help = "Page number to peek")]
        page_number: u64,
    },
    #[command(about = "Peek at small blocks (blocks without pow data)")]
    SmallBlocks,
    #[command(about = "Check for note intersection in a block")]
    CheckNotes {
        #[arg(help = "Block ID in base58 format")]
        block_id: String,
    },
}

pub async fn init_with_kernel(
    cli: NockchainPeekCli,
    kernel_jam: &[u8],
) -> Result<NockApp, Box<dyn Error>> {
    let prover_hot_state = produce_prover_hot_state();

    let mut nockapp = boot::setup(
        kernel_jam,
        cli.nockapp_cli.clone(),
        prover_hot_state.as_slice(),
        "nockchain-peek",
        None,
    )
    .await?;
    boot::init_default_tracing(&cli.nockapp_cli);

    let mut born_slab = NounSlab::new();
    let command = cli.command.clone();

    let command_noun = command.to_noun(&mut born_slab)?;
    let born_noun = T(&mut born_slab, &[D(tas!(b"born")), command_noun]);
    born_slab.set_root(born_noun);
    nockapp
        .add_io_driver(nockapp::one_punch_driver(born_slab, Operation::Poke))
        .await;
    nockapp.add_io_driver(markdown_driver()).await;
    nockapp.add_io_driver(file_driver()).await;
    nockapp.add_io_driver(nockapp::exit_driver()).await;
    nockapp
        .add_io_driver(grpc_listener_driver(cli.grpc_address.clone()))
        .await;
    info!("Connected gRPC listener to {}", cli.grpc_address);

    Ok(nockapp)
}

impl PeekCommand {
    fn to_noun(&self, slab: &mut NounSlab) -> Result<nockvm::noun::Noun, Box<dyn Error>> {
        use nockvm::noun::Atom;
        match self {
            PeekCommand::Heavy => {
                let heavy_atom = make_tas(slab, "heavy");
                let path = T(slab, &[heavy_atom.as_noun(), D(0)]);
                Ok(path)
            }
            PeekCommand::Block { block_id } => {
                let block_id_atom = Atom::from_value(slab, block_id.as_bytes())
                    .map_err(|e| format!("failed to create block_id atom: {}", e))?
                    .as_noun();
                Ok(T(slab, &[D(tas!(b"block")), block_id_atom]))
            }
            PeekCommand::Blocks => {
                let blocks_atom = make_tas(slab, "blocks");
                let path = T(slab, &[blocks_atom.as_noun(), D(0)]);
                Ok(path)
            }
            PeekCommand::HeaviestBlock => {
                let heaviest_block_atom = make_tas(slab, "heaviest-block");
                let path = T(slab, &[heaviest_block_atom.as_noun(), D(0)]);
                Ok(path)
            }
            PeekCommand::HeavyN { page_number } => {
                info!("page_number: {}", page_number);
                let page_number_atom = Atom::from_value(slab, &page_number.to_le_bytes()[..])
                    .map_err(|e| format!("failed to create page_number atom: {}", e))?;
                info!("page_number: {:?}", page_number_atom);

                Ok(T(slab, &[D(tas!(b"heavy-n")), page_number_atom.as_noun()]))
            }
            PeekCommand::SmallBlocks => {
                let small_blocks_atom = make_tas(slab, "small-blocks");
                let path = T(slab, &[small_blocks_atom.as_noun(), D(0)]);
                Ok(path)
            }
            PeekCommand::CheckNotes { block_id } => {
                let block_id_atom = Atom::from_value(slab, block_id.as_bytes())
                    .map_err(|e| format!("failed to create block_id atom: {}", e))?
                    .as_noun();
                Ok(T(slab, &[D(tas!(b"chknote")), block_id_atom]))
            }
        }
    }
}
