use std::sync::Arc;

use chumsky::prelude::*;

use crate::ast::hoon::*;
use crate::utils::*;

/// One `/-`, `/+` or `/#` clause item (hoonc's `$taut`): `*name` imports
/// without a face, `face=name` renames, and a bare `name` is its own face.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportTaut {
    pub face: Option<String>,
    pub name: String,
}

/// One `/`-directive of a file's import header, as hoonc's `+pile-rule`
/// reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportDirective {
    /// `/?  138`: a kelvin pin, which hoonc parses and ignores.
    Kelvin,
    /// `/-  *a, b, c=d`
    Sur(Vec<ImportTaut>),
    /// `/+  *a, b, c=d`
    Lib(Vec<ImportTaut>),
    /// `/=  face  /path` or `/=  *  /path`
    Raw {
        face: Option<String>,
        path: Vec<String>,
    },
    /// `/*  face  %mark  /path/ext`
    Bar {
        face: String,
        mark: String,
        path: Vec<String>,
    },
    /// `/#  *a, b, c=d`
    Dat(Vec<ImportTaut>),
}

/// A file's leading import directives and the byte offset where its body
/// starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportHeader {
    pub directives: Vec<ImportDirective>,
    pub body_start: usize,
}

/// hoonc's `stap`: `/`-separated `urs` segments whose last one is non-empty.
fn import_path<'src>() -> impl Parser<'src, &'src str, Vec<String>, Err<'src>> {
    let segment = any()
        .filter(|c: &char| matches!(c, '0'..='9' | 'a'..='z' | '-' | '.' | '~' | '_'))
        .repeated()
        .collect::<String>();
    just('/')
        .ignore_then(
            segment
                .separated_by(just('/'))
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .try_map(|path: Vec<String>, span| {
            if path.last().is_some_and(|last| !last.is_empty()) {
                Ok(path)
            } else {
                Err(Rich::custom(span, "import path must not end in '/'"))
            }
        })
}

/// hoonc's `taut-rule` items separated by `;~(plug com gaw)`.
fn import_tauts<'src>() -> impl Parser<'src, &'src str, Vec<ImportTaut>, Err<'src>> {
    let taut = choice((
        just('*')
            .ignore_then(symbol())
            .map(|name| ImportTaut { face: None, name }),
        symbol()
            .then_ignore(just('='))
            .then(symbol())
            .map(|(face, name)| ImportTaut {
                face: Some(face),
                name,
            }),
        symbol().map(|name| ImportTaut {
            face: Some(name.clone()),
            name,
        }),
    ));
    taut.separated_by(just(',').then(gaw()))
        .at_least(1)
        .collect::<Vec<_>>()
}

/// One import directive after its leading `/`, following the rune rules in
/// hoonc's `+pile-rule`. The `gap` between rune and arguments may span lines
/// and comments, as may the `,` separators of a `/-`, `/+` or `/#` list.
pub fn import_directive<'src>() -> impl Parser<'src, &'src str, ImportDirective, Err<'src>> {
    let kelvin = just('?')
        .ignore_then(gap())
        .ignore_then(text::digits(10))
        .to(ImportDirective::Kelvin);
    let sur = just('-')
        .ignore_then(gap())
        .ignore_then(import_tauts())
        .map(ImportDirective::Sur);
    let lib = just('+')
        .ignore_then(gap())
        .ignore_then(import_tauts())
        .map(ImportDirective::Lib);
    let raw = just('=')
        .ignore_then(gap())
        .ignore_then(just('*').to(None).or(symbol().map(Some)))
        .then_ignore(gap())
        .then(import_path())
        .map(|(face, path)| ImportDirective::Raw { face, path });
    let bar = just('*')
        .ignore_then(gap())
        .ignore_then(symbol())
        .then_ignore(gap())
        .then(just('%').ignore_then(symbol()))
        .then_ignore(gap())
        .then(import_path())
        .map(|((face, mark), path)| ImportDirective::Bar { face, mark, path });
    let dat = just('#')
        .ignore_then(gap())
        .ignore_then(import_tauts())
        .map(ImportDirective::Dat);
    choice((kelvin, sur, lib, raw, bar, dat)).labelled("Import Directive")
}

/// Directives separated by `gap`, each starting with `/`.
fn import_directives<'src>() -> impl Parser<'src, &'src str, Vec<ImportDirective>, Err<'src>> {
    just('/')
        .ignore_then(import_directive())
        .separated_by(gap())
        .collect::<Vec<_>>()
}

/// Read the import header that opens `source`: leading whitespace and
/// comments, then as many import directives as parse. The body starts after
/// the `gap` that follows the last directive. A malformed directive ends the
/// header, so callers should check whether the body starts with one.
pub fn parse_import_header(source: &str) -> ImportHeader {
    let header = gap()
        .or_not()
        .ignore_then(import_directives())
        .then_ignore(gap().or_not())
        .map_with(|directives, e| (directives, e.span().end))
        .then_ignore(any().repeated());
    match header.parse(source).into_output() {
        Some((directives, body_start)) => ImportHeader {
            directives,
            body_start,
        },
        None => ImportHeader {
            directives: Vec::new(),
            body_start: 0,
        },
    }
}

pub fn fas_runes_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
    wer: Path,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    // The leading `/` is already consumed. Skip the rest of the import
    // header; honk resolves the imports themselves from the same grammar.
    let skip_imports = import_directive()
        .then(
            gap()
                .then(just('/'))
                .ignore_then(import_directive())
                .repeated(),
        )
        .ignored()
        .then_ignore(gap().repeated())
        .ignore_then(hoon.clone());

    let _ = (hoon_wide, wer, linemap);
    skip_imports.boxed()
}

