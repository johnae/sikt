//! AST to VM instructions.

use memchr::memmem::Finder;

use crate::class::CharSet;
use crate::error::Error;
use crate::syntax::{Anchor, Ast, Expr, ExprKind, Span};

/// Keeps patterns like `1000 (1000 "x")` from exhausting memory.
const MAX_INSTS: usize = 1 << 20;

/// An instruction. Jump targets are absolute instruction indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Inst {
    /// One character, or one invalid byte.
    Any,
    /// The rest of the input.
    SpanAny,
    Byte(u8),
    /// The literal at this index in [`Code::literals`].
    Literal(u32),
    /// One character from the set at this index in [`Code::sets`].
    Set(u32),
    /// Zero or more characters from a set, without pushing backtrack entries.
    SpanSet(u32),
    /// Advances to the next occurrence of [`Code::finders`] at this index,
    /// failing if there is none.
    UntilLiteral(u32),
    /// Pushes a backtrack entry that resumes at the target.
    Choice(usize),
    /// Pops the backtrack entry and jumps.
    Commit(usize),
    /// Updates the top backtrack entry to the current state and jumps: one
    /// loop iteration done.
    PartialCommit(usize),
    /// Pops the backtrack entry, restores its position and jumps: the
    /// lookahead matched.
    BackCommit(usize),
    /// Pops the backtrack entry, then fails: the negative lookahead matched.
    FailTwice,
    Fail,
    Assert(Anchor),
    /// Moves back this many characters, failing at the start of the text.
    Behind(u32),
    Open(u32),
    Close(u32),
    /// Pushes a return entry and jumps to a rule.
    Call(usize),
    /// Pops the return entry and jumps back.
    Return,
    Match,
}

#[derive(Debug)]
pub(crate) struct Code {
    pub insts: Vec<Inst>,
    pub literals: Vec<Box<[u8]>>,
    pub finders: Vec<Finder<'static>>,
    pub sets: Vec<CharSet>,
    /// Capture names, indexed by slot.
    pub captures: Vec<String>,
    /// For each instruction that can fail on the input, the pattern source it
    /// came from. Explanations describe what was expected with it.
    pub spans: Vec<Option<Span>>,
}

pub(crate) fn compile(ast: &Ast, src: &str) -> Result<Code, Error> {
    let mut c = Compiler {
        src,
        rules: &ast.rules,
        variants: Vec::new(),
        calls: Vec::new(),
        program: Code {
            insts: Vec::new(),
            literals: Vec::new(),
            finders: Vec::new(),
            sets: Vec::new(),
            captures: Vec::new(),
            spans: Vec::new(),
        },
    };
    c.expr(&ast.main)?;
    c.push(Inst::Match);
    // Compiling a rule can call further rules, adding variants as it goes.
    let mut addrs = Vec::new();
    while addrs.len() < c.variants.len() {
        let (rule, nocase) = c.variants[addrs.len()];
        addrs.push(c.here());
        let mut body = c.rules[rule].clone();
        if nocase {
            body = body.fold_case();
        }
        c.expr(&body)?;
        c.push(Inst::Return);
    }
    for (at, variant) in std::mem::take(&mut c.calls) {
        c.program.insts[at] = Inst::Call(addrs[variant]);
    }
    Ok(c.program)
}

struct Compiler<'s> {
    src: &'s str,
    rules: &'s [Expr],
    /// Each rule body to compile, as `(rule, nocase)`.
    variants: Vec<(usize, bool)>,
    /// Call instructions to point at their variant once it's compiled.
    calls: Vec<(usize, usize)>,
    program: Code,
}

