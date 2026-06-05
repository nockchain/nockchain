use std::any;

use bytes::Bytes;
use ibig::UBig;
use nockvm::jets::cold::{Nounable, NounableResult};
use nockvm::noun::{Atom, NounAllocator, Slots, D, T};

use crate::utils::error::ConversionError;
use crate::{Noun, Result};

pub trait ToBytes {
    fn to_bytes(&self) -> Result<Vec<u8>>;
}

impl<T: ToBytes> ToBytes for Vec<T> {
    fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();

        for item in self.iter() {
            let item = item.to_bytes()?;
            bytes.extend(item);
        }

        Ok(bytes)
    }
}

pub trait ToBytesExt: ToBytes {
    /// size of `size`.
    fn to_n_bytes(&self, size: usize) -> Result<Vec<u8>>
    where
        Self: Sized;
    fn to_u64(&self) -> Result<u64>;
    fn as_bytes(&self) -> Result<Bytes>;
}

impl<T> ToBytesExt for T
where
    T: ToBytes,
{
    fn to_n_bytes(&self, size: usize) -> Result<Vec<u8>>
    where
        Self: Sized,
    {
        let mut data = T::to_bytes(self)?;

        if data.len() > size {
            return Err(ConversionError::TooBig(any::type_name::<T>().to_string()))?;
        }

        data.resize(size, 0);
        Ok(data)
    }

    fn to_u64(&self) -> Result<u64> {
        let bytes = T::to_bytes(self)?;
        Ok(u64::from_le_bytes(bytes.try_into().unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        })))
    }
    fn as_bytes(&self) -> Result<Bytes> {
        let bytes = T::to_bytes(self)?;
        Ok(Bytes::from(bytes))
    }
}

impl ToBytes for u64 {
    fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(self.to_le_bytes().to_vec())
    }
}

impl<T: ToBytes, const SIZE: usize> ToBytes for [T; SIZE] {
    fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();

        for item in self.iter() {
            let item = item.to_bytes()?;
            bytes.extend(item);
        }

        Ok(bytes)
    }
}

impl ToBytes for String {
    fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(self.bytes().collect())
    }
}

impl ToBytes for [u8] {
    fn to_bytes(&self) -> Result<Vec<u8>> {
        let data = self.to_vec();
        Ok(data)
    }
}

impl ToBytes for &[u8] {
    fn to_bytes(&self) -> Result<Vec<u8>> {
        let data = self.to_vec();
        Ok(data)
    }
}

impl ToBytes for Vec<u8> {
    fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(self.clone())
    }
}
impl ToBytes for &str {
    fn to_bytes(&self) -> Result<Vec<u8>> {
        let bytes = self.bytes();
        Ok(bytes.collect::<Vec<u8>>())
    }
}

/// Byts is a Vec of bytes in big-endian order. It is the rust
/// representation of the $byts hoon type which consists of [wid=@ dat=@]
/// We do not achieve noun isomorphism due to trailing zeros being
/// implicit in the hoon implementation given the `wid` field while being
/// explicitly stored in the rust implementation.
#[derive(Debug, Clone)]
pub struct Byts(pub Vec<u8>);

impl Byts {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}

impl Nounable for Byts {
    type Target = Self;
    fn into_noun<A: NounAllocator>(self, stack: &mut A) -> Noun {
        let big = UBig::from_be_bytes(&self.0);
        let wid = D(self.0.len() as u64);
        let dat = Atom::from_ubig(stack, &big).as_noun();
        T(stack, &[wid, dat])
    }
    fn from_noun<A: NounAllocator>(_stack: &mut A, noun: &Noun) -> NounableResult<Self::Target> {
        let size = noun.slot(2)?;
        let dat = noun.slot(3)?.as_atom()?;

        let wid = size.as_atom()?.as_u64()? as usize;
        let mut res = vec![0; wid];

        let bytes_be = dat.to_be_bytes();

        // Iterate over the bytes in reverse order
        // Start copying at the first non zero value encountered
        let mut start_copying = false;
        let mut copy_index = 0;
        for byte in bytes_be.iter() {
            if *byte != 0 {
                start_copying = true;
            }
            if start_copying {
                res[copy_index] = *byte;
                copy_index += 1;
            }
        }
        Ok(Byts(res))
    }
}

#[cfg(test)]
mod test {
    use ibig::ubig;
    use nockvm::interpreter::Context;
    use nockvm::jets;
    use nockvm::jets::cold::{FromNounError, Nounable};
    use nockvm::jets::util::test::{assert_noun_eq, A};
    use nockvm::noun::{D, T};

