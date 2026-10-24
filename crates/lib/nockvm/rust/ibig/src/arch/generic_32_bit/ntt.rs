use crate::mul::ntt::{Prime, NUM_PRIMES};

/// Maximum order of the number-theoretic transform.
///
/// 2^27 * 32 = 2^32 bits.
pub(crate) const MAX_ORDER: u32 = 27;

/// Primes to be used for the number-theoretic transform.
pub(crate) const PRIMES: [Prime; NUM_PRIMES] = [
    Prime {
        prime: 0xc0000001,
        max_order_root: 0x3,
    },
    Prime {
        prime: 0xd0000001,
        max_order_root: 0x79,
    },
    Prime {
        prime: 0xe8000001,
        max_order_root: 0x23,
    },
];
