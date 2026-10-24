use ibig::{ibig, ubig};

#[test]
fn test_pow_ubig() {
    let test_cases = [
        (ubig!(0), 0, ubig!(1)),
        (ubig!(100), 0, ubig!(1)),
        (ubig!(0), 1, ubig!(0)),
        (ubig!(100), 1, ubig!(100)),
        (ubig!(0), 2, ubig!(0)),
        (ubig!(100), 2, ubig!(10000)),
        (ubig!(0), 100, ubig!(0)),
        (ubig!(1), 100, ubig!(1)),
        (ubig!(2), 10, ubig!(1024)),
        (ubig!(7), 10, ubig!(282475249)),
        (ubig!(123), 13, ubig!(_1474913153392179474539944683)),
    ];

    for (a, b, c) in &test_cases {
        assert_eq!(a.pow(*b), *c);
    }
}

#[test]
fn test_pow_ibig() {
    let test_cases = [
        (ibig!(0), 0, ibig!(1)),
        (ibig!(0), 12, ibig!(0)),
        (ibig!(0), 13, ibig!(0)),
        (ibig!(7), 2, ibig!(49)),
        (ibig!(7), 3, ibig!(343)),
        (ibig!(-7), 2, ibig!(49)),
        (ibig!(-7), 3, ibig!(-343)),
    ];

    for (a, b, c) in &test_cases {
        assert_eq!(a.pow(*b), *c);
    }
}
