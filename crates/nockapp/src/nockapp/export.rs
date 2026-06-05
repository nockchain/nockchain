use bincode::{config, decode_from_slice, encode_to_vec, Decode, Encode};
use blake3::Hash;
use nockvm_macros::tas;

use crate::kernel::form::LoadState;
use crate::noun::slab::NounSlab;
use crate::{JammedNoun, NockAppError};

const EXPORTED_STATE_MAGIC_BYTES: u64 = tas!(b"EXPJAM");
const EXPORTED_STATE_VERSION: u32 = 0;

/// A structure for exporting just the kernel state, without the cold state
#[derive(Encode, Decode, PartialEq, Debug)]
pub struct ExportedState {
    /// Magic bytes to identify exported state format
    pub magic_bytes: u64,
    /// Version of exported state
    pub version: u32,
    /// Hash of the boot kernel
    #[bincode(with_serde)]
    pub ker_hash: Hash,
    /// Event number
    pub event_num: u64,
    /// Jammed noun of kernel_state
    pub jam: JammedNoun,
}

impl ExportedState {
    pub fn encode(&self) -> Result<Vec<u8>, bincode::error::EncodeError> {
        encode_to_vec(self, config::standard())
    }

    pub fn decode(data: &[u8]) -> Result<Self, bincode::error::DecodeError> {
        let (state, _) = decode_from_slice(data, config::standard())?;
        Ok(state)
    }

    pub fn from_loadstate(state: LoadState) -> Self {
        let jam = JammedNoun::new(state.kernel_state.jam());

        Self {
            magic_bytes: EXPORTED_STATE_MAGIC_BYTES,
            version: EXPORTED_STATE_VERSION,
            ker_hash: state.ker_hash,
            event_num: state.event_num,
            jam,
        }
    }

    pub fn to_loadstate(self) -> Result<LoadState, NockAppError> {
        let mut kernel_state = NounSlab::new();
        let kernel_state_noun = kernel_state.cue_into(self.jam.0)?;
        kernel_state.set_root(kernel_state_noun);

        Ok(LoadState {
            kernel_state,
            ker_hash: self.ker_hash,
            event_num: self.event_num,
        })
    }
}
