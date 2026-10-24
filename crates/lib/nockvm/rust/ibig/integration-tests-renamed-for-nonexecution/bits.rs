use ibig::{
    ibig,
    ops::{AndNot, NextPowerOfTwo},
    ubig, IBig,
};

#[test]
#[allow(clippy::bool_assert_comparison)]
fn test_bit() {
    assert_eq!(ubig!(0).bit(0), false);
    assert_eq!(ubig!(0).bit(1000), false);
    assert_eq!(ubig!(0b11101).bit(0), true);
    assert_eq!(ubig!(0b11101).bit(1), false);
    assert_eq!(ubig!(0b11101).bit(4), true);
    assert_eq!(ubig!(0b11101).bit(5), false);
    assert_eq!(ubig!(0b11101).bit(1000), false);

    assert_eq!(ubig!(_0xffffffffffffffffffffffffffffffff).bit(127), true);
    assert_eq!(ubig!(_0xffffffffffffffffffffffffffffffff).bit(128), false);
}

#[test]
fn test_set_bit() {
    let mut a = ubig!(0);
    a.set_bit(3);
    assert_eq!(a, ubig!(0b1000));
    a.set_bit(129);
    assert_eq!(a, ubig!(_0x200000000000000000000000000000008));
    a.set_bit(1);
    assert_eq!(a, ubig!(_0x20000000000000000000000000000000a));
    a.set_bit(1);
    assert_eq!(a, ubig!(_0x20000000000000000000000000000000a));
    a.set_bit(127);
    assert_eq!(a, ubig!(_0x28000000000000000000000000000000a));
    a.set_bit(194);
    assert_eq!(
        a,
        ubig!(_0x400000000000000028000000000000000000000000000000a)
    );
}

#[test]
fn test_clear_bit() {
    let mut a = ubig!(_0x400000000000000028000000000000000000000000000000a);
    a.clear_bit(10000);
    assert_eq!(
        a,
        ubig!(_0x400000000000000028000000000000000000000000000000a)
    );
    a.clear_bit(194);
    assert_eq!(a, ubig!(_0x28000000000000000000000000000000a));
    a.clear_bit(1);
    assert_eq!(a, ubig!(_0x280000000000000000000000000000008));
    a.clear_bit(129);
    assert_eq!(a, ubig!(_0x80000000000000000000000000000008));
    a.clear_bit(127);
    assert_eq!(a, ubig!(0b1000));
    a.clear_bit(3);
    assert_eq!(a, ubig!(0));
}

#[test]
fn test_trailing_zeros() {
    assert_eq!(ubig!(0).trailing_zeros(), None);
    assert_eq!(ubig!(0xf0000).trailing_zeros(), Some(16));
    assert_eq!(
        ubig!(_0xfffffffffffffffffffff00000000000000000000000000000000000000000000000000)
            .trailing_zeros(),
        Some(200)
    );

    assert_eq!(ibig!(0).trailing_zeros(), None);
    assert_eq!(ibig!(0xf0000).trailing_zeros(), Some(16));
    assert_eq!(ibig!(-0xf0000).trailing_zeros(), Some(16));
}

#[test]
fn test_bit_len() {
    assert_eq!(ubig!(0).bit_len(), 0);
    assert_eq!(ubig!(0xf0000).bit_len(), 20);
    assert_eq!(
        ubig!(_0xfffffffffffffffffffff00000000000000000000000000000000000000000000000000).bit_len(),
        284
    );
}

#[test]
#[allow(clippy::bool_assert_comparison)]
fn test_is_power_of_two() {
    assert_eq!(ubig!(0).is_power_of_two(), false);
    assert_eq!(ubig!(1).is_power_of_two(), true);
    assert_eq!(ubig!(16).is_power_of_two(), true);
    assert_eq!(ubig!(17).is_power_of_two(), false);
    assert_eq!(
        ubig!(_0x4000000000000000000000000000000000000000000000).is_power_of_two(),
        true
    );
    assert_eq!(
        ubig!(_0x5000000000000000000000000000000000000000000000).is_power_of_two(),
        false
    );
    assert_eq!(
        ubig!(_0x4000000000000000000000010000000000000000000000).is_power_of_two(),
        false
    );
}

