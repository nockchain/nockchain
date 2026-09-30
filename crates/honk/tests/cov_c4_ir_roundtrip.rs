//! Coverage for `NativeCompiler::compile_expr_with_options`: with
//! `HONK_IR_ROUNDTRIP` set, every minted formula is re-emitted through the
//! native Formula IR and compared byte for byte. This lives in its own test
//! binary because it sets a process environment variable.

use std::path::Path;

#[tokio::test]
async fn c4_native_compile_runs_ir_roundtrip_when_requested() {
    std::env::set_var("HONK_IR_ROUNDTRIP", "1");
    let expr = honk::pipeline::parse_native_hoon_source_without_docs(
        Path::new("c4-roundtrip.hoon"),
        "=/  a  [1 [2 3]]\n?:  =(1 -.a)  +.a  a",
        Vec::new(),
        false,
    )
    .expect("parse");
    let mut compiler = honk::Compiler::new().await.expect("compiler");
    let compiled = compiler
        .compile_expr(&expr)
        .expect("compile with the IR round-trip check");
    let space = compiled.noun_space();
    assert!(compiled.ty().tag(&space).is_some());
}
