//  Sail markdown, hoon-138 `++cram`. A block of markdown is read line by
//  line (`++main`): a stack of open containers (lists, list items, block
//  quotes, verse, headings) and the lines of the current paragraph. Each
//  paragraph is recomposed with its indentation and reparsed as inline
//  markdown (`++para`, `++head`) from the position where it began; `;`
//  lines are sail, and `---` and ``` blocks are rules and code.

use chumsky::input::InputRef;

use super::*;

/// An inline markdown element, hoon-138 `graf`.
#[derive(Clone, Debug)]
enum Graf {
    Bold(Vec<Graf>),
    Talc(Vec<Graf>),
    Quod(Vec<Graf>),
    Code(Vec<u8>),
    Text(Vec<u8>),
    Link(Vec<Graf>, Vec<u8>),
    Mage(Vec<u8>, Vec<u8>),
    Expr(Tuna),
}

fn element(g: Marx, c: Marl) -> Tuna {
    Tuna::Manx(Manx { g, c })
}

fn bytes_beers(bytes: &[u8]) -> Vec<Beer> {
    bytes.iter().copied().map(byte_beer).collect()
}

/// `++down`: consecutive text becomes one text node.
fn down(gaf: Vec<Graf>) -> Marl {
    let mut out = Vec::new();
    let mut text: Option<Vec<u8>> = None;
    for graf in gaf {
        match graf {
            Graf::Text(bytes) => text.get_or_insert_with(Vec::new).extend(bytes),
            other => {
                if let Some(bytes) = text.take() {
                    out.push(text_node(&bytes));
                }
                out.extend(down_item(other));
            }
        }
    }
    if let Some(bytes) = text {
        out.push(text_node(&bytes));
    }
    out
}

/// `++item` of `++down`.
fn down_item(graf: Graf) -> Marl {
    match graf {
        Graf::Text(bytes) => vec![text_node(&bytes)],
        Graf::Expr(tuna) => vec![tuna],
        Graf::Bold(gaf) => vec![element(tag("b"), down(gaf))],
        Graf::Talc(gaf) => vec![element(tag("i"), down(gaf))],
        Graf::Code(bytes) => vec![element(tag("code"), vec![text_node(&bytes)])],
        //  smart quotes, U+201C and U+201D
        Graf::Quod(gaf) => {
            let mut quoted = vec![Graf::Text("\u{201c}".as_bytes().to_vec())];
            quoted.extend(gaf);
            quoted.push(Graf::Text("\u{201d}".as_bytes().to_vec()));
            down(quoted)
        }
        Graf::Link(gaf, url) => {
            let head = Marx {
                n: Mane::Tag("a".to_string()),
                a: vec![(Mane::Tag("href".to_string()), bytes_beers(&url))],
            };
            vec![element(head, down(gaf))]
        }
        Graf::Mage(alt, url) => {
            let mut a = vec![(Mane::Tag("src".to_string()), bytes_beers(&url))];
            if !alt.is_empty() {
                a.push((Mane::Tag("alt".to_string()), bytes_beers(&alt)));
            }
            let head = Marx {
                n: Mane::Tag("img".to_string()),
                a,
            };
            vec![element(head, vec![])]
        }
    }
}

/// `++contents-to-id`: a heading's text as an element id.
fn contents_to_id(kids: &[Tuna]) -> Vec<u8> {
    fn collect(kids: &[Tuna], out: &mut Vec<u8>) {
        for kid in kids {
            let Tuna::Manx(manx) = kid else {
                continue;
            };
            match manx.g.a.as_slice() {
                [(Mane::Tag(attr), beers)]
                    if manx.g.n == Mane::Tag(String::new())
                        && attr.is_empty()
                        && manx.c.is_empty() =>
                {
                    for beer in beers {
                        if let Beer::Char(text) = beer {
                            out.extend(text.chars().map(|c| c as u32 as u8));
                        }
                    }
                }
                _ => collect(&manx.c, out),
            }
        }
    }
    let mut raw = Vec::new();
    collect(kids, &mut raw);
    raw.into_iter()
        .map(|byte| match byte {
            b'a'..=b'z' | b'0'..=b'9' => byte,
            b'A'..=b'Z' => byte + 32,
            _ => b'-',
        })
        .collect()
}

