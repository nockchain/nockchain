use std::error::Error;
use std::path::{Path, PathBuf};

use chaff::Chaff;
use nockapp::export::ExportedState;
use nockapp::kernel::boot::{self, NockStackSize, SetupResult};
use zkvm_jetpack::hot::produce_prover_hot_state;

const CHECKPOINT_ENV: &str = "NOCKCHAIN_H_ZOON_CHECKPOINT";
const MIN_EVENT_ENV: &str = "NOCKCHAIN_H_ZOON_CHECKPOINT_MIN_EVENT";
const TEST_JETS_ENV: &str = "NOCKCHAIN_H_ZOON_TEST_JETS";
const DEFAULT_MIN_EVENT: u64 = 40_000;

const H_ZOON_TEST_JETS: &str = concat!(
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/gor-hip,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/mor-hip,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/zh-molt,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/zh-silt,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/zh-milt,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/zh-balmilt,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/zh-jult,",
    // 22 non-gate h-by / h-in container arm jets (open jetpack).
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/h-by/get,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/h-by/got,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/h-by/gut,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/h-by/has,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/h-by/put,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/h-by/del,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/h-by/mar,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/h-by/gas,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/h-by/uni,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/h-by/int,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/h-by/dif,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/h-by/bif,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/h-by/dig,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/h-in/has,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/h-in/put,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/h-in/del,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/h-in/gas,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/h-in/uni,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/h-in/int,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/h-in/dif,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/h-in/bif,",
    "k.138/one/two/tri/qua/pen/zeke/ext-field/misc-lib/proof-lib/utils/fri/table-lib/",
    "stark-core/fock-core/pow/stark-engine/h-zoon/h-in/dig",
);

struct EnvVarGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires NOCKCHAIN_H_ZOON_CHECKPOINT pointing at a large nockchain chkjam"]
async fn state_8_migration_real_checkpoint_exports_migrated_state() -> Result<(), Box<dyn Error>> {
    // This is a gated checkpoint oracle for the state-8 h-zoon migration.
    // Leaving NOCKCHAIN_H_ZOON_TEST_JETS unset exercises the production jet path.
    // Set it to `default` to run the expensive jet/Hoon fallback list, or to a
    // comma-separated NOCK_TEST_JETS value for focused local differential checks.
    let checkpoint = checkpoint_path()?;
    let checkpoint = checkpoint.canonicalize().map_err(|err| {
        format!(
            "failed to canonicalize {} from {CHECKPOINT_ENV}: {err}",
            checkpoint.display()
        )
    })?;
    if !checkpoint.is_file() {
        return Err(format!(
            "{CHECKPOINT_ENV} must point at a chkjam file: {}",
            checkpoint.display()
        )
        .into());
    }

    let temp = tempfile::TempDir::new()?;
    let app_root = temp.path().join("nockchain");
    let checkpoint_dir = app_root.join("checkpoints");
    std::fs::create_dir_all(&checkpoint_dir)?;
    link_checkpoint(&checkpoint, &checkpoint_dir.join("0.chkjam"))?;

    let export_path = temp.path().join("state-8-export.jam");
    let mut cli = boot::default_boot_cli(false);
    cli.gc_interval = None;
    cli.rotating_snapshot_interval_event_time = None;
    cli.disable_fsync = true;
    cli.export_state_jam = Some(export_path.to_string_lossy().into_owned());
    cli.stack_size = NockStackSize::Huge;

    let test_jets = test_jets_for_env();
    let _guard = EnvVarGuard::set("NOCK_TEST_JETS", &test_jets);
    let hot_state = produce_prover_hot_state();

    match boot::setup_::<Chaff>(
        kernels_open_dumb::KERNEL,
        cli,
        &hot_state,
        "nockchain",
        Some(temp.path().to_path_buf()),
    )
    .await?
    {
        SetupResult::ExportedState => {}
        SetupResult::App(_) => return Err("checkpoint migration did not export state".into()),
    }

    let encoded = tokio::fs::read(&export_path).await?;
    let exported = ExportedState::decode(&encoded)?;
    let min_event = min_event()?;
    if exported.event_num < min_event {
        return Err(format!(
            "checkpoint event_num {} is below required floor {}",
            exported.event_num, min_event
        )
        .into());
    }

    let state_hash = blake3::hash(&encoded);
    println!(
        "h-zoon checkpoint migration oracle passed: input={} event_num={} ker_hash={} exported_state_hash={}",
        checkpoint.display(),
        exported.event_num,
        exported.ker_hash,
        state_hash,
    );

    Ok(())
}

fn checkpoint_path() -> Result<PathBuf, Box<dyn Error>> {
    std::env::var_os(CHECKPOINT_ENV)
        .map(PathBuf::from)
        .ok_or_else(|| {
            format!(
                "set {CHECKPOINT_ENV} to a large nockchain checkpoint, for example \
                 /Users/jake/.data.nockchain/checkpoints/0.chkjam"
            )
            .into()
        })
}

fn min_event() -> Result<u64, Box<dyn Error>> {
    match std::env::var(MIN_EVENT_ENV) {
        Ok(value) => value
            .parse()
            .map_err(|err| format!("invalid {MIN_EVENT_ENV}={value}: {err}").into()),
        Err(_) => Ok(DEFAULT_MIN_EVENT),
    }
}

fn test_jets_for_env() -> String {
    match std::env::var(TEST_JETS_ENV) {
        Ok(value) if value == "default" => H_ZOON_TEST_JETS.to_owned(),
        Ok(value) => value,
        Err(_) => String::new(),
    }
}

fn link_checkpoint(source: &Path, destination: &Path) -> Result<(), Box<dyn Error>> {
    if destination.exists() {
        std::fs::remove_file(destination)?;
    }

    if std::fs::hard_link(source, destination).is_ok() {
        return Ok(());
    }

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, destination)?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        std::fs::copy(source, destination)?;
        Ok(())
    }
}
