//! Doc-anchoring TDD harness: dump the parsed Hoon AST of a source file as a jam.
//!
//! Two modes:
//!   dump_ast hatch <file> <docs:0|1> <dbug:0|1> <out.jam>
//!       Parse <file> with hatch (honk's native parser) and jam the AST noun.
//!       This is the fast per-iteration CANDIDATE — only hatch runs, no ++ut.
//!
//! The GROUND-TRUTH fixture (real ++vast / ++ream docs-on) is generated once by a
//! separate path (see dump_ast_ream / scripts), so per-iteration cost is just hatch.

use std::path::PathBuf;

use hatch::utils::hoon_to_noun;
use honk::pipeline::{parse_native_hoon_source, parse_native_hoon_source_without_docs};
use nockapp::noun::slab::NounSlab;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: dump_ast hatch <file> <docs:0|1> <dbug:0|1> <out.jam>");
        std::process::exit(2);
    }
    match args[1].as_str() {
        "hatch" => {
            if args.len() < 6 {
                eprintln!("usage: dump_ast hatch <file> <docs:0|1> <dbug:0|1> <out.jam>");
                std::process::exit(2);
            }
            let file = PathBuf::from(&args[2]);
            let docs = args[3] == "1";
            let dbug = args[4] == "1";
            let out = &args[5];
            let source = std::fs::read_to_string(&file).expect("read source file");
            // Optional arg 6: comma-separated wer/path (e.g. "tests,hoon-compiler,hoon_138.hoon")
            // so spots match hoonc's parse path for an apples-to-apples AST diff.
            let wer: Vec<String> = args
                .get(6)
                .map(|s| s.split(',').map(|x| x.to_string()).collect())
                .unwrap_or_default();
            let hoon = if docs {
                parse_native_hoon_source(&file, &source, wer, dbug)
            } else {
                parse_native_hoon_source_without_docs(&file, &source, wer, dbug)
            }
            .expect("hatch parse");
            let mut slab: NounSlab = NounSlab::new();
            let noun = hoon_to_noun(&mut slab, &hoon);
            slab.set_root(noun);
            std::fs::write(out, slab.jam().to_vec()).expect("write jam");
            eprintln!("dump_ast hatch: wrote {out} (docs={docs} dbug={dbug})");
        }
        other => {
            eprintln!("dump_ast: unknown mode {other:?}");
            std::process::exit(2);
        }
    }
}
