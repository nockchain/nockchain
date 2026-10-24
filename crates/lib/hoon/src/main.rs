// Execute nock scripts
use clap::Parser;
use tracing::info;
use zkvm_jetpack::hot::produce_prover_hot_state;

#[tokio::main]
pub async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = hoon::HoonCli::parse();
    nockapp::kernel::boot::init_default_tracing(&cli.boot.clone());
    if cli.out_dir.is_none() {
        info!("WARNING: out_dir is not set so output will not be saved")
    }
    info!("NOTE: save is set to: {:?}", cli.out_dir);
    let prover_hot_state = produce_prover_hot_state();
    hoon::run(cli, &prover_hot_state).await
}
