use core::fmt::Debug;
use core::ops::{Add, AddAssign, Sub, SubAssign};
use ibig::{ibig, ubig};

/// Test a + b = c in various ways.
fn test_add_sub<'a, T>(a: &'a T, b: &'a T, c: &'a T)
where
    T: Add<T, Output = T>,
    T: Add<&'a T, Output = T>,
    &'a T: Add<T, Output = T>,
    &'a T: Add<&'a T, Output = T>,
    T: AddAssign<T>,
    T: AddAssign<&'a T>,
    T: Sub<T, Output = T>,
    T: Sub<&'a T, Output = T>,
    &'a T: Sub<T, Output = T>,
    &'a T: Sub<&'a T, Output = T>,
    T: SubAssign<T>,
    T: SubAssign<&'a T>,
    T: Clone,
    T: Debug,
    T: Eq,
{
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

// FIXME: ibig makes invalid assumptions about how Vec works, we need to audit this.
#[test]
#[ignore]
fn test_add_sub_ubig() {
    let test_cases = [
        (ubig!(3), ubig!(4), ubig!(7)),
        (
            ubig!(0xffffffffffffffff),
            ubig!(1),
            ubig!(0x10000000000000000),
        ),
        (
            ubig!(0x10000000000000003),
            ubig!(4),
            ubig!(0x10000000000000007),
        ),
        (
            ubig!(0xeeeeeeeeeeeeeeeeffffffffffffffff),
            ubig!(1),
            ubig!(0xeeeeeeeeeeeeeeef0000000000000000),
        ),
        (
            ubig!(0xeeeeeeeeeeeeeeeeffffffffffffffff),
            ubig!(1),
            ubig!(0xeeeeeeeeeeeeeeef0000000000000000),
        ),
        (
            ubig!(0xffffffffffffffffffffffffffffffff),
            ubig!(2),
            ubig!(_0x100000000000000000000000000000001),
        ),
        (
            ubig!(0x88888888888888888888888888888888),
            ubig!(0x88888888888888888888888888888888),
            ubig!(_0x111111111111111111111111111111110),
        ),
        (
            ubig!(_0x888888888888888888888888888888888888888888888888),
            ubig!(0x88888888888888888888888888888888),
            ubig!(_0x888888888888888911111111111111111111111111111110),
        ),
        (
            ubig!(_0x888888888888888888888888888888888888888888888888),
            ubig!(0),
            ubig!(_0x888888888888888888888888888888888888888888888888),
        ),
    ];

    for (a, b, c) in &test_cases {
        test_add_sub(a, b, c);
        test_add_sub(b, a, c);
    }
}

#[test]
#[should_panic]
fn test_sub_ubig_overflow() {
    let _ = ubig!(3) - ubig!(4);
}

// FIXME: ibig makes invalid assumptions about how Vec works, we need to audit this.
#[test]
#[ignore]
fn test_add_sub_ibig() {
    let test_cases = [
        (ibig!(3), ibig!(4), ibig!(7)),
        (ibig!(3), ibig!(-4), ibig!(-1)),
        (ibig!(-3), ibig!(4), ibig!(1)),
        (ibig!(-3), ibig!(-4), ibig!(-7)),
        (
            ibig!(0x10000000000000000),
            ibig!(-4),
            ibig!(0xfffffffffffffffc),
        ),
        (
            ibig!(0x10000000000000000),
            ibig!(_0x200000000000000000000000000000000),
            ibig!(_0x200000000000000010000000000000000),
        ),
        (
            ibig!(-_0x200000000000000010000000000000000),
            ibig!(_0x200000000000000000000000000000000),
            ibig!(-0x10000000000000000),
        ),
        (
            ibig!(_0x200000000000000010000000000000000),
            ibig!(-_0x200000000000000000000000000000000),
            ibig!(0x10000000000000000),
        ),
    ];

    for (a, b, c) in &test_cases {
        test_add_sub(a, b, c);
        test_add_sub(b, a, c);
    }
}

#[test]
#[allow(clippy::op_ref)]
fn test_add_sub_ubig_primitive() {
    assert_eq!(ubig!(3) + 7u16, ubig!(10));
    assert_eq!(ubig!(3) + &7u16, ubig!(10));
    assert_eq!(&ubig!(3) + 7u16, ubig!(10));
    assert_eq!(&ubig!(3) + &7u16, ubig!(10));
    assert_eq!(7u16 + ubig!(3), ubig!(10));
    assert_eq!(7u16 + &ubig!(3), ubig!(10));
    assert_eq!(&7u16 + ubig!(3), ubig!(10));
    assert_eq!(&7u16 + &ubig!(3), ubig!(10));
    let mut x = ubig!(3);
    x += 3u8;
    x += &3u8;
    assert_eq!(x, ubig!(9));

    assert_eq!(ubig!(7) - 5u16, ubig!(2));
    assert_eq!(ubig!(7) - &5u16, ubig!(2));
    assert_eq!(&ubig!(7) - 5u16, ubig!(2));
    assert_eq!(&ubig!(7) - &5u16, ubig!(2));
    let mut x = ubig!(10);
    x -= 1u8;
    x -= &2u8;
    assert_eq!(x, ubig!(7));

    assert_eq!(ubig!(3) + (-1), ubig!(2));
    assert_eq!(ubig!(3) + &(-1), ubig!(2));
    assert_eq!(&ubig!(3) + (-1), ubig!(2));
    assert_eq!(&ubig!(3) + &(-1), ubig!(2));
    assert_eq!((-1) + ubig!(3), ubig!(2));
    assert_eq!((-1) + &ubig!(3), ubig!(2));
    assert_eq!(&(-1) + ubig!(3), ubig!(2));
    assert_eq!(&(-1) + &ubig!(3), ubig!(2));
    let mut x = ubig!(3);
    x += -1;
    x += &2;
    assert_eq!(x, ubig!(4));

    assert_eq!(ubig!(3) - (-1), ubig!(4));
    assert_eq!(ubig!(3) - &(-1), ubig!(4));
    assert_eq!(&ubig!(3) - (-1), ubig!(4));
    assert_eq!(&ubig!(3) - &(-1), ubig!(4));
    let mut x = ubig!(3);
    x -= -1;
    x -= &2;
    assert_eq!(x, ubig!(2));
}

#[test]
#[should_panic]
fn test_add_ubig_primitive_overflow() {
    let _ = ubig!(3) + (-5i16);
}

#[test]
#[should_panic]
fn test_sub_ubig_primitive_overflow() {
    let _ = ubig!(3) - 5u16;
}

#[test]
#[allow(clippy::op_ref)]
fn test_add_sub_ibig_primitive() {
    assert_eq!(ibig!(-3) + 7u16, ibig!(4));
    assert_eq!(ibig!(-3) + &7u16, ibig!(4));
    assert_eq!(&ibig!(-3) + 7u16, ibig!(4));
    assert_eq!(&ibig!(-3) + &7u16, ibig!(4));
    assert_eq!(7u16 + ibig!(-3), ibig!(4));
    assert_eq!(7u16 + &ibig!(-3), ibig!(4));
    assert_eq!(&7u16 + ibig!(-3), ibig!(4));
    assert_eq!(&7u16 + &ibig!(-3), ibig!(4));
    let mut x = ibig!(-3);
    x += 3u8;
    x += &3u8;
    assert_eq!(x, ibig!(3));

    assert_eq!(ibig!(7) - 5u16, ibig!(2));
    assert_eq!(ibig!(7) - &5u16, ibig!(2));
    assert_eq!(&ibig!(7) - 5u16, ibig!(2));
    assert_eq!(&ibig!(7) - &5u16, ibig!(2));
    assert_eq!(5u16 - ibig!(7), ibig!(-2));
    assert_eq!(5u16 - &ibig!(7), ibig!(-2));
    assert_eq!(&5u16 - ibig!(7), ibig!(-2));
    assert_eq!(&5u16 - &ibig!(7), ibig!(-2));

    let mut x = ibig!(10);
    x -= 7u8;
    x -= &7u8;
    assert_eq!(x, ibig!(-4));

    assert_eq!(ibig!(3) + (-1), ibig!(2));
    assert_eq!(ibig!(3) + &(-1), ibig!(2));
    assert_eq!(&ibig!(3) + (-1), ibig!(2));
    assert_eq!(&ibig!(3) + &(-1), ibig!(2));
    assert_eq!(-1 + ibig!(3), ibig!(2));
    assert_eq!(-1 + &ibig!(3), ibig!(2));
    assert_eq!(&-1 + ibig!(3), ibig!(2));
    assert_eq!(&-1 + &ibig!(3), ibig!(2));
    let mut x = ibig!(3);
    x += -10;
    x += &20;
    assert_eq!(x, ibig!(13));

    assert_eq!(ibig!(3) - -1, ibig!(4));
    assert_eq!(ibig!(3) - &-1, ibig!(4));
    assert_eq!(&ibig!(3) - -1, ibig!(4));
    assert_eq!(&ibig!(3) - &-1, ibig!(4));
    assert_eq!(3 - ibig!(4), ibig!(-1));
    assert_eq!(3 - &ibig!(4), ibig!(-1));
    assert_eq!(&3 - ibig!(4), ibig!(-1));
    assert_eq!(&3 - &ibig!(4), ibig!(-1));
    let mut x = ibig!(3);
    x -= -1;
    x -= &10;
    assert_eq!(x, ibig!(-6));
}
