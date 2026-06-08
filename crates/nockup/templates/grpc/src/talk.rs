use std::error::Error;
use std::fs;
use std::path::Path;
use std::time::Duration;

use {{rust_crate_name}}::string_to_atom;
use {{rust_crate_name}}::GRPC_PORT;
use nockapp::driver::Operation;
use nockapp::kernel::boot;
use nockapp::noun::slab::NounSlab;
use nockapp::utils::make_tas;
use nockapp::NockApp;
use nockapp_grpc::services::private_nockapp::grpc_listener_driver;
use nockvm::noun::T;

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
        .add_io_driver(grpc_listener_driver(format!(
            "http://127.0.0.1:{}",
            GRPC_PORT.to_string()
        )))
        .await;

    match tokio::time::timeout(Duration::from_secs(2), nockapp.run()).await {
        Ok(result) => result.expect("Failed to run app"),
        Err(_) => println!("Finished gRPC demo request window"),
    }

    Ok(())
}
