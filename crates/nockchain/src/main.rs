use std::error::Error;

use chaff::Chaff;
use kernels_open_dumb::KERNEL;
use nockapp::kernel::boot;
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
    let cli = nockchain::NockchainCli::parse_with_default_stack_size(boot::NockStackSize::Large);
    boot::init_default_tracing(&cli.nockapp_cli);

    // Shared jet registry: the STARK prover jets + the AI-PoW consensus verify
    // jet. roswell must extend the identical set (crates/roswell) so `check-pow`'s
    // `~/ %ai-pow-verify` matches in both binaries.
    let mut prover_hot_state = produce_prover_hot_state();
    prover_hot_state.extend(ai_pow_jets::produce_ai_pow_hot_state());

    let api_config = if let Some(addr) = cli.bind_public_grpc_addr {
        NockchainAPIConfig::EnablePublicServer(addr)
    } else {
        NockchainAPIConfig::DisablePublicServer
    };

    // Resolve the node data dir the same way `boot::setup` does (explicit
    // --data-dir, else `./.data.nockchain`) BEFORE `cli` is moved into
    // `init_with_kernel`, so we can load the AI-PoW verifier-setup cache from it.
    let data_dir = cli
        .nockapp_cli
        .data_dir
        .clone()
        .unwrap_or_else(|| nockapp::default_data_dir("nockchain"));

    let mut nockchain =
        nockchain::init_with_kernel::<Chaff>(cli, KERNEL, prover_hot_state.as_slice(), api_config)
            .await?;

    // Install the AI-PoW compact verifier-setup table from the data dir BEFORE
    // processing any block. If the cache is present + valid this is fast (load seeds
    // + rebuild; no proving); if it is absent (or corrupt) the node GENERATES the
    // table once (a one-time ~5-minute boot delay; it logs this), caches it, and
    // injects it — validating it against the committed v0 consensus digest either
    // way. A node with no valid verifier setup cannot validate %ai-pow blocks, so any
    // failure is FATAL: we propagate the error and shut down rather than run blind.
    let buckets = ai_pow_jets::setup::production_verifier_setup_buckets();
    ai_pow_jets::setup::install_or_build_verifier_setup(&data_dir, &buckets)?;

    nockchain.run().await?;
    Ok(())
}
