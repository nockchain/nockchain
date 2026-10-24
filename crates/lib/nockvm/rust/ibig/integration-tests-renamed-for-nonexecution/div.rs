use ibig::{
    ibig,
    ops::{DivEuclid, DivRem, DivRemEuclid, RemEuclid},
    ubig, IBig,
};

#[test]
fn test_div_rem_ubig() {
    let test_cases = [
        (ubig!(331), ubig!(10), ubig!(33), ubig!(1)),
        (ubig!(17),
         ubig!(_0x987987123984798abbcc213789723948792138479837492837498cc),
         ubig!(0),
         ubig!(17)
        ),
        (ubig!(_0x987987123984798abbcc213789723948792138479837492837498cc),
         ubig!(_0x1234),
         ubig!(_0x86054c502f0a4e43e2d0de91f1029d251ce67bbdb88dc3edbb40),
         ubig!(_0xfcc)
        ),
        (ubig!(_0x987987123984798abbcc213789723948792138479837492837498cc),
         ubig!(_0x1000),
         ubig!(_0x987987123984798abbcc21378972394879213847983749283749),
         ubig!(_0x8cc)
        ),
        (ubig!(_0x987987123984798abbcc213789723948792138479837492837498cc),
         ubig!(_0xf234567812345678),
         ubig!(_0xa128cfb49d0d746cc0295e163c343aafbffbfa8),
         ubig!(_0x9068d997bb10520c)
        ),
        (ubig!(_0x987987123984798abbcc213789723948792138479837492837498ce),
         ubig!(_0x987987123984798abbcc213789723948792138479837492837498cc),
         ubig!(1),
         ubig!(2),
        ),
        // Special case for division (64-bit words): top 2 / top 1 overflows.
        (
         ubig!(_0xffffffffffffffffffffffffffffffff00000000000000000000000000000000),
         ubig!(_0xffffffffffffffffffffffffffffffff0000000000000001),
         ubig!(_0xffffffffffffffff),
         ubig!(_0xfffffffffffffffffffffffffffffffe0000000000000001),
        ),
        // Random 500-bit by random 250-bit.
        (
         ubig!(_0x2b8f1bb75f1ca5bf3400549a663d503d298da7f53942cd3c5c6a1bc50598d091e8ca30896413783e9b001572e28808c4dc9598bdd17ef3ce35b40e0368b60),
         ubig!(_0x3e880309f5e48d145337aae47694a74f2860db8e49665f03978f1b11665dc80),
         ubig!(_0xb254145d6f736c22ed5fca6a41f4c883a59fc32c638710758bb50fa532b31f),
         ubig!(_0x195afda8e35e347c65ed01409c73d1c820ed78a87e83cf6cfdad1a25fb357e0),
        ),
        (
         ubig!(_0x3e880309f5e48d145337aae47694a74f2860db8e49665f03978f1b11665dc80),
         ubig!(_0x2b8f1bb75f1ca5bf3400549a663d503d298da7f53942cd3c5c6a1bc50598d091e8ca30896413783e9b001572e28808c4dc9598bdd17ef3ce35b40e0368b60),
         ubig!(0),
         ubig!(_0x3e880309f5e48d145337aae47694a74f2860db8e49665f03978f1b11665dc80),
        ),
        // 3^300 - 1 by 3^150
        (
         ubig!(_0xb39cfff485a5dbf4d6aae030b91bfb0ec6bba389cd8d7f85bba3985c19c5e24e40c543a123c6e028a873e9e3874e1b4623a44be39b34e67dc5c2670),
         ubig!(_0x359ba2b98ca11d6864a331b45ae7114c01ffbdcf60cc16e692fb63c6e219),
         ubig!(_0x359ba2b98ca11d6864a331b45ae7114c01ffbdcf60cc16e692fb63c6e218),
         ubig!(_0x359ba2b98ca11d6864a331b45ae7114c01ffbdcf60cc16e692fb63c6e218),
        ),
        // 7^70-1 by 7^35
        (
         ubig!(_0x16dc8782276b9f7addf9768f33c8007ce903866a4546c1a190),
         ubig!(_0x4c8077a58a0a8cb7c24960e57),
         ubig!(_0x4c8077a58a0a8cb7c24960e56),
         ubig!(_0x4c8077a58a0a8cb7c24960e56),
        ),
        // 2^20480-1 by 2^5120-1
        (
            (ubig!(1) << 20480) - ubig!(1),
            (ubig!(1) << 5120) - ubig!(1),
            ubig!(1) + (ubig!(1) << 5120) + (ubig!(1) << 10240) + (ubig!(1) << 15360),
            ubig!(0)
        ),
        // 2^20480-1 by 2^15360-1
        (
            (ubig!(1) << 20480) - ubig!(1),
            (ubig!(1) << 15360) - ubig!(1),
            ubig!(1) << 5120,
            (ubig!(1) << 5120) - ubig!(1)
        ),
        // 2^19000-1 by 2^5000-1
        (
            (ubig!(1) << 19000) - ubig!(1),
            (ubig!(1) << 5000) - ubig!(1),
            (ubig!(1) << 14000) + (ubig!(1) << 9000) + (ubig!(1) << 4000),
            (ubig!(1) << 4000) - ubig!(1)
        ),
    ];

    for (a, b, q, r) in &test_cases {
        let qr = (q.clone(), r.clone());

        assert_eq!(a / b, *q);
        assert_eq!(a.clone() / b, *q);
        assert_eq!(a / b.clone(), *q);
        assert_eq!(a.clone() / b.clone(), *q);

        let mut x = a.clone();
        x /= b;
        assert_eq!(x, *q);

        let mut x = a.clone();
        x /= b.clone();
        assert_eq!(x, *q);

        assert_eq!(a % b, *r);
        assert_eq!(a.clone() % b, *r);
        assert_eq!(a % b.clone(), *r);
        assert_eq!(a.clone() % b.clone(), *r);

        let mut x = a.clone();
        x %= b;
        assert_eq!(x, *r);

        let mut x = a.clone();
        x %= b.clone();
        assert_eq!(x, *r);

        assert_eq!(a.div_rem(b), qr);
        assert_eq!(a.clone().div_rem(b), qr);
        assert_eq!(a.div_rem(b.clone()), qr);
        assert_eq!(a.clone().div_rem(b.clone()), qr);

        assert_eq!(a.div_euclid(b), *q);
        assert_eq!(a.clone().div_euclid(b), *q);
        assert_eq!(a.div_euclid(b.clone()), *q);
        assert_eq!(a.clone().div_euclid(b.clone()), *q);

        assert_eq!(a.rem_euclid(b), *r);
        assert_eq!(a.clone().rem_euclid(b), *r);
        assert_eq!(a.rem_euclid(b.clone()), *r);
        assert_eq!(a.clone().rem_euclid(b.clone()), *r);

        assert_eq!(a.div_rem_euclid(b), qr);
        assert_eq!(a.clone().div_rem_euclid(b), qr);
        assert_eq!(a.div_rem_euclid(b.clone()), qr);
        assert_eq!(a.clone().div_rem_euclid(b.clone()), qr);
    }
}

