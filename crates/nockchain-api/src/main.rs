use std::error::Error;

use clap::Parser;
use kernels::dumb::KERNEL;
use nockapp::kernel::boot;
use nockchain::NockchainAPIConfig;
use zkvm_jetpack::hot::produce_prover_hot_state;

// Disable jemalloc when we're running Miri
#[cfg(all(not(miri), not(feature = "tracing-heap"), not(feature = "malloc")))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(feature = "tracing-heap")]
#[global_allocator]
static ALLOC: tracy_client::ProfiledAllocator<tikv_jemallocator::Jemalloc> =
    tracy_client::ProfiledAllocator::new(tikv_jemallocator::Jemalloc, 100);

#[cfg(feature = "tracing-heap")]
#[global_allocator]
static ALLOC: tracy_client::ProfiledAllocator<tikv_jemallocator::Jemalloc> =
    tracy_client::ProfiledAllocator::new(tikv_jemallocator::Jemalloc, 100);

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    nockvm::check_endian();
    let mut cli = nockchain::NockchainCli::parse();
    cli.nockapp_cli.color = clap::ColorChoice::Never;
    boot::init_default_tracing(&cli.nockapp_cli);

    let prover_hot_state = produce_prover_hot_state();

    let api_config = if let Some(addr) = cli.bind_public_grpc_addr {
        NockchainAPIConfig::EnablePublicServer(addr)
    } else {
        NockchainAPIConfig::DisablePublicServer
    };

    let mut nockchain: nockapp::NockApp =
        nockchain::init_with_kernel(cli, KERNEL, prover_hot_state.as_slice(), api_config).await?;
    nockchain.run().await?;
    Ok(())
}
