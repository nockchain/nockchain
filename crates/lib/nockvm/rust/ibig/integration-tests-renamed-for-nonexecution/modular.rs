use ibig::{ibig, modular::ModuloRing, ubig};

#[test]
fn test_modulus() {
    let ring = ModuloRing::new(&ubig!(100));
    assert_eq!(ring.modulus(), ubig!(100));

    let ring = ModuloRing::new(&ubig!(10).pow(100));
    assert_eq!(ring.modulus(), ubig!(10).pow(100));
}

#[test]
fn test_clone() {
    let ring1 = ModuloRing::new(&ubig!(100));
    let x = ring1.from(512);
    let y = x.clone();
    assert_eq!(x, y);
    let mut z = ring1.from(513);
    assert_ne!(x, z);
    z.clone_from(&x);
    assert_eq!(x, z);

    let ring2 = ModuloRing::new(&ubig!(_1000000000000000000000000000000));
    let x = ring2.from(512);
    let y = x.clone();
    assert_eq!(x, y);
    let mut z = ring2.from(513);
    assert_ne!(x, z);
    z.clone_from(&x);
    assert_eq!(x, z);

    let mut x = ring1.from(512);
    let y = ring2.from(1);
    x.clone_from(&y);
    assert_eq!(x, y);

    let ring3 = ModuloRing::new(&ubig!(10).pow(100));
    let x = ring2.from(1);
    let mut y = ring3.from(2);
    y.clone_from(&x);
    assert_eq!(x, y);
}

#[test]
fn test_convert() {
    let ring = ModuloRing::new(&ubig!(100));
    let x = ring.from(6);
    assert_eq!(x, ring.from(&ubig!(306)));
    assert_ne!(x, ring.from(&ubig!(313)));
    assert_eq!(x, ring.from(&ubig!(_18297381723918723981723981723906)));
    assert_ne!(x, ring.from(&ubig!(_18297381723918723981723981723913)));
    assert_eq!(x, ring.from(ubig!(_18297381723918723981723981723906)));
    assert_eq!(x, ring.from(ibig!(_18297381723918723981723981723906)));
    assert_eq!(x, ring.from(ibig!(-_18297381723918723981723981723994)));
    assert_eq!(x, ring.from(&ibig!(-_18297381723918723981723981723994)));
    assert_eq!(x, ring.from(106u8));
    assert_eq!(x, ring.from(106u16));
    assert_eq!(x, ring.from(1006u32));
    assert_eq!(x, ring.from(10000000006u64));
    assert_eq!(x, ring.from(1000000000000000000006u128));
    assert_eq!(x, ring.from(106usize));
    assert_eq!(x, ring.from(6i8));
    assert_eq!(x, ring.from(-94i8));
    assert_eq!(x, ring.from(-94i16));
    assert_eq!(x, ring.from(-94i32));
    assert_eq!(x, ring.from(-94i64));
    assert_eq!(x, ring.from(-94i128));
    assert_eq!(x, ring.from(-94isize));

    assert_eq!(ring.from(0), ring.from(false));
    assert_eq!(ring.from(1), ring.from(true));

    let ring = ModuloRing::new(&ubig!(
        _1000000000000000000000000000000000000000000000000000000000000
    ));
    let x = ring.from(6);
    let y = ring.from(ubig!(_333333333333333333333333333333));
    assert_eq!(
        x,
        ring.from(ubig!(
            _1000000000000000000000000000000000000000000000000000000000006
        ))
    );
    assert_eq!(
        x,
        ring.from(&ubig!(
            _1000000000000000000000000000000000000000000000000000000000006
        ))
    );
    assert_ne!(
        x,
        ring.from(ubig!(
            _1000000000000000000000000000000000000000000000000000000000007
        ))
    );
    assert_eq!(
        y,
        ring.from(ubig!(
            _7000000000000000000000000000000333333333333333333333333333333
        ))
    );
}

