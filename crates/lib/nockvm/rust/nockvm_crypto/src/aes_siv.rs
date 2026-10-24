#![allow(unexpected_cfgs)]

use aes::Aes192;
use aes_siv::aead::generic_array::GenericArray;
use aes_siv::aead::KeyInit;
use aes_siv::siv::{Aes128Siv, Aes256Siv, CmacSiv};
type Aes192Siv = CmacSiv<Aes192>;

#[derive(Debug)]
pub enum Error {
    InvalidKeyLength,
    InvalidOutputLength,
    InvalidHeadersLength,
    CipherNotAuthentic,
}

/// AES-SIV encryption function.
pub fn ac_aes_siv_en<const N: usize>(
    key: &mut [u8; N],
    message: &mut [u8],
    data: &mut [&mut [u8]],
    iv: &mut [u8; 16],
    out: &mut [u8],
) -> Result<(), Error> {
    key.reverse();
    message.reverse();
    for item in data.iter_mut() {
        item.reverse();
    }

    let iv_tag;
    if N == 32 {
        if let Ok(mut cipher) = Aes128Siv::new_from_slice(key) {
            match cipher.encrypt_in_place_detached(data, message) {
                Ok(tag) => iv_tag = tag,
                Err(_) => return Err(Error::InvalidOutputLength),
            }
        } else {
            return Err(Error::InvalidKeyLength);
        }
    } else if N == 48 {
        if let Ok(mut cipher) = Aes192Siv::new_from_slice(key) {
            match cipher.encrypt_in_place_detached(data, message) {
                Ok(tag) => iv_tag = tag,
                Err(_) => return Err(Error::InvalidOutputLength),
            }
        } else {
            return Err(Error::InvalidKeyLength);
        }
    } else if N == 64 {
        if let Ok(mut cipher) = Aes256Siv::new_from_slice(key) {
            match cipher.encrypt_in_place_detached(data, message) {
                Ok(tag) => iv_tag = tag,
                Err(_) => return Err(Error::InvalidOutputLength),
            }
        } else {
            return Err(Error::InvalidKeyLength);
        }
    } else {
        return Err(Error::InvalidKeyLength);
    }
    let mut iv_slice = iv_tag.as_slice().to_owned();
    iv_slice.reverse();
    iv.copy_from_slice(&iv_slice);
    key.reverse();
    message.reverse();
    out[0..message.len()].copy_from_slice(message);
    Ok(())
}

/// AES-SIV decryption function.
pub fn ac_aes_siv_de<const N: usize>(
    key: &mut [u8; N],
    message: &mut [u8],
    data: &mut [&mut [u8]],
    iv: &mut [u8; 16],
    out: &mut [u8],
) -> Result<(), Error> {
    key.reverse();
    message.reverse();
    iv.reverse();
    for item in data.iter_mut() {
        item.reverse();
    }

    let iv_array = GenericArray::from_slice(iv);
    if N == 32 {
        if let Ok(mut cipher) = Aes128Siv::new_from_slice(key) {
            match cipher.decrypt_in_place_detached(data, message, iv_array) {
                Ok(_) => (),
                Err(_) => return Err(Error::CipherNotAuthentic),
            }
        } else {
            return Err(Error::InvalidKeyLength);
        }
    } else if N == 48 {
        if let Ok(mut cipher) = Aes192Siv::new_from_slice(key) {
            match cipher.decrypt_in_place_detached(data, message, iv_array) {
                Ok(_) => (),
                Err(_) => return Err(Error::CipherNotAuthentic),
            }
        } else {
            return Err(Error::InvalidKeyLength);
        }
    } else if N == 64 {
        if let Ok(mut cipher) = Aes256Siv::new_from_slice(key) {
            match cipher.decrypt_in_place_detached(data, message, iv_array) {
                Ok(_) => (),
                Err(_) => return Err(Error::CipherNotAuthentic),
            }
        } else {
            return Err(Error::InvalidKeyLength);
        }
    } else {
        return Err(Error::InvalidKeyLength);
    }
    key.reverse();
    message.reverse();
    out.copy_from_slice(message);
    Ok(())
}

#[cfg(test)]
#[cfg(feature = "test_vs_urcrypt")]
/// Compare the results of the nockvm_crypto functions with the corresponding
/// urcrypt functions. To run, use `cargo test --features test_vs_urcrypt`
/// from the `nockvm/rust/nockvm_crypto` directory.
mod urcrypt_tests {
    use aes_siv::aead::rand_core::CryptoRngCore;
    use rand::rngs::OsRng;
    use urcrypt_sys::{
        urcrypt_aes_siv_data, urcrypt_aes_siva_en, urcrypt_aes_sivb_en, urcrypt_aes_sivc_en,
    };

    use super::ac_aes_siv_en;

