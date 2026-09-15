use std::error::Error;
use std::path::{Path, PathBuf};

use nockvm::jets::hot::HotEntry;
use zkvm_jetpack::hot::produce_prover_hot_state;

use crate::NockchainCli;

/// Prepares every consensus jet and verifier artifact before node networking starts.
pub fn prepare_consensus_runtime(cli: &mut NockchainCli) -> Result<Vec<HotEntry>, Box<dyn Error>> {
    if let Err(err) = cli.validate() {
        return Err(err.into());
    }

    let data_dir = configure_ai_pow_data_dir(cli);

    if let Some(cap) = cli.ai_pow_verifier_cache_cap {
        std::env::set_var(
            ai_pow_jets::setup::AI_POW_VERIFIER_CACHE_CAP_ENV,
            cap.to_string(),
        );
    }

    let buckets = ai_pow_jets::setup::production_verifier_setup_buckets();
    ai_pow_jets::setup::install_or_build_verifier_setup(&data_dir, &buckets)?;

    Ok(consensus_hot_state())
}

fn configure_ai_pow_data_dir(cli: &mut NockchainCli) -> PathBuf {
    let data_dir = cli
        .nockapp_cli
        .data_dir
        .clone()
        .unwrap_or_else(|| nockapp::default_data_dir("nockchain"));

    if !cli
        .nockapp_cli
        .new_data_dir_allowlist
        .iter()
        .any(|path| path == Path::new("ai-pow"))
    {
        cli.nockapp_cli.new_data_dir_allowlist.push("ai-pow".into());
    }

    data_dir
}

fn consensus_hot_state() -> Vec<HotEntry> {
    let mut hot_state = produce_prover_hot_state();
    hot_state.extend(ai_pow_jets::produce_ai_pow_hot_state());
    hot_state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consensus_hot_state_contains_stark_and_ai_pow_jets() {
        let stark_jet_count = produce_prover_hot_state().len();
        let ai_pow_jet_count = ai_pow_jets::produce_ai_pow_hot_state().len();

        assert_eq!(
            consensus_hot_state().len(),
            stark_jet_count + ai_pow_jet_count
        );
        assert!(ai_pow_jet_count > 0);
    }
}
