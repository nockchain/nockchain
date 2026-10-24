use std::path::PathBuf;

use kernels_roswell::KERNEL;
use nockapp::kernel::boot::{self, ephemeral_test_boot_cli, NockStackSize};
use nockapp::noun::slab::{Jammer, NounSlab};
use nockapp::wire::{SystemWire, Wire};
use nockapp::{NockApp, NockAppError};
use nockvm::noun::{Atom, IndirectAtom, Noun, NounAllocator, NounSpace, D, T};
use noun_serde::{NounDecode, NounDecodeError};
use tempfile::TempDir;
use zkvm_jetpack::hot::produce_prover_hot_state;

pub async fn run_roswell_test(test_name: &str) -> Result<(), NockAppError> {
    let temp_dir =
        TempDir::new().map_err(|err| NockAppError::OtherError(format!("tempdir: {err}")))?;

    let mut cli = ephemeral_test_boot_cli(true);
    cli.stack_size = NockStackSize::Huge;

    let mut app: NockApp = boot::setup(
        KERNEL,
        cli,
        &produce_prover_hot_state(),
        "roswell",
        Some(PathBuf::from(temp_dir.path())),
    )
    .await
    .map_err(|err| NockAppError::OtherError(format!("boot: {err}")))?;

    let mut slab = NounSlab::new();
    let name = make_tas(&mut slab, test_name).as_noun();
    let cmd = roswell_command(&mut slab, "test", &[name]);
    let effects = app.poke(SystemWire.to_wire(), cmd).await?;
    let success = check_success(effects).map_err(|_| NockAppError::PokeFailed)?;

    if !success {
        return Err(NockAppError::OtherError(String::from(
            "Roswell test failed",
        )));
    }

    Ok(())
}

fn roswell_command(slab: &mut NounSlab, command: &str, args: &[Noun]) -> NounSlab {
    let head = make_tas(slab, command).as_noun();

    let tail = match args.len() {
        0 => D(0),
        1 => args[0],
        _ => T(slab, args),
    };

    let full = T(slab, &[head, tail]);
    slab.set_root(full);
    slab.clone()
}

pub fn make_tas(slab: &mut NounSlab, tas: &str) -> Atom {
    let tas_bytes: &[u8] = tas.as_bytes();
    unsafe {
        IndirectAtom::new_raw_bytes(slab, tas_bytes.len(), tas_bytes.as_ptr())
            .normalize_as_atom_stack()
    }
}

pub struct ExitEffect {
    pub code: u64,
}

pub enum FileEffect {
    Write(String, Vec<u8>),
    Read(String),
}

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
            "read" => {
                let path = file_cell.tail().as_atom()?.into_string()?;
                Ok(FileEffect::Read(path))
            }
            "write" => {
                let write_cell = file_cell.tail().as_cell()?;
                let path = write_cell.head().as_atom()?.into_string()?;
                let contents = write_cell.tail().as_atom()?.to_ne_bytes();
                Ok(FileEffect::Write(path, contents))
            }
            _ => Err(NounDecodeError::InvalidEnumVariant),
        }
    }
}

pub fn check_success<J: Jammer>(effects: Vec<NounSlab<J>>) -> Result<bool, NounDecodeError> {
    effects.into_iter().try_fold(true, |success, slab| {
        let space = slab.noun_space();
        let effect = unsafe { *slab.root() };
        let effect = Effect::from_noun(&effect, &space)?;
        match effect {
            Effect::Exit(exit_effect) => Ok(success && exit_effect.code == 0),
            Effect::File(file_effect) => match file_effect {
                FileEffect::Read(path) => {
                    let _ = path;
                    Ok(success)
                }
                FileEffect::Write(path, contents) => {
                    let _ = (path, contents);
                    Ok(success)
                }
            },
        }
    })
}
