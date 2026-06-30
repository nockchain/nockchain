use std::error::Error;
use std::fs;

use nockapp::kernel::boot;
use nockapp::noun::slab::NounSlab;
use nockapp::wire::{SystemWire, Wire};
use nockapp::NockApp;
use nockvm::noun::{D, NounAllocator, T};
use nockvm_macros::tas;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let cli = boot::default_boot_cli(false);
    boot::init_default_tracing(&cli);

    let kernel = fs::read("out.jam").map_err(|e| format!("Failed to read out.jam: {}", e))?;

    let mut nockapp: NockApp =
        boot::setup(&kernel, cli, &[], "{{project_name}}", None).await?;

    let mut poke_slab = NounSlab::new();
    let command_noun = T(&mut poke_slab, &[D(tas!(b"cause")), D(0x0)]);
    poke_slab.set_root(command_noun);

    let result = match nockapp.poke(SystemWire.to_wire(), poke_slab).await {
        Ok(effects) => {
            let mut results = Vec::new();
            for (_i, effect) in effects.iter().enumerate() {
                let space = effect.noun_space();
                let effect_noun = unsafe { effect.root().in_space(&space) };
                if let Ok(cell) = effect_noun.as_cell() {
                    let Ok(tail_atom) = cell.tail().as_atom() else {
                        continue;
                    };
                    let Ok(tail_string) = std::str::from_utf8(tail_atom.as_ne_bytes()) else {
                        continue;
                    };
                    results.push(tail_string.trim_end_matches('\0').to_string());
                }
            }
            results.last().unwrap_or(&String::new()).clone()
        }
        Err(_e) => "command failed".to_string(),
    };

    println!("{}", result);
    Ok(())
}
