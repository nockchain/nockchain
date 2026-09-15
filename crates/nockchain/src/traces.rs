//! Translates kernel `%span` effects into stdout-visible structured events.
//! Downstream observers parse these log lines to follow per-node chain state
//! and correlate accepted AI-PoW candidate commitments with submitted work.

use std::collections::HashMap;

use nockapp::driver::{make_driver, IODriverFn};
use nockchain_math::structs::HoonList;
use nockvm::noun::{Noun, NounAllocator};
use nockvm_macros::tas;
use tracing::{debug, error, field, info, span, Level};

const NEW_HEAVIEST_CHAIN: &str = "new_heaviest_chain";
const NEW_HEAVIEST_MINER: &str = "new_heaviest_miner";
const AI_POW_ACCEPTED: &str = "ai_pow_accepted";

pub fn traces_driver() -> IODriverFn {
    make_driver(|handle| async move {
        loop {
            match handle.next_effect().await {
                Ok(effect) => {
                    let space = effect.noun_space();
                    let effect_noun = unsafe { *effect.root() };
                    let Ok(effect_cell) = effect_noun.in_space(&space).as_cell() else {
                        continue;
                    };

                    if effect_cell.head().as_atom().and_then(|atom| atom.as_u64())
                        == Ok(tas!(b"log"))
                    {
                        let log_msg = effect_cell.tail().as_atom()?.into_string()?;
                        info!(log_msg);
                    } else if effect_cell.head().as_atom().and_then(|atom| atom.as_u64())
                        == Ok(tas!(b"span"))
                    {
                        let span_eff = effect_cell.tail();
                        let name = span_eff.slot(2)?.as_atom()?.into_string()?;

                        let raw_fields: Vec<Noun> =
                            HoonList::try_from(span_eff.slot(3)?.noun(), &space)?
                                .into_iter()
                                .collect();

                        let mut str_fields: HashMap<String, String> = HashMap::new();
                        let mut num_fields: HashMap<String, u64> = HashMap::new();
                        let mut parse_ok = true;
                        for n in raw_fields {
                            let cell = n.in_space(&space).as_cell()?;
                            let key = cell.head().as_atom()?.into_string()?;
                            let raw_val = cell.tail().as_cell()?;
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

                        let height = num_fields
                            .get("block_height")
                            .or_else(|| num_fields.get("new_height"))
                            .copied()
                            .unwrap_or(0);
                        let digest = str_fields
                            .get("heaviest_block_digest")
                            .cloned()
                            .unwrap_or_default();
                        let target = str_fields.get("block_target").cloned().unwrap_or_default();
                        let block_id = str_fields.get("block_id").cloned().unwrap_or_default();
                        let candidate_commitment = str_fields
                            .get("candidate_commitment")
                            .cloned()
                            .unwrap_or_default();

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
                            "ai-pow-accepted" => {
                                let span = span!(
                                    Level::INFO,
                                    AI_POW_ACCEPTED,
                                    block_height = field::Empty,
                                    block_id = field::Empty,
                                    candidate_commitment = field::Empty
                                );
                                span.record("block_height", height);
                                span.record("block_id", block_id.as_str());
                                span.record("candidate_commitment", candidate_commitment.as_str());
                                let _g = span.enter();
                                info!(
                                    block_height = height,
                                    block_id = block_id.as_str(),
                                    candidate_commitment = candidate_commitment.as_str(),
                                    "ai_pow_accepted"
                                );
                            }
                            "orphaned-block" => {
                                info!(
                                    block_height = height,
                                    block_id = block_id.as_str(),
                                    event_type = str_fields
                                        .get("event_type")
                                        .map(|s| s.as_str())
                                        .unwrap_or(""),
                                    new_heaviest_block = str_fields
                                        .get("new_heaviest_block")
                                        .map(|s| s.as_str())
                                        .unwrap_or(""),
                                    "orphaned_block"
                                );
                            }
                            "chain-reorg" => {
                                info!(
                                    block_height = height,
                                    block_id = block_id.as_str(),
                                    new_heaviest_height =
                                        num_fields.get("new_heaviest_height").copied().unwrap_or(0),
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
