#![allow(unexpected_cfgs)]

use sha1::{Digest, Sha1};
use sha2::{Sha256, Sha512};

/// Hashes a message using SHA-1.
pub fn ac_sha1(message: &mut [u8], out: &mut [u8]) {
    message.reverse();
    let mut hasher = Sha1::new();
    hasher.update(message);
    let mut result = hasher.finalize();
    result.reverse();
    out.copy_from_slice(&result);
}

/// Hashes a message using SHA-256.
pub fn ac_shay(message: &mut [u8], out: &mut [u8]) {
    let mut hasher = Sha256::new();
    hasher.update(message);
    let result = hasher.finalize();
    out.copy_from_slice(&result);
}

/// Hashes a message using SHA-512.
pub fn ac_shal(message: &mut [u8], out: &mut [u8]) {
    let mut hasher = Sha512::new();
    hasher.update(message);
    let result = hasher.finalize();
    out.copy_from_slice(&result);
}

/// Hashes a message and salt using SHA-256.
pub fn ac_shas(message: &mut [u8], salt: &mut [u8], out: &mut [u8]) {
    let mut mid: [u8; 32] = [0; 32];
    ac_shay(message, &mut mid);

    if salt.len() > 32 {
        for i in 0..32 {
            salt[i] ^= mid[i];
        }
        ac_shay(salt, out);
    } else {
        for i in 0..salt.len() {
            mid[i] ^= salt[i];
        }
        ac_shay(&mut mid, out);
    }
}

#[cfg(test)]
#[cfg(feature = "test_vs_urcrypt")]
/// Compare the results of the nockvm_crypto functions with the corresponding
/// urcrypt functions. To run, use `cargo test --features test_vs_urcrypt`
/// from the `nockvm/rust/nockvm_crypto` directory.
mod urcrypt_tests {
    use urcrypt_sys::{urcrypt_sha1, urcrypt_shal, urcrypt_shas, urcrypt_shay};

    use super::{ac_sha1, ac_shal, ac_shas, ac_shay};

    #[test]
    fn test_sha1() {
        let mut message: [u8; 32] = [42; 32];

        let mut uc_out: [u8; 20] = [0; 20];
        unsafe { urcrypt_sha1(message.as_mut_ptr(), message.len(), uc_out.as_mut_ptr()) };

        let mut ac_out: [u8; 20] = [0; 20];
        ac_sha1(&mut message, &mut ac_out);

        assert_eq!(ac_out, uc_out);
    }

    #[test]
    fn test_shay() {
        let mut message: [u8; 32] = [42; 32];

        let mut uc_out: [u8; 32] = [0; 32];
        unsafe { urcrypt_shay(message.as_mut_ptr(), message.len(), uc_out.as_mut_ptr()) };

        let mut ac_out: [u8; 32] = [0; 32];
        ac_shay(&mut message, &mut ac_out);

        assert_eq!(ac_out, uc_out);
    }

    #[test]
    fn test_shal() {
        let mut message: [u8; 32] = [42; 32];

        let mut uc_out: [u8; 64] = [0; 64];
        unsafe { urcrypt_shal(message.as_mut_ptr(), message.len(), uc_out.as_mut_ptr()) };

        let mut ac_out: [u8; 64] = [0; 64];
        ac_shal(&mut message, &mut ac_out);

        assert_eq!(ac_out, uc_out);
    }

    #[test]
    fn test_shas() {
        let mut message: [u8; 32] = [42; 32];

        let mut uc_salt: [u8; 32] = [43; 32];
        let mut uc_out: [u8; 32] = [0; 32];
        unsafe {
            urcrypt_shas(
                uc_salt.as_mut_ptr(),
                uc_salt.len(),
                message.as_ptr(),
                message.len(),
                uc_out.as_mut_ptr(),
            )
        };

        let mut ac_salt: [u8; 32] = [43; 32];
        let mut ac_out: [u8; 32] = [0; 32];
        ac_shas(&mut message, &mut ac_salt, &mut ac_out);

        assert_eq!(ac_out, uc_out);
    }
}
