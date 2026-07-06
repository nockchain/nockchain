use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use bytes::Bytes;
use chumsky::Parser;
use hatch::native_parser;
use hatch::utils::{diff_noun, hoon_to_noun, LineMap};
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockvm::noun::NounAllocator;

pub static MARKDOWNJAM: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/markdown.jam"));

fn repo_path(path: &str) -> PathBuf {
    if let Ok(test_srcdir) = std::env::var("TEST_SRCDIR") {
        let workspace = std::env::var("TEST_WORKSPACE").unwrap_or_else(|_| "_main".to_string());
        let runfile_path = PathBuf::from(test_srcdir).join(workspace).join(path);
        if runfile_path.exists() {
            return runfile_path;
        }
    }

    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join(path)
}

#[test]
fn test_markdown() {
    // Prefer the in-tree copy (repo root is two levels above this crate;
    // repo_path resolves against the repo's PARENT dir). Fall back to the
    // `open` sibling checkout the golden jam was originally generated from.
    let mut source_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../hoon/common/markdown/markdown.hoon");
    if !source_path.exists() {
        source_path = repo_path("open/hoon/common/markdown/markdown.hoon");
    }
    let source = fs::read_to_string(&source_path)
        .unwrap_or_else(|err| panic!("read {source_path:?} failed: {err}"));

    let linemap = Arc::new(LineMap::new(&source));
    let wer_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../hoon/common/markdown/markdown.hoon");
    let wer: Vec<String> = wer_path
        .iter()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    let hoon = match native_parser(wer, true, linemap)
        .parse(source.as_str())
        .into_result()
    {
        Ok(h) => h,
        Err(err) => {
            eprintln!("parse_block error: {err:?}");
            panic!("failed to parse markdown.hoon");
        }
    };

    let mut slab = NounSlab::<NockJammer>::new();
    let jammed = Bytes::from(MARKDOWNJAM);
    let expected_hoon = slab.cue_into(jammed).expect("cue markdown.jam");
    let actual_hoon = hoon_to_noun(&mut slab, &hoon);
    let space = slab.noun_space();
    let mut printed = false;
    assert!(diff_noun(
        expected_hoon.in_space(&space),
        actual_hoon.in_space(&space),
        &mut printed
    )
    .is_ok());
}
