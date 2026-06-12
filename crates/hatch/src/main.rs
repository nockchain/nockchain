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

use std::cell::Cell;
use std::collections::HashMap;
use std::error::Error;
use std::io::Write;
use std::path::{Path as StdPath, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::{env, fs};

use ariadne::{Color, Label, Report, ReportKind, Source};
use bytes::Bytes;
use chumsky::input::{StrInput, Stream, ValueInput};
use chumsky::prelude::*;
use clap::{arg, command, Parser as ClapParser};
use hatch::ast::hoon::*;
use hatch::runes::*;
use hatch::utils::*;
use hoonc::{HOON_TXT, KERNEL_JAM, PREWARM_STATE_JAM};
use ibig::ubig;
use nockapp::drivers::one_punch::OnePunchWire;
use nockapp::kernel::boot;
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::wire::Wire;
use nockapp::AtomExt;
use nockvm::noun::{Atom, Noun, NounAllocator, D, T};
use nockvm_macros::tas;
use tokio::runtime::Builder;
use walkdir::WalkDir;

macro_rules! rune_branch_pair {
    ($token:expr, $tall:expr, $wide:expr) => {
        just($token).ignore_then(choice(($tall, $wide))).boxed()
    };
}

macro_rules! rune_branch {
    ($token:expr, $form:expr) => {
        just($token).ignore_then($form).boxed()
    };
}

fn spec_parser<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> + Clone {
    choice((
        rune_branch_pair!(
            "$",
            buc_spec_tall(hoon.clone(), spec.clone()),
            buc_spec_wide(hoon_wide.clone(), spec_wide.clone())
        ),
        rune_branch_pair!(
            "%",
            cen_spec_tall(hoon.clone(), spec.clone()),
            cen_spec_wide(hoon_wide.clone(), spec_wide.clone())
        ),
        spec_wide.clone(),
    ))
    .boxed()
}

fn spec_wide_parser<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
    hoon_wide: impl ParserExt<'src, Hoon>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> + Clone {
    let parsers = vec![
        just('$')
            .ignore_then(buc_spec_wide(hoon_wide.clone(), spec_wide.clone()))
            .boxed(),
        buccab_spec_irregular(hoon_wide.clone()).boxed(), //  _p
        bucmic_spec_irregular(hoon_wide.clone()).boxed(), //  ,p0
        buctis_irregular(spec_wide.clone()).boxed(),      // foo=bar, =bar,  =foo=bar
        buccol_irregular(spec_wide.clone()).boxed(),      // [foo=bar foo=bar]
        reference_spec(spec_wide.clone()).boxed(),        // foo or foo:bar
        bucwut_irregular_spec(spec_wide.clone()).boxed(), // ?(foo bar)
        parenthesis_spec(hoon_wide.clone(), spec_wide.clone()).boxed(), // (foo bar)
        constant(linemap)
            .try_map(|coin, span| {
                //  %foo
                match coin {
                    Coin::Dime(p, q) => Ok(Spec::Leaf(p, q)),
                    _ => Err(Rich::custom(span, "invalid spec constant")),
                }
            })
            .boxed(),
        aura_spec().boxed(), //  @foo
        loop_spec().boxed(), //  /foo
        just('^').to(Spec::Base(BaseType::Cell)).boxed(),
        just('?').to(Spec::Base(BaseType::Flag)).boxed(),
        just('~').to(Spec::Base(BaseType::Null)).boxed(),
        just('*').to(Spec::Base(BaseType::NounExpr)).boxed(),
        just("!!").to(Spec::Base(BaseType::Void)).boxed(),
    ];

    choice(parsers).boxed()
}

#[derive(serde::Serialize, PartialEq, Debug, Clone)]
enum WideOp {
    KetTis,
    TisGal,
    Pair,
}

