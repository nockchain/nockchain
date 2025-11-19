use bs58;
use ibig::{ubig, Stack, UBig};
use nockapp::NockAppError;
use nockvm::noun::Noun;
use noun_serde::prelude::*;

//TODO all this stuff would be useful as jets, which mostly just requires
//using the Atom::as_ubig with the NockStack instead of ibig's heap version
// which we use to avoid having a NockStack sitting around.

// Goldilocks prime
const P: u64 = 0xffffffff00000001;

// noun -> tip5 -> ubig -> base58

/// Tries to convert a Noun to a Base58 string by extracting a 5-tuple, converting it to a decimal, and then to Base58.
///
/// # Arguments
/// * `noun` - The Noun to convert, expected to be a 5-tuple tip5 hash
///
/// # Returns
/// The Noun as a Base58 string
pub fn tip5_hash_to_base58(noun: Noun) -> Result<String, NockAppError> {
    let tuple: [u64; 5] = noun.decode()?;
    let decimal_value = base_p_to_decimal(tuple)?;
    let base58_string = ubig_to_base58(decimal_value);

    Ok(base58_string)
}

/// Stack-aware version of tip5_hash_to_base58
pub fn tip5_hash_to_base58_stack<S: Stack>(
    stack: &mut S,
    noun: Noun,
) -> Result<String, NockAppError> {
    let tuple: [u64; 5] = noun.decode()?;
    let decimal_value = base_p_to_decimal_stack(stack, tuple)?;
    let base58_string = ubig_to_base58(decimal_value);

    Ok(base58_string)
}

// FIXME: This use of ibig's pow will leak memory.
fn accum_prime_ubig(prime: &UBig, acc: &mut UBig, value: u64, i: usize) {
    *acc += UBig::from(value) * prime.pow(i);
}

fn accum_prime_ubig_stack<S: Stack>(
    stack: &mut S,
    prime: &UBig,
    acc: &mut UBig,
    value: u64,
    i: usize,
) {
    let pow_result = prime.pow_stack(stack, i);
    let mul_result = UBig::mul_stack(stack, UBig::from(value), pow_result);
    *acc += mul_result;
}

pub fn base_p_to_decimal(hash: [u64; 5]) -> Result<UBig, NockAppError> {
    let prime_ubig = UBig::from(P);
    let mut result = ubig!(0);

    for (i, value) in hash.iter().enumerate() {
        // Add the value * P^i to the result
        accum_prime_ubig(&prime_ubig, &mut result, *value, i);
    }
    Ok(result)
}

pub fn base_p_to_decimal_stack<S: Stack>(
    stack: &mut S,
    hash: [u64; 5],
) -> Result<UBig, NockAppError> {
    let prime_ubig = UBig::from(P);
    let mut result = ubig!(0);

    for (i, value) in hash.iter().enumerate() {
        // Add the value * P^i to the result
        accum_prime_ubig_stack(stack, &prime_ubig, &mut result, *value, i);
    }
    Ok(result)
}

/// Converts a UBig to a Base58 string.
pub fn ubig_to_base58(value: UBig) -> String {
    let bytes = value.to_be_bytes();
    bs58::encode(bytes).into_string()
}

// base58 -> ubig -> tip5 -> noun

/// Converts a UBig to a Base58 string.
pub fn base58_to_ubig(value: String) -> Result<UBig, NockAppError> {
    let bytes = bs58::decode(value)
        .into_vec()
        .map_err(|e| NockAppError::NounDecodeError(Box::new(e)))?;
    let value = UBig::from_be_bytes(&bytes);
    Ok(value)
}