/// `++whit`: spaces and newlines.
fn whit<'src>() -> impl Parser<'src, &'src str, (), Err<'src>> + Clone {
    one_of(" \n").repeated().at_least(1)
}

/// A character of non-control bytes other than a space.
fn prn_not_ace<'src>() -> impl Parser<'src, &'src str, char, Err<'src>> + Clone {
    any().filter(|c: &char| is_prn(*c) && *c != ' ')
}

/// `++cash`: raw text up to an unescaped `tem`, and where it starts.
fn cash<'src>(tem: char) -> impl Parser<'src, &'src str, (&'src str, usize), Err<'src>> + Clone {
    choice((
        whit(),
        just('\\').then(just(tem)).ignored(),
        any()
            .filter(move |c: &char| is_prn(*c) && *c != tem)
            .ignored(),
    ))
    .repeated()
    .to_slice()
    .map_with(|text, extra| {
        let span: SimpleSpan = extra.span();
        (text, span.start)
    })
}

/// `++calf`: text up to an unescaped `tem`, reading `\tem` as `tem`.
fn calf<'src>(tem: char) -> impl Parser<'src, &'src str, Vec<u8>, Err<'src>> + Clone {
    choice((
        just('\\')
            .ignore_then(just(tem))
            .map(|c: char| vec![c as u8]),
        any()
            .filter(move |c: &char| is_prn(*c) && *c != tem)
            .map(utf8_bytes),
    ))
    .repeated()
    .collect::<Vec<Vec<u8>>>()
    .map(|chunks| chunks.concat())
}

/// The inline markdown parsers over one text.
struct Inline<'src> {
    werk: Boxed<'src, Vec<Graf>>,
    para: Boxed<'src, Marl>,
    head: Boxed<'src, Marl>,
}

