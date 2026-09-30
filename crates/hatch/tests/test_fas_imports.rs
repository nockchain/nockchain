use std::sync::Arc;

use chumsky::Parser;
use hatch::native_parser;
use hatch::utils::LineMap;

#[test]
fn parses_plus_import_prefix() {
    let src = "/+  dbug\n42\n";
    let linemap = Arc::new(LineMap::new(src));
    let parsed = native_parser(vec!["test".to_string()], true, linemap)
        .parse(src)
        .into_result();

    assert!(
        parsed.is_ok(),
        "expected /+ import form to parse, got: {parsed:?}"
    );
}

#[test]
fn parses_multiline_and_repeated_imports() {
    let src = "/+  dbug\n  helper\n/+  util\n42\n";
    let linemap = Arc::new(LineMap::new(src));
    let parsed = native_parser(vec!["test".to_string()], true, linemap)
        .parse(src)
        .into_result();

    assert!(
        parsed.is_ok(),
        "expected multiline /+ imports to parse, got: {parsed:?}"
    );
}

#[test]
fn body_spot_starts_after_every_import_rune() {
    // hoonc parses the import block apart from the body, so the body's spot
    // starts after it whichever rune opens the block.
    for header in [
        "/-  a\n", "/+  a\n", "/?  310\n", "/=  a  /a\n", "/*  a  %jam  /a/jam\n", "/#  a\n",
    ] {
        let src = format!("{header}[1 2]\n");
        let linemap = Arc::new(LineMap::new(&src));
        let parsed = native_parser(vec!["test".to_string()], true, linemap)
            .parse(src.as_str())
            .into_result()
            .unwrap_or_else(|err| panic!("{src:?}: {err:?}"));
        let hatch::ast::hoon::Hoon::TisSig(items) = parsed else {
            panic!("{src:?}: expected a %tssg body");
        };
        let hatch::ast::hoon::Hoon::Dbug(spot, _) = &items[0] else {
            panic!("{src:?}: expected a traced body");
        };
        assert_eq!(spot.q.p, (2, 1), "{src:?}");
    }
}

#[test]
fn import_runes_open_a_header_only_at_the_top_of_the_file() {
    // Elsewhere, and at the top without whitespace after the rune, `/-1`,
    // `/=/foo` and `/--2` are paths, as in hoonc.
    for src in [
        "|%\n++  cur  /=/foo\n++  neg  /-1\n--\n", "/-1\n", "/--2\n",
        "/+  a\n|%\n++  neg  /-1\n--\n",
    ] {
        let linemap = Arc::new(LineMap::new(src));
        let parsed = native_parser(
            vec!["test".to_string(), "x.hoon".to_string()],
            true,
            linemap,
        )
        .parse(src)
        .into_result();
        assert!(parsed.is_ok(), "{src:?}: {parsed:?}");
    }
}
