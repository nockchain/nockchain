//! Property tests for hatch's ports of hoon-138 arms, with the arms themselves
//! as the oracle. `honk` compiles a tuple of hoon-138 gates against the
//! canonical prelude once; each case slams one of them in nockvm and compares
//! the product with hatch's answer on the same input.
//!
//! `PROPTEST_CASES` sets the number of cases per property (default 256).

use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use chumsky::prelude::end;
use chumsky::Parser;
use hatch::ast::hoon::{Coin, NounExpr, ParsedAtom};
use hatch::utils::{fein, nuck, rend_co, rend_crashes, taft, tuft, wood, yore};
use honk::native::hot::native_hot_state;
use nockapp::utils::{create_context, NOCK_STACK_SIZE};
use nockvm::ext::NounExt;
use nockvm::interpreter::{interpret, Context};
use nockvm::jets::cold::Cold;
use nockvm::mem::NockStack;
use nockvm::noun::{Noun, D, T};
use num_bigint::BigUint;
use proptest::prelude::*;

/// The oracle gates, in the order of the `Gate` indices.
const ORACLE: &str = "\
::  hoon-138 arms that hatch ports, for hoon138_props.rs
:*  |=([a=@ta b=@] (scot a b))
    |=(t=@t (rush t nuck:so))
    |=(t=@t (wood t))
    |=(t=@t (taft t))
    |=(c=@c (tuft c))
    |=(d=@da (yore d))
    |=(p=@ (fein:ob p))
    ~
==
";

#[derive(Clone, Copy)]
enum Gate {
    Scot = 0,
    Nuck = 1,
    Wood = 2,
    Taft = 3,
    Tuft = 4,
    Yore = 5,
    Fein = 6,
}

/// The `--arbitrary` artifact for `ORACLE`: a trap whose kick yields the tuple.
fn oracle_artifact() -> &'static [u8] {
    static ARTIFACT: OnceLock<Vec<u8>> = OnceLock::new();
    ARTIFACT.get_or_init(|| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let dir = tempfile::tempdir().expect("tempdir");
        let deps = dir.path().join("deps");
        fs::create_dir_all(&deps).expect("deps dir");
        let entry = deps.join("oracle.hoon");
        fs::write(&entry, ORACLE).expect("oracle source");
        let output = dir.path().join("oracle.jam");
        let status = Command::new(env!("CARGO_BIN_EXE_honk"))
            .current_dir(dir.path())
            .env("HOME", dir.path())
            .env("TMPDIR", dir.path())
            .args(["--arbitrary", "--output"])
            .arg(&output)
            .arg("--prelude")
            .arg(root.join("hoon/common/hoon.hoon"))
            .arg(&entry)
            .arg(&deps)
            .output()
            .expect("run honk");
        assert!(
            status.status.success(),
            "honk failed to build the oracle: {}",
            String::from_utf8_lossy(&status.stderr)
        );
        fs::read(output).expect("oracle artifact")
    })
}

struct Oracle {
    context: Context,
    gates: Noun,
}

impl Oracle {
    fn new() -> Self {
        let mut stack = NockStack::new(NOCK_STACK_SIZE, 0);
        let cold = Cold::new(&mut stack);
        let mut context = create_context(
            stack,
            native_hot_state(),
            cold,
            None,
            vec![],
            nockvm::jets::JetDispatchMode::HintBlind,
        );
        let trap = Noun::cue_bytes_slice(&mut context.stack, oracle_artifact()).expect("cue");
        let kick = T(&mut context.stack, &[D(9), D(2), D(0), D(1)]);
        let gates = interpret(&mut context, trap, kick).expect("kick the oracle trap");
        Self { context, gates }
    }

