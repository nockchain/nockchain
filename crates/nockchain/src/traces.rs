//! Translates kernel `%span %new-heaviest-chain` and `%new-heaviest-miner`
//! effects (emitted by inner.hoon's `accept-block` and miner candidate paths)
//! into stdout-visible structured events. Downstream observers (cluster
//! tests, monitoring tools) parse these `new_heaviest_chain` log lines
//! (matching `block_height=`, `heaviest_block_digest=`, `block_target=`) to
//! follow per-node chain state from stdout.

use std::collections::HashMap;

use nockapp::driver::{make_driver, IODriverFn};
use nockapp::AtomExt;
use nockvm::noun::{Noun, Slots, D};
use nockvm_macros::tas;
use tracing::{debug, error, field, info, span, Level};

const NEW_HEAVIEST_CHAIN: &str = "new_heaviest_chain";
const NEW_HEAVIEST_MINER: &str = "new_heaviest_miner";

/// Walk a Hoon list (right-nested cells terminated by `~`) into a Vec<Noun>.
fn hoon_list_to_vec(list: Noun) -> Vec<Noun> {
    let mut out = Vec::new();
    let mut cur = list;
    while let Ok(cell) = cur.as_cell() {
        out.push(cell.head());
        cur = cell.tail();
    }
    out
}

pub fn traces_driver() -> IODriverFn {
    make_driver(|handle| async move {
        loop {
            match handle.next_effect().await {
                Ok(effect) => {
                    let Ok(effect_cell) = unsafe { effect.root() }.as_cell() else {
                        continue;
                    };

                    if unsafe { effect_cell.head().raw_equals(&D(tas!(b"log"))) } {
                        let log_msg = effect_cell.tail().as_atom()?.into_string()?;
                        info!(log_msg);
                    } else if unsafe { effect_cell.head().raw_equals(&D(tas!(b"span"))) } {
                        let span_eff = effect_cell.tail();
                        let name = span_eff.slot(2)?.as_atom()?.into_string()?;

                        let raw_fields = hoon_list_to_vec(span_eff.slot(3)?);

                        let mut str_fields: HashMap<String, String> = HashMap::new();
                        let mut num_fields: HashMap<String, u64> = HashMap::new();
                        let mut parse_ok = true;
                        for n in raw_fields {
                            let key = n.as_cell()?.head().as_atom()?.into_string()?;
                            let raw_val = n.as_cell()?.tail().as_cell()?;
                            let typ = raw_val.head().as_atom()?.into_string()?;
                            let val_atom = raw_val.tail().as_atom()?;
                            if typ == "n" {
                                num_fields.insert(key, val_atom.as_u64()?);
                            } else if typ == "s" {
                                str_fields.insert(key, val_atom.into_string()?);
                            } else {
                                error!("Error traces driver: unrecognized field type");
                                parse_ok = false;
                                break;
                            }
                        }
                        if !parse_ok {
                            continue;
                        }

                        let height = num_fields.get("block_height").copied().unwrap_or(0);
                        let digest = str_fields
                            .get("heaviest_block_digest")
                            .cloned()
                            .unwrap_or_default();
                        let target = str_fields.get("block_target").cloned().unwrap_or_default();

                        match name.as_str() {
                            "new-heaviest-chain" => {
                                let span = span!(
                                    Level::INFO,
                                    NEW_HEAVIEST_CHAIN,
                                    block_height = field::Empty,
                                    heaviest_block_digest = field::Empty,
                                    block_target = field::Empty
                                );
                                span.record("block_height", height);
                                span.record("heaviest_block_digest", digest.as_str());
                                span.record("block_target", target.as_str());
                                let _g = span.enter();
                                info!(
                                    block_height = height,
                                    heaviest_block_digest = digest.as_str(),
                                    block_target = target.as_str(),
                                    "new_heaviest_chain"
                                );
                            }
                            "new-heaviest-miner" => {
                                let span = span!(
                                    Level::INFO,
                                    NEW_HEAVIEST_MINER,
                                    block_height = field::Empty,
                                    heaviest_block_digest = field::Empty
                                );
                                span.record("block_height", height);
                                span.record("heaviest_block_digest", digest.as_str());
                                let _g = span.enter();
                                info!(
                                    block_height = height,
                                    heaviest_block_digest = digest.as_str(),
                                    "new_heaviest_miner"
                                );
                            }
                            "orphaned-block" => {
                                debug!(
                                    block_height = height,
                                    block_digest = digest.as_str(),
                                    "orphaned_block"
                                );
                            }
                            "chain-reorg" => {
                                debug!(
                                    block_height = height,
                                    new_tip_digest = digest.as_str(),
                                    "chain_reorg"
                                );
                            }
                            _ => {
                                debug!(span_name = name.as_str(), "traces driver: unknown span");
                            }
                        };
                    }
                }
                Err(e) => {
                    error!("Error in traces driver: {:?}", e);
                    continue;
                }
            }
        }
    })
}
