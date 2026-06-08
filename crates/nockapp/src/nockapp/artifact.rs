use blake3::Hash;
use thiserror::Error;

const CHECKPOINT_ADVICE: &str =
    "Restore the file from a known-good peer or move it to a backup directory so Nockchain can try another checkpoint.";

#[derive(Debug, Error)]
pub enum ArtifactError {
    #[error("{format} is malformed: {details}. {advice}")]
    Malformed {
        format: &'static str,
        details: String,
        advice: &'static str,
    },
    #[error(
        "{format} has unsupported version {version}; supported versions: {supported}. Use a compatible Nockchain binary or regenerate the artifact."
    )]
    UnsupportedVersion {
        format: &'static str,
        version: u32,
        supported: &'static str,
    },
    #[error(
        "{format} has invalid magic bytes; expected {expected}. Use the correct artifact type, or regenerate the artifact from a known-good node."
    )]
    InvalidMagic {
        format: &'static str,
        expected: &'static str,
    },
}

impl ArtifactError {
    pub fn malformed(format: &'static str, details: impl Into<String>) -> Self {
        Self::Malformed {
            format,
            details: details.into(),
            advice: CHECKPOINT_ADVICE,
        }
    }

    pub fn malformed_with_advice(
        format: &'static str,
        details: impl Into<String>,
        advice: &'static str,
    ) -> Self {
        Self::Malformed {
            format,
            details: details.into(),
            advice,
        }
    }
}

pub(crate) struct CheckedReader<'a> {
    input: &'a [u8],
    offset: usize,
    format: &'static str,
    advice: &'static str,
}

impl<'a> CheckedReader<'a> {
    pub(crate) fn new(input: &'a [u8], format: &'static str) -> Self {
        Self {
            input,
            offset: 0,
            format,
            advice: CHECKPOINT_ADVICE,
        }
    }

    pub(crate) fn new_with_advice(
        input: &'a [u8],
        format: &'static str,
        advice: &'static str,
    ) -> Self {
        Self {
            input,
            offset: 0,
            format,
            advice,
        }
    }

    pub(crate) fn read_u64(&mut self, field: &'static str) -> Result<u64, ArtifactError> {
        match self.read_varint_discriminant(field)? {
            byte @ 0..=250 => Ok(u64::from(byte)),
            251 => {
                let bytes = self.read_array::<2>(field)?;
                Ok(u64::from(u16::from_le_bytes(bytes)))
            }
            252 => {
                let bytes = self.read_array::<4>(field)?;
                Ok(u64::from(u32::from_le_bytes(bytes)))
            }
            253 => {
                let bytes = self.read_array::<8>(field)?;
                Ok(u64::from_le_bytes(bytes))
            }
            254 => Err(self.malformed(format!(
                "{field} uses a u128 varint marker where u64 was expected"
            ))),
            _ => Err(self.malformed(format!("{field} uses a reserved varint marker"))),
        }
    }

    pub(crate) fn read_u32(&mut self, field: &'static str) -> Result<u32, ArtifactError> {
        let value = self.read_u64(field)?;
        u32::try_from(value)
            .map_err(|_| self.malformed(format!("{field} value {value} does not fit in u32")))
    }

    pub(crate) fn read_bool(&mut self, field: &'static str) -> Result<bool, ArtifactError> {
        match self.read_byte(field)? {
            0 => Ok(false),
            1 => Ok(true),
            value => Err(self.malformed(format!("{field} has invalid boolean value {value}"))),
        }
    }

    pub(crate) fn read_hash(&mut self, field: &'static str) -> Result<Hash, ArtifactError> {
        let bytes = self.read_array::<32>(field)?;
        Ok(Hash::from(bytes))
    }

    pub(crate) fn read_bytes(&mut self, field: &'static str) -> Result<&'a [u8], ArtifactError> {
        let len = self.read_u64(field)?;
        let len = usize::try_from(len)
            .map_err(|_| self.malformed(format!("{field} length does not fit in usize")))?;
        self.read_exact(field, len)
    }

    pub(crate) fn finish(&self) -> Result<(), ArtifactError> {
        let trailing = self.input.len().saturating_sub(self.offset);
        if trailing == 0 {
            Ok(())
        } else {
            Err(self.malformed(format!("{trailing} trailing bytes after artifact payload")))
        }
    }

    fn read_varint_discriminant(&mut self, field: &'static str) -> Result<u8, ArtifactError> {
        self.read_byte(field)
    }

    fn read_byte(&mut self, field: &'static str) -> Result<u8, ArtifactError> {
        let byte = self
            .input
            .get(self.offset)
            .ok_or_else(|| self.malformed(format!("{field} is missing")))?;
        self.offset += 1;
        Ok(*byte)
    }

    fn read_array<const N: usize>(
        &mut self,
        field: &'static str,
    ) -> Result<[u8; N], ArtifactError> {
        let bytes = self.read_exact(field, N)?;
        bytes
            .try_into()
            .map_err(|_| self.malformed(format!("{field} could not be read")))
    }

    fn read_exact(&mut self, field: &'static str, len: usize) -> Result<&'a [u8], ArtifactError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| self.malformed(format!("{field} length overflows usize")))?;
        if end > self.input.len() {
            return Err(self.malformed(format!(
                "{field} declares {len} bytes, but only {} bytes remain",
                self.input.len().saturating_sub(self.offset)
            )));
        }

        let bytes = &self.input[self.offset..end];
        self.offset = end;
        Ok(bytes)
    }

    fn malformed(&self, details: impl Into<String>) -> ArtifactError {
        ArtifactError::malformed_with_advice(self.format, details, self.advice)
    }
}
