use ibig::{modular::ModuloRing, ubig, UBig};
use rand::{thread_rng, Rng};

fn main() {
    for bits in &[16, 32, 64] {
        print_primes(*bits);
        println!();
    }
}

fn print_primes(bits: usize) {
    let mut order = bits;
    let mut primes = vec![];
    while primes.len() < 3 {
        assert!(order > 1);
        order -= 1;
        let mut mult = ubig!(1);
        while mult.bit_len() <= bits - order {
            if is_prime(&mult, order) {
                primes.push((&mult << order) + ubig!(1));
            }
            mult += ubig!(2);
        }
    }

    primes.sort();
    println!("bits = {} order = {}", bits, order);
    for p in &primes {
        let root = find_root(p, order);
        println!("prime = {:#x} root = {:#x}", p, root);
    }
    let word_max = (ubig!(1) << bits) - ubig!(1);
    let need_range = ((&word_max * &word_max) << (order - 1)) + ubig!(1);
    let mut range = ubig!(1);
    for p in &primes[primes.len() - 3..] {
        range *= p;
    }
    assert!(range >= need_range);
    println!(
        "last 3 primes range / need_range = {:.3}",
        range.to_f64() / need_range.to_f64()
    );
}

/// Check if mult * 2^order + 1 is a prime.
fn is_prime(mult: &UBig, order: usize) -> bool {
    assert!(order >= 1);
    let n = (mult << order) + ubig!(1);
    let ring = ModuloRing::new(&n);
    let one = ring.from(1);

    loop {
        let base = ring.from(thread_rng().gen_range(ubig!(1)..n.clone()));

        // Miller-Rabin test
        let mut p = base.pow(mult);
        let mut passed_miller_rabin = None;
        if p == one {
            passed_miller_rabin = Some(0);
        }
        for i in 0..order {
            if p == -&one {
                passed_miller_rabin = Some(i + 1);
            }
            p = &p * &p;
        }
        if passed_miller_rabin.is_none() {
            return false;
        }
        assert_eq!(p, one);

        // Lucas test: see if base in a generator.
        let mut generator_ok = true;

        if passed_miller_rabin != Some(order) {
            // Not a generator.
            generator_ok = false;
        }

        // Check for divisors of mult.
        let mut divisor = ubig!(2);
        while divisor <= *mult {
            if mult % &divisor == ubig!(0) {
                let exponent = (mult / &divisor) << order;
                if base.pow(&exponent) == one {
                    // Not a generator.
                    generator_ok = false;
                }
            }
            divisor += ubig!(1);
        }

        if generator_ok {
            // Found a generator!
            return true;
        }
    }
}

/// Find 2^order-th root modulo a prime p.
fn find_root(prime: &UBig, order: usize) -> UBig {
    assert!(order >= 1);

    let ring = ModuloRing::new(prime);
    let mut root = ubig!(1);
    let exponent = ubig!(1) << (order - 1);
    while ring.from(&root).pow(&exponent) != ring.from(-1) {
        root += ubig!(1);
    }
    root
}
