use std::sync::Arc;

use chumsky::Parser;
use hatch::native_parser;
use hatch::utils::LineMap;

#[test]
fn parses_multiline_buc_hep_arm_without_explicit_closer() {
    let source = concat!(
        "|%\n", "+$  roon\n", "  $-  [lyc=@]\n", "  (unit (unit @))\n", "++  room\n",
        "  |=  [a=@]\n", "  a\n", "--\n",
    );
    let wer = vec!["test".to_string(), "core.hoon".to_string()];
    let linemap = Arc::new(LineMap::new(source));

    let parsed = native_parser(wer, false, linemap)
        .parse(source)
        .into_result();

    assert!(parsed.is_ok(), "expected parse success, got: {parsed:?}");
}
