//! The backtracking VM. Repetition is possessive, so the backtrack stack only
//! grows with nested choices, not with input length.

use crate::class::{char_len, decode};
use crate::compile::{Inst, Program};
use crate::error::MatchError;

#[derive(Debug, Clone, Copy)]
struct Frame {
    pc: usize,
    pos: usize,
    captures: usize,
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
    pub fn new(program: &Program) -> Self {
        Scratch {
            until: vec![None; program.finders.len()],
            ..Scratch::default()
        }
    }
}

/// Runs one anchored attempt at `start`. On a match, returns its end and
/// leaves the capture events in `scratch.captures`.
pub(crate) fn exec(
    program: &Program,
    hay: &[u8],
    start: usize,
    scratch: &mut Scratch,
    max_steps: u64,
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
    loop {
        steps += 1;
        if steps > max_steps {
            return Err(MatchError::StepLimit { limit: max_steps });
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
            Inst::Match => return Ok(Some(pos)),
        };
        if ok {
            pc += 1;
        } else {
            let Some(frame) = stack.pop() else {
                return Ok(None);
            };
            pc = frame.pc;
            pos = frame.pos;
            captures.truncate(frame.captures);
        }
    }
}