/// `++word`, `++werk`, `++para` and `++head`, for a text that `linemap`
/// maps to its position in the file.
fn inline_parsers<'src>(ctx: &SailCtx, linemap: Arc<LineMap>) -> Inline<'src> {
    let (hoon, hoon_wide, hoon_no_trace, hoon_wide_no_trace) =
        crate::parser_main::grammar(ctx.wer.clone(), linemap.clone());
    let (hoon, hoon_wide) = if ctx.trace {
        (hoon, hoon_wide)
    } else {
        (hoon_no_trace, hoon_wide_no_trace)
    };
    let inline_embed =
        sail_parsers(hoon, hoon_wide.clone(), linemap.clone(), ctx.clone()).inline_embed;

    //  ++cool: reparse the fenced text as `++werk` from where it starts
    let cool = |tem: char| {
        let ctx = ctx.clone();
        let linemap = linemap.clone();
        cash(tem).try_map(move |(text, start), span| {
            reparse_werk(&ctx, linemap.docs_enabled(), text, linemap.hair(start))
                .ok_or_else(|| Rich::custom(span, "sail markdown span"))
        })
    };
    //  whitespace, or nothing at the start of a line (++easy-sol)
    let sol_linemap = linemap.clone();
    let lead = choice((
        whit().to(b" ".to_vec()),
        empty()
            .map_with(move |_, extra| {
                let span: SimpleSpan = extra.span();
                sol_linemap.hair(span.start).1
            })
            .try_map(|col, span| {
                if col == 1 {
                    Ok(Vec::new())
                } else {
                    Err(Rich::custom(span, "not at the start of a line"))
                }
            }),
    ));
    //  a hoon constant (++bisk, ++tash, ++perd, ++twid, `%` and ++nuck)
    let alp = any().filter(|c: &char| c.is_ascii_alphanumeric() || *c == '-');
    let constant = choice((
        just('0').then(alp).rewind().ignore_then(number()).ignored(),
        just('-').rewind().ignore_then(number()).ignored(),
        just('.').ignore_then(perd()).ignored(),
        just('~').ignore_then(twid().or_not()).ignored(),
        just('%').ignore_then(choice((
            symbol().ignored(),
            just('$').ignored(),
            just('&').ignored(),
            just('|').ignored(),
            cord(linemap.clone()).ignored(),
            nuck().ignored(),
        ))),
    ));
    let url = just('(').ignore_then(cash(')')).then_ignore(just(')'));

    let word = choice((
        //  ordinary word
        any()
            .filter(|c: &char| c.is_ascii_alphabetic())
            .then(
                any()
                    .filter(|c: &char| c.is_ascii_alphanumeric() || *c == '-')
                    .repeated(),
            )
            .to_slice()
            .map(|word: &str| vec![Graf::Text(word.as_bytes().to_vec())]),
        //  naked \escape
        just('\\')
            .ignore_then(prn_not_ace())
            .map(|c| vec![Graf::Text(utf8_bytes(c))]),
        //  trailing \ to add <br>
        just("\\\n").to(vec![Graf::Expr(element(tag("br"), vec![]))]),
        //  *bold*, _italic_, "quoted"
        just('*')
            .ignore_then(cool('*'))
            .then_ignore(just('*'))
            .map(|gaf| vec![Graf::Bold(gaf)]),
        just('_')
            .ignore_then(cool('_'))
            .then_ignore(just('_'))
            .map(|gaf| vec![Graf::Talc(gaf)]),
        just('"')
            .ignore_then(cool('"'))
            .then_ignore(just('"'))
            .map(|gaf| vec![Graf::Quod(gaf)]),
        //  `code`
        just('`')
            .ignore_then(calf('`'))
            .then_ignore(just('`'))
            .map(|code| vec![Graf::Code(code)]),
        //  ++arm, +$arm, +*arm, ++arm:core, ...
        just('+')
            .then(one_of("+$*"))
            .then(any().filter(|c: &char| c.is_ascii_lowercase()))
            .then(
                any()
                    .filter(|c: &char| {
                        c.is_ascii_digit() || c.is_ascii_lowercase() || *c == '-' || *c == ':'
                    })
                    .repeated(),
            )
            .to_slice()
            .map(|code: &str| vec![Graf::Code(code.as_bytes().to_vec())]),
        //  [link](url)
        just('[')
            .ignore_then(cool(']'))
            .then_ignore(just(']'))
            .then_ignore(whit().or_not())
            .then(url.clone())
            .map(|(gaf, (url, _))| vec![Graf::Link(gaf, url.as_bytes().to_vec())]),
        //  ![alt](url)
        just('!')
            .ignore_then(just('[').ignore_then(cash(']')).then_ignore(just(']')))
            .then_ignore(whit().or_not())
            .then(url)
            .map(|((alt, _), (url, _))| {
                vec![Graf::Mage(alt.as_bytes().to_vec(), url.as_bytes().to_vec())]
            }),
        //  #hoon
        lead.clone()
            .then(just('#').ignore_then(hoon_wide.to_slice()))
            .then_ignore(whit().rewind())
            .map(|(text, code)| vec![Graf::Text(text), Graf::Code(code.as_bytes().to_vec())]),
        //  a hoon constant
        lead.then(constant.to_slice())
            .then_ignore(whit().rewind())
            .map(|(text, code)| vec![Graf::Text(text), Graf::Code(code.as_bytes().to_vec())]),
        //  whitespace
        whit().to(vec![Graf::Text(b" ".to_vec())]),
        //  {interpolated} sail
        inline_embed.map(|tuna| vec![Graf::Expr(tuna)]),
        //  just a byte
        prn_not_ace().map(|c| vec![Graf::Text(utf8_bytes(c))]),
    ));
    let werk = word
        .repeated()
        .collect::<Vec<Vec<Graf>>>()
        .map(|chunks| chunks.concat())
        .boxed();

    let para = whit()
        .or_not()
        .ignore_then(werk.clone())
        .map(|gaf| {
            let tarp = down(gaf);
            if tarp.is_empty() {
                vec![]
            } else {
                vec![element(tag("p"), tarp)]
            }
        })
        .boxed();
    let head = just(' ')
        .repeated()
        .ignore_then(just('#').repeated().at_least(1).at_most(6).count())
        .then_ignore(whit())
        .then(werk.clone())
        .map(|(level, gaf)| {
            let kids = down(gaf);
            let id = contents_to_id(&kids);
            let head = Marx {
                n: Mane::Tag(format!("h{level}")),
                a: vec![(Mane::Tag("id".to_string()), bytes_beers(&id))],
            };
            vec![element(head, kids)]
        })
        .boxed();
    Inline { werk, para, head }
}

