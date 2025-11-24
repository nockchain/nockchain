use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;

use nockapp::driver::{make_driver, IODriverFn, NockAppHandle, Operation};
use nockapp::kernel::boot;
use nockapp::noun::slab::NounSlab;
use nockapp::wire::{SystemWire, Wire, WireRepr, WireTag as AppWireTag};
use nockapp::{AtomExt, Bytes, NockApp, NockAppError, Noun};
use nockapp::{exit_driver, file_driver};
use nockapp::utils::make_tas;
use nockapp_grpc::NockAppGrpcServer;
use nockapp_grpc::client::NockAppGrpcClient;
use nockapp_grpc::driver::{GrpcEffect, grpc_listener_driver, grpc_server_driver};
use nockapp_grpc::wire_conversion::{create_grpc_wire, grpc_wire_to_nockapp};
use nockvm::noun::{Atom, D, T};
use nockvm_macros::tas;
use noun_serde::{NounDecode, NounDecodeError, NounEncode};
use tracing::{error, info};

use grpc::string_to_atom;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let cli = boot::default_boot_cli(false);
    boot::init_default_tracing(&cli);

    let source_filename = Path::new(file!())
        .file_stem()
        .unwrap()
        .to_str()
        .unwrap();
    let fallback_filename = format!("{}.jam", source_filename);

    let kernel = fs::read("out.jam")
        .or_else(|_| fs::read(&fallback_filename))
        .map_err(|e| format!("Failed to read kernel file: {}", e))?;
    let mut nockapp: NockApp = boot::setup(
        &kernel,
        Some(cli),
        &[],
        source_filename,
        None
    )
    .await
    .map_err(|e| format!("Kernel setup failed: {}", e))?;

    //  Load demo poke.
    let mut poke_slab = NounSlab::new();
    let str_atom = string_to_atom(&mut poke_slab, "hello world")?;
    let head = make_tas(&mut poke_slab, "poke-value").as_noun();
    let command_noun = T(&mut poke_slab, &[head, str_atom.as_noun()]);
    poke_slab.set_root(command_noun);

    //  The demo poke generates a %grpc effect which we want to emit.
    nockapp
        .add_io_driver(nockapp::one_punch_driver(poke_slab, Operation::Poke))
        .await;
    nockapp
        .add_io_driver(grpc_listener_driver(format!("http://127.0.0.1:{}", grpc::GRPC_PORT.to_string())))
        .await;
    nockapp
        .add_io_driver(exit_driver())
        .await;

    nockapp.run().await;

    Ok(())
}