#[test]
fn test_negate() {
    let ring = ModuloRing::new(&ubig!(100));
    let x = ring.from(-1234);
    let y = -&x;
    assert_eq!(y.residue(), ubig!(34));
    let y = -x;
    assert_eq!(y.residue(), ubig!(34));

    let ring = ModuloRing::new(&ubig!(_1000000000000000000000000000000));
    let x = ring.from(ibig!(-_33333123456789012345678901234567890));
    let y = -&x;
    assert_eq!(y, ring.from(ubig!(_44444123456789012345678901234567890)));
    assert_eq!(y.residue(), ubig!(_123456789012345678901234567890));
    let y = -x;
    assert_eq!(y, ring.from(ubig!(_44444123456789012345678901234567890)));
}

#[test]
#[allow(clippy::eq_op)]
fn test_different_rings() {
    let ring1 = ModuloRing::new(&ubig!(100));
    let ring2 = ModuloRing::new(&ubig!(100));
    assert_eq!(ring1, ring1);
    assert_ne!(ring1, ring2);
}

#[test]
#[should_panic]
fn test_cmp_different_rings() {
    let ring1 = ModuloRing::new(&ubig!(100));
    let ring2 = ModuloRing::new(&ubig!(200));
    let x = ring1.from(5);
    let y = ring2.from(5);
    let _ = x == y;
}

#[test]
fn test_add_sub() {
    let ring1 = ModuloRing::new(&ubig!(100));
    let ring2 = ModuloRing::new(&ubig!(_1000000000000000000000000000000));
    let test_cases = [
        (ring1.from(1), ring1.from(2), ring1.from(3)),
        (ring1.from(99), ring1.from(5), ring1.from(4)),
        (ring1.from(99), ring1.from(99), ring1.from(98)),
        (
            ring2.from(ubig!(111111111111111111111111111111)),
            ring2.from(ubig!(222222222222222223333333333333)),
            ring2.from(ubig!(333333333333333334444444444444)),
        ),
        (
            ring2.from(ubig!(111111111111111111111111111111)),
            ring2.from(ubig!(888888888888888888888888888889)),
            ring2.from(ubig!(0)),
        ),
        (
            ring2.from(ubig!(999999999999999999999999999999)),
            ring2.from(ubig!(999999999999999999999999999997)),
            ring2.from(ubig!(999999999999999999999999999996)),
        ),
    ];

    let all_test_cases = test_cases
        .iter()
        .map(|(a, b, c)| (a, b, c))
        .chain(test_cases.iter().map(|(a, b, c)| (b, a, c)));

    for (a, b, c) in all_test_cases {
        assert_eq!(a + b, *c);
        assert_eq!(a.clone() + b, *c);
        assert_eq!(a + b.clone(), *c);
        assert_eq!(a.clone() + b.clone(), *c);
        let mut x = a.clone();
        x += b;
        assert_eq!(x, *c);
        let mut x = a.clone();
        x += b.clone();
        assert_eq!(x, *c);

        assert_eq!(c - a, *b);
        assert_eq!(c.clone() - a, *b);
        assert_eq!(c - a.clone(), *b);
        assert_eq!(c.clone() - a.clone(), *b);
        let mut x = c.clone();
        x -= a;
        assert_eq!(x, *b);
        let mut x = c.clone();
        x -= a.clone();
        assert_eq!(x, *b);
    }
}

#[test]
fn test_mul() {
    let ring1 = ModuloRing::new(&ubig!(100));
    let ring2 = ModuloRing::new(&ubig!(_1000000000000000000000000000000));
    let big = ubig!(10).pow(10000);
    let ring3 = ModuloRing::new(&big);
    let test_cases = [
        (ring1.from(23), ring1.from(96), ring1.from(8)),
        (
            ring2.from(ubig!(_46301564276035228370597101114)),
            ring2.from(ubig!(_170100953649249045221461413048)),
            ring2.from(ubig!(_399394418012748758198974935472)),
        ),
        (
            ring3.from(&big - ubig!(1)),
            ring3.from(&big - ubig!(1)),
            ring3.from(1),
        ),
    ];

    let all_test_cases = test_cases
        .iter()
        .map(|(a, b, c)| (a, b, c))
        .chain(test_cases.iter().map(|(a, b, c)| (b, a, c)));

    for (a, b, c) in all_test_cases {
        assert_eq!(a * b, *c);
        assert_eq!(a.clone() * b, *c);
        assert_eq!(a * b.clone(), *c);
        assert_eq!(a.clone() * b.clone(), *c);
        let mut x = a.clone();
        x *= b;
        assert_eq!(x, *c);
        let mut x = a.clone();
        x *= b.clone();
        assert_eq!(x, *c);
    }
}

