#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

#[cfg(test)]
mod tests {

    use super::*;
    #[test]
    #[cfg_attr(miri, ignore)]
    fn simple_test() {
        unsafe {
            let test = "hello".as_bytes();
            let output: [u8; 16] = [0; 16];
            MurmurHash3_x64_128(
                test.as_ptr() as _,
                test.len() as i32,
                0,
                output.as_ptr() as *mut _,
            );
        }
    }
}