impl Compiler<'_> {
    fn push(&mut self, inst: Inst) -> usize {
        self.program.insts.push(inst);
        self.program.spans.push(None);
        self.program.insts.len() - 1
    }

    /// Pushes an instruction that can fail on the input, noting its source.
    fn push_at(&mut self, inst: Inst, span: Span) {
        let pc = self.push(inst);
        self.program.spans[pc] = Some(span);
    }

    fn here(&self) -> usize {
        self.program.insts.len()
    }

    /// Points the jump at `at` to the next instruction.
    fn patch(&mut self, at: usize) {
        let target = self.here();
        match &mut self.program.insts[at] {
            Inst::Choice(t) | Inst::Commit(t) | Inst::PartialCommit(t) | Inst::BackCommit(t) => {
                *t = target
            }
            inst => unreachable!("patching {inst:?}"),
        }
    }

    fn literal(&mut self, s: &str, span: Span) {
        match s.as_bytes() {
            [] => {}
            &[b] => self.push_at(Inst::Byte(b), span),
            bytes => {
                self.program.literals.push(bytes.into());
                self.push_at(Inst::Literal(self.program.literals.len() as u32 - 1), span);
            }
        }
    }

    fn set(&mut self, set: &CharSet) -> u32 {
        let sets = &mut self.program.sets;
        let i = sets.iter().position(|s| s == set).unwrap_or_else(|| {
            sets.push(set.clone());
            sets.len() - 1
        });
        i as u32
    }

    fn slot(&mut self, name: &str) -> u32 {
        let captures = &mut self.program.captures;
        let slot = captures.iter().position(|n| n == name).unwrap_or_else(|| {
            captures.push(name.to_owned());
            captures.len() - 1
        });
        slot as u32
    }

    fn expr(&mut self, e: &Expr) -> Result<(), Error> {
        match &e.kind {
            ExprKind::Literal(s) => self.literal(s, e.span),
            ExprKind::Any => self.push_at(Inst::Any, e.span),
            ExprKind::Newline => {
                let choice = self.push(Inst::Choice(0));
                self.literal("\r\n", e.span);
                let commit = self.push(Inst::Commit(0));
                self.patch(choice);
                self.literal("\n", e.span);
                self.patch(commit);
            }
            ExprKind::Class(set) => {
                let i = self.set(set);
                self.push_at(Inst::Set(i), e.span);
            }
            ExprKind::Seq(items) => {
                for item in items {
                    self.expr(item)?;
                }
            }
            ExprKind::Choice(alts) => {
                let (last, rest) = alts.split_last().expect("choice has alternatives");
                let mut commits = Vec::new();
                for alt in rest {
                    let choice = self.push(Inst::Choice(0));
                    self.expr(alt)?;
                    commits.push(self.push(Inst::Commit(0)));
                    self.patch(choice);
                }
                self.expr(last)?;
                for commit in commits {
                    self.patch(commit);
                }
            }
            ExprKind::Repeat { min, max, body } => self.repeat(*min, *max, body)?,
            ExprKind::Capture { name, body } => {
                let slot = self.slot(name);
                self.push(Inst::Open(slot));
                self.expr(body)?;
                self.push(Inst::Close(slot));
            }
            ExprKind::Anchor(anchor) => self.push_at(Inst::Assert(*anchor), e.span),
            ExprKind::Look {
                behind,
                negated,
                body,
            } => self.look(*behind, *negated, body)?,
            ExprKind::Call { rule, nocase, .. } => {
                let key = (*rule, *nocase);
                let variant = self
                    .variants
                    .iter()
                    .position(|&v| v == key)
                    .unwrap_or_else(|| {
                        self.variants.push(key);
                        self.variants.len() - 1
                    });
                let at = self.push(Inst::Call(0));
                self.calls.push((at, variant));
            }
            ExprKind::NoCase(_) | ExprKind::Rule(_) => unreachable!("resolved before compiling"),
            ExprKind::Until(x) => self.until(x)?,
            ExprKind::Through(x) => {
                self.until(x)?;
                self.expr(x)?;
            }
        }
        if self.here() > MAX_INSTS {
            let label = Some("expands to too many instructions".into());
            return Err(Error::new(self.src, "pattern is too large", e.span, label)
                .with_help("use smaller repetition counts"));
        }
        Ok(())
    }

    fn repeat(&mut self, min: u32, max: Option<u32>, body: &Expr) -> Result<(), Error> {
        for _ in 0..min {
            self.expr(body)?;
        }
        match max {
            None => {
                if body.kind.nullable() {
                    let label = Some("this can match without consuming anything".into());
                    return Err(Error::new(
                        self.src,
                        "repetition would loop forever",
                        body.span,
                        label,
                    )
                    .with_help("make the repeated part match at least one character"));
                }
                self.star(body)?;
            }
            Some(max) => {
                // Each optional copy jumps to the end on failure, so once one
                // fails no later copy is tried.
                let mut choices = Vec::new();
                for _ in min..max {
                    choices.push(self.push(Inst::Choice(0)));
                    self.expr(body)?;
                    let commit = self.push(Inst::Commit(0));
                    self.patch(commit);
                }
                for choice in choices {
                    self.patch(choice);
                }
            }
        }
        Ok(())
    }

    /// Zero or more, possessive.
    fn star(&mut self, body: &Expr) -> Result<(), Error> {
        match &body.kind {
            ExprKind::Class(set) => {
                let i = self.set(set);
                self.push(Inst::SpanSet(i));
            }
            ExprKind::Any => {
                self.push(Inst::SpanAny);
            }
            _ => {
                let choice = self.push(Inst::Choice(0));
                let start = self.here();
                self.expr(body)?;
                self.push(Inst::PartialCommit(start));
                self.patch(choice);
            }
        }
        Ok(())
    }

    fn look(&mut self, behind: bool, negated: bool, body: &Expr) -> Result<(), Error> {
        let choice = self.push(Inst::Choice(0));
        if behind {
            let Some(len) = body.kind.fixed_len() else {
                let label = Some("this can match different numbers of characters".into());
                return Err(Error::new(
                    self.src,
                    "`after` needs a pattern with a fixed number of characters",
                    body.span,
                    label,
                )
                .with_help("use exact counts like `3 digit` instead of `1+ digit`; `newline` can be one or two characters"));
            };
            self.push(Inst::Behind(len as u32));
        }
        self.expr(body)?;
        if negated {
            self.push(Inst::FailTwice);
            self.patch(choice);
        } else {
            let ok = self.push(Inst::BackCommit(0));
            self.patch(choice);
            self.push(Inst::Fail);
            self.patch(ok);
        }
        Ok(())
    }

    /// `(not followed by x any)* followed by x`.
    fn until(&mut self, x: &Expr) -> Result<(), Error> {
        if let ExprKind::Literal(s) = &x.kind
            && !s.is_empty()
        {
            self.program
                .finders
                .push(Finder::new(s.as_bytes()).into_owned());
            self.push_at(
                Inst::UntilLiteral(self.program.finders.len() as u32 - 1),
                x.span,
            );
            return Ok(());
        }

        let exit = self.push(Inst::Choice(0));
        let start = self.here();
        let not_x = self.push(Inst::Choice(0));
        self.expr(x)?;
        self.push(Inst::FailTwice);
        self.patch(not_x);
        self.push(Inst::Any);
        self.push(Inst::PartialCommit(start));
        self.patch(exit);

        let lookahead = self.push(Inst::Choice(0));
        self.expr(x)?;
        let ok = self.push(Inst::BackCommit(0));
        self.patch(lookahead);
        self.push(Inst::Fail);
        self.patch(ok);
        Ok(())
    }
}
