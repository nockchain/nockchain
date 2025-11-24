use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;

use nockapp::driver::{make_driver, IODriverFn, NockAppHandle, Operation};
use nockapp::kernel::boot;
use nockapp::noun::slab::NounSlab;
use nockapp::wire::{SystemWire, Wire, WireRepr, WireTag as AppWireTag};
use nockapp::{exit_driver, file_driver};
use nockapp::{AtomExt, Bytes, NockApp, NockAppError, Noun};
use nockapp_grpc::client::NockAppGrpcClient;
use nockapp_grpc::driver::{grpc_listener_driver, grpc_server_driver};
use nockapp_grpc::wire_conversion::create_grpc_wire;
use nockapp_grpc::NockAppGrpcServer;
use nockvm::noun::{Atom, D, T};
use nockvm_macros::tas;
use noun_serde::{NounDecode, NounDecodeError, NounEncode};
use tracing::{error, info};

use grpc::string_to_atom;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let cli = boot::default_boot_cli(false);
    boot::init_default_tracing(&cli);

    let source_filename = Path::new(file!()).file_stem().unwrap().to_str().unwrap();
    let fallback_filename = format!("{}.jam", source_filename);

    let kernel = fs::read("out.jam")
        .or_else(|_| fs::read(&fallback_filename))
        .map_err(|e| format!("Failed to read kernel file: {}", e))?;
    let mut nockapp: NockApp = boot::setup(&kernel, Some(cli), &[], source_filename, None)
        .await
        .map_err(|e| format!("Kernel setup failed: {}", e))?;

    //  Set up drivers.
    nockapp.add_io_driver(grpc_server_driver()).await;
    nockapp.add_io_driver(exit_driver()).await;

    //  Run app kernel.
    println!("Starting main kernel loop...");
    nockapp.run().await.expect("Failed to run app");

    Ok(())
}