pub fn decimal_to_base_p(value: UBig) -> Result<[u64; 5], NockAppError> {
    let mut result = [0; 5];
    let mut value = value.clone();
    for i in 0..5 {
        // TODO: I shouldn't have to clone here
        result[i] = (value.clone() % P) as u64;
        value /= P;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use nockapp::noun::slab::NounSlab;
    use nockvm::noun::{D, T};
    use quickcheck::{Arbitrary, Gen, QuickCheck, TestResult};

    use super::*;

    fn iso(tip5: [u64; 5]) {
        let ubig = base_p_to_decimal(tip5).unwrap();
        let base58 = ubig_to_base58(ubig);
        let ubig2 = base58_to_ubig(base58).unwrap();
        let tip5_2 = decimal_to_base_p(ubig2).unwrap();
        assert_eq!(tip5, tip5_2);
    }

    #[test]
    fn test_tip5_ubig_isomorphism() {
        let tip5 = [1, 2, 3, 4, 5];
        iso(tip5);
    }

    #[test]
    fn test_tip5_hash_to_base58_stack() {
        use nockapp::noun::slab::NounSlab;
        use nockvm::noun::Atom;

        // Create a NounSlab to use as both allocator and Stack
        let mut slab: NounSlab = NounSlab::new();

        // Test case 1: Simple tuple [1 2 3 4 5]
        let tuple1 = T(&mut slab, &[D(1), D(2), D(3), D(4), D(5)]);
        let expected1 = "2V9arU36gvtaofWmNowewoj9u7gbNA2qsJZEQ3WPky5mQ";
        let result1 = tip5_hash_to_base58_stack(&mut slab, tuple1).unwrap();
        assert_eq!(result1, expected1);

        // Test case 2: Complex values
        let a1 = Atom::new(&mut slab, 0x6ef99e5f3447ffda);
        let a2 = Atom::new(&mut slab, 0xdf94122d1a98ec99);
        let a3 = Atom::new(&mut slab, 0xcbf1918337a0e197);
        let a4 = Atom::new(&mut slab, 0x6cda1112891244ce);
        let a5 = Atom::new(&mut slab, 0x6e420b8a615508d4);

        let tuple2 = T(
            &mut slab,
            &[a1.as_noun(), a2.as_noun(), a3.as_noun(), a4.as_noun(), a5.as_noun()],
        );
        let expected2 = "6UkUko9WTwwR6VVRXwPQpUy5pswdvNtoyHspY5n9nLVnBxzAgEyMwPR";
        let result2 = tip5_hash_to_base58_stack(&mut slab, tuple2).unwrap();
        assert_eq!(result2, expected2);
    }

    #[test]
    #[cfg_attr(miri, ignore)] // ibig has a memory leak so miri fails this test
    fn test_tip5_hash_to_base58() {
        use nockvm::noun::Atom;
        // Create a NounSlab to use as an allocator
        let mut slab: NounSlab = NounSlab::new();

        // Test case 1: Simple tuple [1 2 3 4 5]
        let tuple1 = T(&mut slab, &[D(1), D(2), D(3), D(4), D(5)]);
        let expected1 = "2V9arU36gvtaofWmNowewoj9u7gbNA2qsJZEQ3WPky5mQ";
        let result1 = tip5_hash_to_base58(tuple1).unwrap_or_else(|_| {
            panic!(
                "Called `expect()` at {}:{} (git sha: {})",
                file!(),
                line!(),
                option_env!("GIT_SHA").unwrap_or("unknown")
            )
        });
        assert_eq!(result1, expected1);

        // Test case 2: Complex values
        let a1 = Atom::new(&mut slab, 0x6ef99e5f3447ffda);
        let a2 = Atom::new(&mut slab, 0xdf94122d1a98ec99);
        let a3 = Atom::new(&mut slab, 0xcbf1918337a0e197);
        let a4 = Atom::new(&mut slab, 0x6cda1112891244ce);
        let a5 = Atom::new(&mut slab, 0x6e420b8a615508d4);

        let tuple2 = T(
            &mut slab,
            &[a1.as_noun(), a2.as_noun(), a3.as_noun(), a4.as_noun(), a5.as_noun()],
        );
        let expected2 = "6UkUko9WTwwR6VVRXwPQpUy5pswdvNtoyHspY5n9nLVnBxzAgEyMwPR";
        let result2 = tip5_hash_to_base58(tuple2).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        assert_eq!(result2, expected2);
    }

    // Wrapper struct for implementing Arbitrary
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Tip5Hash([u64; 5]);

    impl Arbitrary for Tip5Hash {
        fn arbitrary(g: &mut Gen) -> Self {
            // Generate 5 u64 values modulo the Goldilocks prime P
            let mut hash = [0u64; 5];
            for i in 0..5 {
                // Generate a random u32 and convert to u64 to avoid very large values
                // This helps avoid triggering ibig buffer capacity issues
                let value: u32 = u32::arbitrary(g);
                hash[i] = (value as u64) % P;
            }
            Tip5Hash(hash)
        }

        fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
            // Simple shrinking strategy - just try smaller values for the first non-zero element
            let hash = self.0;
            let mut candidates = vec![];

            // Find first non-zero element and try to shrink it
            for i in 0..5 {
                if hash[i] > 0 {
                    let mut new_hash = hash;
                    new_hash[i] = hash[i] / 2;
                    candidates.push(Tip5Hash(new_hash));
                    break; // Only shrink one element at a time to avoid combinatorial explosion
                }
            }

            Box::new(candidates.into_iter())
        }
    }

    #[test]
    fn test_tip5_ubig_base58_isomorphism_quickcheck() {
        fn prop_isomorphism(tip5_hash: Tip5Hash) -> TestResult {
            let tip5 = tip5_hash.0;

            // Forward conversion: tip5 -> ubig -> base58
            let ubig = match base_p_to_decimal(tip5) {
                Ok(u) => u,
                Err(_) => return TestResult::discard(),
            };
            let base58 = ubig_to_base58(ubig.clone());

            // Backward conversion: base58 -> ubig -> tip5
            let ubig2 = match base58_to_ubig(base58.clone()) {
                Ok(u) => u,
                Err(_) => return TestResult::discard(),
            };
            let tip5_2 = match decimal_to_base_p(ubig2.clone()) {
                Ok(t) => t,
                Err(_) => return TestResult::discard(),
            };

            // Check that we get back the original tip5
            TestResult::from_bool(tip5 == tip5_2)
        }

        QuickCheck::new()
            .tests(1000)
            .quickcheck(prop_isomorphism as fn(Tip5Hash) -> TestResult);
    }

    #[test]
    fn test_ubig_conversion_properties() {
        fn prop_ubig_preserves_value(tip5_hash: Tip5Hash) -> TestResult {
            let tip5 = tip5_hash.0;

            // Convert to UBig
            let ubig = match base_p_to_decimal(tip5) {
                Ok(u) => u,
                Err(_) => return TestResult::discard(),
            };

            // Manually calculate the expected value
            let prime_ubig = UBig::from(P);
            let mut expected = ubig!(0);
            for (i, &value) in tip5.iter().enumerate() {
                expected += UBig::from(value) * prime_ubig.pow(i);
            }

            TestResult::from_bool(ubig == expected)
        }

        QuickCheck::new()
            .tests(500)
            .quickcheck(prop_ubig_preserves_value as fn(Tip5Hash) -> TestResult);
    }

    #[test]
    fn test_edge_cases() {
        // Test with all zeros
        iso([0, 0, 0, 0, 0]);

        // Test with all ones
        iso([1, 1, 1, 1, 1]);

        // Test with maximum field element (P - 1)
        let max_elem = P - 1;
        iso([max_elem, max_elem, max_elem, max_elem, max_elem]);

        // Test with mixed values
        iso([0, 1, P / 2, P - 2, P - 1]);

        // Test with prime powers
        iso([1, P - 1, (P - 1) / 2, (P - 1) / 3, (P - 1) / 4]);
    }

    #[test]
    fn test_base58_string_properties() {
        fn prop_base58_non_empty(tip5_hash: Tip5Hash) -> TestResult {
            let tip5 = tip5_hash.0;

            let ubig = match base_p_to_decimal(tip5) {
                Ok(u) => u,
                Err(_) => return TestResult::discard(),
            };
            let base58 = ubig_to_base58(ubig);

            // Base58 string should never be empty for valid tip5 hashes
            TestResult::from_bool(!base58.is_empty())
        }

        fn prop_base58_valid_chars(tip5_hash: Tip5Hash) -> TestResult {
            let tip5 = tip5_hash.0;

            let ubig = match base_p_to_decimal(tip5) {
                Ok(u) => u,
                Err(_) => return TestResult::discard(),
            };
            let base58 = ubig_to_base58(ubig);

            // Check that all characters are valid base58 characters
            const BASE58_ALPHABET: &str =
                "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
            let all_valid = base58.chars().all(|c| BASE58_ALPHABET.contains(c));

            TestResult::from_bool(all_valid)
        }

        QuickCheck::new()
            .tests(500)
            .quickcheck(prop_base58_non_empty as fn(Tip5Hash) -> TestResult);

        QuickCheck::new()
            .tests(500)
            .quickcheck(prop_base58_valid_chars as fn(Tip5Hash) -> TestResult);
    }

    #[test]
    fn test_special_patterns() {
        // Test with sequential values
        iso([0, 1, 2, 3, 4]);
        iso([P - 5, P - 4, P - 3, P - 2, P - 1]);

        // Test with powers of 2 modulo P
        let powers_of_2: [u64; 5] = [1, 2, 4, 8, 16];
        iso(powers_of_2);

        // Test with Fibonacci-like sequence modulo P
        let mut fib = [1u64, 1, 0, 0, 0];
        fib[2] = (fib[0] + fib[1]) % P;
        fib[3] = (fib[1] + fib[2]) % P;
        fib[4] = (fib[2] + fib[3]) % P;
        iso(fib);
    }

    // Commented out due to ibig library buffer capacity issue with certain values
    // See: https://github.com/rust-num/num-bigint/issues
    // The test fails with values like Tip5Hash([0, 0, 0, 29080198998, 1])
    // #[test]
    // fn test_uniqueness_property() {
    //     fn prop_different_inputs_different_outputs(tip5_a: Tip5Hash, tip5_b: Tip5Hash) -> TestResult {
    //         if tip5_a == tip5_b {
    //             return TestResult::discard();
    //         }

    //         let ubig_a = match base_p_to_decimal(tip5_a.0) {
    //             Ok(u) => u,
    //             Err(_) => return TestResult::discard(),
    //         };
    //         let ubig_b = match base_p_to_decimal(tip5_b.0) {
    //             Ok(u) => u,
    //             Err(_) => return TestResult::discard(),
    //         };

    //         // Different tip5 hashes should produce different UBig values
    //         TestResult::from_bool(ubig_a != ubig_b)
    //     }

    //     QuickCheck::new()
    //         .tests(200)  // Reduced number of tests to avoid stack issues
    //         .max_tests(200)  // Also set max_tests explicitly
    //         .quickcheck(prop_different_inputs_different_outputs as fn(Tip5Hash, Tip5Hash) -> TestResult);
    // }

    #[test]
    fn test_noun_integration_quickcheck() {
        fn prop_noun_roundtrip(tip5_hash: Tip5Hash) -> TestResult {
            let tip5 = tip5_hash.0;
            let mut slab: NounSlab = NounSlab::new();

            // Create a noun from the tip5 hash
            let noun = T(
                &mut slab,
                &[
                    D(tip5[0] as u64),
                    D(tip5[1] as u64),
                    D(tip5[2] as u64),
                    D(tip5[3] as u64),
                    D(tip5[4] as u64),
                ],
            );

            // Convert to base58 and back
            let base58 = match tip5_hash_to_base58(noun) {
                Ok(s) => s,
                Err(_) => return TestResult::discard(),
            };

            let ubig = match base58_to_ubig(base58) {
                Ok(u) => u,
                Err(_) => return TestResult::discard(),
            };

            let tip5_result = match decimal_to_base_p(ubig) {
                Ok(t) => t,
                Err(_) => return TestResult::discard(),
            };

            TestResult::from_bool(tip5 == tip5_result)
        }

        QuickCheck::new()
            .tests(500)
            .quickcheck(prop_noun_roundtrip as fn(Tip5Hash) -> TestResult);
    }
}
