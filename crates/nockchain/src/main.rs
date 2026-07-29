use std::error::Error;

use chaff::Chaff;
use kernels_open_dumb::KERNEL;
use nockapp::kernel::boot;
use nockchain::NockchainAPIConfig;
use zkvm_jetpack::hot::produce_prover_hot_state;

// jemalloc is REQUIRED (not optional): the AI-PoW verifier-setup table pages large
// contexts in/out of memory, and returning that freed memory to the OS relies on
// jemalloc's decay-based purging (the system allocator retains it, so RSS would grow
// with every page-in).
#[cfg(not(feature = "tracing-heap"))]
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

    // Fail fast on invalid CLI config (e.g. a malformed --fakenet-*-asert-* combo)
    // before the multi-minute AI-PoW verifier-setup generation below.
    if let Err(e) = cli.validate() {
        return Err(e.into());
    }

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

    // CLI takes precedence over the environment. Lower caps are an explicit
    // memory-for-latency tradeoff; the default keeps all 13 shape keys resident after
    // first use.
    if let Some(cap) = cli.ai_pow_verifier_cache_cap {
        std::env::set_var(
            ai_pow_jets::setup::AI_POW_VERIFIER_CACHE_CAP_ENV,
            cap.to_string(),
        );
    }

    // The installer may prove every production setup bucket. It must complete before
    // `init_with_kernel` starts I/O drivers, so an unready node cannot accept or dial
    // network traffic while its consensus verifier is unavailable.
    let buckets = ai_pow_jets::setup::production_verifier_setup_buckets();
    ai_pow_jets::setup::install_or_build_verifier_setup(&data_dir, &buckets)?;

    let mut nockchain =
        nockchain::init_with_kernel::<Chaff>(cli, KERNEL, prover_hot_state.as_slice(), api_config)
            .await?;
    nockchain.run().await?;
    Ok(())
}
