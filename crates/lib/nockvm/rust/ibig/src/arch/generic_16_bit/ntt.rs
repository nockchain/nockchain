use crate::mul::ntt::{Prime, NUM_PRIMES};

/// Maximum order of the number-theoretic transform.
///
/// 2^12 * 16 = 2^16 bits.
pub(crate) const MAX_ORDER: u32 = 12;

/// Primes to be used for the number-theoretic transform.
pub(crate) const PRIMES: [Prime; NUM_PRIMES] = [
    Prime {
        prime: 0x3001,
        max_order_root: 0x29,
    },
    Prime {
        prime: 0xa001,
        max_order_root: 0x1c,
    },
    Prime {
        prime: 0xf001,
        max_order_root: 0x13,
    },
];
