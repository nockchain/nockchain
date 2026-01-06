use std::collections::HashMap;

use anyhow::{anyhow, Result};
use either::Either;
use flume::Sender;
use nockapp::kernel::boot::Cli;
use nockapp::noun::slab::NounSlab;
use nockapp::Bytes;
use nockvm::jets::util::slot;
use nockvm::jets::JetErr;
use nockvm::noun::*;
use nockvm_macros::tas;
use reedline::{DefaultPrompt, DefaultPromptSegment, FileBackedHistory, Reedline, Signal};

use crate::parse::{parse_tree, Node};
use crate::{run_kernel, SendSlab};

fn input_func(out: Sender<Option<String>>) -> Result<()> {
    loop {
        if out.send(None).is_err() {
            break;
        }

        let history = Box::new(FileBackedHistory::with_file(50, "jojo.hist".into())?);
        let mut rl = Reedline::create().with_history(history);

        let prompt = DefaultPrompt {
            left_prompt: DefaultPromptSegment::Basic("~nockvm:jojo".to_string()),
            right_prompt: DefaultPromptSegment::Empty,
        };

        match rl.read_line(&prompt)? {
            Signal::Success(line) => {
                if out.send(Some(line)).is_err() {
                    break;
                }
                let _ = rl.sync_history();
            }
            Signal::CtrlD | Signal::CtrlC => break,
        }
    }

    Ok(())
}

#[derive(Default)]
struct ShellCommand {
    hoon: Option<String>,
    in_sample: Option<Node>,
    in_subject: Option<Node>,
    out_sample: Option<String>,
    jam_out: Option<String>,
    cue_inp: Option<(String, usize)>,
    list_vars: bool,
    debug_print: bool,
}

impl ShellCommand {
    fn parse(mut line: &str) -> Result<Self> {
        let mut in_sample = None;
        let mut in_subject = None;
        let mut out_sample = None;
        let mut jam_out = None;
        let mut cue_inp = None;
        let mut list_vars = false;
        let mut file_hoon = false;
        let mut debug_print = false;

        let hoon = if line.starts_with('/') {
            line = line.split_once('/').unwrap().1;

            loop {
                let (mut cmd, mut rest) = line.split_once(' ').unwrap_or((line, ""));

                fn parse_vars<'a>(rest: &mut &'a str) -> Result<(Option<Node>, &'a str)> {
                    let Some((tree, tail)) = parse_tree(rest) else {
                        return Err(anyhow!("Malformed command: {rest}"));
                    };

                    *rest = tail.trim_ascii_start();

                    Ok((Some(tree), rest))
                }

                match cmd {
                    "?" | "help" => {
                        return Err(anyhow!(
                            r"Usage:

/?, /help - display this message.
/. sam func - evaluate `func` with the given sample `sam`. Given sample can be constructed from variable assignments (using /.), and can be a cell, e.g. [a b].
// sam path - load `path` and evaluate its hoon with sample `sam`. Given sample can be constructed from variable assignments.
/: sub hoon - evaluate `hoon` with subject `sub`. Subject can be constructed from variable assignments.
/= var hoon - evaluage `hoon` and assign output to `var`.
/+ v s hoon - evaluate `hoon` on subject `s`, and assign output to `v`. Subject can be constructed from variable assignments.
/| v s func - evaluate `func` with the given sample `s` and assign output to `v`.
/p sam      - print given sample `sam`. Note: if single variable is provided, it is pretty-printed, but mutliple variables are printed as raw nouns.
/D sam      - (fast) debug print given sample `sam`. This will print the noun on rust side.
/v          - list defined variables.
/c var path - cue a jamfile at `path` and assign it to `var`.
/s var path - cue a subject jamfile at `path`, take the sample at axis 6, and assign it to `var`.
/j sam path - jam the given sample `sam` and write it to `path`.
hoon - evaluate `hoon` and print the result out to screen.
                            "
                        ))
                    }
                    "." => (in_sample, line) = parse_vars(&mut rest)?,
                    "/" => {
                        (in_sample, line) = {
                            file_hoon = true;
                            parse_vars(&mut rest)?
                        }
                    }
                    ":" => (in_subject, line) = parse_vars(&mut rest)?,
                    "=" => {
                        (cmd, line) = rest.split_once(' ').unwrap_or((rest, ""));
                        if cmd.is_empty() {
                            return Err(anyhow!("Sample is not specified"));
                        }
                        out_sample = Some(cmd.to_string());
                    }
                    "+" => {
                        (cmd, rest) = rest.split_once(' ').unwrap_or((rest, ""));
                        if cmd.is_empty() {
                            return Err(anyhow!("Sample is not specified"));
                        }
                        out_sample = Some(cmd.to_string());
                        (in_subject, line) = parse_vars(&mut rest)?;
                    }
                    "|" => {
                        (cmd, rest) = rest.split_once(' ').unwrap_or((rest, ""));
                        if cmd.is_empty() {
                            return Err(anyhow!("Sample is not specified"));
                        }
                        out_sample = Some(cmd.to_string());
                        (in_sample, line) = parse_vars(&mut rest)?;
                    }
                    "p" => {
                        (in_sample, _) = parse_vars(&mut rest)?;
                        break None;
                    }
                    "D" => {
                        (in_sample, _) = parse_vars(&mut rest)?;
                        debug_print = true;
                        break None;
                    }
                    "v" => {
                        list_vars = true;
                        break None;
                    }
                    "c" => {
                        let (var, path) = rest
                            .split_once(' ')
                            .ok_or_else(|| anyhow!("Unable to cue: invalid input"))?;
                        out_sample = Some(var.to_string());
                        cue_inp = Some((path.to_string(), 1));
                    }
                    "s" => {
                        let (var, path) = rest
                            .split_once(' ')
                            .ok_or_else(|| anyhow!("Unable to cue: invalid input"))?;
                        out_sample = Some(var.to_string());
                        cue_inp = Some((path.to_string(), 6));
                    }
                    "j" => {
                        (in_sample, line) = parse_vars(&mut rest)?;
                        jam_out = Some(line.to_string());
                        break None;
                    }
                    _ => return Err(anyhow!("Invalid command! (use /help for usage)")),
                }

                break Some(line.trim_ascii_start());
            }
        } else {
            Some(line)
        };

