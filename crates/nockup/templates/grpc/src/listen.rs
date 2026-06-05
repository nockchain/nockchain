use std::error::Error;
use std::fs;
use std::net::SocketAddr;
use std::path::Path;

use {{rust_crate_name}}::GRPC_PORT;
use nockapp::kernel::boot;
use nockapp::{exit_driver, NockApp};
use nockapp_grpc::services::private_nockapp::grpc_server_driver;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let cli = boot::default_boot_cli(false);
    boot::init_default_tracing(&cli);

    let source_filename = Path::new(file!()).file_stem().unwrap().to_str().unwrap();
    let fallback_filename = format!("{}.jam", source_filename);

    let kernel = fs::read("out.jam")
        .or_else(|_| fs::read(&fallback_filename))
        .map_err(|e| format!("Failed to read kernel file: {}", e))?;
    let mut nockapp: NockApp = boot::setup(&kernel, cli, &[], source_filename, None)
        .await
        .map_err(|e| format!("Kernel setup failed: {}", e))?;

    //  Set up drivers.
    let addr: SocketAddr = format!("127.0.0.1:{GRPC_PORT}").parse()?;
    nockapp.add_io_driver(grpc_server_driver(addr)).await;
    nockapp.add_io_driver(exit_driver()).await;

    //  Run app kernel.
    println!("Starting main kernel loop...");
    nockapp.run().await.expect("Failed to run app");

    Ok(())
}