#[test]
#[should_panic]
fn test_divide_by_0_ubig() {
    let _ = ubig!(5) / ubig!(0);
}

#[test]
fn test_div_rem_ibig() {
    for a in -20i8..=20i8 {
        for b in -20i8..=20i8 {
            if b == 0 {
                continue;
            }

            let a_big: IBig = a.into();
            let b_big: IBig = b.into();
            let q: IBig = (a / b).into();
            let r: IBig = (a % b).into();
            let qr = (q.clone(), r.clone());

            assert_eq!(a_big.clone() / b_big.clone(), q);
            assert_eq!(&a_big / b_big.clone(), q);
            assert_eq!(a_big.clone() / &b_big, q);
            assert_eq!(&a_big / &b_big, q);

            let mut x = a_big.clone();
            x /= b_big.clone();
            assert_eq!(x, q);

            let mut x = a_big.clone();
            x /= &b_big;
            assert_eq!(x, q);

            assert_eq!(a_big.clone() % b_big.clone(), r);
            assert_eq!(&a_big % b_big.clone(), r);
            assert_eq!(a_big.clone() % &b_big, r);
            assert_eq!(&a_big % &b_big, r);

            let mut x = a_big.clone();
            x %= b_big.clone();
            assert_eq!(x, r);

            let mut x = a_big.clone();
            x %= &b_big;
            assert_eq!(x, r);

            assert_eq!(a_big.clone().div_rem(b_big.clone()), qr);
            assert_eq!((&a_big).div_rem(b_big.clone()), qr);
            assert_eq!(a_big.clone().div_rem(&b_big), qr);
            assert_eq!((&a_big).div_rem(&b_big), qr);
        }
    }
}