        let hoon = hoon
            .map(|hoon| {
                if file_hoon {
                    std::fs::read_to_string(hoon)
                } else if !hoon.starts_with(char::is_alphabetic) || !hoon.contains(' ') {
                    Ok(hoon.to_string())
                } else {
                    Ok(format!("({hoon})"))
                }
            })
            .transpose()?;

        Ok(Self {
            hoon,
            in_sample,
            in_subject,
            out_sample,
            jam_out,
            cue_inp,
            list_vars,
            debug_print,
        })
    }
}

#[derive(Default)]
pub struct Shell {
    samples: HashMap<String, (bool, NounSlab)>,
    save_state: Option<String>,
}

enum Command {
    Eval(NounSlab),
    Jam(Bytes, String),
    Cue(String, usize),
    ListVars(Vec<String>),
    DebugPrint(Noun),
}

impl Shell {
    fn next_command(&mut self, line: &str) -> Result<Command> {
        let ShellCommand {
            hoon,
            in_sample,
            in_subject,
            out_sample,
            jam_out,
            cue_inp,
            list_vars,
            debug_print,
        } = ShellCommand::parse(line)?;

        let prt = D(out_sample.is_some() as u64);
        self.save_state = out_sample;

        let mut slab: NounSlab = NounSlab::new();

        let mut vases = [None, None];

        for (vas, sam) in vases.iter_mut().zip([in_sample, in_subject]) {
            if let Some(tree) = sam {
                let pull_noun = |slab: &mut NounSlab, sample: String| {
                    self.samples
                        .get(&sample)
                        .map(|(vased, v)| (vased, unsafe { *v.root() }))
                        .map(|(vased, v)| {
                            (vased, unsafe {
                                slab.copy_into(v);
                                *slab.root()
                            })
                        })
                        .ok_or_else(|| anyhow!("{sample} is undefined"))
                };

                if let Node::Leaf(sample) = &tree {
                    let (vased, mut noun) = pull_noun(&mut slab, sample.clone())?;
                    if !vased {
                        noun = T(&mut slab, &[D(tas!(b"noun")), noun]);
                    }
                    *vas = Some(noun);
                } else {
                    let noun = tree.fold(
                        &mut slab,
                        &mut |sample: String, slab| {
                            pull_noun(slab, sample).map(|(vased, v)| {
                                if *vased {
                                    slot(v, 3).unwrap()
                                } else {
                                    v
                                }
                            })
                        },
                        &mut |nouns: Vec<Noun>, slab| Ok(T(slab, &nouns[..])),
                    )?;
                    *vas = Some(T(&mut slab, &[D(tas!(b"noun")), noun]));
                }
            }
        }

        let [vased_sample, vased_subject] = vases;

        let vased_subject = vased_subject
            .map(|v| T(&mut slab, &[D(0), v]))
            .unwrap_or(D(0));

        if let Some(jam) = jam_out {
            let sam = vased_sample
                .and_then(|v| slot(v, 3).ok())
                .ok_or_else(|| anyhow!("Cannot jam without sample (impossible)"))?;
            let slab: NounSlab = NounSlab::from(sam);
            let bytes = slab.jam();
            return Ok(Command::Jam(bytes, jam));
        } else if let Some((cue, axis)) = cue_inp {
            return Ok(Command::Cue(cue, axis));
        } else if list_vars {
            return Ok(Command::ListVars(self.samples.keys().cloned().collect()));
        } else if debug_print {
            let sam = vased_sample.unwrap();
            return Ok(Command::DebugPrint(slot(sam, 3).unwrap()));
        }

        let hoon = hoon.as_deref().map(|v| unsafe {
            IndirectAtom::new_raw_bytes(&mut slab, v.len(), v.as_ptr()).as_noun()
        });

        let poke = match (hoon, vased_sample) {
            (Some(hoon), Some(sam)) => {
                let sam = slot(sam, 3).unwrap();
                slab.copy_into(sam);
                let sam = unsafe { *slab.root() };
                T(&mut slab, &[D(tas!(b"sam")), prt, hoon, vased_subject, sam])
            }
            (Some(hoon), None) => T(&mut slab, &[D(tas!(b"raw")), prt, hoon, vased_subject]),
            (None, Some(sam)) => {
                slab.copy_into(sam);
                let sam = unsafe { *slab.root() };
                T(&mut slab, &[D(tas!(b"prt")), prt, sam])
            }
            _ => return Err(anyhow!("Invalid command")),
        };

        slab.set_root(poke);

        Ok(Command::Eval(slab))
    }