    /// Slams gate `gate` with `sample`; `None` if hoon-138 crashes.
    fn slam(&mut self, gate: Gate, sample: Noun) -> Option<Noun> {
        let space = self.context.stack.noun_space();
        // The tuple ends in `~`, so gate i is the head of the i-th tail.
        let axis = (2u64 << (gate as u64 + 1)) - 2;
        let gate_noun = self
            .gates
            .in_space(&space)
            .slot(axis)
            .expect("oracle gate")
            .noun();
        let stack = &mut self.context.stack;
        let quoted = T(stack, &[D(1), sample]);
        let edit = T(stack, &[D(6), quoted]);
        let whole = T(stack, &[D(0), D(1)]);
        let edited = T(stack, &[D(10), edit, whole]);
        let formula = T(stack, &[D(9), D(2), edited]);
        interpret(&mut self.context, gate_noun, formula).ok()
    }

    fn atom(&mut self, value: &BigUint) -> Noun {
        let parsed = ParsedAtom::from_biguint(value.clone());
        honk::native::noun::parsed_atom_to_noun(&mut self.context.stack, &parsed)
    }

    fn text(&mut self, text: &[u8]) -> Noun {
        self.atom(&BigUint::from_bytes_le(text))
    }

    fn big(&self, noun: Noun) -> BigUint {
        let space = self.context.stack.noun_space();
        let atom = noun.in_space(&space).as_atom().expect("atom");
        BigUint::from_bytes_le(&atom.to_le_bytes())
    }

    /// The bytes of a cord, empty for `0`.
    fn bytes(&self, noun: Noun) -> Vec<u8> {
        let value = self.big(noun);
        if value == BigUint::from(0u8) {
            Vec::new()
        } else {
            value.to_bytes_le()
        }
    }

    fn noun_expr(&self, noun: Noun) -> NounExpr {
        let space = self.context.stack.noun_space();
        match noun.in_space(&space).as_cell() {
            Ok(cell) => NounExpr::Cell(
                Box::new(self.noun_expr(cell.head().noun())),
                Box::new(self.noun_expr(cell.tail().noun())),
            ),
            Err(_) => NounExpr::ParsedAtom(ParsedAtom::from_biguint(self.big(noun))),
        }
    }

    /// Reads a hoon-138 `coin` noun into hatch's `Coin`.
    fn coin(&self, noun: Noun) -> Coin {
        let space = self.context.stack.noun_space();
        let cell = noun.in_space(&space).as_cell().expect("coin cell");
        let tag = self.big(cell.head().noun());
        let body = cell.tail().noun();
        if tag == BigUint::from(0u8) {
            let dime = body.in_space(&space).as_cell().expect("dime");
            let aura =
                String::from_utf8(self.big(dime.head().noun()).to_bytes_le()).expect("aura text");
            let aura = if aura == "\0" { String::new() } else { aura };
            return Coin::Dime(aura, ParsedAtom::from_biguint(self.big(dime.tail().noun())));
        }
        if tag == BigUint::from_bytes_le(b"blob") {
            return Coin::Blob(self.noun_expr(body));
        }
        let mut coins = Vec::new();
        let mut list = body;
        while let Ok(cell) = list.in_space(&space).as_cell() {
            coins.push(self.coin(cell.head().noun()));
            list = cell.tail().noun();
        }
        Coin::Many(coins)
    }

    /// Reads a `(unit coin)`.
    fn unit_coin(&self, noun: Noun) -> Option<Coin> {
        let space = self.context.stack.noun_space();
        let cell = noun.in_space(&space).as_cell().ok()?;
        Some(self.coin(cell.tail().noun()))
    }
}

thread_local! {
    static ORACLE_CELL: RefCell<Option<Oracle>> = const { RefCell::new(None) };
}

fn with_oracle<R>(f: impl FnOnce(&mut Oracle) -> R) -> R {
    ORACLE_CELL.with(|cell| {
        let mut cell = cell.borrow_mut();
        f(cell.get_or_insert_with(Oracle::new))
    })
}

/// `Coin` with atoms normalized, since hatch may hold a small value as `Big`.
fn canonical(coin: &Coin) -> String {
    fn noun(expr: &NounExpr) -> String {
        match expr {
            NounExpr::ParsedAtom(atom) => atom.to_biguint().to_string(),
            NounExpr::Cell(h, t) => format!("[{} {}]", noun(h), noun(t)),
        }
    }
    match coin {
        Coin::Dime(aura, atom) => format!("[%{aura} {}]", atom.to_biguint()),
        Coin::Blob(expr) => format!("[%blob {}]", noun(expr)),
        Coin::Many(coins) => {
            let inner: Vec<String> = coins.iter().map(canonical).collect();
            format!("[%many {}]", inner.join(" "))
        }
    }
}