#[test]
fn test_next_power_of_two() {
    assert_eq!(ubig!(0).next_power_of_two(), ubig!(1));
    assert_eq!((&ubig!(0)).next_power_of_two(), ubig!(1));
    assert_eq!(ubig!(16).next_power_of_two(), ubig!(16));
    assert_eq!(ubig!(17).next_power_of_two(), ubig!(32));
    assert_eq!(ubig!(_0xffffffff).next_power_of_two(), ubig!(_0x100000000));
    assert_eq!(
        ubig!(_0xffffffffffffffff).next_power_of_two(),
        ubig!(_0x10000000000000000)
    );
    assert_eq!(
        ubig!(_0xffffffffffffffffffffffffffffffff).next_power_of_two(),
        ubig!(_0x100000000000000000000000000000000)
    );
    assert_eq!(
        ubig!(_0xf0000000000000000000000000000000).next_power_of_two(),
        ubig!(_0x100000000000000000000000000000000)
    );
    assert_eq!(
        ubig!(_0xffffffffffffffff0000000000000000).next_power_of_two(),
        ubig!(_0x100000000000000000000000000000000)
    );
    assert_eq!(
        ubig!(_0xffffffffffffffff0000000000000000).next_power_of_two(),
        ubig!(_0x100000000000000000000000000000000)
    );
    assert_eq!(
        ubig!(_0x100000000000000000000000000000000).next_power_of_two(),
        ubig!(_0x100000000000000000000000000000000)
    );
    assert_eq!(
        ubig!(_0x100000000000000000000000000000001).next_power_of_two(),
        ubig!(_0x200000000000000000000000000000000)
    );
    assert_eq!(
        ubig!(_0x100100000000000000000000000000000).next_power_of_two(),
        ubig!(_0x200000000000000000000000000000000)
    );
}

#[test]
fn test_and_ubig() {
    let cases = [
        (ubig!(0xf0f0), ubig!(0xff00), ubig!(0xf000)),
        (
            ubig!(_0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee),
            ubig!(0xff),
            ubig!(0xee),
        ),
        (
            ubig!(0xff),
            ubig!(_0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee),
            ubig!(0xee),
        ),
        (
            ubig!(_0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee),
            ubig!(_0xdddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd),
            ubig!(_0xcccccccccccccccccccccccccccccccc),
        ),
        (
            ubig!(_0xdddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd),
            ubig!(_0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee),
            ubig!(_0xcccccccccccccccccccccccccccccccc),
        ),
    ];

    for (a, b, c) in cases.iter() {
        assert_eq!(a & b, *c);
        assert_eq!(a.clone() & b, *c);
        assert_eq!(a & b.clone(), *c);
        assert_eq!(a.clone() & b.clone(), *c);

        {
            let mut a1 = a.clone();
            a1 &= b;
            assert_eq!(a1, *c);
        }
        {
            let mut a1 = a.clone();
            a1 &= b.clone();
            assert_eq!(a1, *c);
        }
    }
}

#[test]
fn test_or_ubig() {
    let cases = [
        (ubig!(0xf0f0), ubig!(0xff00), ubig!(0xfff0)),
        (
            ubig!(_0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee),
            ubig!(0xff),
            ubig!(_0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeff),
        ),
        (
            ubig!(0xff),
            ubig!(_0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee),
            ubig!(_0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeff),
        ),
        (
            ubig!(_0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee),
            ubig!(_0xdddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd),
            ubig!(_0xddddddddddddddddddddddddddddddddffffffffffffffffffffffffffffffff),
        ),
        (
            ubig!(_0xdddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd),
            ubig!(_0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee),
            ubig!(_0xddddddddddddddddddddddddddddddddffffffffffffffffffffffffffffffff),
        ),
    ];

    for (a, b, c) in cases.iter() {
        assert_eq!(a | b, *c);
        assert_eq!(a.clone() | b, *c);
        assert_eq!(a | b.clone(), *c);
        assert_eq!(a.clone() | b.clone(), *c);

        {
            let mut a1 = a.clone();
            a1 |= b;
            assert_eq!(a1, *c);
        }
        {
            let mut a1 = a.clone();
            a1 |= b.clone();
            assert_eq!(a1, *c);
        }
    }
}