/// hoon-138 `++cool`: parse `text`, which begins at `origin` of the file,
/// entirely as `++werk`.
fn reparse_werk(ctx: &SailCtx, docs: bool, text: &str, origin: (u64, u64)) -> Option<Vec<Graf>> {
    let linemap = Arc::new(LineMap::with_origin(text, docs, origin));
    inline_parsers(ctx, linemap)
        .werk
        .then_ignore(end())
        .parse(text)
        .into_result()
        .ok()
}

/// A paragraph (or heading) of markdown, recomposed as `text` and parsed
/// entirely from `origin`.
fn reparse_block(
    ctx: &SailCtx,
    docs: bool,
    text: &str,
    origin: (u64, u64),
    heading: bool,
) -> Option<Marl> {
    let linemap = Arc::new(LineMap::with_origin(text, docs, origin));
    let inline = inline_parsers(ctx, linemap);
    let block = if heading { inline.head } else { inline.para };
    block.then_ignore(end()).parse(text).into_result().ok()
}

/// `++hrul`: a horizontal rule.
fn hrul<'src>() -> impl Parser<'src, &'src str, Marl, Err<'src>> {
    just(' ')
        .repeated()
        .then(just("---"))
        .then(just('-').repeated())
        .then(just('\n'))
        .to(vec![element(tag("hr"), vec![])])
}

/// `++fens`: a code block fenced by ``` lines, its lines indented to
/// column `col`.
fn fens<'src>(col: u64) -> impl Parser<'src, &'src str, Marl, Err<'src>> {
    let ind = just(' ').repeated().exactly(col.saturating_sub(1) as usize);
    let tics = just("```\n");
    let line = choice((
        ind.clone()
            .ignore_then(tics.clone().not())
            .ignore_then(any().filter(|c: &char| is_prn(*c)).repeated().to_slice())
            .then_ignore(just('\n'))
            .map(|line: &str| line.as_bytes().to_vec()),
        just(' ').repeated().then(just('\n')).to(Vec::new()),
    ));
    just(' ')
        .repeated()
        .then(tics.clone())
        .ignore_then(line.repeated().collect::<Vec<Vec<u8>>>())
        .then_ignore(ind.then(tics))
        .map(|lines| {
            let text: Vec<u8> = lines
                .into_iter()
                .flat_map(|mut line| {
                    line.push(b'\n');
                    line
                })
                .collect();
            vec![element(tag("pre"), vec![text_node(&text)])]
        })
}

/// Container kinds, hoon-138 `mite`.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Mite {
    Down,
    Lunt,
    Lime,
    Lord,
    Poem,
    Bloc,
    Head,
}

/// Line kinds, hoon-138 `trig-style`.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Sty {
    EndDone,
    EndStet,
    EndDent,
    OneRule,
    OneFens,
    OneExpr,
    NewLite,
    NewLint,
    NewHead,
    NewBloc,
    NewPoem,
    OldText,
}

impl Sty {
    fn is_end(self) -> bool {
        matches!(self, Sty::EndDone | Sty::EndStet | Sty::EndDent)
    }
}

/// A container under construction. `q` holds its children in order; hoon
/// keeps them reversed, so what it prepends is appended here reversed.
struct Item {
    mite: Mite,
    q: Vec<Tuna>,
}

/// How `read-line` ended: a line, a line before `==` or an outdent, or an
/// error.
enum ReadLine {
    Line(Vec<u8>),
    Stop(Vec<u8>),
    Fail,
}

