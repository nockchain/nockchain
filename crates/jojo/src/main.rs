use anyhow::Result;
use clap::{Parser, Subcommand};
use flume::Receiver;
use futures::{Stream, StreamExt};
use kernels_open_jojo::KERNEL;
use nockapp::kernel::boot::{self, Cli};
use nockapp::kernel::form::Kernel;
use nockapp::noun::slab::NounSlab;
use nockapp::save::SaveableCheckpoint;
use nockapp::wire::Wire;
use zkvm_jetpack::hot::produce_prover_hot_state;

mod parse;
mod shell;
use shell::Shell;

struct SendSlab(NounSlab);

unsafe impl Send for SendSlab {}
unsafe impl Sync for SendSlab {}

pub enum JojoWire {
    Run,
}

impl JojoWire {
    pub fn verb(&self) -> &'static str {
        match self {
            JojoWire::Run => "run",
        }
    }
}

impl Wire for JojoWire {
    const VERSION: u64 = 1;
    const SOURCE: &'static str = "jojo";

    fn to_wire(&self) -> nockapp::wire::WireRepr {
        let tags = vec![self.verb().into()];
        nockapp::wire::WireRepr::new(JojoWire::SOURCE, JojoWire::VERSION, tags)
    }
}

#[derive(Subcommand, Debug, Clone)]
pub enum Mode {
    Shell,
}

/// Command line arguments
#[derive(Parser, Debug, Clone)]
#[command(name = "jojo")]
pub struct JojoCli {
    #[command(flatten)]
    pub nockapp_cli: Cli,
    #[command(subcommand)]
    pub mode: Mode,
}

async fn run_kernel(
    pokes: impl Stream<Item = SendSlab> + Send + 'static,
    cli: Cli,
) -> Receiver<SendSlab> {
    let hot_state = produce_prover_hot_state();

    let kernel = Kernel::<SaveableCheckpoint>::load_with_hot_state(
        KERNEL,
        None,
        &hot_state,
        vec![],
        cli.trace_opts.into(),
    )
    .await
    .expect("Could not load jojo kernel");

    let (tx, rx) = flume::bounded(0);

    let task = async move {
        let mut pokes = core::pin::pin!(pokes);
        while let Some(slab) = pokes.next().await {
            let effects = kernel
                .poke(JojoWire::Run.to_wire(), slab.0)
                .await
                .expect("Could not poke jojo kernel with slab");

            if tx.send_async(SendSlab(effects)).await.is_err() {
                break;
            }
        }
    };

    tokio::spawn(task);

    rx
}

#[tokio::main]
async fn main() -> Result<()> {
    nockvm::check_endian();
    let cli = JojoCli::parse();
    boot::init_default_tracing(&cli.nockapp_cli);

    match cli.mode {
        Mode::Shell => Shell::default().run(cli.nockapp_cli).await,
    }
}