#[test]
fn test_xor_ubig() {
    let cases = [
        (ubig!(0xf0f0), ubig!(0xff00), ubig!(0xff0)),
        (
            ubig!(_0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee),
            ubig!(0xff),
            ubig!(_0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeee11),
        ),
        (
            ubig!(0xff),
            ubig!(_0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee),
            ubig!(_0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeee11),
        ),
        (
            ubig!(_0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee),
            ubig!(_0xdddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd),
            ubig!(_0xdddddddddddddddddddddddddddddddd33333333333333333333333333333333),
        ),
        (
            ubig!(_0xdddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd),
            ubig!(_0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee),
            ubig!(_0xdddddddddddddddddddddddddddddddd33333333333333333333333333333333),
        ),
    ];

    for (a, b, c) in cases.iter() {
        assert_eq!(a ^ b, *c);
        assert_eq!(a.clone() ^ b, *c);
        assert_eq!(a ^ b.clone(), *c);
        assert_eq!(a.clone() ^ b.clone(), *c);

        {
            let mut a1 = a.clone();
            a1 ^= b;
            assert_eq!(a1, *c);
        }
        {
            let mut a1 = a.clone();
            a1 ^= b.clone();
            assert_eq!(a1, *c);
        }
    }
}

#[test]
fn test_and_not_ubig() {
    let cases = [
        (ubig!(0xf0f0), ubig!(0xff00), ubig!(0xf0)),
        (
            ubig!(_0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee),
            ubig!(0xff),
            ubig!(_0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeee00),
        ),
        (
            ubig!(0xff),
            ubig!(_0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee),
            ubig!(0x11),
        ),
        (
            ubig!(_0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee),
            ubig!(_0xdddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd),
            ubig!(_0x22222222222222222222222222222222),
        ),
        (
            ubig!(_0xdddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd),
            ubig!(_0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee),
            ubig!(_0xdddddddddddddddddddddddddddddddd11111111111111111111111111111111),
        ),
    ];

    for (a, b, c) in cases.iter() {
        assert_eq!(a.and_not(b), *c);
        assert_eq!(a.clone().and_not(b), *c);
        assert_eq!(a.and_not(b.clone()), *c);
        assert_eq!(a.clone().and_not(b.clone()), *c);
    }
}

#[test]
fn test_not_ibig() {
    for a in -20i8..=20i8 {
        let a_big: IBig = a.into();
        let res: IBig = (!a).into();

        assert_eq!(!&a_big, res);
        assert_eq!(!a_big, res);
    }
}

#[test]
fn test_and_ibig() {
    for a in -20i8..=20i8 {
        for b in -20i8..=20i8 {
            let a_big: IBig = a.into();
            let b_big: IBig = b.into();
            let res: IBig = (a & b).into();

            assert_eq!(&a_big & &b_big, res);
            assert_eq!(&a_big & b_big.clone(), res);
            assert_eq!(a_big.clone() & &b_big, res);
            assert_eq!(a_big.clone() & b_big.clone(), res);

            let mut x = a_big.clone();
            x &= &b_big;
            assert_eq!(x, res);

            let mut x = a_big.clone();
            x &= b_big.clone();
            assert_eq!(x, res);
        }
    }
}

#[test]
fn test_or_ibig() {
    for a in -20i8..=20i8 {
        for b in -20i8..=20i8 {
            let a_big: IBig = a.into();
            let b_big: IBig = b.into();
            let res: IBig = (a | b).into();

            assert_eq!(&a_big | &b_big, res);
            assert_eq!(&a_big | b_big.clone(), res);
            assert_eq!(a_big.clone() | &b_big, res);
            assert_eq!(a_big.clone() | b_big.clone(), res);

            let mut x = a_big.clone();
            x |= &b_big;
            assert_eq!(x, res);

            let mut x = a_big.clone();
            x |= b_big.clone();
            assert_eq!(x, res);
        }
    }
}

#[test]
fn test_xor_ibig() {
    for a in -20i8..=20i8 {
        for b in -20i8..=20i8 {
            let a_big: IBig = a.into();
            let b_big: IBig = b.into();
            let res: IBig = (a ^ b).into();

            assert_eq!(&a_big ^ &b_big, res);
            assert_eq!(&a_big ^ b_big.clone(), res);
            assert_eq!(a_big.clone() ^ &b_big, res);
            assert_eq!(a_big.clone() ^ b_big.clone(), res);

            let mut x = a_big.clone();
            x ^= &b_big;
            assert_eq!(x, res);

            let mut x = a_big.clone();
            x ^= b_big.clone();
            assert_eq!(x, res);
        }
    }
}

