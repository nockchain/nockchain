//! Architecture dependent functionality.

pub(crate) use arch_impl::{add, digits, ntt, word};
use cfg_if::cfg_if;

// Architecture choice. The logic works like this:
// 1. If the configuration option force_bits is set to 16, 32 or 64, use generic_<n>_bit.
// 2. Otherwise if target_arch is known, select that architecture.
// 3. Otherwise target_pointer_width is 16 or 32, use generic_<n>_bit.
// 4. Otherwise, use generic_64_bit.
cfg_if! {
    // Step 1. Check force_bits.
    if #[cfg(force_bits = "16")] {
        #[path = "generic_16_bit/mod.rs"]
        mod arch_impl;
    }
    else if #[cfg(force_bits = "32")] {
        #[path = "generic_32_bit/mod.rs"]
        mod arch_impl;
    }
    else if #[cfg(force_bits = "64")] {
        #[path = "generic_64_bit/mod.rs"]
        mod arch_impl;
    }
    // Step 2. Specific architectures.
    else if #[cfg(target_arch = "x86")] {
        #[path = "x86/mod.rs"]
        mod arch_impl;
    }
    else if #[cfg(target_arch = "x86_64")] {
        #[path = "x86_64/mod.rs"]
        mod arch_impl;
    }
    else if #[cfg(any(
            target_arch = "arm",
            target_arch = "mips",
            target_arch = "powerpc",
            target_arch = "sparc",
            target_arch = "wasm32"))] {
        #[path = "generic_32_bit/mod.rs"]
        mod arch_impl;
    }
    else if #[cfg(any(
            target_arch = "aarch64",
            target_arch = "mips64",
            target_arch = "powerpc64"))] {
        #[path = "generic_64_bit/mod.rs"]
        mod arch_impl;
    }
    // Step 3. target_pointer_width 16 or 32.
    else if #[cfg(target_pointer_width = "16")] {
        #[path = "generic_16_bit/mod.rs"]
        mod arch_impl;
    }
    else if #[cfg(target_pointer_width = "32")] {
        #[path = "generic_32_bit/mod.rs"]
        mod arch_impl;
    }
    // Step 4. Fall back on generic_64_bit.
    else {
        #[path = "generic_64_bit/mod.rs"]
        mod arch_impl;
    }
}
