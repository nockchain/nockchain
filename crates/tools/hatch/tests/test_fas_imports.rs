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