/// hoon-138 `++main` of `++cram`.
struct Machine<'m, 'src> {
    //  the text from where the markdown begins, and its offset in the input
    rest: &'m [u8],
    base: usize,
    pos: usize,
    loc: (u64, u64),
    failed: bool,
    out: u64,
    inr: u64,
    hac: Vec<Item>,
    cur: Item,
    par: Option<((u64, u64), Vec<Vec<u8>>)>,
    expr: &'m Boxed<'src, Marl>,
    linemap: &'m LineMap,
    ctx: &'m SailCtx,
}

type Input<'src, 'p> = InputRef<'src, 'p, &'src str, Err<'src>>;

/// Move the input forward to byte `offset`.
fn advance_to(inp: &mut Input<'_, '_>, offset: usize) {
    while *inp.cursor().inner() < offset {
        if inp.next().is_none() {
            break;
        }
    }
}

impl<'m, 'src> Machine<'m, 'src> {
    /// `++look`: the kind of the line at `pos`, `None` if it is blank.
    fn look(&self, pos: usize, loc: (u64, u64)) -> Option<(u64, Sty)> {
        let mut at = pos;
        while self.rest.get(at) == Some(&b' ') {
            at += 1;
        }
        let col = loc.1 + (at - pos) as u64;
        let text = &self.rest[at..];
        let hashes = text.iter().take_while(|&&b| b == b'#').count();
        let sty = match text {
            [b'\n', ..] => return None,
            [] => Sty::EndDone,
            [b'=', b'=', ..] => Sty::EndStet,
            [b'-', b'-', b'-', ..] => Sty::OneRule,
            [b'`', b'`', b'`', ..] => Sty::OneFens,
            [b';', ..] => Sty::OneExpr,
            _ if hashes > 0 && text.get(hashes) == Some(&b' ') => Sty::NewHead,
            [b'-', b' ', ..] => Sty::NewLint,
            [b'+', b' ', ..] => Sty::NewLite,
            [b'>', b' ', ..] => Sty::NewBloc,
            _ => Sty::OldText,
        };
        //  an outdented line ends the markdown
        if !sty.is_end() && col < self.out {
            return Some((col, Sty::EndDent));
        }
        Some((col, sty))
    }

    fn cur_indent(&self) -> u64 {
        match self.cur.mite {
            Mite::Down => 2,
            Mite::Head => 0,
            Mite::Lunt => 0,
            Mite::Lime => 2,
            Mite::Lord => 0,
            Mite::Poem => 8,
            Mite::Bloc => 2,
        }
    }

    /// `++back`: close containers until column `luc`.
    fn back(&mut self, luc: u64) {
        while luc < self.inr {
            let nex = self.cur_indent();
            if nex > self.inr - luc {
                self.inr = luc;
                self.failed = true;
                return;
            }
            self.close_item();
            self.inr -= nex;
        }
    }

    /// `++cur-to-tarp`
    fn cur_to_tarp(item: Item) -> Marl {
        let name = match item.mite {
            Mite::Down | Mite::Head => return item.q,
            Mite::Lunt => "ul",
            Mite::Lord => "ol",
            Mite::Lime => "li",
            Mite::Poem => "div",
            Mite::Bloc => "blockquote",
        };
        vec![element(tag(name), item.q)]
    }

    /// `++close-item`: finish the current container into its parent.
    fn close_item(&mut self) {
        let Some(parent) = self.hac.pop() else {
            return;
        };
        let done = std::mem::replace(&mut self.cur, parent);
        self.cur.q.extend(Self::cur_to_tarp(done).into_iter().rev());
    }

    fn push(&mut self, mite: Mite) {
        let cur = std::mem::replace(&mut self.cur, Item { mite, q: vec![] });
        self.hac.push(cur);
    }

