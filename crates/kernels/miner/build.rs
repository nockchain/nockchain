#[path = "../../../scripts/kernel_jam_build.rs"]
mod kernel_jam_build;

fn main() {
    kernel_jam_build::configure("open/assets/miner.jam");
}