#[test]
fn test_and_not_ibig() {
    for a in -20i8..=20i8 {
        for b in -20i8..=20i8 {
            let a_big: IBig = a.into();
            let b_big: IBig = b.into();
            let res: IBig = (a & !b).into();

            assert_eq!((&a_big).and_not(&b_big), res);
            assert_eq!((&a_big).and_not(b_big.clone()), res);
            assert_eq!(a_big.clone().and_not(&b_big), res);
            assert_eq!(a_big.clone().and_not(b_big.clone()), res);
        }
    }
}

#[test]
#[allow(clippy::identity_op, clippy::op_ref)]
fn test_bit_ops_ubig_unsigned() {
    assert_eq!(ubig!(0xf0f) & 0xffu8, 0xfu8);
    assert_eq!(ubig!(0xf0f) & &0xffu8, 0xfu8);
    assert_eq!(&ubig!(0xf0f) & 0xffu8, 0xfu8);
    assert_eq!(&ubig!(0xf0f) & &0xffu8, 0xfu8);

    assert_eq!(0xffu8 & ubig!(0xf0f), 0xfu8);
    assert_eq!(0xffu8 & &ubig!(0xf0f), 0xfu8);
    assert_eq!(&0xffu8 & ubig!(0xf0f), 0xfu8);
    assert_eq!(&0xffu8 & &ubig!(0xf0f), 0xfu8);

    let mut x = ubig!(0xf0f);
    x &= 0xffu8;
    assert_eq!(x, ubig!(0xf));

    let mut x = ubig!(0xf0f);
    x &= &0xffu8;
    assert_eq!(x, ubig!(0xf));

    assert_eq!(ubig!(0xf0f) | 0xffu8, ubig!(0xfff));
    assert_eq!(ubig!(0xf0f) | &0xffu8, ubig!(0xfff));
    assert_eq!((&ubig!(0xf0f)) | 0xffu8, ubig!(0xfff));
    assert_eq!((&ubig!(0xf0f)) | &0xffu8, ubig!(0xfff));

    assert_eq!(0xffu8 | ubig!(0xf0f), ubig!(0xfff));
    assert_eq!(0xffu8 | &ubig!(0xf0f), ubig!(0xfff));
    assert_eq!(&0xffu8 | ubig!(0xf0f), ubig!(0xfff));
    assert_eq!(&0xffu8 | &ubig!(0xf0f), ubig!(0xfff));

    let mut x = ubig!(0xf0f);
    x |= 0xffu8;
    assert_eq!(x, ubig!(0xfff));

    let mut x = ubig!(0xf0f);
    x |= &0xffu8;
    assert_eq!(x, ubig!(0xfff));

    assert_eq!(ubig!(0xf0f) ^ 0xffu8, ubig!(0xff0));
    assert_eq!(ubig!(0xf0f) ^ &0xffu8, ubig!(0xff0));
    assert_eq!(&ubig!(0xf0f) ^ 0xffu8, ubig!(0xff0));
    assert_eq!(&ubig!(0xf0f) ^ &0xffu8, ubig!(0xff0));

    assert_eq!(0xffu8 ^ ubig!(0xf0f), ubig!(0xff0));
    assert_eq!(0xffu8 ^ &ubig!(0xf0f), ubig!(0xff0));
    assert_eq!(&0xffu8 ^ ubig!(0xf0f), ubig!(0xff0));
    assert_eq!(&0xffu8 ^ &ubig!(0xf0f), ubig!(0xff0));

    let mut x = ubig!(0xf0f);
    x ^= 0xffu8;
    assert_eq!(x, ubig!(0xff0));

    let mut x = ubig!(0xf0f);
    x ^= &0xffu8;
    assert_eq!(x, ubig!(0xff0));

    assert_eq!(ubig!(0xf0f).and_not(0xffu8), ubig!(0xf00));
    assert_eq!(ubig!(0xf0f).and_not(&0xffu8), ubig!(0xf00));
    assert_eq!((&ubig!(0xf0f)).and_not(0xffu8), ubig!(0xf00));
    assert_eq!((&ubig!(0xf0f)).and_not(&0xffu8), ubig!(0xf00));
}