    /// `++read-line`: the rest of this line past the inner indentation,
    /// without trailing spaces.
    fn read_line(&mut self) -> ReadLine {
        let mut lin = Vec::new();
        loop {
            let Some(&byte) = self.rest.get(self.pos) else {
                return ReadLine::Fail;
            };
            if byte != b'\n' {
                if self.inr > self.loc.1 {
                    if byte != b' ' {
                        return ReadLine::Fail;
                    }
                } else {
                    lin.push(byte);
                }
                self.pos += 1;
                self.loc.1 += 1;
                continue;
            }
            while lin.last() == Some(&b' ') {
                lin.pop();
            }
            let next = (self.pos + 1, (self.loc.0 + 1, 1));
            if let Some((_, Sty::EndStet | Sty::EndDent)) = self.look(next.0, next.1) {
                return ReadLine::Stop(lin);
            }
            (self.pos, self.loc) = next;
            return ReadLine::Line(lin);
        }
    }

    /// `++close-par`: make the paragraph into nodes.
    fn close_par(&mut self) -> Result<(), ()> {
        let Some((origin, lines)) = self.par.take() else {
            return Ok(());
        };
        if self.cur.mite == Mite::Poem {
            //  each line of verse is a paragraph; stanzas are split by <br>
            if !self.cur.q.is_empty() {
                self.cur.q.push(element(tag("br"), vec![]));
            }
            for mut line in lines {
                line.push(b'\n');
                self.cur.q.push(element(tag("p"), vec![text_node(&line)]));
            }
            self.inr = self.inr.saturating_sub(8);
            self.close_item();
            return Ok(());
        }
        let indent = " ".repeat(self.inr.saturating_sub(1) as usize);
        let mut text = String::new();
        for line in &lines {
            text.push_str(&indent);
            text.push_str(std::str::from_utf8(line).map_err(|_| ())?);
            text.push('\n');
        }
        let heading = self.cur.mite == Mite::Head;
        let tarp = reparse_block(
            self.ctx,
            self.linemap.docs_enabled(),
            &text,
            origin,
            heading,
        )
        .ok_or(())?;
        self.cur.q.extend(tarp.into_iter().rev());
        if heading {
            self.close_item();
        }
        Ok(())
    }

    /// `++parse-block`: run a leaf parser from here in the input.
    fn parse_block(&mut self, inp: &mut Input<'src, '_>, sty: Sty) {
        advance_to(inp, self.base + self.pos);
        let result = match sty {
            Sty::OneExpr => inp.parse(self.expr.clone()),
            Sty::OneRule => inp.parse(hrul()),
            _ => inp.parse(fens(self.inr)),
        };
        match result {
            Ok(tarp) => {
                let offset = *inp.cursor().inner();
                self.pos = offset - self.base;
                self.loc = self.linemap.hair(offset);
                self.cur.q.extend(tarp);
            }
            Err(_) => self.failed = true,
        }
    }

    /// `++entr`: enter a container whose marker is 2 columns wide.
    fn entr(&mut self, mite: Mite) {
        self.inr += 2;
        let Some(skip) = self.inr.checked_sub(self.loc.1) else {
            self.failed = true;
            return;
        };
        self.pos = (self.pos + skip as usize).min(self.rest.len());
        self.loc.1 = self.inr;
        self.push(mite);
    }

    /// `++open-item`
    fn open_item(&mut self, sty: Sty) {
        match sty {
            Sty::NewPoem => self.push(Mite::Poem),
            Sty::NewHead => self.push(Mite::Head),
            Sty::NewBloc => self.entr(Mite::Bloc),
            Sty::NewLint | Sty::NewLite => {
                let list = if sty == Sty::NewLint {
                    Mite::Lunt
                } else {
                    Mite::Lord
                };
                if self.cur.mite != list {
                    self.push(list);
                }
                self.entr(Mite::Lime);
            }
            _ => {}
        }
    }