pub fn fastis<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .ignore_then(gap())
        .ignore_then(hoon_wide.clone())
        .ignore_then(gap())
        .ignore_then(hoon.clone())
}

pub fn fastar<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon_wide.clone())
        .ignore_then(gap())
        .ignore_then(hoon_wide.clone())
        .ignore_then(gap())
        .ignore_then(hoon_wide.clone())
        .ignore_then(gap())
        .ignore_then(hoon.clone())
}

pub fn fashax<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon_wide.clone())
        .ignore_then(gap())
        .ignore_then(hoon.clone())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chumsky::Parser;

    use super::*;

    fn lib(face: Option<&str>, name: &str) -> ImportTaut {
        ImportTaut {
            face: face.map(str::to_string),
            name: name.to_string(),
        }
    }

    fn path(segments: &[&str]) -> Vec<String> {
        segments.iter().map(|segment| segment.to_string()).collect()
    }

    #[test]
    fn import_header_reads_pile_rule_directives() {
        let src = concat!(
            "::  leading comment\n", "\n", "/?  138\n", "/-  *types, t=things\n",
            "/+  *alpha,  ::  a comment inside the list\n", "    beta,\n", "gamma\n",
            "::  a comment between directives\n", "/=  *  /common/wrapper\n",
            "  /=  w  /common/wrapper\n", "/*  blob  %jam  /dat/blob/jam\n", "/#  konst\n", "::\n",
            "|%  --\n",
        );
        let header = parse_import_header(src);

        assert_eq!(
            header.directives,
            vec![
                ImportDirective::Kelvin,
                ImportDirective::Sur(vec![lib(None, "types"), lib(Some("t"), "things")]),
                ImportDirective::Lib(vec![
                    lib(None, "alpha"),
                    lib(Some("beta"), "beta"),
                    lib(Some("gamma"), "gamma"),
                ]),
                ImportDirective::Raw {
                    face: None,
                    path: path(&["common", "wrapper"]),
                },
                ImportDirective::Raw {
                    face: Some("w".to_string()),
                    path: path(&["common", "wrapper"]),
                },
                ImportDirective::Bar {
                    face: "blob".to_string(),
                    mark: "jam".to_string(),
                    path: path(&["dat", "blob", "jam"]),
                },
                ImportDirective::Dat(vec![lib(Some("konst"), "konst")]),
            ]
        );
        assert_eq!(&src[header.body_start..], "|%  --\n");
    }

    #[test]
    fn import_header_stops_before_a_malformed_directive() {
        // hoonc's `gap` is two spaces, a newline or a comment, so a single
        // space after the rune is a syntax error, as is a `/=` without a path.
        for (src, rest) in [
            ("/+ alpha\n|%  --\n", "/+ alpha\n|%  --\n"),
            (
                "/+  alpha\n/=  onlyface\n|%  --\n", "/=  onlyface\n|%  --\n",
            ),
        ] {
            let header = parse_import_header(src);
            assert_eq!(&src[header.body_start..], rest, "{src:?}");
        }
    }

    #[test]
    fn import_header_is_empty_without_directives() {
        let src = "::  just a comment\n|%  --\n";
        let header = parse_import_header(src);
        assert!(header.directives.is_empty());
        assert_eq!(&src[header.body_start..], "|%  --\n");
    }

    /// hoonc's `+pile-rule` strips the whole import header before `++vest`
    /// parses the body, so the body carries exactly one `%dbug` and it starts
    /// at the body, whichever import runes open the file.
    #[test]
    fn body_spot_starts_after_every_import_header_form() {
        for header in [
            "/-  *types\n", "/+  *alpha\n", "/+  alpha, b=beta\n", "/=  *  /lib/alpha\n",
            "/*  blob  %jam  /dat/blob/jam\n", "/#  konst\n", "/?  138\n/+  *alpha\n",
            "::  file comment\n::\n/+  *alpha\n", "/+  *alpha\n::  comment\n/+  *beta\n",
            "/+  *alpha\n\n/+  *beta\n", "/+  *alpha,  ::  first\n    *beta\n",
            "/+  *alpha,\n*beta\n", "/-  *types\n  /+  *alpha\n",
        ] {
            let src = format!("{header}alpha-val\n");
            let body_line = header.lines().count() as u64 + 1;
            let linemap = Arc::new(LineMap::new(&src));
            let parsed = crate::native_parser(
                vec!["app".to_string(), "entry.hoon".to_string()],
                true,
                linemap,
            )
            .parse(src.as_str())
            .into_result()
            .unwrap_or_else(|errors| panic!("{header:?} failed to parse: {errors:?}"));

            let Hoon::TisSig(items) = parsed else {
                panic!("{header:?}: expected a top-level TisSig");
            };
            let [Hoon::Dbug(spot, inner)] = items.as_slice() else {
                panic!("{header:?}: expected one spotted body, got {items:?}");
            };
            assert_eq!(spot.q.p, (body_line, 1), "{header:?}: body spot start");
            assert!(
                !matches!(**inner, Hoon::Dbug(..)),
                "{header:?}: the import header added a second %dbug"
            );
        }
    }
}