fn hoon_wide_parser<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
    hoon_wide_with_trace: impl ParserExt<'src, Hoon>,
    hoon_wide_no_trace: impl ParserExt<'src, Hoon>,
    wer: Path,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> + Clone {
    let parsers = vec![
        rune_branch!(
            '|',
            bar_runes_wide(hoon_wide.clone(), spec_wide.clone(), linemap.clone())
        ),
        just('=')
            .ignore_then(choice((
                tis_runes_wide(hoon_wide.clone(), spec_wide.clone()),
                dottis_irregular(hoon_wide.clone()), //  =(p q)
                kettis_irregular(spec_wide.clone()).boxed(), // =bar
            )))
            .boxed(),
        just('?')
            .ignore_then(choice((
                wut_runes_wide(hoon_wide.clone(), spec_wide.clone()),
                bucwut_irregular(spec_wide.clone()).boxed(), // ?(foo bar)
                just('?').to(Hoon::Base(BaseType::Flag)).boxed(),
                empty().to(Hoon::Base(BaseType::Flag)).boxed(),
            )))
            .boxed(),
        just('%')
            .ignore_then(choice((
                cen_runes_wide(hoon_wide.clone()),
                just('|').to(Hoon::Rock(
                    "f".to_string(),
                    NounExpr::ParsedAtom(ParsedAtom::Small(1)),
                )),
                just('&').to(Hoon::Rock(
                    "f".to_string(),
                    NounExpr::ParsedAtom(ParsedAtom::Small(0)),
                )),
                nuck().map(|coin| jock(true, &coin)),
            )))
            .boxed(),
        just(':')
            .ignore_then(choice((
                col_runes_wide(hoon_wide.clone()),
                miccol_irregular(hoon_wide.clone()).boxed(), //  :(a b .. z)
            )))
            .boxed(),
        just('~')
            .ignore_then(choice((
                sig_runes_wide(hoon_wide.clone()),
                censig_irregular(hoon_wide.clone()), //  ~(a b c)
                twid().map(|coin| jock(false, &coin)),
            )))
            .boxed(),
        rune_branch!('$', buc_runes_wide(hoon_wide.clone(), spec_wide.clone())),
        rune_branch!('^', ket_runes_wide(hoon_wide.clone(), spec_wide.clone())),
        rune_branch!(
            '!',
            zap_runes_wide(
                hoon_wide.clone(),
                spec_wide.clone(),
                hoon_wide_with_trace.clone(),
                hoon_wide_no_trace.clone()
            )
        ),
        rune_branch!(
            ';',
            choice((
                sail_wide(hoon.clone(), hoon_wide.clone()),
                mic_runes_wide(hoon_wide.clone(), spec_wide.clone()),
            ))
        ),
        just('.')
            .ignore_then(choice((
                dot_runes_wide(hoon_wide.clone(), spec_wide.clone()),
                perd().map(|coin| jock(false, &coin)),
            )))
            .boxed(),
        just('`')
            .ignore_then(choice((
                tic_aura(hoon_wide.clone()),                                    //  `@p`q
                kethep_noun_irregular(hoon_wide.clone()).boxed(),               //  `*`q
                kethep_irregular(hoon_wide.clone(), spec_wide.clone()).boxed(), //  `p`q
                ketlus_irregular(hoon_wide.clone()),                            // `+p`q
                tic_cell_construction(hoon_wide.clone()).boxed(),               //  `a
            )))
            .boxed(),
        function_call(hoon_wide.clone()).boxed(), //  (a b)
        centis_irregular(hoon_wide.clone()).boxed(), //  a(b c, d e, f g)
        aura_hoon().boxed(),
        buccab_irregular(hoon_wide.clone()).boxed(), //  _p
        constant_separator_hoon(hoon_wide.clone()).boxed(), //  const+hoon,  const/hoon
        list_syntax(hoon.clone(), hoon_wide.clone()).boxed(), // [p ... pn], ~[foo], [foo]~
        kettar_irregular(spec_wide.clone()).boxed(), //  *foo
        wutzap_irregular(hoon_wide.clone()).boxed(), //  !p
        wutbar_irregular(hoon_wide.clone()).boxed(), //  |(p q)
        wutpam_irregular(hoon_wide.clone()).boxed(), //  &(p q)
        increment(hoon_wide.clone()).boxed(),        //  +(a) or .+(a)
        ketcol_irregular(spec_wide.clone()).boxed(), //  ,p
        tell(hoon_wide.clone()).boxed(),             // <foo> render as tape
        yell_parser(hoon_wide.clone()).boxed(),      // >foo< render as tank
        number()
            .map(|(p, q)| Hoon::Sand(p, NounExpr::ParsedAtom(q)))
            .boxed(), //  111.111, 0x1111, etc.
        wing().boxed(),                              //   foo, foo.bar, etc.
        just('^').to(Hoon::Base(BaseType::Cell)).boxed(),
        constant(linemap.clone())
            .map(|coin| jock(true, &coin))
            .boxed(), //  %foo
        cord(linemap.clone())
            .map(|s| Hoon::Sand("t".to_string(), NounExpr::ParsedAtom(s)))
            .boxed(), //  'foo'
        path(hoon_wide.clone(), wer, linemap.clone()).boxed(), //  /a/b/c
        tape(hoon_wide.clone(), linemap).boxed(),              //  "foo"
        just('~').to(Hoon::Bust(BaseType::Null)).boxed(),
        just('&')
            .to(Hoon::Sand(
                "f".to_string(),
                NounExpr::ParsedAtom(ParsedAtom::Small(0)),
            ))
            .boxed(),
        just('|')
            .to(Hoon::Sand(
                "f".to_string(),
                NounExpr::ParsedAtom(ParsedAtom::Small(1)),
            ))
            .boxed(),
        just('*').to(Hoon::Base(BaseType::NounExpr)).boxed(),
    ];

    choice(parsers)
        .boxed()
        .then(
            choice((
                just('=').to(WideOp::KetTis),
                just(':').to(WideOp::TisGal),
                just('^').to(WideOp::Pair),
            ))
            .then(hoon_wide.clone())
            .or_not(),
        )
        .try_map(|(p, maybe_separator), span| match maybe_separator {
            Some((WideOp::KetTis, q)) => {
                let maybe_skin = flay(p);
                match maybe_skin {
                    None => Err(Rich::custom(span, "invalid p in p=q")),
                    Some(s) => Ok(Hoon::KetTis(s, Box::new(q))),
                }
            }
            Some((WideOp::TisGal, q)) => Ok(Hoon::TisGal(Box::new(p), Box::new(q))),
            Some((WideOp::Pair, q)) => Ok(Hoon::Pair(Box::new(p), Box::new(q))),
            None => Ok(p),
        })
}