    /// `++line`: the body line loop.
    fn line(&mut self, inp: &mut Input<'src, '_>) {
        while !self.failed {
            let Some((col, mut sty)) = self.look(self.pos, self.loc) else {
                //  a blank line breaks the paragraph
                match self.read_line() {
                    ReadLine::Line(_) => {}
                    ReadLine::Stop(_) => return,
                    ReadLine::Fail => {
                        self.failed = true;
                        return;
                    }
                }
                if self.close_par().is_err() {
                    self.failed = true;
                }
                continue;
            };
            if sty.is_end() {
                self.loc.1 = col;
                return;
            }
            if self.out == 0 {
                self.out = col;
                self.inr = col;
            }
            let open = match self.par {
                None => true,
                Some(_) => {
                    matches!(self.cur.mite, Mite::Down | Mite::Lime | Mite::Bloc)
                        && (sty != Sty::OldText || col > self.inr)
                }
            };
            if open {
                if self.close_par().is_err() {
                    self.failed = true;
                    return;
                }
                self.back(col);
                if self.failed {
                    return;
                }
                match col - self.inr {
                    0 => {}
                    8 => sty = Sty::NewPoem,
                    _ => {
                        self.failed = true;
                        return;
                    }
                }
                self.inr = col;
                //  unless adding a matching item, close lists
                if (self.cur.mite == Mite::Lunt && sty != Sty::NewLint)
                    || (self.cur.mite == Mite::Lord && sty != Sty::NewLite)
                {
                    self.close_item();
                }
                match sty {
                    Sty::OneExpr | Sty::OneRule | Sty::OneFens => self.parse_block(inp, sty),
                    Sty::OldText => {}
                    _ => self.open_item(sty),
                }
                self.par = Some((self.loc, vec![]));
                continue;
            }
            //  continue the paragraph
            let started = self
                .par
                .as_ref()
                .is_some_and(|(_, lines)| !lines.is_empty());
            let fits = !started
                || match self.cur.mite {
                    //  hoon-138 crashes here (bad-leaf-container)
                    Mite::Lord | Mite::Lunt => false,
                    Mite::Head => false,
                    Mite::Poem => col >= self.inr,
                    Mite::Down | Mite::Lime | Mite::Bloc => col == self.inr,
                };
            if !fits {
                self.failed = true;
                return;
            }
            let (lin, stop) = match self.read_line() {
                ReadLine::Line(lin) => (lin, false),
                ReadLine::Stop(lin) => (lin, true),
                ReadLine::Fail => {
                    self.failed = true;
                    return;
                }
            };
            if let Some((_, lines)) = &mut self.par {
                lines.push(lin);
            }
            if stop {
                return;
            }
        }
    }

    /// `++$`: the nodes, how far they reach, and the column there.
    fn run(mut self, inp: &mut Input<'src, '_>) -> Option<(Marl, usize, u64)> {
        let start = self.loc;
        self.line(inp);
        if self.failed {
            return None;
        }
        //  a last paragraph that does not parse is dropped
        let _ = self.close_par();
        while !self.hac.is_empty() {
            self.close_item();
        }
        //  ++non-empty
        if self.pos == 0 && self.loc == start {
            return None;
        }
        let (end, col) = (self.pos, self.loc.1);
        Some((Self::cur_to_tarp(self.cur), end, col))
    }
}

/// hoon-138 `++cram`: markdown from here, as a list of nodes. `expr` is
/// `++expr`, a `;` sail line.
pub(super) fn cram<'src>(
    expr: Boxed<'src, Marl>,
    linemap: Arc<LineMap>,
    ctx: SailCtx,
) -> Boxed<'src, Marl> {
    custom(move |inp: &mut Input<'src, '_>| {
        let start = inp.cursor();
        let base = *start.inner();
        let rest: &'src str = inp.slice_from(&start..);
        let machine = Machine {
            rest: rest.as_bytes(),
            base,
            pos: 0,
            loc: linemap.hair(base),
            failed: false,
            out: 0,
            inr: 0,
            hac: vec![],
            cur: Item {
                mite: Mite::Down,
                q: vec![],
            },
            par: None,
            expr: &expr,
            linemap: &linemap,
            ctx: &ctx,
        };
        match machine.run(inp) {
            Some((tarp, end, col)) => {
                //  at `==` or an outdent after a rule or code block, hoon-138
                //  sets its column past the indentation it leaves unread
                linemap.set_column(base + end, col);
                advance_to(inp, base + end);
                Ok(tarp)
            }
            None => Err(Rich::custom(inp.span_since(&start), "sail markdown")),
        }
    })
    .boxed()
}
