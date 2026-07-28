#![allow(clippy::result_large_err)]

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use kernels_roswell::KERNEL;
use nockapp::kernel::boot::{self, Cli as BootCli, NockStackSize};
use nockapp::noun::slab::{Jammer, NockJammer, NounSlab};
use nockapp::utils::NOCK_STACK_SIZE_HUGE;
use nockapp::wire::{SystemWire, Wire};
use nockapp::{NockApp, NockAppError, NounExt};
use nockvm::jets::hot::HotEntry;
use nockvm::mem::NockStack;
use nockvm::noun::{Atom, IndirectAtom, Noun, NounAllocator, NounSpace, D, DIRECT_MAX, T};
use noun_serde::{NounDecode, NounDecodeError, NounEncode};
use zkvm_jetpack::form::{
    Proof, ProofSnapshot, ProofStreamContext, ProofStreamRange, ProofStreamWindow, ProofVersion,
};
use zkvm_jetpack::hot::produce_prover_hot_state;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RoswellCommand {
    Test,
    TestCi,
    TestCrypto,
    TestDumb,
    BenchDumb,
    BenchHZoonNoop,
    BenchHZoonZMapBuild,
    BenchHZoonHMapBuild,
    BenchHZoonZMapRead,
    BenchHZoonHMapRead,
    BenchHZoonZMapUpdate,
    BenchHZoonHMapUpdate,
    TestWallet,
    TestWalletShard,
    TestZoon,
    TestBridge,
    TestVerifier,
    PrepareVerifierBench,
    BenchVerifier,
    VerifyProof,
    TestPuzzle,
    ProvePuzzle,
    MakeProofSnapshot,
    MakeProofStreamWindow,
    AssembleProofStream,
    AssembleProofContinuation,
    Compute,
    DecBenchmark,
}

