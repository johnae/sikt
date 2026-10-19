//! The backtracking VM. Repetition is possessive, so the backtrack stack only
//! grows with nested choices, not with input length.

use crate::class::{Kind, char_len, decode, decode_last};
use crate::compile::{Code, Inst};
use crate::error::MatchError;
use crate::syntax::Anchor;

#[derive(Debug, Clone, Copy)]
struct Frame {
    pc: usize,
    pos: usize,
    captures: usize,
    /// A return address from [`Inst::Call`], skipped when backtracking.
    call: bool,
}

/// Bounds on one match attempt.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub steps: u64,
    pub depth: u32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CaptureEvent {
    pub slot: u32,
    pub pos: usize,
    pub open: bool,
}

/// Per-search state, reused across attempts on one haystack.
#[derive(Debug, Default)]
pub(crate) struct Scratch {
    stack: Vec<Frame>,
    pub captures: Vec<CaptureEvent>,
    /// Per finder: the last search as `(from, found)`. Lets `until "x"` skip
    /// searching again from every start position.
    until: Vec<Option<(usize, Option<usize>)>>,
}

impl Scratch {
    pub fn new(program: &Code) -> Self {
        Scratch {
            until: vec![None; program.finders.len()],
            ..Scratch::default()
        }
    }
}

/// Runs one anchored attempt at `start`. On a match, returns its end and
/// leaves the capture events in `scratch.captures`.
pub(crate) fn exec(
    program: &Code,
    hay: &[u8],
    start: usize,
    scratch: &mut Scratch,
    limits: Limits,
) -> Result<Option<usize>, MatchError> {
    let insts = &program.insts[..];
    let Scratch {
        stack,
        captures,
        until,
    } = scratch;
    stack.clear();
    captures.clear();
    let mut pc = 0;
    let mut pos = start;
    let mut steps = 0u64;
    let mut depth = 0u32;
    loop {
        steps += 1;
        if steps > limits.steps {
            return Err(MatchError::StepLimit {
                limit: limits.steps,
            });
        }
        let ok = match insts[pc] {
            Inst::Any => {
                if pos < hay.len() {
                    pos += char_len(&hay[pos..]);
                    true
                } else {
                    false
                }
            }
            Inst::SpanAny => {
                pos = hay.len();
                true
            }
            Inst::Byte(b) => {
                if hay.get(pos) == Some(&b) {
                    pos += 1;
                    true
                } else {
                    false
                }
            }
            Inst::Literal(i) => {
                let lit = &program.literals[i as usize];
                if hay[pos..].starts_with(lit) {
                    pos += lit.len();
                    true
                } else {
                    false
                }
            }
            Inst::Set(i) => {
                let set = &program.sets[i as usize];
                if pos < hay.len()
                    && let (Some(c), len) = decode(&hay[pos..])
                    && set.matches(c)
                {
                    pos += len;
                    true
                } else {
                    false
                }
            }
            Inst::SpanSet(i) => {
                let set = &program.sets[i as usize];
                while pos < hay.len() {
                    match decode(&hay[pos..]) {
                        (Some(c), len) if set.matches(c) => pos += len,
                        _ => break,
                    }
                }
                true
            }
            Inst::UntilLiteral(i) => {
                let cached = until[i as usize]
                    .filter(|&(from, found)| from <= pos && found.is_none_or(|f| f >= pos));
                let found = match cached {
                    Some((_, found)) => found,
                    None => {
                        let found = program.finders[i as usize]
                            .find(&hay[pos..])
                            .map(|f| f + pos);
                        until[i as usize] = Some((pos, found));
                        found
                    }
                };
                match found {
                    Some(f) => {
                        pos = f;
                        true
                    }
                    None => false,
                }
            }
            Inst::Choice(target) => {
                stack.push(Frame {
                    pc: target,
                    pos,
                    captures: captures.len(),
                    call: false,
                });
                true
            }
            Inst::Commit(target) => {
                stack.pop();
                pc = target;
                continue;
            }
            Inst::PartialCommit(target) => {
                let top = stack.last_mut().expect("partial commit without choice");
                top.pos = pos;
                top.captures = captures.len();
                pc = target;
                continue;
            }
            Inst::BackCommit(target) => {
                let frame = stack.pop().expect("back commit without choice");
                pos = frame.pos;
                captures.truncate(frame.captures);
                pc = target;
                continue;
            }
            Inst::FailTwice => {
                stack.pop();
                false
            }
            Inst::Fail => false,
            Inst::Assert(anchor) => holds(anchor, hay, pos),
            Inst::Behind(n) => {
                let mut ok = true;
                for _ in 0..n {
                    if pos == 0 {
                        ok = false;
                        break;
                    }
                    pos -= decode_last(&hay[..pos]).1;
                }
                ok
            }
            Inst::Open(slot) => {
                captures.push(CaptureEvent {
                    slot,
                    pos,
                    open: true,
                });
                true
            }
            Inst::Close(slot) => {
                captures.push(CaptureEvent {
                    slot,
                    pos,
                    open: false,
                });
                true
            }
            Inst::Call(target) => {
                depth += 1;
                if depth > limits.depth {
                    return Err(MatchError::DepthLimit {
                        limit: limits.depth,
                    });
                }
                stack.push(Frame {
                    pc: pc + 1,
                    pos,
                    captures: captures.len(),
                    call: true,
                });
                pc = target;
                continue;
            }
            Inst::Return => {
                let frame = stack.pop().expect("return without call");
                debug_assert!(frame.call);
                depth -= 1;
                pc = frame.pc;
                continue;
            }
            Inst::Match => return Ok(Some(pos)),
        };
        if ok {
            pc += 1;
        } else {
            // Unwind to the latest choice, leaving the rules it was inside.
            let frame = loop {
                match stack.pop() {
                    Some(frame) if frame.call => depth -= 1,
                    Some(frame) => break frame,
                    None => return Ok(None),
                }
            };
            pc = frame.pc;
            pos = frame.pos;
            captures.truncate(frame.captures);
        }
    }
}

fn holds(anchor: Anchor, hay: &[u8], pos: usize) -> bool {
    let is_word = |c: Option<char>| c.is_some_and(|c| Kind::WordChar.matches(c));
    let word_before = || pos > 0 && is_word(decode_last(&hay[..pos]).0);
    let word_after = || pos < hay.len() && is_word(decode(&hay[pos..]).0);
    match anchor {
        Anchor::LineStart => pos == 0 || hay[pos - 1] == b'\n',
        Anchor::LineEnd => {
            pos == hay.len() || hay[pos..].starts_with(b"\n") || hay[pos..].starts_with(b"\r\n")
        }
        Anchor::TextStart => pos == 0,
        Anchor::TextEnd => pos == hay.len(),
        Anchor::WordStart => !word_before() && word_after(),
        Anchor::WordEnd => word_before() && !word_after(),
    }
}