pub fn hoon_parser<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
    spec_wide: impl ParserExt<'src, Spec>,
    hoon_with_trace: impl ParserExt<'src, Hoon>,
    hoon_no_trace: impl ParserExt<'src, Hoon>,
    hoon_wide_with_trace: impl ParserExt<'src, Hoon>,
    hoon_wide_no_trace: impl ParserExt<'src, Hoon>,
    wer: Path,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    let parsers = vec![
        rune_branch_pair!(
            '|',
            bar_runes_tall(hoon.clone(), spec.clone(), linemap.clone()),
            bar_runes_wide(hoon_wide.clone(), spec_wide.clone(), linemap.clone())
        ),
        rune_branch_pair!(
            '=',
            tis_runes_tall(hoon.clone(), spec.clone(), spec_wide.clone()),
            tis_runes_wide(hoon_wide.clone(), spec_wide.clone())
        ),
        just('?')
            .ignore_then(choice((
                wut_runes_tall(
                    hoon.clone(),
                    hoon_wide.clone(),
                    spec.clone(),
                    spec_wide.clone(),
                )
                .boxed(),
                wut_runes_wide(hoon_wide.clone(), spec_wide.clone()).boxed(),
                bucwut_irregular(spec_wide.clone()).boxed(), // ?(foo bar)
                just('?').to(Hoon::Base(BaseType::Flag)).boxed(),
                empty().to(Hoon::Base(BaseType::Flag)).boxed(),
            )))
            .boxed(),
        rune_branch_pair!(
            '%',
            cen_runes_tall(hoon.clone()),
            cen_runes_wide(hoon_wide.clone())
        ),
        rune_branch_pair!(
            ':',
            col_runes_tall(hoon.clone()),
            col_runes_wide(hoon_wide.clone())
        ),
        rune_branch_pair!(
            '~',
            sig_runes_tall(hoon.clone()),
            sig_runes_wide(hoon_wide.clone())
        ),
        rune_branch_pair!(
            '$',
            buc_runes_tall(hoon.clone(), spec.clone()),
            buc_runes_wide(hoon_wide.clone(), spec_wide.clone())
        ),
        rune_branch_pair!(
            '^',
            ket_runes_tall(hoon.clone(), spec.clone()),
            ket_runes_wide(hoon_wide.clone(), spec_wide.clone())
        ),
        rune_branch_pair!(
            '!',
            zap_runes_tall(
                hoon.clone(),
                spec.clone(),
                hoon_with_trace.clone(),
                hoon_no_trace.clone()
            ),
            zap_runes_wide(
                hoon_wide.clone(),
                spec_wide.clone(),
                hoon_wide_with_trace.clone(),
                hoon_wide_no_trace.clone()
            )
        ),
        rune_branch_pair!(
            ';',
            choice((
                sail_tall(hoon.clone(), hoon_wide.clone()),
                mic_runes_tall(hoon.clone(), spec.clone()),
            )),
            choice((
                sail_wide(hoon.clone(), hoon_wide.clone()),
                mic_runes_wide(hoon_wide.clone(), spec_wide.clone()),
            ))
        ),
        rune_branch_pair!(
            '.',
            dot_runes_tall(hoon.clone(), spec.clone()),
            dot_runes_wide(hoon_wide.clone(), spec_wide.clone())
        ),
        just('/') // skip imports...
            .ignore_then(fas_runes_tall(
                hoon.clone(),
                hoon_wide.clone(),
                wer.clone(),
                linemap.clone(),
            ))
            .boxed(),
        hoon_wide.clone().boxed(),
        noun_tall(hoon.clone()).boxed(),
    ];

    choice(parsers)
}

