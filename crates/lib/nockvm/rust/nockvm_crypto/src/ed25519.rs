#![allow(unexpected_cfgs)]

use curve25519_dalek::edwards::CompressedEdwardsY;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use x25519_dalek::{PublicKey, StaticSecret};

/// Generate a public key from the given seed and write it to the given output slice.
pub fn ac_ed_puck(seed: &mut [u8; 32], out: &mut [u8; 32]) {
    let signing_key = SigningKey::from_bytes(seed);
    let verifying_key = signing_key.verifying_key();
    *out = verifying_key.to_bytes();
}

/// Perform a key exchange between the given public key and the private key of
/// the keypair generate from the given seed, writing the resulting shared key
/// to the given output slice.
pub fn ac_ed_shar(public: &[u8; 32], seed: &[u8; 32], out: &mut [u8; 32]) {
    let self_key = SigningKey::from_bytes(seed);
    let self_secret = StaticSecret::from(self_key.to_scalar_bytes());

    if let Ok(compressed_ed_pt) = CompressedEdwardsY::from_slice(public) {
        if let Some(ed_pt) = compressed_ed_pt.decompress() {
            let public_key = PublicKey::from(ed_pt.to_montgomery().to_bytes());
            let shared_secret = self_secret.diffie_hellman(&public_key);
            *out = shared_secret.to_bytes();
        }
    }
}

/// Sign a message with the given seed and write the resulting signature to the
/// given output slice.
pub fn ac_ed_sign(msg: &[u8], seed: &[u8; 32], out: &mut [u8; 64]) {
    let signing_key = SigningKey::from_bytes(seed);
    let signature = signing_key.sign(msg);
    *out = signature.to_bytes();
}

/// Verify a signature of the given message with the given public key.
pub fn ac_ed_veri(msg: &[u8], public: &[u8; 32], signature: &[u8; 64]) -> bool {
    if let Ok(verifying_key) = VerifyingKey::from_bytes(public) {
        verifying_key
            .verify(msg, &Signature::from_bytes(signature))
            .is_ok()
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use ibig::ubig;

    use super::{ac_ed_puck, ac_ed_sign, ac_ed_veri};

    #[test]
    fn test_ed_puck() {
        let mut seed: [u8; 32] = [0; 32];
        let mut public_key: [u8; 32] = [0; 32];
        ac_ed_puck(&mut seed, &mut public_key);
        assert_eq!(
            public_key,
            [
                59, 106, 39, 188, 206, 182, 164, 45, 98, 163, 168, 208, 42, 111, 13, 115, 101, 50,
                21, 119, 29, 226, 67, 166, 58, 192, 72, 161, 139, 89, 218, 41
            ]
        );
    }

    // #[test]
    // fn test_ed_shar() {
    // }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_ed_sign() {
        // from https://datatracker.ietf.org/doc/html/rfc8032#section-7.1
        let msg = b"Ares has long exerted a pull on the human imagination.";
        let seed_src = &ubig!(_0x4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb)
            .to_le_bytes();
        let mut seed: [u8; 32] = [0; 32];
        seed.copy_from_slice(seed_src);
        let mut signature: [u8; 64] = [0; 64];
        ac_ed_sign(msg, &seed, &mut signature);
        let constant_signature = [
            112, 132, 235, 218, 21, 180, 5, 48, 145, 211, 212, 153, 255, 229, 198, 165, 64, 140,
            246, 27, 0, 97, 185, 143, 180, 10, 38, 68, 200, 71, 231, 108, 141, 26, 97, 207, 199,
            204, 0, 123, 250, 161, 182, 92, 48, 116, 144, 42, 204, 6, 199, 162, 10, 66, 173, 185,
            155, 96, 240, 56, 224, 187, 160, 1,
        ];

        assert_eq!(signature, constant_signature);
    }

    #[test]
    fn test_ed_veri() {
        let msg = b"The erratically moving red star in the sky was seen as sinister or violent by the ancients.";

        let mut seed = [42; 32];
        let mut public_key: [u8; 32] = [0; 32];
        ac_ed_puck(&mut seed, &mut public_key);

        let mut signature: [u8; 64] = [0; 64];
        ac_ed_sign(msg, &seed, &mut signature);

        let valid = ac_ed_veri(msg, &public_key, &signature);

        assert!(valid);
    }
}

