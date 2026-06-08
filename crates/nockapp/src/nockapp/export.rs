use std::time::Instant;

use bincode::{config, encode_to_vec, Decode, Encode};
use blake3::Hash;
use bytes::Bytes;
use nockvm_macros::tas;
use tracing::info;

use super::artifact::{ArtifactError, CheckedReader};
use crate::kernel::form::LoadState;
use crate::noun::slab::{Jammer, NounSlab};
use crate::{JammedNoun, NockAppError};

const EXPORTED_STATE_MAGIC_BYTES: u64 = tas!(b"EXPJAM");
const EXPORTED_STATE_VERSION: u32 = 0;
const EXPORTED_STATE_ADVICE: &str =
    "Re-export the state jam from a known-good node, or start from a checkpoint instead.";

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

    pub fn decode(data: &[u8]) -> Result<Self, ArtifactError> {
        let mut reader =
            CheckedReader::new_with_advice(data, "exported state jam", EXPORTED_STATE_ADVICE);
        let magic_bytes = reader.read_u64("magic bytes")?;
        if magic_bytes != EXPORTED_STATE_MAGIC_BYTES {
            return Err(ArtifactError::InvalidMagic {
                format: "exported state jam",
                expected: "EXPJAM",
            });
        }

        let version = reader.read_u32("version")?;
        if version != EXPORTED_STATE_VERSION {
            return Err(ArtifactError::UnsupportedVersion {
                format: "exported state jam",
                version,
                supported: "0",
            });
        }

        let ker_hash = reader.read_hash("kernel hash")?;
        let event_num = reader.read_u64("event number")?;
        let jam = JammedNoun::new(Bytes::copy_from_slice(reader.read_bytes("state jam")?));
        reader.finish()?;

        Ok(Self {
            magic_bytes,
            version,
            ker_hash,
            event_num,
            jam,
        })
    }

    pub fn from_loadstate<J: Jammer>(state: LoadState) -> Self {
        let LoadState {
            kernel_state,
            ker_hash,
            event_num,
        } = state;

        info!(
            event_num = event_num,
            ker_hash = %ker_hash,
            jammer = std::any::type_name::<J>(),
            "state jam jamming kernel state start"
        );
        let jam_start = Instant::now();
        let jam = JammedNoun::new(kernel_state.coerce_jammer::<J>().jam());
        info!(
            event_num = event_num,
            ker_hash = %ker_hash,
            jammer = std::any::type_name::<J>(),
            jam_bytes = jam.0.len(),
            elapsed_ms = jam_start.elapsed().as_secs_f64() * 1000.0,
            "state jam jamming kernel state done"
        );

        Self {
            magic_bytes: EXPORTED_STATE_MAGIC_BYTES,
            version: EXPORTED_STATE_VERSION,
            ker_hash,
            event_num,
            jam,
        }
    }

    pub fn to_loadstate<J: Jammer>(self) -> Result<LoadState, NockAppError> {
        info!(
            event_num = self.event_num,
            ker_hash = %self.ker_hash,
            jammer = std::any::type_name::<J>(),
            "state jam cueing kernel state start"
        );
        let cue_start = Instant::now();
        let mut kernel_state = NounSlab::<J>::new();
        let kernel_state_noun = kernel_state.cue_into(self.jam.0)?;
        kernel_state.set_root(kernel_state_noun);
        let kernel_state: NounSlab = kernel_state.coerce_jammer();
        info!(
            event_num = self.event_num,
            ker_hash = %self.ker_hash,
            jammer = std::any::type_name::<J>(),
            elapsed_ms = cue_start.elapsed().as_secs_f64() * 1000.0,
            "state jam cueing kernel state done"
        );

        Ok(LoadState {
            kernel_state,
            ker_hash: self.ker_hash,
            event_num: self.event_num,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{catch_unwind, AssertUnwindSafe};

    use blake3::hash;

    use super::*;

    fn exported_state_with_absurd_jam_length() -> Vec<u8> {
        let state = ExportedState {
            magic_bytes: EXPORTED_STATE_MAGIC_BYTES,
            version: EXPORTED_STATE_VERSION,
            ker_hash: hash(b"state-jam"),
            event_num: 7,
            jam: JammedNoun::new(Bytes::new()),
        };
        let mut bytes = state.encode().expect("encode exported state");

        // The final byte is the bincode varint length for the empty Bytes field.
        assert_eq!(bytes.pop(), Some(0));
        bytes.push(253);
        bytes.extend_from_slice(&u64::MAX.to_le_bytes());
        bytes
    }

    #[test]
    fn decodes_exported_state_roundtrip() {
        let state = ExportedState {
            magic_bytes: EXPORTED_STATE_MAGIC_BYTES,
            version: EXPORTED_STATE_VERSION,
            ker_hash: hash(b"state-jam"),
            event_num: 7,
            jam: JammedNoun::new(Bytes::from_static(b"state")),
        };
        let encoded = state.encode().expect("encode exported state");
        let decoded = ExportedState::decode(&encoded).expect("decode exported state");

        assert_eq!(decoded, state);
    }

    #[test]
    fn corrupt_exported_state_length_is_reported_without_panic() {
        let bytes = exported_state_with_absurd_jam_length();
        let decode_result = catch_unwind(AssertUnwindSafe(|| ExportedState::decode(&bytes)));

        assert!(
            decode_result.is_ok(),
            "corrupt exported state length must be reported as a decode error, not a panic"
        );
        let err = decode_result
            .expect("checked above")
            .expect_err("corrupt exported state should fail to decode");
        assert!(
            err.to_string().contains("Re-export the state jam"),
            "error should give an operator a recovery path: {err}"
        );
    }
}
