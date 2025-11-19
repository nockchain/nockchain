use std::error::Error;

use clap::Parser;
use kernels::nockchain_peek::KERNEL;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("default provider already set elsewhere");

    let cli = nockchain_peek::NockchainPeekCli::parse();
    let mut nockchain_peek = nockchain_peek::init_with_kernel(cli, KERNEL).await?;
    nockchain_peek.run().await?;
    Ok(())
}