    use crate::utils::bytes::Byts;

    fn test_byt_direct_atom(context: &mut Context, n: u64) -> Result<(), FromNounError> {
        // Start with a byt_noun which is a direct atom and consists of zeroes
        let byt_noun = T(&mut context.stack, &[D(n), D(0x0)]);
        // Convert it to byt
        let byt = super::Byts::from_noun(&mut context.stack, &byt_noun)?;
        // Check that it is equal
        assert_eq!(byt.0, vec![0x00; n as usize]);
        // Convert it back to noun
        let roundtrip = Byts::into_noun(byt, &mut context.stack);
        // Check that the noun is as expected, we will truncate trailing zeroes when they aren't meaningful
        let byt_noun_with_trailing_zero = T(&mut context.stack, &[D(n), D(0)]);
        assert_noun_eq(&mut context.stack, roundtrip, byt_noun_with_trailing_zero);
        Ok(())
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_by_conversion_direct() -> Result<(), FromNounError> {
        let mut context = jets::util::test::init_context();

        // Start with a byt_noun which is a direct atom and a trailing zero
        let byt_noun = T(&mut context.stack, &[D(3), D(0x8765)]);
        // Convert it to byt
        let byt = super::Byts::from_noun(&mut context.stack, &byt_noun)?;
        // Check that it is equal
        assert_eq!(byt.0, vec![0x87, 0x65, 0x00]);
        // Convert it back to noun
        let roundtrip = Byts::into_noun(byt, &mut context.stack);
        let byt_noun_with_trailing_zero = T(&mut context.stack, &[D(3), D(0x876500)]);
        // Check that the noun is as expected, we include trailing zeros when they are meaningful
        assert_noun_eq(&mut context.stack, roundtrip, byt_noun_with_trailing_zero);

        test_byt_direct_atom(&mut context, 4)?;
        test_byt_direct_atom(&mut context, 10)?;
        // // Start with a byt_noun which is a direct atom and consists of zeroes
        // let byt_noun = T(&mut context.stack, &[D(10), D(0x0)]);
        // // Convert it to byt
        // let byt = super::Byts::from_noun(&mut context.stack, &byt_noun)?;
        // // Check that it is equal
        // assert_eq!(byt.0, vec![0x00; 10]);
        // // Convert it back to noun
        // let roundtrip = Byts::into_noun(byt, &mut context.stack);
        // // Check that the noun is as expected, we will truncate trailing zeroes when they aren't meaningful
        // let byt_noun_with_trailing_zero = T(&mut context.stack, &[D(10), D(0)]);
        // assert_noun_eq(&mut context.stack, roundtrip, byt_noun_with_trailing_zero);

        Ok(())
    }

    #[test]
    //  APOLOGIA: ibig/ubig ManuallyDrops Vec, we are aware, we plan on purging it
    #[cfg_attr(miri, ignore)]
    fn test_byt_conversion_indirect() -> Result<(), FromNounError> {
        let mut context = jets::util::test::init_context();

        // Start with a byt_noun is an indirect atom but fits in a u64
        let byt = Byts(vec![0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0]);
        // Convert it to noun
        let byt_noun = byt.clone().into_noun(&mut context.stack);
        // Check that the noun is as expected
        let expected_byt_dat = A(&mut context.stack, &ubig!(0x123456789ABCDEF0));
        let expected_byt_noun = T(&mut context.stack, &[D(8), expected_byt_dat]);
        assert_noun_eq(&mut context.stack, byt_noun, expected_byt_noun);
        // Convert it back to a byt and check if it matches
        let byt_roundtrip = Byts::from_noun(&mut context.stack, &byt_noun)?;
        assert_eq!(byt.0, byt_roundtrip.0);

        // Start with a byt_noun is an indirect atom which does not fit in a u64
        let byt = Byts(vec![0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0, 0x00]);
        // Convert it to noun
        let byt_noun = byt.clone().into_noun(&mut context.stack);
        let expected_byt_dat = A(&mut context.stack, &ubig!(0x123456789ABCDEF000));
        let expected_byt_noun = T(&mut context.stack, &[D(9), expected_byt_dat]);
        // Check that the noun is as expected
        assert_noun_eq(&mut context.stack, byt_noun, expected_byt_noun);
        // Convert it back to a byt
        let byt_roundtrip = Byts::from_noun(&mut context.stack, &byt_noun)?;
        // Check that it is the same
        assert_eq!(byt.0, byt_roundtrip.0);
        Ok(())
    }
}
