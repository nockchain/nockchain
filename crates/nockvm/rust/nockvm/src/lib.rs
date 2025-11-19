#![feature(cold_path)]
#![feature(cfg_boolean_literals)]
#![allow(dead_code)]

extern crate lazy_static;
extern crate num_derive;
#[macro_use]
extern crate static_assertions;
pub mod ext;
mod flog;
pub mod hamt;
pub mod interpreter;
pub mod jets;
pub mod mem;
pub mod mug;
pub mod noun;
pub mod serialization;
pub mod site;
pub mod substantive;
pub mod trace;
pub mod unifying_equality;

/** Introduce useful functions for debugging
 *
 * The main difficulty with these is that rust wants to strip them out if they're not used in the
 * code.  Even if you get it past the compiler, the linker will get rid of them.  The solution here
 * is to call use_gdb() from main.rs on each module.  This is ugly, but I haven't found another way
 * that keeps these available in the debugger.
 *
 * Thus, every file that touches nouns should include `crate::gdb!();` at the top, and main.rs should
 * call use_gdb on that module.
 */
macro_rules! gdb {
    () => {
        fn pretty_noun(noun: crate::noun::Noun) -> String {
            format!("{:?}", noun)
        }

        pub fn use_gdb() {
            pretty_noun(crate::noun::D(0));
        }
    };
}

pub fn check_endian() {
    if cfg!(target_endian = "little") {
    } else if cfg!(target_endian = "big") {
        panic!("Sword only supports little-endian. This is a big-endian system, which is not supported.");
    } else {
        panic!("Sword only supports little-endian. This system has an unknown endianness, which is not supported.");
    }
}

pub(crate) use gdb;

#[cfg(test)]
mod tests {

    #[test]
    fn tas() {
        use nockvm_macros::tas;
        assert_eq!(tas!(b"cut"), 0x747563);
        assert_eq!(tas!(b"dec"), 0x636564);
        assert_eq!(tas!(b"prop"), 0x706f7270);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_jam() {
        use crate::mem::NockStack;
        use crate::noun::*;
        use crate::serialization::jam;
        let mut stack = NockStack::new(8 << 10 << 10, 0);
        let head = Atom::new(&mut stack, 0).as_noun();
        let tail = Atom::new(&mut stack, 1).as_noun();
        let cell = Cell::new(&mut stack, head, tail).as_noun();
        let res = jam(&mut stack, cell)
            .as_direct()
            .unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            })
            .data();
        assert_eq!(res, 201);
    }
}
