use clap::Parser;
use futures::FutureExt;
use hoonc::*;
use nockapp::kernel::boot;
use nockvm::mem::{AllocationError, NewStackError};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let cli = HoonCli::parse();
    boot::init_default_tracing(&cli.boot.clone());
    if cli.parse_only_ast_jam {
        let out_path = cli.output.clone().ok_or_else(|| {
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "--parse-only-ast-jam requires --output",
            )) as Error
        })?;
        let exported = export_parse_cache_ast_jam_if_missing(
            cli.entry.clone(),
            cli.directory.clone(),
            out_path,
            true,
        )
        .await?;
        println!("parse-cache AST jam saved to {}", exported.display());
        return Ok(());
    }
    let result = std::panic::AssertUnwindSafe(async {
        let (mut nockapp, _) = initialize_hoonc(cli).await?;
        nockapp.run().await?;
        Ok::<(), Error>(())
    })
    .catch_unwind()
    .await;

    match result {
        Ok(Ok(_)) => println!("no panic!"),
        Ok(Err(e)) => println!("Error initializing NockApp: {e:?}"),
        Err(e) => {
            println!("Caught panic!");
            // now we downcast the error
            // and print it out
            if let Some(e) = e.downcast_ref::<AllocationError>() {
                println!("Allocation error occurred: {}", e);
            } else if let Some(e) = e.downcast_ref::<NewStackError>() {
                println!("NockStack creation error occurred: {}", e);
            } else {
                println!("Unknown panic: {e:?}");
            }
        }
    };
    Ok(())
}