pub fn parser<'src>(
    wer: Path,
    bug: bool,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    let mut hoon = Recursive::declare();
    let mut hoon_wide = Recursive::declare();
    let mut spec = Recursive::declare();
    let mut spec_wide = Recursive::declare();

    let mut hoon_no_trace = Recursive::declare();
    let mut hoon_wide_no_trace = Recursive::declare();
    let mut spec_no_trace = Recursive::declare();
    let mut spec_wide_no_trace = Recursive::declare();

    let spec_body = spec_parser(
        hoon.clone(),
        hoon_wide.clone(),
        spec.clone(),
        spec_wide.clone(),
    )
    .map_with(wrap_spec_with_trace(wer.clone(), linemap.clone()))
    .labelled("Spec")
    .boxed();

    spec.define(spec_body);

    let spec_wide_body = spec_wide_parser(spec_wide.clone(), hoon_wide.clone(), linemap.clone())
        .map_with(wrap_spec_with_trace(wer.clone(), linemap.clone()))
        .labelled("Spec Wide")
        .boxed();

    spec_wide.define(spec_wide_body);

    let hoon_wide_body = hoon_wide_parser(
        hoon.clone(),
        hoon_wide.clone(),
        spec_wide.clone(),
        hoon_wide.clone(),
        hoon_wide_no_trace.clone(),
        wer.clone(),
        linemap.clone(),
    )
    .map_with(wrap_hoon_with_trace(wer.clone(), linemap.clone()))
    .labelled("Hoon Wide")
    .boxed();

    hoon_wide.define(hoon_wide_body);

    let hoon_body = hoon_parser(
        hoon.clone(),
        hoon_wide.clone(),
        spec.clone(),
        spec_wide.clone(),
        hoon.clone(),
        hoon_no_trace.clone(),
        hoon_wide.clone(),
        hoon_wide_no_trace.clone(),
        wer.clone(),
        linemap.clone(),
    )
    .map_with(wrap_hoon_with_trace(wer.clone(), linemap.clone()))
    .labelled("Hoon")
    .boxed();

    hoon.define(hoon_body);

    let hoon_no_trace_body = hoon_parser(
        hoon_no_trace.clone(),
        hoon_wide_no_trace.clone(),
        spec_no_trace.clone(),
        spec_wide_no_trace.clone(),
        hoon.clone(),
        hoon_no_trace.clone(),
        hoon_wide.clone(),
        hoon_wide_no_trace.clone(),
        wer.clone(),
        linemap.clone(),
    )
    .map_with(wrap_hoon_with_docs(linemap.clone()))
    .labelled("Hoon")
    .boxed();

    hoon_no_trace.define(hoon_no_trace_body);

    let hoon_wide_no_trace_body = hoon_wide_parser(
        hoon_no_trace.clone(),
        hoon_wide_no_trace.clone(),
        spec_wide_no_trace.clone(),
        hoon_wide.clone(),
        hoon_wide_no_trace.clone(),
        wer.clone(),
        linemap.clone(),
    )
    .map_with(wrap_hoon_with_docs(linemap.clone()))
    .labelled("Hoon Wide")
    .boxed();

    hoon_wide_no_trace.define(hoon_wide_no_trace_body);

    let spec_body_no_trace = spec_parser(
        hoon_no_trace.clone(),
        hoon_wide_no_trace.clone(),
        spec_no_trace.clone(),
        spec_wide_no_trace.clone(),
    )
    .map_with(wrap_spec_with_docs(linemap.clone()))
    .labelled("Spec")
    .boxed();

    spec_no_trace.define(spec_body_no_trace);

    let spec_wide_no_trace_body = spec_wide_parser(
        spec_wide_no_trace.clone(),
        hoon_wide_no_trace.clone(),
        linemap.clone(),
    )
    .map_with(wrap_spec_with_docs(linemap.clone()))
    .labelled("Spec Wide")
    .boxed();

    spec_wide_no_trace.define(spec_wide_no_trace_body);

    let hoon = if bug { hoon } else { hoon_no_trace };

    hoon.separated_by(gap())
        .at_least(1)
        .collect::<Vec<Hoon>>()
        .map(|hoons| Hoon::TisSig(hoons))
        .delimited_by(gap().or_not(), gap().or_not())
        .boxed()
}

