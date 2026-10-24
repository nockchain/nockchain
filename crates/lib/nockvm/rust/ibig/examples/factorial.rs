use ibig::UBig;

// a * (a+1) * ... * (b-1)
fn product(a: u32, b: u32) -> UBig {
    if b == a + 1 {
        UBig::from(a)
    } else {
        let mid = a + (b - a) / 2;
        product(a, mid) * product(mid, b)
    }
}

fn main() {
    let n: u32 = 1000000;
    let factorial = product(1, n + 1);
    println!("{}! = {:#x}", n, factorial);
}
