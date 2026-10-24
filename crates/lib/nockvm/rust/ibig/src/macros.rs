//! Macros for big integer literals.

/// Create a [UBig](crate::UBig) value.
///
/// Usually just pass use a numeric literal. This works for bases 2, 8, 10 or 16 using standard
/// prefixes:
/// ```
/// # use ibig::ubig;
/// let a = ubig!(100);
/// let b = ubig!(0b101);
/// let c = ubig!(0o202);
/// let d = ubig!(0x2ff);
/// ```
///
/// For an arbitrary base, add `base N`:
/// ```
/// # use ibig::ubig;
/// let e = ubig!(a3gp1 base 32);
/// ```
///
/// If the sequence of digits is not a valid Rust literal or identifier, put an underscore before
/// the digits. This may be necessary when:
/// * There are a lot of digits. Rust will fail to compile without an underscore if the number
///   wouldn't fit in `u128`.
/// * The first digit is decimal, but not all digits are decimal.
/// ```
/// # use ibig::ubig;
/// let f = ubig!(_314159265358979323846264338327950288419716939937);
/// let g = ubig!(_0b102 base 32);
/// let h = ubig!(b102 base 32);
/// assert_eq!(g, h);
/// let i = ubig!(_100ef base 32);
/// ```
#[macro_export]
macro_rules! ubig {
    ($val:ident) => {{
        let s = ::core::stringify!($val);
        let s = ::core::option::Option::unwrap_or(::core::primitive::str::strip_prefix(s, "_"), s);
        ::core::result::Result::expect(
            $crate::UBig::from_str_with_radix_prefix(s),
            "invalid number",
        )
    }};
    ($val:ident base $radix:literal) => {{
        let s = ::core::stringify!($val);
        let s = ::core::option::Option::unwrap_or(::core::primitive::str::strip_prefix(s, "_"), s);
        ::core::result::Result::expect($crate::UBig::from_str_radix(s, $radix), "invalid number")
    }};
    ($val:literal) => {{
        let val: ::core::primitive::u128 = $val;
        <$crate::UBig as ::core::convert::From<::core::primitive::u128>>::from(val)
    }};
    ($val:literal base $radix:literal) => {{
        let s = ::core::stringify!($val);
        let s = ::core::option::Option::unwrap_or(::core::primitive::str::strip_prefix(s, "_"), s);
        ::core::result::Result::expect($crate::UBig::from_str_radix(s, $radix), "invalid number")
    }};
}

/// Create an [IBig](crate::IBig) value.
///
/// Usually just pass use a numeric literal. This works for bases 2, 8, 10 or 16 using standard
/// prefixes:
/// ```
/// # use ibig::ibig;
/// let a = ibig!(100);
/// let b = ibig!(-0b101);
/// let c = ibig!(0o202);
/// let d = ibig!(-0x2ff);
/// ```
///
/// For an arbitrary base, add `base N`:
/// ```
/// # use ibig::ibig;
/// let e = ibig!(-a3gp1 base 32);
/// ```
///
/// If the sequence of digits is not a valid Rust literal or identifier, put an underscore before
/// the digits. This may be necessary when:
/// * There are a lot of digits. Rust will fail to compile without an underscore if the number
///   wouldn't fit in `u128`.
/// * The first digit is decimal, but not all digits are decimal.
/// ```
/// # use ibig::ibig;
/// let f = ibig!(-_314159265358979323846264338327950288419716939937);
/// let g = ibig!(_0b102 base 32);
/// let h = ibig!(b102 base 32);
/// assert_eq!(g, h);
/// let i = ibig!(-_100ef base 32);
/// ```
#[macro_export]
macro_rules! ibig {
    (- $val:ident) => {
        - <$crate::IBig as ::core::convert::From<$crate::UBig>>::from($crate::ubig!($val))
    };
    (- $val:ident base $radix:literal) => {
        - <$crate::IBig as ::core::convert::From<$crate::UBig>>::from(
            $crate::ubig!($val base $radix)
        )
    };
    (- $val:literal) => {
        - <$crate::IBig as ::core::convert::From<$crate::UBig>>::from($crate::ubig!($val))
    };
    (- $val:literal base $radix:literal) => {
        - <$crate::IBig as ::core::convert::From<$crate::UBig>>::from(
            $crate::ubig!($val base $radix)
        )
    };
    ($val:ident) => {
        <$crate::IBig as ::core::convert::From<$crate::UBig>>::from($crate::ubig!($val))
    };
    ($val:ident base $radix:literal) => {
        <$crate::IBig as ::core::convert::From<$crate::UBig>>::from(
            $crate::ubig!($val base $radix)
        )
    };
    ($val:literal) => {
        <$crate::IBig as ::core::convert::From<$crate::UBig>>::from($crate::ubig!($val))
    };
    ($val:literal base $radix:literal) => {
        <$crate::IBig as ::core::convert::From<$crate::UBig>>::from(
            $crate::ubig!($val base $radix)
        )
    };
}