#[cfg(not(feature = "bazel_build"))]
pub static HOON138JAM: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/test/parsed-hoon138.jam"
));

#[derive(ClapParser, Debug)]
struct Cli {
    /// input file or directory (required unless --test)
    #[arg(value_name = "PATH", required = false)]
    input: Option<PathBuf>,

    /// disable debug traces
    #[arg(long = "no-dbug", short = 'b')]
    no_dbug: bool,

    /// write JAM instead of JSON
    #[arg(long = "jam")]
    jam: bool,

    /// output file (defaults to stdout)
    #[arg(long = "out", short = 'o', value_name = "PATH")]
    out: Option<PathBuf>,

    /// run hardcoded hoon-138 test
    #[arg(long = "test")]
    test: bool,
}

const BLACKLISTED_DIRS: &[&str] = &["packages", "node_modules", ".git", "target"];
const VALID_EXTENSIONS: &[&str] =
    &["jock", "hoon", "txt", "jam", "html", "css", "js", "jpg", "png", "gif"];

struct HooncTempPaths {
    prewarm_state: Option<PathBuf>,
    data_dir: PathBuf,
}

impl Drop for HooncTempPaths {
    fn drop(&mut self) {
        if let Some(path) = &self.prewarm_state {
            let _ = fs::remove_file(path);
        }
        let _ = fs::remove_dir_all(&self.data_dir);
    }
}

fn make_temp_path(prefix: &str, suffix: &str) -> Result<PathBuf, Box<dyn Error>> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let mut path = env::temp_dir();
    path.push(format!("{prefix}-{}-{nanos}{suffix}", std::process::id()));
    Ok(path)
}

fn canonicalize_and_string(path: &StdPath) -> Result<String, Box<dyn Error>> {
    let canonical = path.canonicalize()?;
    let string = canonical.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "path contains invalid UTF-8",
        )
    })?;
    Ok(string.to_string())
}

fn hoon_138_source_path() -> Result<PathBuf, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../hoonc/hoon/hoon-138.hoon");
    Ok(path.canonicalize()?)
}

fn is_valid_hoonc_entry(entry: &walkdir::DirEntry) -> bool {
    let file_type = entry.file_type();
    let file_name = entry.file_name().to_string_lossy();
    if file_type.is_dir() {
        return !BLACKLISTED_DIRS.contains(&file_name.as_ref());
    }
    if !file_type.is_file() {
        return false;
    }
    entry
        .path()
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| VALID_EXTENSIONS.contains(&ext))
        .unwrap_or(false)
}

fn build_directory_noun(slab: &mut NounSlab, deps_dir: &StdPath) -> Result<Noun, Box<dyn Error>> {
    let base = canonicalize_and_string(deps_dir)?;
    let mut directory_noun = D(0);

    for entry in WalkDir::new(&base)
        .follow_links(true)
        .into_iter()
        .filter_entry(is_valid_hoonc_entry)
    {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let entry_path = entry.path().to_str().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "dependency path contains invalid UTF-8",
            )
        })?;
        let rel_path = entry_path.strip_prefix(&base).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "dependency path does not share base prefix",
            )
        })?;
        let path_cord = Atom::from_value(slab, rel_path)?.as_noun();
        let contents = fs::read(entry.path())?;
        let contents_atom = Atom::from_value(slab, contents)?.as_noun();
        let entry_cell = T(slab, &[path_cord, contents_atom]);
        directory_noun = T(slab, &[entry_cell, directory_noun]);
    }

    Ok(directory_noun)
}

