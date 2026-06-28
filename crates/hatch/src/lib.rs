#![allow(
    dead_code, redundant_semicolons, unreachable_patterns, unused_assignments, unused_doc_comments,
    unused_imports, unused_mut, unused_parens, unused_variables
)]
#![allow(
    clippy::assign_op_pattern, clippy::borrow_deref_ref, clippy::clone_on_copy,
    clippy::collapsible_else_if, clippy::collapsible_if, clippy::cmp_owned, clippy::empty_docs,
    clippy::get_first, clippy::if_same_then_else, clippy::implied_bounds_in_impls,
    clippy::into_iter_on_ref, clippy::io_other_error, clippy::iter_skip_next,
    clippy::let_and_return, clippy::manual_contains, clippy::manual_div_ceil,
    clippy::manual_is_ascii_check, clippy::manual_is_multiple_of, clippy::manual_map,
    clippy::manual_range_contains, clippy::manual_repeat_n, clippy::manual_saturating_arithmetic,
    clippy::match_like_matches_macro, clippy::needless_borrow,
    clippy::needless_borrows_for_generic_args, clippy::needless_range_loop,
    clippy::needless_return, clippy::nonminimal_bool, clippy::only_used_in_recursion,
    clippy::op_ref, clippy::ptr_arg, clippy::redundant_closure, clippy::redundant_pattern_matching,
    clippy::result_unit_err, clippy::should_implement_trait, clippy::too_many_arguments,
    clippy::type_complexity, clippy::unnecessary_cast, clippy::unnecessary_map_or,
    clippy::unnecessary_to_owned, clippy::unnecessary_unwrap, clippy::useless_conversion,
    clippy::useless_format
)]

pub mod ast;
pub mod runes;
pub mod utils;

extern crate self as hatch;

#[path = "main.rs"]
mod parser_main;

pub use parser_main::parser as native_parser;