fn hatch_nuck(text: &str) -> Option<Coin> {
    nuck().then_ignore(end()).parse(text).into_result().ok()
}

/// hoon-138's `(rush text nuck:so)`, canonicalized. A crash (as `++taft` does
/// on some `~-` knots) rejects the source just as a failed parse does, so it
/// reads as `None`.
fn hoon_nuck(text: &str) -> Option<String> {
    with_oracle(|o| {
        let cord = o.text(text.as_bytes());
        let parsed = o.slam(Gate::Nuck, cord)?;
        o.unit_coin(parsed).map(|c| canonical(&c))
    })
}

/// Atoms with a bit width drawn from 0 to `max_bits`.
fn atom(max_bits: u64) -> impl Strategy<Value = BigUint> {
    (0..=max_bits).prop_flat_map(|bits| {
        let bytes = bits.div_ceil(8) as usize;
        proptest::collection::vec(any::<u8>(), bytes).prop_map(move |raw| {
            let mut value = BigUint::from_bytes_le(&raw);
            if bits % 8 != 0 {
                value &= (BigUint::from(1u8) << bits) - 1u8;
            }
            value
        })
    })
}

/// An aura and a value in the range hoon-138 renders for it.
fn dime() -> impl Strategy<Value = (String, BigUint)> {
    let wide = ["ud", "ux", "ub", "uv", "uw", "sd", "sx", "sb", "sv", "sw", "p", "q", "da", "dr"];
    prop_oneof![
        (prop::sample::select(wide.to_vec()), atom(200)).prop_map(|(a, v)| (a.to_string(), v)),
        atom(1).prop_map(|v| ("f".to_string(), v)),
        Just(("n".to_string(), BigUint::from(0u8))),
        atom(32).prop_map(|v| ("if".to_string(), v)),
        atom(128).prop_map(|v| ("is".to_string(), v)),
        atom(16).prop_map(|v| ("rh".to_string(), v)),
        atom(32).prop_map(|v| ("rs".to_string(), v)),
        atom(64).prop_map(|v| ("rd".to_string(), v)),
        atom(128).prop_map(|v| ("rq".to_string(), v)),
        "[a-z][a-z0-9-]{0,8}"
            .prop_map(|s| ("tas".to_string(), BigUint::from_bytes_le(s.as_bytes()))),
        "[a-z0-9._~-]{0,10}".prop_map(|s| ("ta".to_string(), BigUint::from_bytes_le(s.as_bytes()))),
        text().prop_map(|t| ("t".to_string(), BigUint::from_bytes_le(&t))),
    ]
}

/// Strings shaped like knots of every kind, with enough slack to produce near
/// misses: ship names, absolute and relative dates, every number radix, floats,
/// IP addresses, text knots, and `._..__` lists.
fn knot_shape() -> impl Strategy<Value = String> {
    prop_oneof![
        "~[a-z]{3}(-?[a-z]{3,6}){0,4}",
        "~[0-9]{1,6}-?\\.[0-9]{1,3}\\.[0-9]{1,3}(\\.\\.[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}(\\.\\.[0-9a-fA-F]{1,5}(\\.[0-9a-f]{4}){0,4})?)?",
        "~[dhmsx][0-9]{1,8}(\\.[dhms][0-9]{1,3}){0,3}(\\.\\.[0-9a-f]{1,5}(\\.[0-9a-f]{4}){0,3})?",
        "-{0,2}[0-9]{1,4}(\\.[0-9]{3}){0,4}",
        "-{0,2}0[xbvwc][0-9a-zA-Z~-]{1,5}(\\.[0-9a-zA-Z~-]{4,5}){0,3}",
        "\\.(~{0,3})-?[0-9]{1,5}(\\.[0-9]{1,5})?(e-?[0-9]{1,4})?",
        "\\.(~{0,3})-?(inf|nan)",
        "\\.[0-9]{1,4}(\\.[0-9]{1,4}){2,4}",
        "\\.[0-9a-f]{1,5}(\\.[0-9a-f]{1,5}){6,8}",
        "\\.[yn]",
        "~~[a-z0-9~.-]{0,10}",
        "~\\.[a-z0-9._~-]{0,10}",
        "~-[a-z0-9~.-]{0,10}",
        "\\._([a-z0-9~.-]{1,6}_){0,3}_?",
        "~0[0-9a-zA-Z~-]{0,8}",
    ]
}

