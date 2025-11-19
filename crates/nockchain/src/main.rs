use std::error::Error;

use clap::Parser;
use kernels::dumb::KERNEL;
use nockapp::kernel::boot;
use nockapp::NockApp;
use nockchain::NockchainAPIConfig;
use zkvm_jetpack::hot::produce_prover_hot_state;

// When enabled, use jemalloc for more stable memory allocation
#[cfg(all(feature = "jemalloc", not(feature = "tracing-heap")))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(feature = "tracing-heap")]
#[global_allocator]
static ALLOC: tracy_client::ProfiledAllocator<tikv_jemallocator::Jemalloc> =
    tracy_client::ProfiledAllocator::new(tikv_jemallocator::Jemalloc, 100);

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    nockvm::check_endian();
    let cli = nockchain::NockchainCli::parse();
    boot::init_default_tracing(&cli.nockapp_cli);

    let prover_hot_state = produce_prover_hot_state();

    let api_config = if let Some(addr) = cli.bind_public_grpc_addr {
        NockchainAPIConfig::EnablePublicServer(addr)
    } else {
        NockchainAPIConfig::DisablePublicServer
    };

    let mut nockchain: NockApp =
        nockchain::init_with_kernel(cli, KERNEL, prover_hot_state.as_slice(), api_config).await?;
    nockchain.run().await?;
    Ok(())
}