#[test]
fn test_div_rem_euclid_ibig() {
    for a in -20i8..=20i8 {
        for b in -20i8..=20i8 {
            if b == 0 {
                continue;
            }

            let a_big: IBig = a.into();
            let b_big: IBig = b.into();
            let q: IBig = a.div_euclid(b).into();
            let r: IBig = a.rem_euclid(b).into();
            let qr = (q.clone(), r.clone());

            assert_eq!(a_big.clone().div_euclid(b_big.clone()), q);
            assert_eq!((&a_big).div_euclid(b_big.clone()), q);
            assert_eq!(a_big.clone().div_euclid(&b_big), q);
            assert_eq!((&a_big).div_euclid(&b_big), q);

            assert_eq!(a_big.clone().rem_euclid(b_big.clone()), r);
            assert_eq!((&a_big).rem_euclid(b_big.clone()), r);
            assert_eq!(a_big.clone().rem_euclid(&b_big), r);
            assert_eq!((&a_big).rem_euclid(&b_big), r);

            assert_eq!(a_big.clone().div_rem_euclid(b_big.clone()), qr);
            assert_eq!((&a_big).div_rem_euclid(b_big.clone()), qr);
            assert_eq!(a_big.clone().div_rem_euclid(&b_big), qr);
            assert_eq!((&a_big).div_rem_euclid(&b_big), qr);
        }
    }
}

#[test]
#[should_panic]
fn test_divide_by_0_ibig() {
    let _ = ibig!(5) / ibig!(0);
}

#[test]
#[allow(clippy::op_ref)]
fn test_div_rem_ubig_unsigned() {
    assert_eq!(ubig!(23) / 10u8, ubig!(2));
    assert_eq!(ubig!(23) / &10u8, ubig!(2));
    assert_eq!(&ubig!(23) / 10u8, ubig!(2));
    assert_eq!(&ubig!(23) / &10u8, ubig!(2));
    let mut x = ubig!(23);
    x /= 10u8;
    assert_eq!(x, ubig!(2));
    let mut x = ubig!(23);
    x /= &10u8;
    assert_eq!(x, ubig!(2));

    assert_eq!(ubig!(23) % 10u8, 3u8);
    assert_eq!(ubig!(23) % &10u8, 3u8);
    assert_eq!(&ubig!(23) % 10u8, 3u8);
    assert_eq!(&ubig!(23) % &10u8, 3u8);
    let mut x = ubig!(23);
    x %= 10u8;
    assert_eq!(x, ubig!(3));
    let mut x = ubig!(23);
    x %= &10u8;
    assert_eq!(x, ubig!(3));

    assert_eq!(ubig!(23).div_rem(10u8), (ubig!(2), 3u8));
    assert_eq!(ubig!(23).div_rem(&10u8), (ubig!(2), 3u8));
    assert_eq!((&ubig!(23)).div_rem(10u8), (ubig!(2), 3u8));
    assert_eq!((&ubig!(23)).div_rem(&10u8), (ubig!(2), 3u8));
}

#[test]
fn test_div_rem_euclid_ubig_unsigned() {
    assert_eq!(ubig!(23).div_euclid(10u8), ubig!(2));
    assert_eq!(ubig!(23).div_euclid(&10u8), ubig!(2));
    assert_eq!((&ubig!(23)).div_euclid(10u8), ubig!(2));
    assert_eq!((&ubig!(23)).div_euclid(&10u8), ubig!(2));

    assert_eq!(ubig!(23).rem_euclid(10u8), 3u8);
    assert_eq!(ubig!(23).rem_euclid(&10u8), 3u8);
    assert_eq!((&ubig!(23)).rem_euclid(10u8), 3u8);
    assert_eq!((&ubig!(23)).rem_euclid(&10u8), 3u8);

    assert_eq!(ubig!(23).div_rem_euclid(10u8), (ubig!(2), 3u8));
    assert_eq!(ubig!(23).div_rem_euclid(&10u8), (ubig!(2), 3u8));
    assert_eq!((&ubig!(23)).div_rem_euclid(10u8), (ubig!(2), 3u8));
    assert_eq!((&ubig!(23)).div_rem_euclid(&10u8), (ubig!(2), 3u8));
}