    /// Returns if should print
    fn process_out(&mut self, vased: bool, out: Noun) -> bool {
        if let Some(sam) = self.save_state.take() {
            self.samples.insert(sam, (vased, out.into()));
            false
        } else {
            true
        }
    }

    pub async fn run(mut self, cli: Cli) -> Result<()> {
        let (tx, rx) = flume::bounded(0);
        let effects = run_kernel(rx.into_stream(), cli).await;

        // Intentionally rendezvous!
        let (lines_out, lines) = flume::bounded(0);

        let task = tokio::task::spawn_blocking(move || input_func(lines_out));

        while let Ok(line) = lines.recv_async().await {
            let Some(line) = line else { continue };

            if line.is_empty() {
                continue;
            }

            let command = match self.next_command(&line) {
                Ok(s) => s,
                Err(e) => {
                    println!("{e}");
                    continue;
                }
            };

            match command {
                Command::Jam(bytes, path) => {
                    if let Err(e) = tokio::fs::write(&path, bytes).await {
                        println!("Unable to save jam to {path}: {e}");
                    }
                }
                Command::Cue(path, axis) => {
                    if let Err(e) = async {
                        let mut slab: NounSlab = NounSlab::new();
                        let bytes = tokio::fs::read(&path).await?;
                        let noun = slab.cue_into(bytes.into())?;
                        if let Ok(noun) = slot(noun, axis as u64) {
                            self.process_out(false, noun);
                        } else {
                            println!("Unable to cue - invalid axis");
                        }
                        anyhow::Ok(())
                    }
                    .await
                    {
                        println!("Unable to cue at path {path}: {e}");
                    }
                }
                Command::ListVars(vars) => {
                    println!("Defined variables:");
                    for v in vars {
                        println!("{v}");
                    }
                }
                Command::DebugPrint(noun) => {
                    if let Ok(c) = noun.as_cell() {
                        println!("Cell: {:?}", FullDebugCell(&c));
                    } else {
                        println!("Noun: {noun:?}");
                    }
                }
                Command::Eval(slab) => {
                    tx.send_async(SendSlab(slab)).await?;
                    let slab = effects.recv_async().await?.0;
                    let effect = unsafe { slab.root() };

                    let handle_eval =
                        |shell: &mut Shell, effect: Noun| -> core::result::Result<(), JetErr> {
                            let Ok(cell) = effect.as_cell() else {
                                println!("Unrecognized result {effect:?}");
                                return Ok(());
                            };

                            match cell.head().as_either_atom_cell() {
                                Either::Left(a) => {
                                    match std::str::from_utf8(a.as_ne_bytes())
                                        .map(|v| v.trim_end_matches('\0'))
                                    {
                                        Ok("poke") => {
                                            println!("Error running command");
                                        }
                                        _ => println!("Unrecognized result"),
                                    }
                                }
                                Either::Right(cell) => {
                                    let res = cell.tail();
                                    match cell
                                        .head()
                                        .as_atom()
                                        .map(|a| a.to_le_bytes())
                                        .as_deref()
                                        .ok()
                                        .and_then(|v| std::str::from_utf8(v).ok())
                                        .map(|v| v.trim_end_matches('\0'))
                                    {
                                        Some("jojo") => {
                                            let vase = slot(res, 2).unwrap();
                                            if shell.process_out(true, vase) {
                                                if let Ok(c) = slot(res, 3).unwrap().as_cell() {
                                                    let pretty = c.tail().as_atom().unwrap();
                                                    let pretty = pretty.as_ne_bytes();
                                                    let pretty = std::str::from_utf8(pretty)
                                                        .unwrap()
                                                        .trim_end_matches('\0');
                                                    println!("{pretty}");
                                                }
                                            }
                                        }
                                        v => {
                                            println!("Unrecognized result {v:?}");
                                        }
                                    }
                                }
                            }

                            Ok(())
                        };

                    handle_eval(&mut self, *effect).unwrap();
                }
            }
        }

        task.await?
    }
}
