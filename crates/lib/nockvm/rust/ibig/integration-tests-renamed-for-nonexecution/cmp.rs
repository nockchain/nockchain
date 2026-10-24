use core::cmp::Ordering;
use ibig::{ibig, ubig};

#[test]
fn test_cmp() {
    assert_eq!(ubig!(500).cmp(&ubig!(500)), Ordering::Equal);
    assert!(ubig!(100) < ubig!(500));
    assert!(ubig!(500) > ubig!(100));
    assert!(ubig!(0x10000000000000000) > ubig!(100));
    assert!(ubig!(100) < ubig!(_0x100000000000000000000000000000000));
    assert!(
        ubig!(_0x100000000000000020000000000000003) < ubig!(_0x100000000000000030000000000000002)
    );
    assert!(
        ubig!(_0x100000000000000030000000000000002) > ubig!(_0x100000000000000020000000000000003)
    );
    assert_eq!(
        ubig!(_0x100000000000000030000000000000002)
            .cmp(&ubig!(_0x100000000000000030000000000000002)),
        Ordering::Equal
    );

    assert_eq!(ibig!(500).cmp(&ibig!(500)), Ordering::Equal);
    assert_eq!(ibig!(-500).cmp(&ibig!(-500)), Ordering::Equal);
    assert!(ibig!(5) < ibig!(10));
    assert!(ibig!(10) > ibig!(5));
    assert!(ibig!(-5) < ibig!(10));
    assert!(ibig!(-15) < ibig!(10));
    assert!(ibig!(10) > ibig!(-5));
    assert!(ibig!(10) > ibig!(-15));
    assert!(ibig!(-10) < ibig!(-5));
    assert!(ibig!(-5) > ibig!(-10));
}