/// Cords: mostly printable text, with control bytes and multi-byte UTF-8.
fn text() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        "[ -~]{0,12}".prop_map(|s| s.into_bytes()),
        "\\PC{0,6}".prop_map(|s| s.into_bytes()),
        proptest::collection::vec(any::<u8>(), 0..8),
    ]
    .prop_map(|mut bytes| {
        // Trailing NUL bytes are not part of an atom.
        while bytes.last() == Some(&0) {
            bytes.pop();
        }
        bytes
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(
        std::env::var("PROPTEST_CASES").ok().and_then(|n| n.parse().ok()).unwrap_or(256)
    ))]

    /// `rend_co` renders a dime the way `++scot` does, and crashes exactly
    /// where it does (a `~~` or `~-` knot that `++wood` cannot escape).
    #[test]
    fn scot_matches_rend_co((aura, value) in dime()) {
        let coin = Coin::Dime(aura.clone(), ParsedAtom::from_biguint(value.clone()));
        let hoon = with_oracle(|o| {
            let aura_noun = o.text(aura.as_bytes());
            let value_noun = o.atom(&value);
            let sample = T(&mut o.context.stack, &[aura_noun, value_noun]);
            o.slam(Gate::Scot, sample).map(|text| o.bytes(text))
        });
        match hoon {
            None => prop_assert!(rend_crashes(&coin), "hoon-138 crashed on {coin:?}"),
            Some(text) => {
                prop_assert!(!rend_crashes(&coin), "hatch flags {coin:?} but hoon-138 renders it");
                prop_assert_eq!(
                    String::from_utf8_lossy(&text).into_owned(),
                    rend_co(&coin).concat(),
                    "{:?}", coin
                );
            }
        }
    }

    /// hatch's `nuck` parses what `++scot` renders into the same coin as
    /// hoon-138's `nuck:so`.
    #[test]
    fn nuck_reads_scot_output_like_hoon((aura, value) in dime()) {
        let coin = Coin::Dime(aura.clone(), ParsedAtom::from_biguint(value.clone()));
        // `++scot` crashes on these, so there is no text to read back.
        if rend_crashes(&coin) {
            return Ok(());
        }
        let text = rend_co(&coin).concat();
        let hoon = hoon_nuck(&text);
        prop_assert_eq!(hatch_nuck(&text).map(|c| canonical(&c)), hoon, "{}", text);
    }

    /// On arbitrary knot-alphabet strings, hatch's `nuck` accepts exactly what
    /// `nuck:so` accepts, with the same result.
    #[test]
    fn nuck_matches_hoon_on_random_knots(text in "[a-z0-9.~_-]{0,14}") {
        let hoon = hoon_nuck(&text);
        prop_assert_eq!(hatch_nuck(&text).map(|c| canonical(&c)), hoon, "{:?}", text);
    }

    /// On strings shaped like each kind of knot (and near misses of them),
    /// hatch's `nuck` agrees with `nuck:so`.
    #[test]
    fn nuck_matches_hoon_on_knot_shapes(text in knot_shape()) {
        let hoon = hoon_nuck(&text);
        prop_assert_eq!(hatch_nuck(&text).map(|c| canonical(&c)), hoon, "{:?}", text);
    }

    /// `wood` escapes a cord as `++wood` does, and crashes where it crashes.
    #[test]
    fn wood_matches_hoon(bytes in text()) {
        let atom = ParsedAtom::from_biguint(BigUint::from_bytes_le(&bytes));
        let crashes = rend_crashes(&Coin::Dime("t".to_string(), atom.clone()));
        let hoon = with_oracle(|o| {
            let cord = o.text(&bytes);
            o.slam(Gate::Wood, cord).map(|w| o.big(w))
        });
        match hoon {
            None => prop_assert!(crashes, "++wood crashed on {bytes:?}"),
            Some(escaped) => {
                prop_assert!(!crashes, "hatch flags {bytes:?} but ++wood escapes it");
                prop_assert_eq!(wood(&atom).to_biguint(), escaped, "{:?}", bytes);
            }
        }
    }

    /// `taft` decodes UTF-8 to `@c` as `++taft` does, failing where it crashes;
    /// `tuft` encodes back as `++tuft` does.
    #[test]
    fn taft_and_tuft_match_hoon(bytes in text()) {
        let atom = ParsedAtom::from_biguint(BigUint::from_bytes_le(&bytes));
        let hoon = with_oracle(|o| {
            let cord = o.text(&bytes);
            o.slam(Gate::Taft, cord).map(|c| o.big(c))
        });
        let ours = taft(&atom).map(|c| c.to_biguint());
        prop_assert_eq!(&ours, &hoon, "taft {:?}", bytes);
        if let Some(chars) = hoon {
            let back = with_oracle(|o| {
                let c = o.atom(&chars);
                o.slam(Gate::Tuft, c).map(|t| o.big(t))
            });
            prop_assert_eq!(
                Some(tuft(&ParsedAtom::from_biguint(chars.clone())).to_biguint()),
                back,
                "tuft {:?}", chars
            );
        }
    }

    /// `yore` splits an `@da` into the same date as `++yore`.
    #[test]
    fn yore_matches_hoon(value in atom(80)) {
        let date = yore(&ParsedAtom::from_biguint(value.clone()));
        let hoon = with_oracle(|o| {
            let da = o.atom(&value);
            let noun = o.slam(Gate::Yore, da).expect("++yore crashed");
            let space = o.context.stack.noun_space();
            let at = |axis: u64| noun.in_space(&space).slot(axis).expect("date field").noun();
            // [[a=? y=@ud] m=@ud d=@ud h=@ud m=@ud s=@ud f=(list @ux)]
            let era = o.big(at(4)) == BigUint::from(0u8);
            let fields: Vec<BigUint> = [5u64, 6, 14, 30, 62, 126].iter().map(|&a| o.big(at(a))).collect();
            let mut f = Vec::new();
            let mut list = at(127);
            while let Ok(cell) = list.in_space(&space).as_cell() {
                f.push(o.big(cell.head().noun()));
                list = cell.tail().noun();
            }
            (era, fields, f)
        });
        let (era, fields, f) = hoon;
        prop_assert_eq!(date.era, era, "era of {}", value);
        prop_assert_eq!(&date.y, &fields[0], "year of {}", value);
        prop_assert_eq!(BigUint::from(date.m), fields[1].clone(), "month of {}", value);
        prop_assert_eq!(&date.t.d, &fields[2], "day of {}", value);
        prop_assert_eq!(BigUint::from(date.t.h), fields[3].clone(), "hour of {}", value);
        prop_assert_eq!(BigUint::from(date.t.m), fields[4].clone(), "minute of {}", value);
        prop_assert_eq!(BigUint::from(date.t.s), fields[5].clone(), "second of {}", value);
        let ours: Vec<BigUint> = date.t.f.iter().map(|&x| BigUint::from(x)).collect();
        prop_assert_eq!(ours, f, "fraction of {}", value);
    }

    /// `fein` scrambles a ship name's atom as `fein:ob` does.
    #[test]
    fn fein_matches_hoon(value in atom(130)) {
        let hoon = with_oracle(|o| {
            let p = o.atom(&value);
            o.slam(Gate::Fein, p).map(|s| o.big(s))
        });
        prop_assert_eq!(Some(fein(ParsedAtom::from_biguint(value.clone())).to_biguint()), hoon, "{}", value);
    }
}
