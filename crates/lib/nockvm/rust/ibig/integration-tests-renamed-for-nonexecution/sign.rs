use ibig::{
    ibig,
    ops::{Abs, UnsignedAbs},
    ubig,
};

#[test]
#[allow(clippy::double_neg)]
fn test_neg() {
    assert_eq!(-ibig!(123), ibig!(-123));
    assert_eq!(-ibig!(-123), ibig!(123));
    assert_eq!(-ibig!(0), ibig!(0));

    assert_eq!(-&ibig!(123), ibig!(-123));
}

#[test]
fn test_abs() {
    assert_eq!(ibig!(123).abs(), ibig!(123));
    assert_eq!(ibig!(-123).abs(), ibig!(123));

    assert_eq!((&ibig!(-123)).abs(), ibig!(123));
}

#[test]
fn test_unsigned_abs() {
    assert_eq!(ibig!(123).unsigned_abs(), ubig!(123));
    assert_eq!(ibig!(-123).unsigned_abs(), ubig!(123));

    assert_eq!((&ibig!(-123)).unsigned_abs(), ubig!(123));
}

#[test]
fn test_signum() {
    assert_eq!(ibig!(-500).signum(), ibig!(-1));
    assert_eq!(ibig!(0).signum(), ibig!(0));
    assert_eq!(ibig!(500).signum(), ibig!(1));
}