impl RoswellCommand {
    pub const fn as_str(self) -> &'static str {
        match self {
            RoswellCommand::Test => "test",
            RoswellCommand::TestCi => "test-ci",
            RoswellCommand::TestCrypto => "test-crypto",
            RoswellCommand::TestDumb => "test-dumb",
            RoswellCommand::BenchDumb => "bench-dumb",
            RoswellCommand::BenchHZoonNoop => "bench-h-zoon-noop",
            RoswellCommand::BenchHZoonZMapBuild => "bench-h-zoon-z-map-build",
            RoswellCommand::BenchHZoonHMapBuild => "bench-h-zoon-h-map-build",
            RoswellCommand::BenchHZoonZMapRead => "bench-h-zoon-z-map-read",
            RoswellCommand::BenchHZoonHMapRead => "bench-h-zoon-h-map-read",
            RoswellCommand::BenchHZoonZMapUpdate => "bench-h-zoon-z-map-update",
            RoswellCommand::BenchHZoonHMapUpdate => "bench-h-zoon-h-map-update",
            RoswellCommand::TestWallet => "test-wallet",
            RoswellCommand::TestWalletShard => "test-wallet-shard",
            RoswellCommand::TestZoon => "test-zoon",
            RoswellCommand::TestBridge => "test-bridge",
            RoswellCommand::TestVerifier => "test-verifier",
            RoswellCommand::PrepareVerifierBench => "prepare-verifier-bench",
            RoswellCommand::BenchVerifier => "bench-verifier",
            RoswellCommand::VerifyProof => "verify-proof",
            RoswellCommand::TestPuzzle => "test-puzzle",
            RoswellCommand::ProvePuzzle => "prove-puzzle",
            RoswellCommand::MakeProofSnapshot => "make-proof-snapshot",
            RoswellCommand::MakeProofStreamWindow => "make-proof-stream-window",
            RoswellCommand::AssembleProofStream => "assemble-proof-stream",
            RoswellCommand::AssembleProofContinuation => "assemble-proof-continuation",
            RoswellCommand::Compute => "compute",
            RoswellCommand::DecBenchmark => "dec-benchmark",
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ExitEffect {
    pub code: u64,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum FileEffect {
    Write { path: String, contents: Vec<u8> },
    Read { path: String },
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum Effect {
    Exit(ExitEffect),
    File(FileEffect),
}

impl NounDecode for Effect {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        let effect_cell = noun
            .in_space(space)
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;
        let tag = effect_cell.head().as_atom()?.into_string()?;

        match tag.as_str() {
            "exit" => Ok(Effect::Exit(ExitEffect::from_noun(noun, space)?)),
            "file" => Ok(Effect::File(FileEffect::from_noun(noun, space)?)),
            _ => Err(NounDecodeError::InvalidEnumVariant),
        }
    }
}

impl NounDecode for ExitEffect {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        let effect_cell = noun
            .in_space(space)
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;

        if effect_cell.head().as_atom()?.into_string()? != "exit" {
            return Err(NounDecodeError::InvalidTag);
        }
        Ok(ExitEffect {
            code: effect_cell.tail().as_atom()?.as_u64()?,
        })
    }
}

impl NounDecode for FileEffect {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        let effect_cell = noun
            .in_space(space)
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;

        if effect_cell.head().as_atom()?.into_string()? != "file" {
            return Err(NounDecodeError::InvalidTag);
        }

        let file_cell = effect_cell.tail().as_cell()?;
        let tag = file_cell.head().as_atom()?.into_string()?;

        match tag.as_str() {
            "read" => Ok(FileEffect::Read {
                path: file_cell.tail().as_atom()?.into_string()?,
            }),
            "write" => {
                let write_cell = file_cell.tail().as_cell()?;
                let path = write_cell.head().as_atom()?.into_string()?;
                let contents = write_cell.tail().as_atom()?.to_ne_bytes();
                Ok(FileEffect::Write { path, contents })
            }
            _ => Err(NounDecodeError::InvalidEnumVariant),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CommandOutput {
    pub effects: Vec<Effect>,
}

impl CommandOutput {
    pub fn from_effect_slabs<J: Jammer>(
        mut effects: Vec<NounSlab<J>>,
    ) -> Result<Self, NockAppError> {
        let mut decoded = Vec::with_capacity(effects.len());
        for slab in effects.iter_mut() {
            let space = slab.noun_space();
            let effect = unsafe { *slab.root() };
            decoded.push(Effect::from_noun(&effect, &space).map_err(|_| NockAppError::PokeFailed)?);
        }
        Ok(Self { effects: decoded })
    }

    pub fn success(&self) -> bool {
        self.effects.iter().all(|effect| match effect {
            Effect::Exit(exit) => exit.code == 0,
            Effect::File(FileEffect::Write { .. }) => true,
            Effect::File(FileEffect::Read { .. }) => false,
        })
    }

    pub fn ensure_success(&self, context: &str) -> Result<(), NockAppError> {
        if self.success() {
            Ok(())
        } else {
            Err(NockAppError::OtherError(format!(
                "Roswell command failed: {context}"
            )))
        }
    }

    pub fn write_files(&self) -> Result<(), NockAppError> {
        for effect in &self.effects {
            match effect {
                Effect::File(FileEffect::Write { path, contents }) => {
                    std::fs::write(path, contents).map_err(|err| {
                        NockAppError::OtherError(format!("failed to write {path}: {err}"))
                    })?;
                }
                Effect::File(FileEffect::Read { path }) => {
                    return Err(NockAppError::OtherError(format!(
                        "unexpected file read request: {path}"
                    )));
                }
                Effect::Exit(_) => {}
            }
        }
        Ok(())
    }
}

pub fn check_success<J: Jammer>(effects: Vec<NounSlab<J>>) -> Result<bool, NockAppError> {
    let output = CommandOutput::from_effect_slabs(effects)?;
    output.write_files()?;
    Ok(output.success())
}

#[derive(Debug, Clone)]
pub struct PuzzleRequest {
    pub version: ProofVersion,
    pub length: u64,
    pub filename_stem: Option<String>,
}

impl PuzzleRequest {
    pub fn new(version: ProofVersion, length: u64) -> Self {
        Self {
            version,
            length,
            filename_stem: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PuzzleTestRequest {
    pub version: ProofVersion,
    pub length: u64,
    pub override_terms: Vec<String>,
}

impl PuzzleTestRequest {
    pub fn new(version: ProofVersion, length: u64) -> Self {
        Self {
            version,
            length,
            override_terms: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProofStreamWindowRequest {
    pub puzzle: PuzzleRequest,
    pub range: ProofStreamRange,
}

#[derive(Debug, Clone)]
pub struct ProofResult {
    pub proof: Proof,
    pub output: CommandOutput,
}

#[derive(Clone)]
pub struct ProofSnapshotResult {
    pub snapshot: ProofSnapshot,
    pub output: CommandOutput,
}

#[derive(Debug, Clone)]
pub struct ProofStreamWindowResult {
    pub window: ProofStreamWindow,
    pub output: CommandOutput,
}

pub struct Roswell {
    pub app: NockApp,
}

/// The single shared jet registry roswell + nockchain must both use: the STARK
/// prover jets plus the AI-PoW consensus verify jet (`~/ %ai-pow-verify`). Both
/// binaries call this so the jet matches identically in tests and in production.
pub fn produce_full_hot_state() -> Vec<HotEntry> {
    let mut hs = produce_prover_hot_state();
    hs.extend(ai_pow_jets::produce_ai_pow_hot_state());
    hs
}

impl Roswell {
    pub fn new(nockapp: NockApp) -> Self {
        Self { app: nockapp }
    }

    pub async fn boot(boot_cli: BootCli) -> Result<Self, NockAppError> {
        Self::boot_with_hot_state(boot_cli, &produce_full_hot_state()).await
    }

    pub async fn boot_with_hot_state(
        mut boot_cli: BootCli,
        hot_state: &[HotEntry],
    ) -> Result<Self, NockAppError> {
        if boot_cli.data_dir.is_none() {
            if let Some(data_dir) = temp_data_dir("roswell") {
                boot_cli.data_dir = Some(data_dir);
            }
        }
        boot_cli.stack_size = NockStackSize::Huge;
        // Resolve the effective data dir the same way `boot::setup` does, BEFORE it
        // moves `boot_cli`, so we can load the AI-PoW verifier-setup cache from it.
        let data_dir = boot_cli
            .data_dir
            .clone()
            .unwrap_or_else(|| nockapp::default_data_dir("roswell"));
        let kernel = boot::setup(KERNEL, boot_cli, hot_state, "roswell", None)
            .await
            .map_err(|err| NockAppError::OtherError(format!("failed to boot Roswell: {err}")))?;
        // Load the AI-PoW verifier-setup table from the data dir IF a cache is
        // present (fast: load seeds + rebuild; no proving). roswell is a
        // conformance/CLI tool, not the consensus node, so it does NOT prove a table
        // at boot (that would stall every harness boot) — it uses the lenient loader
        // and only errors on a corrupt cache. The consensus node (nockchain)
        // generates-or-shuts-down; both share the identical verify jet + rebuild.
        let n = ai_pow_jets::setup::install_verifier_setup_from_cache(&data_dir)
            .map_err(|e| NockAppError::OtherError(format!("AI-PoW verifier-setup: {e}")))?;
        if n > 0 {
            tracing::info!("AI-PoW verifier-setup table loaded: {n} bucket(s)");
        }
        Ok(Self::new(kernel))
    }

    pub async fn save(&mut self) -> Result<(), NockAppError> {
        Ok(())
    }

    pub fn roswell_command(
        &mut self,
        command: &str,
        args: &[Noun],
        slab: &mut NounSlab,
    ) -> Result<NounSlab, NockAppError> {
        self.roswell_command_with_space(command, args, None, slab)
    }

    pub fn roswell_command_with_space(
        &mut self,
        command: &str,
        args: &[Noun],
        arg_space: Option<&NounSpace>,
        slab: &mut NounSlab,
    ) -> Result<NounSlab, NockAppError> {
        let imported_args;
        let args = if let Some(space) = arg_space {
            imported_args = args
                .iter()
                .map(|arg| slab.copy_into(*arg, space))
                .collect::<Vec<_>>();
            imported_args.as_slice()
        } else {
            args
        };

        let head = make_tas(slab, command).as_noun();
        let tail = match args.len() {
            0 => D(0),
            1 => args[0],
            _ => T(slab, args),
        };
        let full = T(slab, &[head, tail]);
        slab.set_root(full);
        Ok(slab.clone())
    }

    pub async fn poke_command(
        &mut self,
        command: RoswellCommand,
        args: &[Noun],
        arg_space: Option<&NounSpace>,
    ) -> Result<CommandOutput, NockAppError> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let command_slab =
            self.roswell_command_with_space(command.as_str(), args, arg_space, &mut slab)?;
        let effects = self.app.poke(SystemWire.to_wire(), command_slab).await?;
        CommandOutput::from_effect_slabs(effects)
    }

    async fn poke_command_slab(
        &mut self,
        command: RoswellCommand,
        args: &[Noun],
        slab: &mut NounSlab<NockJammer>,
    ) -> Result<CommandOutput, NockAppError> {
        let command_slab = self.roswell_command(command.as_str(), args, slab)?;
        let effects = self.app.poke(SystemWire.to_wire(), command_slab).await?;
        CommandOutput::from_effect_slabs(effects)
    }

    pub async fn test_puzzle(
        &mut self,
        request: &PuzzleTestRequest,
    ) -> Result<CommandOutput, NockAppError> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let length = validate_puzzle_length(request.length)?;
        let overrides = override_terms_to_noun(&mut slab, &request.override_terms);
        let output = self
            .poke_command_slab(
                RoswellCommand::TestPuzzle,
                &[D(proof_version_atom(request.version)), length, overrides],
                &mut slab,
            )
            .await?;
        output.ensure_success(RoswellCommand::TestPuzzle.as_str())?;
        Ok(output)
    }

    pub async fn prove_puzzle(
        &mut self,
        request: &PuzzleRequest,
    ) -> Result<ProofResult, NockAppError> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let length = validate_puzzle_length(request.length)?;
        let filename = filename_to_noun(&mut slab, request.filename_stem.as_deref());
        let output = self
            .poke_command_slab(
                RoswellCommand::ProvePuzzle,
                &[D(proof_version_atom(request.version)), length, filename, D(0)],
                &mut slab,
            )
            .await?;
        output.write_files()?;
        output.ensure_success(RoswellCommand::ProvePuzzle.as_str())?;
        let proof = self.peek_proof().await?;
        Ok(ProofResult { proof, output })
    }

    pub async fn make_proof_snapshot(
        &mut self,
        request: &PuzzleRequest,
    ) -> Result<ProofSnapshotResult, NockAppError> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let length = validate_puzzle_length(request.length)?;
        let filename = filename_to_noun(&mut slab, request.filename_stem.as_deref());
        let output = self
            .poke_command_slab(
                RoswellCommand::MakeProofSnapshot,
                &[D(proof_version_atom(request.version)), length, filename, D(0)],
                &mut slab,
            )
            .await?;
        output.write_files()?;
        output.ensure_success(RoswellCommand::MakeProofSnapshot.as_str())?;
        let snapshot = self.peek_decode("snapshot").await?;
        Ok(ProofSnapshotResult { snapshot, output })
    }

    pub async fn make_proof_stream_window(
        &mut self,
        request: &ProofStreamWindowRequest,
    ) -> Result<ProofStreamWindowResult, NockAppError> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let length = validate_puzzle_length(request.puzzle.length)?;
        let range = request.range.to_noun(&mut slab);
        let filename = filename_to_noun(&mut slab, request.puzzle.filename_stem.as_deref());
        let output = self
            .poke_command_slab(
                RoswellCommand::MakeProofStreamWindow,
                &[D(proof_version_atom(request.puzzle.version)), length, range, filename, D(0)],
                &mut slab,
            )
            .await?;
        output.write_files()?;
        output.ensure_success(RoswellCommand::MakeProofStreamWindow.as_str())?;
        let window = self.peek_decode("proof-stream-window").await?;
        Ok(ProofStreamWindowResult { window, output })
    }

    pub async fn assemble_proof_stream(
        &mut self,
        windows: &[ProofStreamWindow],
        filename_stem: Option<&str>,
    ) -> Result<ProofResult, NockAppError> {
        let mut stack = NockStack::new(NOCK_STACK_SIZE_HUGE, 0);
        let window_nouns = windows
            .iter()
            .map(|window| window.to_noun(&mut stack))
            .collect::<Vec<_>>();
        let windows_list = list_to_noun(&mut stack, window_nouns);
        let filename = filename_to_noun(&mut stack, filename_stem);
        let space = stack.noun_space();
        let output = self
            .poke_command(
                RoswellCommand::AssembleProofStream,
                &[windows_list, filename],
                Some(&space),
            )
            .await?;
        output.write_files()?;
        output.ensure_success(RoswellCommand::AssembleProofStream.as_str())?;
        let proof = self.peek_proof().await?;
        Ok(ProofResult { proof, output })
    }

    pub async fn assemble_proof_continuation(
        &mut self,
        snapshot: &ProofSnapshot,
        context: &ProofStreamContext,
        windows: &[ProofStreamWindow],
        filename_stem: Option<&str>,
    ) -> Result<ProofResult, NockAppError> {
        let mut stack = NockStack::new(NOCK_STACK_SIZE_HUGE, 0);
        let snapshot_noun = snapshot.to_noun(&mut stack);
        let context_noun = context.to_noun(&mut stack);
        let window_nouns = windows
            .iter()
            .map(|window| window.to_noun(&mut stack))
            .collect::<Vec<_>>();
        let windows_list = list_to_noun(&mut stack, window_nouns);
        let filename = filename_to_noun(&mut stack, filename_stem);
        let space = stack.noun_space();
        let output = self
            .poke_command(
                RoswellCommand::AssembleProofContinuation,
                &[snapshot_noun, context_noun, windows_list, filename],
                Some(&space),
            )
            .await?;
        output.write_files()?;
        output.ensure_success(RoswellCommand::AssembleProofContinuation.as_str())?;
        let proof = self.peek_proof().await?;
        Ok(ProofResult { proof, output })
    }

    pub async fn check_proof(&mut self, proof: &Proof) -> Result<bool, NockAppError> {
        let mut stack = NockStack::new(NOCK_STACK_SIZE_HUGE, 0);
        let proof_noun = proof.to_noun(&mut stack);
        let inner_some = T(&mut stack, &[D(0), proof_noun]);
        let outer_some = T(&mut stack, &[D(0), inner_some]);
        let space = stack.noun_space();
        let output = self
            .poke_command(RoswellCommand::VerifyProof, &[outer_some], Some(&space))
            .await?;
        Ok(output.success())
    }

    pub async fn compute(
        &mut self,
        nock: Noun,
        space: &NounSpace,
    ) -> Result<CommandOutput, NockAppError> {
        let output = self
            .poke_command(RoswellCommand::Compute, &[nock], Some(space))
            .await?;
        output.ensure_success(RoswellCommand::Compute.as_str())?;
        Ok(output)
    }

    async fn peek_noun(&mut self, path: &str, stack: &mut NockStack) -> Result<Noun, NockAppError> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let peek_cmd = self.roswell_command(path, &[], &mut slab)?;
        let slab = self.app.peek(peek_cmd).await?;
        Ok(slab.copy_to_stack(stack))
    }

    pub async fn peek_decode<T: NounDecode>(&mut self, path: &str) -> Result<T, NockAppError> {
        let mut stack = NockStack::new(NOCK_STACK_SIZE_HUGE, 0);
        let noun = self.peek_noun(path, &mut stack).await?;
        let space = stack.noun_space();
        let value = unwrap_peeked_value(noun, &space)?;
        T::from_noun(&value, &space).map_err(|err| {
            NockAppError::OtherError(format!("failed to decode Roswell peek {path}: {err:?}"))
        })
    }

    pub async fn peek_proof(&mut self) -> Result<Proof, NockAppError> {
        let mut stack = NockStack::new(NOCK_STACK_SIZE_HUGE, 0);
        let noun = self.peek_noun("proof", &mut stack).await?;
        let space = stack.noun_space();
        decode_peeked_proof(noun, &space)
    }
}

pub fn proof_version_atom(version: ProofVersion) -> u64 {
    match version {
        ProofVersion::V0 => 0,
        ProofVersion::V1 => 1,
        ProofVersion::V2 => 2,
        ProofVersion::V3 => 3,
    }
}

pub fn validate_puzzle_length(n: u64) -> Result<Noun, NockAppError> {
    if n > DIRECT_MAX {
        Err(NockAppError::OtherError(String::from(
            "number exceeds direct atom maximum",
        )))
    } else if n != 0 && (n & (n - 1)) == 0 {
        Ok(D(n))
    } else {
        Err(NockAppError::OtherError(String::from(
            "puzzle length must be a power of two",
        )))
    }
}

pub fn make_tas<A: NounAllocator>(allocator: &mut A, tas: &str) -> Atom {
    let tas_bytes: &[u8] = tas.as_bytes();
    unsafe {
        IndirectAtom::new_raw_bytes(allocator, tas_bytes.len(), tas_bytes.as_ptr())
            .normalize_as_atom_stack()
    }
}

pub fn filename_to_noun<A: NounAllocator>(allocator: &mut A, filename: Option<&str>) -> Noun {
    match filename {
        Some(filename) => {
            let file_tas = make_tas(allocator, filename).as_noun();
            T(allocator, &[D(0), file_tas])
        }
        None => D(0),
    }
}

pub fn override_terms_to_noun<A: NounAllocator>(allocator: &mut A, terms: &[String]) -> Noun {
    if terms.is_empty() {
        D(0)
    } else {
        let terms = terms
            .iter()
            .map(|term| make_tas(allocator, term).as_noun())
            .collect::<Vec<_>>();
        list_to_noun(allocator, terms)
    }
}

pub fn list_to_noun<A: NounAllocator>(allocator: &mut A, terms: Vec<Noun>) -> Noun {
    terms
        .into_iter()
        .rev()
        .fold(D(0), |acc, term| T(allocator, &[term, acc]))
}

pub fn cue_file_to_stack(path: &Path, stack: &mut NockStack) -> Result<Noun, NockAppError> {
    let bytes = std::fs::read(path).map_err(|err| {
        NockAppError::OtherError(format!("failed to read {}: {err}", path.display()))
    })?;
    Noun::cue_bytes_slice(stack, &bytes).map_err(|err| {
        NockAppError::OtherError(format!("failed to cue {}: {err:?}", path.display()))
    })
}

enum UnitPeel {
    NotUnit,
    None,
    Some(Noun),
}

fn peel_unit(noun: Noun, space: &NounSpace) -> Result<UnitPeel, NockAppError> {
    if noun.is_atom() {
        let atom = noun
            .in_space(space)
            .as_atom()
            .map_err(|_| NockAppError::OtherError("unit decode failed".to_string()))?;
        let value = atom
            .as_u64()
            .map_err(|_| NockAppError::OtherError("unit decode failed".to_string()))?;
        return if value == 0 {
            Ok(UnitPeel::None)
        } else {
            Ok(UnitPeel::NotUnit)
        };
    }

    let cell = noun
        .in_space(space)
        .as_cell()
        .map_err(|_| NockAppError::OtherError("unit decode failed".to_string()))?;
    let head = cell.head();
    let head_atom = match head.as_atom() {
        Ok(atom) => atom,
        Err(_) => return Ok(UnitPeel::NotUnit),
    };
    let head_value = match head_atom.as_u64() {
        Ok(value) => value,
        Err(_) => return Ok(UnitPeel::NotUnit),
    };
    if head_value != 0 {
        return Ok(UnitPeel::NotUnit);
    }
    Ok(UnitPeel::Some(cell.tail().noun()))
}

fn unwrap_peeked_value(noun: Noun, space: &NounSpace) -> Result<Noun, NockAppError> {
    let inner = match peel_unit(noun, space)? {
        UnitPeel::Some(next) => next,
        UnitPeel::None => {
            return Err(NockAppError::OtherError(
                "Roswell peek returned empty outer unit".to_string(),
            ));
        }
        UnitPeel::NotUnit => noun,
    };
    match peel_unit(inner, space)? {
        UnitPeel::Some(next) => Ok(next),
        UnitPeel::None => Err(NockAppError::OtherError(
            "Roswell peek returned empty inner unit".to_string(),
        )),
        UnitPeel::NotUnit => Ok(inner),
    }
}

fn looks_like_proof(noun: &Noun, space: &NounSpace) -> bool {
    let cell = match noun.in_space(space).as_cell() {
        Ok(cell) => cell,
        Err(_) => return false,
    };
    let head_atom = match cell.head().as_atom() {
        Ok(atom) => atom,
        Err(_) => return false,
    };
    let head_value = match head_atom.as_u64() {
        Ok(value) => value,
        Err(_) => return false,
    };
    if head_value > 2 {
        return false;
    }
    let tail_cell = match cell.tail().as_cell() {
        Ok(cell) => cell,
        Err(_) => return false,
    };
    if let Ok(atom) = tail_cell.head().as_atom() {
        if let Ok(value) = atom.as_u64() {
            if value <= 2 {
                return false;
            }
        }
    }
    true
}

fn decode_peeked_proof(noun: Noun, space: &NounSpace) -> Result<Proof, NockAppError> {
    let inner = match peel_unit(noun, space)? {
        UnitPeel::Some(next) => next,
        UnitPeel::None => {
            return Err(NockAppError::OtherError(
                "Hoon proof peek returned ~".to_string(),
            ))
        }
        UnitPeel::NotUnit => noun,
    };

    let candidate = if looks_like_proof(&inner, space) {
        inner
    } else {
        match peel_unit(inner, space)? {
            UnitPeel::Some(next) => next,
            UnitPeel::None => {
                return Err(NockAppError::OtherError("Hoon proof is empty".to_string()))
            }
            UnitPeel::NotUnit => inner,
        }
    };

    if !looks_like_proof(&candidate, space) {
        return Err(NockAppError::OtherError(
            "Hoon proof peek has unexpected shape".to_string(),
        ));
    }

    Proof::from_noun(&candidate, space)
        .map_err(|err| NockAppError::OtherError(format!("failed to decode Hoon proof: {err:?}")))
}

fn temp_data_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os("TEST_TMPDIR").map(|tmpdir| {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        PathBuf::from(tmpdir).join(format!("{name}-{}-{timestamp}", std::process::id()))
    })
}