async fn setup_hoonc_nockapp(
) -> Result<(nockapp::NockApp<NockJammer>, HooncTempPaths), Box<dyn Error>> {
    let mut boot_cli = boot::default_boot_cli(true);
    boot_cli.rotating_snapshot_interval_event_time = None;

    let data_dir = make_temp_path("hoonc-parse-data", "")?;
    let mut prewarm_state = None;

    if env::var("HOONC_DISABLE_PREWARM").is_err() {
        let prewarm_path = make_temp_path("hoonc-prewarm", ".jam")?;
        fs::write(&prewarm_path, PREWARM_STATE_JAM)?;
        boot_cli.state_jam = Some(prewarm_path.to_string_lossy().into_owned());
        prewarm_state = Some(prewarm_path);
    }

    let nockapp =
        boot::setup::<NockJammer>(KERNEL_JAM, boot_cli, &[], "hoonc", Some(data_dir.clone()))
            .await?;
    Ok((
        nockapp,
        HooncTempPaths {
            prewarm_state,
            data_dir,
        },
    ))
}

fn boot_hoonc(nockapp: &mut nockapp::NockApp<NockJammer>) -> Result<(), Box<dyn Error>> {
    let mut boot_slab = NounSlab::new();
    let hoon_cord = Atom::from_value(&mut boot_slab, HOON_TXT)?.as_noun();
    let bootstrap_poke = T(&mut boot_slab, &[D(tas!(b"boot")), hoon_cord]);
    boot_slab.set_root(bootstrap_poke);
    nockapp.poke_sync(OnePunchWire::Poke.to_wire(), boot_slab)?;
    Ok(())
}

fn run_original_parser_timing(
    source_path: &StdPath,
    source_bytes: &[u8],
) -> Result<Duration, Box<dyn Error>> {
    let runtime = Builder::new_multi_thread().enable_all().build()?;
    let (mut nockapp, _temp_paths) = runtime.block_on(setup_hoonc_nockapp())?;
    boot_hoonc(&mut nockapp)?;

    let deps_dir = source_path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "source path has no parent",
        )
    })?;
    let entry_string = canonicalize_and_string(source_path)?;
    let mut slab = NounSlab::new();
    let entry_path = Atom::from_value(&mut slab, entry_string)?.as_noun();
    let entry_contents = Atom::from_value(&mut slab, source_bytes.to_vec())?.as_noun();
    let directory_noun = build_directory_noun(&mut slab, deps_dir)?;

    let parse_poke = T(
        &mut slab,
        &[D(tas!(b"parse")), entry_path, entry_contents, directory_noun],
    );
    slab.set_root(parse_poke);

    let start = Instant::now();
    nockapp.poke_sync(OnePunchWire::Poke.to_wire(), slab)?;
    Ok(start.elapsed())
}

fn run_test() {
    if let Err(err) = run_test_inner() {
        eprintln!("parser --test failed: {err}");
        std::process::exit(1);
    }
}

fn run_test_inner() -> Result<(), Box<dyn Error>> {
    let source_path = hoon_138_source_path()?;
    let source = std::str::from_utf8(hoonc::HOON_138_HOON)?.to_string();
    let source_bytes = hoonc::HOON_138_HOON.to_vec();
    let linemap = Arc::new(LineMap::new(&source));

    let wer = vec![
        "hoonc".to_string(),
        "hoon".to_string(),
        "hoon-138".to_string(),
        "hoon".to_string(),
    ];

    let start = Instant::now();

    match parser(wer, false, linemap)
        .parse(source.as_str())
        .into_result()
    {
        Ok(res) => {
            let end = start.elapsed();

            let mut slab = NounSlab::new();
            let conversion_start = Instant::now();
            let parsed_hoon = hoon_to_noun(&mut slab, &res);
            let conversion_took = conversion_start.elapsed();
            let jammed = Bytes::from(HOON138JAM);
            let cued = slab.cue_into(jammed)?;

            let space = slab.noun_space();
            diff_and_report(cued.in_space(&space), parsed_hoon.in_space(&space));

            println!("test parsing took: {:?}", end);
            println!("native ast to noun took: {:?}", conversion_took);
        }
        Err(errs) => {
            for err in errs {
                let span = err.span().into_range();
                let file_id = source_path.to_string_lossy().to_string();

                let _ = Report::build(ReportKind::Error, (file_id.clone(), span.clone()))
                    .with_config(ariadne::Config::new().with_index_type(ariadne::IndexType::Byte))
                    .with_label(
                        Label::new((file_id.clone(), span))
                            .with_message(err.reason().to_string())
                            .with_color(Color::Red),
                    )
                    .finish()
                    .eprint((file_id.clone(), Source::from(source.clone())));
            }
            return Err(
                std::io::Error::new(std::io::ErrorKind::Other, "native parser failed").into(),
            );
        }
    };

    let original_timing = run_original_parser_timing(&source_path, &source_bytes)?;
    println!("original hoon parser parse took: {:?}", original_timing);
    Ok(())
}

