use crate::mul::ntt::{Prime, NUM_PRIMES};

/// Maximum order of the number-theoretic transform.
///
/// 2^57 * 64 = 2^63 bits.
pub(crate) const MAX_ORDER: u32 = 57;

/// Primes to be used for the number-theoretic transform.
pub(crate) const PRIMES: [Prime; NUM_PRIMES] = [
    Prime {
        prime: 0xbe00000000000001,
        max_order_root: 0x37,
    },
    Prime {
        prime: 0xd800000000000001,
        max_order_root: 0x40,
    },
    Prime {
        prime: 0xf600000000000001,
        max_order_root: 0x1ed,
    },
];