    #[test]
    fn test_aes_siva_en() {
        let mut uc_key = [0u8; 32];
        let mut uc_message = [0u8; 56];
        let mut uc_iv = [0u8; 16];
        let mut uc_bytes = [0u8; 32];
        let mut osrng = OsRng;
        let csprng = osrng.as_rngcore();
        csprng.fill_bytes(&mut uc_key);
        csprng.fill_bytes(&mut uc_message);
        csprng.fill_bytes(&mut uc_bytes);
        let mut uc_data: [urcrypt_aes_siv_data; 1] = [urcrypt_aes_siv_data {
            bytes: uc_bytes.as_mut_ptr(),
            length: uc_bytes.len(),
        }];
        let mut uc_out = [0u8; 56];

        let mut ac_key = uc_key;
        let mut ac_message = uc_message;
        let mut ac_iv = [0u8; 16];
        let mut ac_bytes = uc_bytes;
        let ac_data: &mut [&mut [u8]] = &mut [&mut ac_bytes];
        let mut ac_out = [0u8; 56];

        unsafe {
            urcrypt_aes_siva_en(
                uc_message.as_mut_ptr(),
                uc_message.len(),
                uc_data.as_mut_ptr(),
                uc_data.len(),
                uc_key.as_mut_ptr(),
                uc_iv.as_mut_ptr(),
                uc_out.as_mut_ptr(),
            )
        };

        ac_aes_siv_en::<32>(
            &mut ac_key, &mut ac_message, ac_data, &mut ac_iv, &mut ac_out,
        )
        .expect(&format!(
            "Called `expect()` at {}:{} (git sha: {})",
            file!(),
            line!(),
            option_env!("GIT_SHA").unwrap_or("unknown")
        ));

        assert_eq!(ac_out, uc_out);
    }

    #[test]
    fn test_aes_sivb_en() {
        let mut uc_key = [0u8; 48];
        let mut uc_message = [0u8; 56];
        let mut uc_iv = [0u8; 16];
        let mut uc_bytes = [0u8; 32];
        let mut osrng = OsRng;
        let csprng = osrng.as_rngcore();
        csprng.fill_bytes(&mut uc_key);
        csprng.fill_bytes(&mut uc_message);
        csprng.fill_bytes(&mut uc_bytes);
        let mut uc_data: [urcrypt_aes_siv_data; 1] = [urcrypt_aes_siv_data {
            bytes: uc_bytes.as_mut_ptr(),
            length: uc_bytes.len(),
        }];
        let mut uc_out = [0u8; 56];

        let mut ac_key = uc_key;
        let mut ac_message = uc_message;
        let mut ac_iv = [0u8; 16];
        let mut ac_bytes = uc_bytes;
        let ac_data: &mut [&mut [u8]] = &mut [&mut ac_bytes];
        let mut ac_out = [0u8; 56];

        unsafe {
            urcrypt_aes_sivb_en(
                uc_message.as_mut_ptr(),
                uc_message.len(),
                uc_data.as_mut_ptr(),
                uc_data.len(),
                uc_key.as_mut_ptr(),
                uc_iv.as_mut_ptr(),
                uc_out.as_mut_ptr(),
            )
        };

        ac_aes_siv_en::<48>(
            &mut ac_key, &mut ac_message, ac_data, &mut ac_iv, &mut ac_out,
        )
        .expect(&format!(
            "Called `expect()` at {}:{} (git sha: {})",
            file!(),
            line!(),
            option_env!("GIT_SHA").unwrap_or("unknown")
        ));

        assert_eq!(ac_out, uc_out);
    }

    #[test]
    fn test_aes_sivc_en() {
        let mut uc_key = [0u8; 64];
        let mut uc_message = [0u8; 56];
        let mut uc_iv = [0u8; 16];
        let mut uc_bytes = [0u8; 32];
        let mut osrng = OsRng;
        let csprng = osrng.as_rngcore();
        csprng.fill_bytes(&mut uc_key);
        csprng.fill_bytes(&mut uc_message);
        csprng.fill_bytes(&mut uc_bytes);
        let mut uc_data: [urcrypt_aes_siv_data; 1] = [urcrypt_aes_siv_data {
            bytes: uc_bytes.as_mut_ptr(),
            length: uc_bytes.len(),
        }];
        let mut uc_out = [0u8; 56];

        let mut ac_key = uc_key;
        let mut ac_message = uc_message;
        let mut ac_iv = [0u8; 16];
        let mut ac_bytes = uc_bytes;
        let ac_data: &mut [&mut [u8]] = &mut [&mut ac_bytes];
        let mut ac_out = [0u8; 56];

        unsafe {
            urcrypt_aes_sivc_en(
                uc_message.as_mut_ptr(),
                uc_message.len(),
                uc_data.as_mut_ptr(),
                uc_data.len(),
                uc_key.as_mut_ptr(),
                uc_iv.as_mut_ptr(),
                uc_out.as_mut_ptr(),
            )
        };

        ac_aes_siv_en::<64>(
            &mut ac_key, &mut ac_message, ac_data, &mut ac_iv, &mut ac_out,
        )
        .expect(&format!(
            "Called `expect()` at {}:{} (git sha: {})",
            file!(),
            line!(),
            option_env!("GIT_SHA").unwrap_or("unknown")
        ));

        assert_eq!(ac_out, uc_out);
    }
}