#[test]
#[allow(clippy::op_ref)]
fn test_div_rem_ubig_signed() {
    assert_eq!(ubig!(23) / 10, ubig!(2));
    assert_eq!(ubig!(23) / &10, ubig!(2));
    assert_eq!(&ubig!(23) / 10, ubig!(2));
    assert_eq!(&ubig!(23) / &10, ubig!(2));
    let mut x = ubig!(23);
    x /= 10;
    assert_eq!(x, ubig!(2));
    let mut x = ubig!(23);
    x /= &10;
    assert_eq!(x, ubig!(2));

    assert_eq!(ubig!(23) % 10, 3);
    assert_eq!(ubig!(23) % &10, 3);
    assert_eq!(&ubig!(23) % 10, 3);
    assert_eq!(&ubig!(23) % &10, 3);
    let mut x = ubig!(23);
    x %= 10;
    assert_eq!(x, ubig!(3));
    let mut x = ubig!(23);
    x %= &10;
    assert_eq!(x, ubig!(3));

    assert_eq!(ubig!(23).div_rem(10), (ubig!(2), 3));
    assert_eq!(ubig!(23).div_rem(&10), (ubig!(2), 3));
    assert_eq!((&ubig!(23)).div_rem(10), (ubig!(2), 3));
    assert_eq!((&ubig!(23)).div_rem(&10), (ubig!(2), 3));

    assert_eq!(ubig!(3) / (-100), ubig!(0));
    assert_eq!(ubig!(21) % (-10), 1);
    assert_eq!(ubig!(1).div_rem(-10), (ubig!(0), 1));
}

#[test]
#[should_panic]
fn test_div_ubig_signed_overflow() {
    let _ = ubig!(100) / (-10);
}

#[test]
#[should_panic]
fn test_div_rem_ubig_signed_overflow() {
    let _ = ubig!(101).div_rem(-10);
}

#[test]
fn test_div_rem_euclid_ubig_signed() {
    assert_eq!(ubig!(23).div_euclid(10), ubig!(2));
    assert_eq!(ubig!(23).div_euclid(&10), ubig!(2));
    assert_eq!((&ubig!(23)).div_euclid(10), ubig!(2));
    assert_eq!((&ubig!(23)).div_euclid(&10), ubig!(2));

    assert_eq!(ubig!(23).rem_euclid(10), 3);
    assert_eq!(ubig!(23).rem_euclid(&10), 3);
    assert_eq!((&ubig!(23)).rem_euclid(10), 3);
    assert_eq!((&ubig!(23)).rem_euclid(&10), 3);

    assert_eq!(ubig!(23).div_rem_euclid(10), (ubig!(2), 3));
    assert_eq!(ubig!(23).div_rem_euclid(&10), (ubig!(2), 3));
    assert_eq!((&ubig!(23)).div_rem_euclid(10), (ubig!(2), 3));
    assert_eq!((&ubig!(23)).div_rem_euclid(&10), (ubig!(2), 3));

    assert_eq!(ubig!(1).div_euclid(-10), ubig!(0));
    assert_eq!(ubig!(21).rem_euclid(-10), 1);
    assert_eq!(ubig!(1).div_rem_euclid(-10), (ubig!(0), 1));
}

#[test]
#[should_panic]
fn test_div_euclid_ubig_signed_overflow() {
    let _ = ubig!(11).div_euclid(-10);
}

#[test]
#[should_panic]
fn test_div_rem_euclid_ubig_signed_overflow() {
    let _ = ubig!(11).div_rem_euclid(-10);
}

#[test]
#[allow(clippy::op_ref)]
fn test_div_rem_ibig_unsigned() {
    assert_eq!(ibig!(-23) / 10u8, ibig!(-2));
    assert_eq!(ibig!(-23) / &10u8, ibig!(-2));
    assert_eq!(&ibig!(-23) / 10u8, ibig!(-2));
    assert_eq!(&ibig!(-23) / &10u8, ibig!(-2));
    let mut x = ibig!(-23);
    x /= 10u8;
    assert_eq!(x, ibig!(-2));
    let mut x = ibig!(-23);
    x /= &10u8;
    assert_eq!(x, ibig!(-2));

    assert_eq!(ibig!(-23) % 10u8, ibig!(-3));
    assert_eq!(ibig!(-23) % &10u8, ibig!(-3));
    assert_eq!(&ibig!(-23) % 10u8, ibig!(-3));
    assert_eq!(&ibig!(-23) % &10u8, ibig!(-3));
    let mut x = ibig!(-23);
    x %= 10u8;
    assert_eq!(x, ibig!(-3));
    let mut x = ibig!(-23);
    x %= &10u8;
    assert_eq!(x, ibig!(-3));

    assert_eq!(ibig!(-23).div_rem(10u8), (ibig!(-2), ibig!(-3)));
    assert_eq!(ibig!(-23).div_rem(&10u8), (ibig!(-2), ibig!(-3)));
    assert_eq!((&ibig!(-23)).div_rem(10u8), (ibig!(-2), ibig!(-3)));
    assert_eq!((&ibig!(-23)).div_rem(&10u8), (ibig!(-2), ibig!(-3)));
}