fn run_parser(source_path: &PathBuf, jam: bool, dbug: bool, out: Option<PathBuf>) {
    let source = fs::read_to_string(source_path).unwrap_or_else(|err| {
        eprintln!("Error reading file '{}': {}", source_path.display(), err);
        std::process::exit(1);
    });

    let start = Instant::now();

    let wer: Vec<String> = source_path
        .iter()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();

    let linemap = Arc::new(LineMap::new(&source));

    match parser(wer, dbug, linemap)
        .parse(source.as_str())
        .into_result()
    {
        Ok(res) => {
            let took = start.elapsed();

            let mut slab = NounSlab::new();
            let start2 = Instant::now();
            let parsed_hoon = hoon_to_noun(&mut slab, &res);
            let took2 = start2.elapsed();

            if jam {
                slab.set_root(parsed_hoon);
                let jammed = slab.jam();

                match &out {
                    Some(out) if out.is_dir() => {
                        let file_name = source_path.file_name().unwrap_or_else(|| {
                            eprintln!("Input path '{}' has no file name", source_path.display());
                            std::process::exit(1);
                        });
                        let out_file = out.join(file_name);
                        fs::write(&out_file, &jammed).unwrap_or_else(|err| {
                            eprintln!("Failed to write '{}': {}", out_file.display(), err);
                            std::process::exit(1);
                        });
                    }
                    Some(out) => fs::write(out, &jammed).unwrap_or_else(|err| {
                        eprintln!("Failed to write '{}': {}", out.display(), err);
                        std::process::exit(1);
                    }),
                    None => std::io::stdout().write_all(&jammed).unwrap_or_else(|err| {
                        eprintln!("Failed to write jam to stdout: {err}");
                        std::process::exit(1);
                    }),
                }
            } else {
                let json =
                    serde_json::to_string_pretty(&res).expect("AST JSON serialization failed");

                match &out {
                    None => {
                        println!("{json}");
                    }
                    Some(out) if out.is_dir() => {
                        let mut out_file =
                            out.join(source_path.file_name().expect("input has no filename"));
                        out_file.set_extension("json");
                        fs::write(&out_file, json).unwrap_or_else(|e| {
                            eprintln!("Failed to write '{}': {}", out_file.display(), e);
                            std::process::exit(1);
                        });
                    }
                    Some(out) => {
                        fs::write(out, json).unwrap_or_else(|e| {
                            eprintln!("Failed to write '{}': {}", out.display(), e);
                            std::process::exit(1);
                        });
                    }
                }
            }

            println!(
                "parsed file {}, took {:?}, noun creation time {:?}",
                source_path.display(),
                took,
                took2
            );
        }

        Err(errs) => {
            for err in errs {
                let span = err.span().into_range();
                let file_id = source_path.to_string_lossy().to_string();

                if let Err(err) = Report::build(ReportKind::Error, (file_id.clone(), span.clone()))
                    .with_config(ariadne::Config::new().with_index_type(ariadne::IndexType::Byte))
                    .with_label(
                        Label::new((file_id.clone(), span))
                            .with_message(err.reason().to_string())
                            .with_color(Color::Red),
                    )
                    .finish()
                    .eprint((file_id.clone(), Source::from(source.clone())))
                {
                    eprintln!("Failed to render parse error for '{}': {}", file_id, err);
                }
            }
        }
    };
}

fn main() {
    let cli = Cli::parse();

    if cli.test {
        run_test();
        return;
    }

    let input = cli.input.clone().unwrap_or_else(|| {
        eprintln!("Input file or directory is required unless --test");
        std::process::exit(2);
    });

    let inputs = collect_inputs(&input);

    let start = Instant::now();

    for source_path in inputs {
        run_parser(&source_path, cli.jam, !cli.no_dbug, cli.out.clone());
    }

    println!("total running time {:?} ", start.elapsed());
}
