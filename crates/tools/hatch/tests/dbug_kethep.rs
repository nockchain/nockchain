use std::sync::Arc;

use chumsky::Parser;
use hatch::ast::hoon::{BaseType, Hoon, Spec};
use hatch::native_parser;
use hatch::utils::LineMap;

fn unwrap_dbug(hoon: &Hoon) -> &Hoon {
    match hoon {
        Hoon::Dbug(_, inner) => unwrap_dbug(inner),
        other => other,
    }
}

#[test]
fn backtick_noun_spec_is_not_dbug_wrapped() {
    let src = "`*`1\n";
    let linemap = Arc::new(LineMap::new(src));
    let parsed = native_parser(vec!["test".to_string()], true, linemap)
        .parse(src)
        .into_result()
        .expect("parse failed");

    let hoon = match &parsed {
        Hoon::TisSig(list) => list.first().expect("expected one hoon"),
        other => other,
    };
    let hoon = unwrap_dbug(hoon);

    match hoon {
        Hoon::KetHep(spec, _hoon) => {
            assert!(
                matches!(**spec, Spec::Base(BaseType::NounExpr)),
                "expected `* to lower to base noun without dbug wrapping"
            );
        }
        other => panic!("expected KetHep, got {other:?}"),
    }
}