#[test]
#[allow(clippy::identity_op, clippy::op_ref)]
fn test_bit_ops_ubig_signed() {
    assert_eq!(ubig!(0xf0f) & 0xff, ubig!(0xf));
    assert_eq!(ubig!(0xf0f) & &0xff, ubig!(0xf));
    assert_eq!(&ubig!(0xf0f) & 0xff, ubig!(0xf));
    assert_eq!(&ubig!(0xf0f) & &0xff, ubig!(0xf));

    assert_eq!(ubig!(0xf0f) & -2, ubig!(0xf0e));
    assert_eq!(ubig!(0xf0f) & &-2, ubig!(0xf0e));
    assert_eq!(&ubig!(0xf0f) & -2, ubig!(0xf0e));
    assert_eq!(&ubig!(0xf0f) & &-2, ubig!(0xf0e));

    assert_eq!(0xff & ubig!(0xf0f), ubig!(0xf));
    assert_eq!(0xff & &ubig!(0xf0f), ubig!(0xf));
    assert_eq!(&0xff & ubig!(0xf0f), ubig!(0xf));
    assert_eq!(&0xff & &ubig!(0xf0f), ubig!(0xf));

    let mut x = ubig!(0xf0f);
    x &= 0xff;
    assert_eq!(x, ubig!(0xf));

    let mut x = ubig!(0xf0f);
    x &= &0xff;
    assert_eq!(x, ubig!(0xf));

    assert_eq!(ubig!(0xf0f) | 0xff, ubig!(0xfff));
    assert_eq!(ubig!(0xf0f) | &0xff, ubig!(0xfff));
    assert_eq!(&ubig!(0xf0f) | 0xff, ubig!(0xfff));
    assert_eq!(&ubig!(0xf0f) | &0xff, ubig!(0xfff));

    assert_eq!(0xff | ubig!(0xf0f), ubig!(0xfff));
    assert_eq!(0xff | (&ubig!(0xf0f)), ubig!(0xfff));
    assert_eq!(&0xff | ubig!(0xf0f), ubig!(0xfff));
    assert_eq!(&0xff | &ubig!(0xf0f), ubig!(0xfff));

    let mut x = ubig!(0xf0f);
    x |= 0xff;
    assert_eq!(x, ubig!(0xfff));

    let mut x = ubig!(0xf0f);
    x |= &0xff;
    assert_eq!(x, ubig!(0xfff));

    assert_eq!(ubig!(0xf0f) ^ 0xff, ubig!(0xff0));
    assert_eq!(ubig!(0xf0f) ^ &0xff, ubig!(0xff0));
    assert_eq!(&ubig!(0xf0f) ^ 0xff, ubig!(0xff0));
    assert_eq!(&ubig!(0xf0f) ^ &0xff, ubig!(0xff0));

    assert_eq!(0xff ^ ubig!(0xf0f), ubig!(0xff0));
    assert_eq!(0xff ^ &ubig!(0xf0f), ubig!(0xff0));
    assert_eq!(&0xff ^ ubig!(0xf0f), ubig!(0xff0));
    assert_eq!(&0xff ^ &ubig!(0xf0f), ubig!(0xff0));

    let mut x = ubig!(0xf0f);
    x ^= 0xff;
    assert_eq!(x, ubig!(0xff0));

    let mut x = ubig!(0xf0f);
    x ^= &0xff;
    assert_eq!(x, ubig!(0xff0));

    assert_eq!(ubig!(0xf0f).and_not(0xff), ubig!(0xf00));
    assert_eq!(ubig!(0xf0f).and_not(&0xff), ubig!(0xf00));
    assert_eq!((&ubig!(0xf0f)).and_not(0xff), ubig!(0xf00));
    assert_eq!((&ubig!(0xf0f)).and_not(&0xff), ubig!(0xf00));

    assert_eq!(ubig!(0xf0f).and_not(-2), ubig!(1));
    assert_eq!(ubig!(0xf0f).and_not(&-2), ubig!(1));
    assert_eq!((&ubig!(0xf0f)).and_not(-2), ubig!(1));
    assert_eq!((&ubig!(0xf0f)).and_not(&-2), ubig!(1));
}

#[test]
#[should_panic]
fn test_ubig_or_signed_overflow() {
    let _ = ubig!(1) | -1;
}

