use std::time::Instant;

use rand::rngs::OsRng;
use rand::TryRngCore;

fn main() {
    let mut msg = [0u8; 65536];
    OsRng
        .try_fill_bytes(&mut msg)
        .expect("random bytes should be available");
    let mut builder = equix::EquiXBuilder::new();
    builder.runtime(equix::RuntimeOption::CompileOnly);

    let start = Instant::now();
    let sol_array_res = builder.solve(&msg[..]);
    let dur = start.elapsed();
    println!("solve() took {dur:?}");

    for ref sol in sol_array_res.unwrap_or_else(|err| {
        panic!(
            "Panicked with {err:?} at {}:{} (git sha: {:?})",
            file!(),
            line!(),
            option_env!("GIT_SHA")
        )
    }) {
        let start_v = Instant::now();
        let _ = builder.verify(&msg[..], sol);
        let dur_v = start_v.elapsed();
        println!("verify() took {dur_v:?}");
    }
}