#[test]
fn test_div_rem_euclid_ibig_unsigned() {
    assert_eq!(ibig!(-23).div_euclid(10u8), ibig!(-3));
    assert_eq!(ibig!(-23).div_euclid(&10u8), ibig!(-3));
    assert_eq!((&ibig!(-23)).div_euclid(10u8), ibig!(-3));
    assert_eq!((&ibig!(-23)).div_euclid(&10u8), ibig!(-3));

    assert_eq!(ibig!(-23).rem_euclid(10u8), 7u8);
    assert_eq!(ibig!(-23).rem_euclid(&10u8), 7u8);
    assert_eq!((&ibig!(-23)).rem_euclid(10u8), 7u8);
    assert_eq!((&ibig!(-23)).rem_euclid(&10u8), 7u8);

    assert_eq!(ibig!(-23).div_rem_euclid(10u8), (ibig!(-3), 7u8));
    assert_eq!(ibig!(-23).div_rem_euclid(&10u8), (ibig!(-3), 7u8));
    assert_eq!((&ibig!(-23)).div_rem_euclid(10u8), (ibig!(-3), 7u8));
    assert_eq!((&ibig!(-23)).div_rem_euclid(&10u8), (ibig!(-3), 7u8));
}

#[test]
#[allow(clippy::op_ref)]
fn test_div_rem_ibig_signed() {
    assert_eq!(ibig!(-23) / (-10), ibig!(2));
    assert_eq!(ibig!(-23) / &(-10), ibig!(2));
    assert_eq!(&ibig!(-23) / (-10), ibig!(2));
    assert_eq!(&ibig!(-23) / &(-10), ibig!(2));
    let mut x = ibig!(-23);
    x /= -10;
    assert_eq!(x, ibig!(2));
    let mut x = ibig!(-23);
    x /= &(-10);
    assert_eq!(x, ibig!(2));

    assert_eq!(ibig!(-23) % (-10), -3);
    assert_eq!(ibig!(-23) % &(-10), -3);
    assert_eq!(&ibig!(-23) % (-10), -3);
    assert_eq!(&ibig!(-23) % &(-10), -3);
    let mut x = ibig!(-23);
    x %= -10;
    assert_eq!(x, ibig!(-3));
    let mut x = ibig!(-23);
    x %= &(-10);
    assert_eq!(x, ibig!(-3));

    assert_eq!(ibig!(-23).div_rem(-10), (ibig!(2), -3));
    assert_eq!(ibig!(-23).div_rem(&(-10)), (ibig!(2), -3));
    assert_eq!((&ibig!(-23)).div_rem(-10), (ibig!(2), -3));
    assert_eq!((&ibig!(-23)).div_rem(&(-10)), (ibig!(2), -3));
}

#[test]
fn test_div_rem_euclid_ibig_signed() {
    assert_eq!(ibig!(-23).div_euclid(-10), ibig!(3));
    assert_eq!(ibig!(-23).div_euclid(&(-10)), ibig!(3));
    assert_eq!((&ibig!(-23)).div_euclid(-10), ibig!(3));
    assert_eq!((&ibig!(-23)).div_euclid(&(-10)), ibig!(3));

    assert_eq!(ibig!(-23).rem_euclid(-10), 7);
    assert_eq!(ibig!(-23).rem_euclid(&(-10)), 7);
    assert_eq!((&ibig!(-23)).rem_euclid(-10), 7);
    assert_eq!((&ibig!(-23)).rem_euclid(&(-10)), 7);

    assert_eq!(ibig!(-23).div_rem_euclid(-10), (ibig!(3), 7));
    assert_eq!(ibig!(-23).div_rem_euclid(&(-10)), (ibig!(3), 7));
    assert_eq!((&ibig!(-23)).div_rem_euclid(-10), (ibig!(3), 7));
    assert_eq!((&ibig!(-23)).div_rem_euclid(&(-10)), (ibig!(3), 7));
}