#[test]
#[should_panic]
fn test_ubig_xor_signed_overflow() {
    let _ = ubig!(1) ^ -1;
}

#[test]
#[allow(clippy::identity_op, clippy::op_ref)]
fn test_bit_ops_ibig_primitive() {
    assert_eq!(ibig!(0xf0f) & 0xffu8, 0xfu8);
    assert_eq!(ibig!(0xf0f) & &0xffu8, 0xfu8);
    assert_eq!(&ibig!(0xf0f) & 0xffu8, 0xfu8);
    assert_eq!(&ibig!(0xf0f) & &0xffu8, 0xfu8);

    assert_eq!(0xffu8 & ibig!(0xf0f), 0xfu8);
    assert_eq!(0xffu8 & &ibig!(0xf0f), 0xfu8);
    assert_eq!(&0xffu8 & ibig!(0xf0f), 0xfu8);
    assert_eq!(&0xffu8 & &ibig!(0xf0f), 0xfu8);

    assert_eq!(ibig!(0xf0f) & 0xff, ibig!(0xf));
    assert_eq!(ibig!(0xf0f) & &0xff, ibig!(0xf));
    assert_eq!(&ibig!(0xf0f) & 0xff, ibig!(0xf));
    assert_eq!(&ibig!(0xf0f) & &0xff, ibig!(0xf));
    assert_eq!(ibig!(-1) & 0xffu8, 0xffu8);
    assert_eq!(ibig!(-1) & -1, ibig!(-1));

    let mut x = ibig!(0xf0f);
    x &= 0xff;
    assert_eq!(x, ibig!(0xf));

    let mut x = ibig!(0xf0f);
    x &= &0xff;
    assert_eq!(x, ibig!(0xf));

    assert_eq!(ibig!(0xf0f) | 0xff, ibig!(0xfff));
    assert_eq!(ibig!(0xf0f) | &0xff, ibig!(0xfff));
    assert_eq!((&ibig!(0xf0f)) | 0xff, ibig!(0xfff));
    assert_eq!((&ibig!(0xf0f)) | &0xff, ibig!(0xfff));

    assert_eq!(0xff | ibig!(0xf0f), ibig!(0xfff));
    assert_eq!(0xff | &ibig!(0xf0f), ibig!(0xfff));
    assert_eq!(&0xff | ibig!(0xf0f), ibig!(0xfff));
    assert_eq!(&0xff | &ibig!(0xf0f), ibig!(0xfff));

    assert_eq!(ibig!(17) | -1, ibig!(-1));

    let mut x = ibig!(0xf0f);
    x |= 0xff;
    assert_eq!(x, ibig!(0xfff));

    let mut x = ibig!(0xf0f);
    x |= &0xff;
    assert_eq!(x, ibig!(0xfff));

    assert_eq!(ibig!(0xf0f) ^ 0xff, ibig!(0xff0));
    assert_eq!(ibig!(0xf0f) ^ &0xff, ibig!(0xff0));
    assert_eq!(&ibig!(0xf0f) ^ 0xff, ibig!(0xff0));
    assert_eq!(&ibig!(0xf0f) ^ &0xff, ibig!(0xff0));

    assert_eq!(0xffu8 ^ ibig!(0xf0f), ibig!(0xff0));
    assert_eq!(0xffu8 ^ &ibig!(0xf0f), ibig!(0xff0));
    assert_eq!(&0xffu8 ^ ibig!(0xf0f), ibig!(0xff0));
    assert_eq!(&0xffu8 ^ &ibig!(0xf0f), ibig!(0xff0));

    assert_eq!(ibig!(-1) ^ -1, ibig!(0));

    let mut x = ibig!(0xf0f);
    x ^= 0xff;
    assert_eq!(x, ibig!(0xff0));

    let mut x = ibig!(0xf0f);
    x ^= &0xff;
    assert_eq!(x, ibig!(0xff0));

    assert_eq!(ibig!(0xf0f).and_not(0xff), ibig!(0xf00));
    assert_eq!(ibig!(0xf0f).and_not(&0xff), ibig!(0xf00));
    assert_eq!((&ibig!(0xf0f)).and_not(0xff), ibig!(0xf00));
    assert_eq!((&ibig!(0xf0f)).and_not(&0xff), ibig!(0xf00));
    assert_eq!(ibig!(-13).and_not(-1), ibig!(0));
}
