use std::fs::{File, OpenOptions};
use std::ptr::{copy_nonoverlapping, write_bytes};
use std::time::SystemTime;
use std::{env, io, mem};

use nockvm::mem::NockStack;
use nockvm::noun::{DirectAtom, IndirectAtom};
use nockvm::serialization::{cue, jam};

fn main() -> io::Result<()> {
    let filename = env::args().nth(1).expect("Must provide input filename");
    let output_filename = format!("{}.out", filename);
    let f = File::open(filename)?;
    let in_len = f.metadata()?.len();
    let mut stack = NockStack::new(1 << 10 << 10 << 10, 0);
    let jammed_input = unsafe {
        let in_map = memmap2::Mmap::map(&f)?;
        let word_len = (in_len + 7) >> 3;
        let (mut atom, dest) = IndirectAtom::new_raw_mut(&mut stack, word_len as usize);
        write_bytes(dest.add(word_len as usize - 1), 0, 8);
        copy_nonoverlapping(in_map.as_ptr(), dest as *mut u8, in_len as usize);
        mem::drop(in_map);
        atom.normalize_as_atom()
    };

    let now = SystemTime::now();

    let mut i = 0;
    let mut input = unsafe { DirectAtom::new_unchecked(0).as_atom().as_noun() };
    loop {
        if i >= 1 {
            break;
        };
        i += 1;
        input = cue(&mut stack, jammed_input).unwrap_or_else(|_| {
            panic!(
                "Called `expect()` at {}:{} (git sha: {})",
                file!(),
                line!(),
                option_env!("GIT_SHA").unwrap_or("unknown")
            )
        });
    }

    match now.elapsed() {
        Ok(elapse) => {
            println!("{}", elapse.as_secs_f64());
        }
        Err(_) => println!("NO TIME FOR YOU!"),
    };

    let nuw = SystemTime::now();

    let jammed_output = jam(&mut stack, input);

    match nuw.elapsed() {
        Ok(elapse) => {
            println!("Jam: {}", elapse.as_secs_f64());
        }
        Err(_) => println!("NO TIME FOR YOU!"),
    };

    let f_out = OpenOptions::new()
        .read(true)
        .write(true)
        .truncate(true)
        .open(output_filename)?;
    f_out.set_len((jammed_output.size() << 3) as u64)?;
    unsafe {
        let mut out_map = memmap2::MmapMut::map_mut(&f_out)?;
        copy_nonoverlapping(
            jammed_output.data_pointer() as *mut u8,
            out_map.as_mut_ptr(),
            jammed_output.size() << 3,
        );
        out_map.flush()?;
    };

    Ok(())
}