#[test]
fn test_inverse() {
    let ring = ModuloRing::new(&ubig!(1));
    assert_eq!(ring.from(0).inverse(), Some(ring.from(0)));

    let ring = ModuloRing::new(&ubig!(100));
    let x = ring.from(9);
    let y = x.inverse().unwrap_or_else(|| panic!("Panicked at {}:{} (git sha: {:?})", file!(), line!(), option_env!("GIT_SHA")));
    assert_eq!(x * y, ring.from(1));

    assert!(ring.from(10).inverse().is_none());

    let ring = ModuloRing::new(&ubig!(103));
    assert_eq!(ring.from(20).inverse(), Some(ring.from(67))); // inverse is unique for prime modulus

    let ring = ModuloRing::new(&ubig!(1000000000000000000000000000000));
    let x = ring.from(ibig!(3333312345678901234567890123456789));
    let y = x.inverse().unwrap_or_else(|| panic!("Panicked at {}:{} (git sha: {:?})", file!(), line!(), option_env!("GIT_SHA")));
    assert_eq!(x * y, ring.from(1));

    assert!(ring.from(10).inverse().is_none());

    let ring = ModuloRing::new(&ubig!(1000000000000000000000000000057)); // prime
    assert_eq!(
        ring.from(123456789).inverse(),
        Some(ring.from(ubig!(951144331155413413514262063034)))
    );
}

#[test]
fn test_div() {
    let ring = ModuloRing::new(&ubig!(1));
    assert_eq!(ring.from(0) / ring.from(0), ring.from(0));

    let ring = ModuloRing::new(&ubig!(10));
    // 2 / 3 == 4
    let a = ring.from(2);
    let b = ring.from(3);
    let res = ring.from(4);
    assert_eq!(a.clone() / b.clone(), res);
    assert_eq!(a.clone() / &b, res);
    assert_eq!(&a / b.clone(), res);
    assert_eq!(&a / &b, res);

    let mut a = ring.from(2);
    a /= b.clone();
    assert_eq!(a, res);

    let mut a = ring.from(2);
    a /= &b;
    assert_eq!(a, res);
}

#[test]
#[should_panic]
fn test_add_different_rings() {
    let ring1 = ModuloRing::new(&ubig!(100));
    let ring2 = ModuloRing::new(&ubig!(200));
    let x = ring1.from(5);
    let y = ring2.from(5);
    let _ = x + y;
}

#[test]
#[should_panic]
fn test_sub_different_rings() {
    let ring1 = ModuloRing::new(&ubig!(100));
    let ring2 = ModuloRing::new(&ubig!(200));
    let x = ring1.from(5);
    let y = ring2.from(5);
    let _ = x - y;
}

#[test]
#[should_panic]
fn test_mul_different_rings() {
    let ring1 = ModuloRing::new(&ubig!(100));
    let ring2 = ModuloRing::new(&ubig!(200));
    let x = ring1.from(5);
    let y = ring2.from(5);
    let _ = x * y;
}

#[test]
#[should_panic]
fn test_div_different_rings() {
    let ring1 = ModuloRing::new(&ubig!(100));
    let ring2 = ModuloRing::new(&ubig!(200));
    let x = ring1.from(1);
    let y = ring2.from(1);
    let _ = x / y;
}

#[test]
#[should_panic]
fn test_div_by_noninvertible() {
    let ring = ModuloRing::new(&ubig!(100));
    let x = ring.from(10);
    let y = ring.from(2);
    let _ = x / y;
}

