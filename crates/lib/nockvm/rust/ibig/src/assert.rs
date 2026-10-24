#[inline]
pub(crate) const fn assert_in_const_fn(val: bool) {
    [(); 1][!val as usize]
}

macro_rules! debug_assert_in_const_fn {
    ($val:expr) => {
        #[cfg(debug_assertions)]
        crate::assert::assert_in_const_fn($val);
    };
}

pub(crate) use debug_assert_in_const_fn;
