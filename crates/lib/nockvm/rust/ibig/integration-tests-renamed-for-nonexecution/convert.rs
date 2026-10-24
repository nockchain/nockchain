use ibig::{error::OutOfBoundsError, ibig, ubig, IBig, UBig};
use std::convert::TryFrom;

#[test]
fn test_from_to_le_bytes() {
    let empty: [u8; 0] = [];
    assert_eq!(UBig::from_le_bytes(&[]).to_le_bytes(), empty);
    assert_eq!(UBig::from_le_bytes(&[0; 100]).to_le_bytes(), empty);
    assert_eq!(UBig::from_le_bytes(&[1, 2, 3, 0]).to_le_bytes(), [1, 2, 3]);
    let bytes = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17];
    assert_eq!(UBig::from_le_bytes(&bytes).to_le_bytes(), bytes);
}

#[test]
fn test_from_to_be_bytes() {
    let empty: [u8; 0] = [];
    assert_eq!(UBig::from_be_bytes(&[]).to_be_bytes(), empty);
    assert_eq!(UBig::from_be_bytes(&[0; 100]).to_be_bytes(), empty);
    assert_eq!(UBig::from_be_bytes(&[0, 1, 2, 3]).to_be_bytes(), [1, 2, 3]);
    let bytes = [100, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17];
    assert_eq!(UBig::from_be_bytes(&bytes).to_be_bytes(), bytes);
}

#[test]
fn test_ubig_from_unsigned() {
    assert_eq!(UBig::from(0xf1u8), UBig::from_be_bytes(&[0xf1]));
    assert_eq!(UBig::from(0xf123u16), UBig::from_be_bytes(&[0xf1, 0x23]));
    assert_eq!(
        UBig::from(0xf1234567u32),
        UBig::from_be_bytes(&[0xf1, 0x23, 0x45, 0x67])
    );
    assert_eq!(
        UBig::from(0xf123456701234567u64),
        UBig::from_be_bytes(&[0xf1, 0x23, 0x45, 0x67, 0x01, 0x23, 0x45, 0x67])
    );
    assert_eq!(
        UBig::from(0xf1234567012345670123456701234567u128),
        UBig::from_be_bytes(&[
            0xf1, 0x23, 0x45, 0x67, 0x01, 0x23, 0x45, 0x67, 0x01, 0x23, 0x45, 0x67, 0x01, 0x23,
            0x45, 0x67
        ])
    );

    assert_eq!(UBig::from(5u128), UBig::from_be_bytes(&[5]));
    assert_eq!(UBig::from(5usize), UBig::from_be_bytes(&[5]));
}

#[test]
fn test_ubig_from_bool() {
    assert_eq!(UBig::from(false), UBig::from(0u8));
    assert_eq!(UBig::from(true), UBig::from(1u8));
}

#[test]
fn test_ubig_from_signed() {
    assert!(UBig::try_from(-5i32).is_err());
    assert_eq!(UBig::try_from(5i32), Ok(UBig::from(5u32)));
    assert_eq!(UBig::try_from(5i128 << 120), Ok(UBig::from(5u128 << 120)));
}