#[test]
fn test_pow() {
    let ring = ModuloRing::new(&ubig!(100));
    assert_eq!(ring.from(0).pow(&ubig!(0)), ring.from(1));
    assert_eq!(ring.from(13).pow(&ubig!(0)), ring.from(1));
    assert_eq!(ring.from(13).pow(&ubig!(1)), ring.from(13));
    assert_eq!(ring.from(13).pow(&ubig!(2)), ring.from(69));
    assert_eq!(ring.from(13).pow(&ubig!(12837918273)), ring.from(53));
    assert_eq!(
        ring.from(13)
            .pow(&((ubig!(1) << 10000) * ubig!(40) + ubig!(3))),
        ring.from(97)
    );

    let ring = ModuloRing::new(&ubig!(_1000000000000000000000000000000));
    let x = ring.from(ubig!(_658571505947767552546868380533));
    assert_eq!(x.pow(&ubig!(0)), ring.from(1));
    assert_eq!(x.pow(&ubig!(1)), x);
    assert_eq!(
        x.pow(&ubig!(_794990856522773482558337459018)),
        ring.from(ubig!(_660533815789733011052086421209))
    );

    // A Mersenne prime.
    let prime = ubig!(2).pow(4423) - ubig!(1);
    let ring = ModuloRing::new(&prime);
    // Fermat theorem: a^(p-1) = 1
    assert_eq!(ring.from(13).pow(&(prime - ubig!(1))), ring.from(1));
}

#[test]
fn test_pow_signed() {
    let ring = ModuloRing::new(&ubig!(100));
    assert_eq!(ring.from(2).pow_signed(&ibig!(10)), ring.from(24));
    assert_eq!(ring.from(3).pow_signed(&ibig!(-3)), ring.from(63));
}

#[test]
#[should_panic]
fn test_pow_signed_noninvertible() {
    let ring = ModuloRing::new(&ubig!(100));
    let _ = ring.from(2).pow_signed(&ibig!(-2));
}

#[test]
fn test_format() {
    let ring = ModuloRing::new(&ubig!(100));
    let x = ring.from(105);
    assert_eq!(format!("{}", ring), "mod 100");
    assert_eq!(format!("{}", x), "5 (mod 100)");
    assert_eq!(format!("{:?}", x), "5 (mod 100)");
    assert_eq!(format!("{:=^5}", x), "==5== (mod =100=)");
    assert_eq!(format!("{:b}", x), "101 (mod 1100100)");
    assert_eq!(format!("{:o}", x), "5 (mod 144)");
    assert_eq!(format!("{:#x}", x), "0x5 (mod 0x64)");
    assert_eq!(format!("{:X}", x), "5 (mod 64)");

    let ring = ModuloRing::new(&ubig!(_1000000000000000000000000000000));
    let x = -ring.from(1);
    assert_eq!(format!("{}", ring), "mod 1000000000000000000000000000000");
    assert_eq!(
        format!("{:?}", x),
        "999999999999999999999999999999 (mod 1000000000000000000000000000000)"
    );
    assert_eq!(
        format!("{:35}", x),
        "     999999999999999999999999999999 (mod     1000000000000000000000000000000)"
    );
    assert_eq!(format!("{:b}", x),
        "1100100111110010110010011100110100000100011001110100111011011110101000111111111111111111111111111111 (mod 1100100111110010110010011100110100000100011001110100111011011110101001000000000000000000000000000000)");
    assert_eq!(
        format!("{:#o}", x),
        "0o1447626234640431647336507777777777 (mod 0o1447626234640431647336510000000000)"
    );
    assert_eq!(
        format!("{:x}", x),
        "c9f2c9cd04674edea3fffffff (mod c9f2c9cd04674edea40000000)"
    );
    assert_eq!(
        format!("{:X}", x),
        "C9F2C9CD04674EDEA3FFFFFFF (mod C9F2C9CD04674EDEA40000000)"
    );
}