#[cfg(test)]
#[cfg(feature = "test_vs_urcrypt")]
/// Compare the results of the nockvm_crypto functions with the corresponding
/// urcrypt functions. To run, use `cargo test --features test_vs_urcrypt`
/// from the `nockvm/rust/nockvm_crypto` directory.
mod urcrypt_tests {
    use ibig::ubig;
    use urcrypt_sys::{urcrypt_ed_puck, urcrypt_ed_shar, urcrypt_ed_sign, urcrypt_ed_veri};

    use super::{ac_ed_puck, ac_ed_shar, ac_ed_sign, ac_ed_veri};

    #[test]
    fn test_ed_puck() {
        let mut seed: [u8; 32] = [42; 32];

        let mut uc_out: [u8; 32] = [0; 32];
        unsafe { urcrypt_ed_puck(seed.as_ptr(), uc_out.as_mut_ptr()) };

        let mut ac_out: [u8; 32] = [0; 32];
        ac_ed_puck(&mut seed, &mut ac_out);

        assert_eq!(ac_out, uc_out);
    }

    #[test]
    fn test_ed_shar() {
        let public_key_src =
            &ubig!(_0xd75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a)
                .to_le_bytes();
        let mut public_key: [u8; 32] = [0; 32];
        public_key.copy_from_slice(public_key_src);

        let seed_src = &ubig!(_0x4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb)
            .to_le_bytes();
        let mut seed: [u8; 32] = [0; 32];
        seed.copy_from_slice(seed_src);

        let mut uc_out: [u8; 32] = [0; 32];
        unsafe { urcrypt_ed_shar(public_key.as_ptr(), seed.as_ptr(), uc_out.as_mut_ptr()) };

        let mut ac_out: [u8; 32] = [0; 32];
        ac_ed_shar(&public_key, &seed, &mut ac_out);

        assert_eq!(ac_out, uc_out);
    }

    #[test]
    fn test_ed_sign() {
        let msg = b"The Greeks identified it with Ares, the god of war.";

        let seed_src = &ubig!(_0x4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb)
            .to_le_bytes();
        let mut seed: [u8; 32] = [0; 32];
        seed.copy_from_slice(seed_src);

        let mut uc_out: [u8; 64] = [0; 64];
        unsafe { urcrypt_ed_sign(msg.as_ptr(), msg.len(), seed.as_ptr(), uc_out.as_mut_ptr()) };

        let mut ac_out: [u8; 64] = [0; 64];
        ac_ed_sign(msg, &seed, &mut ac_out);

        assert_eq!(ac_out, uc_out);
    }

    #[test]
    fn test_ed_veri() {
        let msg = b"The Babylonians named it after Nergal, god of the underworld.";

        let seed_src = &ubig!(_0x4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb)
            .to_le_bytes();
        let mut seed: [u8; 32] = [0; 32];
        seed.copy_from_slice(seed_src);

        let mut public_key: [u8; 32] = [0; 32];
        ac_ed_puck(&mut seed, &mut public_key);

        let mut signature: [u8; 64] = [0; 64];
        ac_ed_sign(msg, &seed, &mut signature);

        let uc_out = unsafe {
            urcrypt_ed_veri(
                msg.as_ptr(),
                msg.len(),
                public_key.as_ptr(),
                signature.as_ptr(),
            )
        };

        let ac_out = ac_ed_veri(msg, &public_key, &signature);

        assert_eq!(ac_out, uc_out);
    }
}