#[test]
fn test_ubig_to_unsigned() {
    assert_eq!(u8::try_from(UBig::from(0xeeu8)), Ok(0xeeu8));
    assert!(u8::try_from(UBig::from(0x123u16)).is_err());

    assert_eq!(u16::try_from(UBig::from(0x1234u16)), Ok(0x1234u16));
    assert!(u16::try_from(UBig::from(0x12345u32)).is_err());

    assert_eq!(u32::try_from(UBig::from(0xf1234567u32)), Ok(0xf1234567u32));
    assert!(u32::try_from(UBig::from(0x101234567u64)).is_err());

    assert_eq!(
        u64::try_from(UBig::from(0xf123456789abcdefu64)),
        Ok(0xf123456789abcdefu64)
    );
    assert!(u64::try_from(UBig::from(0x10123456789abcdefu128)).is_err());

    assert_eq!(
        u128::try_from(UBig::from(0xf123456789abcdef0123456789abcdefu128)),
        Ok(0xf123456789abcdef0123456789abcdefu128)
    );

    let big = UBig::from_be_bytes(&[1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);
    assert!(u8::try_from(&big).is_err());
    assert!(u128::try_from(&big).is_err());

    assert_eq!(usize::try_from(UBig::from(5u8)), Ok(5usize));
}

#[test]
fn test_ubig_to_signed() {
    assert_eq!(i8::try_from(UBig::from(0x7eu8)), Ok(0x7ei8));
    assert!(i8::try_from(UBig::from(0xeeu8)).is_err());
    assert!(i8::try_from(UBig::from(0x100u16)).is_err());

    assert_eq!(i16::try_from(&UBig::from(0x1234u16)), Ok(0x1234i16));
    assert!(i16::try_from(UBig::from(0x8234u32)).is_err());

    assert_eq!(i32::try_from(UBig::from(0x61234567u32)), Ok(0x61234567i32));
    assert!(i32::try_from(UBig::from(0x91234567u32)).is_err());

    assert_eq!(
        i64::try_from(UBig::from(0x3123456789abcdefu64)),
        Ok(0x3123456789abcdefi64)
    );
    assert!(i64::try_from(UBig::from(0xf123456789abcdefu64)).is_err());

    assert_eq!(
        i128::try_from(UBig::from(0x6123456789abcdef0123456789abcdefu128)),
        Ok(0x6123456789abcdef0123456789abcdefi128)
    );
    assert!(i128::try_from(UBig::from(0xf123456789abcdef0123456789abcdefu128)).is_err());

    let big = UBig::from_be_bytes(&[1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);
    assert!(i8::try_from(&big).is_err());
    assert!(i128::try_from(&big).is_err());

    assert_eq!(isize::try_from(UBig::from(5u8)), Ok(5isize));
}

#[test]
fn test_ibig_from_unsigned() {
    assert_eq!(IBig::from(0u32), IBig::from(UBig::from(0u32)));
    assert_eq!(IBig::from(100u32), IBig::from(UBig::from(100u32)));
}

#[test]
fn test_ibig_from_bool() {
    assert_eq!(IBig::from(false), IBig::from(0u8));
    assert_eq!(IBig::from(true), IBig::from(1u8));
}

#[test]
fn test_ibig_from_signed() {
    assert_eq!(IBig::from(0i32), IBig::from(UBig::from(0u32)));
    assert_eq!(IBig::from(100i32), IBig::from(UBig::from(100u32)));
    assert_eq!(IBig::from(-0xeei32).in_radix(16).to_string(), "-ee");
}

#[test]
fn test_ibig_to_unsigned() {
    assert_eq!(u8::try_from(IBig::from(0i32)), Ok(0u8));
    assert_eq!(u8::try_from(IBig::from(0x7fi32)), Ok(0x7fu8));
    assert_eq!(u8::try_from(&IBig::from(0xffi32)), Ok(0xffu8));
    assert!(u8::try_from(IBig::from(0x100i32)).is_err());
    assert!(u8::try_from(IBig::from(-1i32)).is_err());
    assert!(u8::try_from(IBig::from(-0x7fi32)).is_err());
    assert!(u8::try_from(IBig::from(-0x80i32)).is_err());
}

#[test]
fn test_ibig_to_signed() {
    assert_eq!(i8::try_from(IBig::from(0i32)), Ok(0i8));
    assert_eq!(i8::try_from(&IBig::from(0x7fi32)), Ok(0x7fi8));
    assert!(i8::try_from(IBig::from(0x80i32)).is_err());
    assert!(i8::try_from(IBig::from(0xffi32)).is_err());
    assert!(i8::try_from(IBig::from(0x100i32)).is_err());
    assert_eq!(i8::try_from(IBig::from(-1i32)), Ok(-1i8));
    assert_eq!(i8::try_from(IBig::from(-0x7fi32)), Ok(-0x7fi8));
    assert_eq!(i8::try_from(IBig::from(-0x80i32)), Ok(-0x80i8));
    assert!(i8::try_from(IBig::from(-0x81i32)).is_err());
    assert!(i8::try_from(IBig::from(-0x100i32)).is_err());
}

#[test]
fn test_ubig_to_ibig() {
    assert_eq!(IBig::from(UBig::from(0u32)), IBig::from(0i32));
    assert_eq!(IBig::from(UBig::from(100u32)), IBig::from(100i32));
    assert_eq!(IBig::from(&UBig::from(100u32)), IBig::from(100i32));
}

#[test]
fn test_ibig_to_ubig() {
    assert_eq!(UBig::try_from(IBig::from(0i32)), Ok(UBig::from(0u32)));
    assert_eq!(UBig::try_from(IBig::from(1000i32)), Ok(UBig::from(1000u32)));
    assert_eq!(
        UBig::try_from(&IBig::from(1000i32)),
        Ok(UBig::from(1000u32))
    );
    assert!(UBig::try_from(IBig::from(-1000i32)).is_err());
    assert!(UBig::try_from(&IBig::from(-1000i32)).is_err());
}

#[test]
fn test_default() {
    assert_eq!(UBig::default(), ubig!(0));
    assert_eq!(IBig::default(), ibig!(0));
}

#[test]
fn test_display_out_of_bounds_error() {
    assert_eq!(OutOfBoundsError.to_string(), "number out of bounds");
}

#[test]
#[allow(clippy::float_cmp)]
fn test_to_f32() {
    assert_eq!(ubig!(0).to_f32(), 0.0f32);
    assert_eq!(ubig!(7).to_f32(), 7.0f32);
    // 2^24 - 1 is still exactly representable
    assert_eq!(ubig!(0xffffff).to_f32(), 16777215.0f32);
    assert_eq!(ubig!(0x1000000).to_f32(), 16777216.0f32);
    // Now round to even should begin.
    assert_eq!(ubig!(0x1000001).to_f32(), 16777216.0f32);
    assert_eq!(ubig!(0x1000002).to_f32(), 16777218.0f32);
    assert_eq!(ubig!(0x1000003).to_f32(), 16777220.0f32);
    assert_eq!(ubig!(0x1000004).to_f32(), 16777220.0f32);
    assert_eq!(ubig!(0x1000005).to_f32(), 16777220.0f32);

    for i in 10..80 {
        assert_eq!(
            (ubig!(0xfff3330) << i).to_f32(),
            (0xfff3330 as f32) * (i as f32).exp2()
        );
        assert_eq!(
            (ubig!(0xfff3331) << i).to_f32(),
            (0xfff3330 as f32) * (i as f32).exp2()
        );
        assert_eq!(
            (ubig!(0xfff3337) << i).to_f32(),
            (0xfff3330 as f32) * (i as f32).exp2()
        );
        assert_eq!(
            (ubig!(0xfff3338) << i).to_f32(),
            (0xfff3340 as f32) * (i as f32).exp2()
        );
        assert_eq!(
            ((ubig!(0xfff3338) << i) + ubig!(1)).to_f32(),
            (0xfff3340 as f32) * (i as f32).exp2()
        );
        assert_eq!(
            (ubig!(0xfff3339) << i).to_f32(),
            (0xfff3340 as f32) * (i as f32).exp2()
        );
        assert_eq!(
            (ubig!(0xfff3347) << i).to_f32(),
            (0xfff3340 as f32) * (i as f32).exp2()
        );
        assert_eq!(
            (ubig!(0xfff3348) << i).to_f32(),
            (0xfff3340 as f32) * (i as f32).exp2()
        );
        assert_eq!(
            ((ubig!(0xfff3348) << i) + ubig!(1)).to_f32(),
            (0xfff3350 as f32) * (i as f32).exp2()
        );
        assert_eq!(
            (ubig!(0xfff3349) << i).to_f32(),
            (0xfff3350 as f32) * (i as f32).exp2()
        );
    }

    assert!((ubig!(0xffffff7) << 100).to_f32() < f32::INFINITY);
    assert!((ubig!(0xffffff8) << 100).to_f32() == f32::INFINITY);
    assert!((ubig!(1) << 1000).to_f32() == f32::INFINITY);

    assert_eq!(ibig!(0).to_f32(), 0.0f32);
    assert_eq!(ibig!(7).to_f32(), 7.0f32);
    assert_eq!(ibig!(-7).to_f32(), -7.0f32);
    assert!((ibig!(-0xffffff7) << 100).to_f32() > -f32::INFINITY);
    assert!((ibig!(-0xffffff8) << 100).to_f32() == -f32::INFINITY);
}

#[test]
#[allow(clippy::float_cmp)]
fn test_to_f64() {
    assert_eq!(ubig!(0).to_f64(), 0.0f64);
    assert_eq!(ubig!(7).to_f64(), 7.0f64);
    // 2^53 - 1 is still exactly representable
    assert_eq!(ubig!(0x1fffffffffffff).to_f64(), 9007199254740991.0f64);
    assert_eq!(ubig!(0x20000000000000).to_f64(), 9007199254740992.0f64);
    // Now round to even should begin.
    assert_eq!(ubig!(0x20000000000001).to_f64(), 9007199254740992.0f64);
    assert_eq!(ubig!(0x20000000000002).to_f64(), 9007199254740994.0f64);
    assert_eq!(ubig!(0x20000000000003).to_f64(), 9007199254740996.0f64);
    assert_eq!(ubig!(0x20000000000004).to_f64(), 9007199254740996.0f64);
    assert_eq!(ubig!(0x20000000000005).to_f64(), 9007199254740996.0f64);

    for i in 10..500 {
        assert_eq!(
            (ubig!(0x1ffffffffff3330) << i).to_f64(),
            (0x1ffffffffff3330u64 as f64) * (i as f64).exp2()
        );
        assert_eq!(
            (ubig!(0x1ffffffffff3331) << i).to_f64(),
            (0x1ffffffffff3330u64 as f64) * (i as f64).exp2()
        );
        assert_eq!(
            (ubig!(0x1ffffffffff3337) << i).to_f64(),
            (0x1ffffffffff3330u64 as f64) * (i as f64).exp2()
        );
        assert_eq!(
            (ubig!(0x1ffffffffff3338) << i).to_f64(),
            (0x1ffffffffff3340u64 as f64) * (i as f64).exp2()
        );
        assert_eq!(
            ((ubig!(0x1ffffffffff3338) << i) + ubig!(1)).to_f64(),
            (0x1ffffffffff3340u64 as f64) * (i as f64).exp2()
        );
        assert_eq!(
            (ubig!(0x1ffffffffff3339) << i).to_f64(),
            (0x1ffffffffff3340u64 as f64) * (i as f64).exp2()
        );
        assert_eq!(
            (ubig!(0x1ffffffffff3347) << i).to_f64(),
            (0x1ffffffffff3340u64 as f64) * (i as f64).exp2()
        );
        assert_eq!(
            (ubig!(0x1ffffffffff3348) << i).to_f64(),
            (0x1ffffffffff3340u64 as f64) * (i as f64).exp2()
        );
        assert_eq!(
            ((ubig!(0x1ffffffffff3348) << i) + ubig!(1)).to_f64(),
            (0x1ffffffffff3350u64 as f64) * (i as f64).exp2()
        );
        assert_eq!(
            (ubig!(0x1ffffffffff3349) << i).to_f64(),
            (0x1ffffffffff3350u64 as f64) * (i as f64).exp2()
        );
    }

    assert!((ubig!(0x1fffffffffffff7) << 967).to_f64() < f64::INFINITY);
    assert!((ubig!(0x1fffffffffffff8) << 967).to_f64() == f64::INFINITY);
    assert!((ubig!(1) << 10000).to_f64() == f64::INFINITY);

    assert_eq!(ibig!(0).to_f64(), 0.0f64);
    assert_eq!(ibig!(7).to_f64(), 7.0f64);
    assert_eq!(ibig!(-7).to_f64(), -7.0f64);
    assert!((ibig!(-0x1fffffffffffff7) << 967).to_f64() > -f64::INFINITY);
    assert!((ibig!(-0x1fffffffffffff8) << 967).to_f64() == -f64::INFINITY);
}
