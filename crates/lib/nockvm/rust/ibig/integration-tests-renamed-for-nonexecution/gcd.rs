use ibig::{
    ibig,
    ops::{Abs, UnsignedAbs},
    ubig, IBig, UBig,
};

#[test]
fn test_gcd_ubig() {
    // test cases (x, y, gcd(x,y))
    let test_cases = [
        // trivial cases
        (ubig!(0), ubig!(123), ubig!(123)),
        (ubig!(123), ubig!(0), ubig!(123)),
        (ubig!(1), ubig!(123), ubig!(1)),
        (ubig!(123), ubig!(1), ubig!(1)),
        (ubig!(123), ubig!(123), ubig!(123)),
        (
            ubig!(0),
            ubig!(_0x123456789123456789123456789123456789),
            ubig!(_0x123456789123456789123456789123456789),
        ),
        (
            ubig!(1),
            ubig!(_0x123456789123456789123456789123456789),
            ubig!(1),
        ),
        (
            ubig!(_0x123456789123456789123456789123456789),
            ubig!(_0x123456789123456789123456789123456789),
            ubig!(_0x123456789123456789123456789123456789),
        ),
        // small cases
        (ubig!(3), ubig!(7), ubig!(1)),
        (ubig!(8), ubig!(9), ubig!(1)),
        (ubig!(9), ubig!(8), ubig!(1)),
        (ubig!(42), ubig!(56), ubig!(14)),
        (ubig!(7966496), ubig!(314080416), ubig!(32)),
        // big cases
        (
            ubig!(0xffffffffffffffffffffffff1), // largest prime under 2^100
            ubig!(0x7ffffffffffffffffffffff8d), // largest prime under 2^99
            ubig!(1),
        ),
        (
            ubig!(0xffffffffffffffffffffffffffffff61), // largest prime under 2^128
            ubig!(0xffffffffffffffffffffffffffffff53), // second largest prime under 2^128
            ubig!(1),
        ),
        (
            ubig!(_0x3ffffffffffffffffffffffffffffffffffffd), // largest prime under 2^150
            ubig!(_0x1fffffffffffffffffffffffffffffffffffe1), // largest prime under 2^149
            ubig!(1),
        ),
        (
            ubig!(_0x123456789123456789123456789123456789),
            ubig!(_0x987654321),
            ubig!(_0x2d),
        ),
        (
            ubig!(_0x123456789123456789123456789123456789),
            ubig!(_0x987654321987654321987654321987654321),
            ubig!(_0x2d00000002d00000002d00000002d),
        ),
        (
            ubig!(_0x5a4653ca673768565b41f775d6947d55cf3813d1), // 3^100
            ubig!(_0x1000000000000000000000000000000000000000),
            ubig!(1),
        ),
    ];

    for (a, b, c) in &test_cases {
        for (a, b) in [(a, b), (b, a)] {
            assert_eq!(a.gcd(b), *c);

            let (g, x, y) = a.extended_gcd(b);
            assert_eq!(g, *c);
            assert_eq!(&x * IBig::from(a) + &y * IBig::from(b), IBig::from(g));

            assert!(x.unsigned_abs() <= *b.max(&ubig!(1)));
            assert!(y.unsigned_abs() <= *a.max(&ubig!(1)));
        }
    }

    for a in 0u8..=20 {
        for b in 0u8..=20 {
            if a == 0 && b == 0 {
                continue;
            }
            let a = UBig::from(a);
            let b = UBig::from(b);
            let (g, x, y) = a.extended_gcd(&b);
            assert_eq!(g, a.gcd(&b));
            assert_eq!(&x * IBig::from(&a) + &y * IBig::from(&b), IBig::from(g));
            assert!(x.unsigned_abs() <= b.max(ubig!(1)));
            assert!(y.unsigned_abs() <= a.max(ubig!(1)));
        }
    }
}

#[test]
#[should_panic]
fn test_gcd_ubig_0_0() {
    let _ = ubig!(0).gcd(&ubig!(0));
}

#[test]
#[should_panic]
fn test_extended_gcd_ubig_0_0() {
    let _ = ubig!(0).extended_gcd(&ubig!(0));
}

#[test]
fn test_gcd_ibig() {
    assert_eq!(ibig!(12).gcd(&ibig!(18)), ibig!(6));
    assert_eq!(ibig!(12).gcd(&ibig!(-18)), ibig!(6));
    assert_eq!(ibig!(-12).gcd(&ibig!(18)), ibig!(6));
    assert_eq!(ibig!(-12).gcd(&ibig!(-18)), ibig!(6));

    for a in -20i8..=20 {
        for b in -20i8..=20 {
            if a == 0 && b == 0 {
                continue;
            }
            let a = IBig::from(a);
            let b = IBig::from(b);
            let (g, x, y) = a.extended_gcd(&b);
            assert_eq!(g, a.gcd(&b));
            assert_eq!(&x * &a + &y * &b, g);
            assert!(x.abs() <= b.abs().max(ibig!(1)));
            assert!(y.abs() <= a.abs().max(ibig!(1)));
        }
    }
}

#[test]
#[should_panic]
fn test_gcd_ibig_0_0() {
    let _ = ibig!(0).gcd(&ibig!(0));
}

#[test]
#[should_panic]
fn test_extended_gcd_ibig_0_0() {
    let _ = ibig!(0).extended_gcd(&ibig!(0));
}
