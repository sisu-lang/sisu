//! The writer of decision 1: every generator rule writes its text once, and the writer puts it
//! into both the program and its rewritten copy. Only a rewrite point writes different text into
//! each (decision 2).

use std::fmt::Write as _;
use std::ops::Range;

use super::scope::{Ty, fresh_name};

/// Text written into both copies.
#[derive(Default)]
pub(crate) struct Piece {
    pub(crate) program: String,
    pub(crate) rewritten: String,
}

/// The rewrites of decision 2, each of which keeps what the program prints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rewrite {
    /// `e` becomes `({ e })`.
    Block,
    /// `if false { s }`, where `s` is a copy of the statement, goes before it.
    DeadIf,
    /// `if c { a } else { b }` becomes `if !(c) { b } else { a }`.
    Swap,
    /// `e` becomes a fresh `r<n>`, and `let r<n>: T = e` goes before its statement.
    Let,
    /// `e` becomes a fresh `C<n>`, and `const C<n>: T = e` goes after everything else.
    Const,
}

/// What a rewrite's condition reads of the point it would rewrite (decision 2).
#[derive(Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each is one condition of decision 2, which `allowed` reads apart from the others"
)]
pub(crate) struct Site {
    /// The expression reads no input, prints nothing, calls no function, reads no field, no
    /// element and no mutable local, and cannot panic.
    pub(crate) pure: bool,
    /// It runs whenever its statement does, after no operand that has an effect.
    pub(crate) unconditional: bool,
    /// `check/constant.rs` folds it, with no step that overflows or divides by zero.
    pub(crate) folds: bool,
    /// It is in a `const`'s initializer. False in the generator: a `const`'s initializer is
    /// written by `const_expr`, which never calls `Gen::expr`.
    pub(crate) in_const: bool,
    /// It is a place that is assigned to. False in the generator: `stmt.rs` writes every
    /// assignment's target as text, never through `Gen::expr`.
    pub(crate) place: bool,
    /// The point is after a block's last expression. False in the generator: `Writer::end`
    /// puts an `if false` copy before its statement, and a block's last expression is none.
    pub(crate) last: bool,
}

/// Whether `rewrite` keeps the meaning at `site`: the condition of each rewrite of the spec's
/// property 4.
pub(crate) fn allowed(rewrite: Rewrite, site: &Site) -> bool {
    match rewrite {
        // A block in a `const` does not fold (E0086), and a block is no place.
        Rewrite::Block => !site.in_const && !site.place,
        // After a block's last expression, it would take the block's value (E0031).
        Rewrite::DeadIf => !site.last,
        Rewrite::Swap => true,
        Rewrite::Let => site.pure && site.unconditional && !site.in_const && !site.place,
        Rewrite::Const => site.folds && !site.place,
    }
}

/// Where an expression starts: in each copy, and in what the rewrites have added.
#[derive(Clone, Copy)]
pub(crate) struct Mark {
    program: usize,
    rewritten: usize,
    lets: usize,
    consts: usize,
    #[cfg(test)]
    made: usize,
}

#[derive(Default)]
pub(crate) struct Writer {
    pub(crate) out: Piece,
    /// How many levels the next line is indented.
    indent: usize,
    /// For each statement being written, innermost last, the `let`s that go before it in the
    /// rewritten copy; `None` for a block, where a statement's own `let`s go before it.
    lets: Vec<Option<Vec<String>>>,
    /// The `const`s of the rewrites, each on a line of its own, which go after everything
    /// else.
    pub(crate) consts: String,
    /// The last number a rewrite's fresh name took.
    names: usize,
    /// The rewrites made, in order.
    #[cfg(test)]
    pub(crate) made: Vec<Rewrite>,
}

impl Writer {
    /// Writes `text` into both copies.
    pub(crate) fn text(&mut self, text: &str) {
        self.out.program.push_str(text);
        self.out.rewritten.push_str(text);
    }

    /// Ends the line and starts the next at the current indentation.
    pub(crate) fn line(&mut self) {
        self.text("\n");
        for _ in 0..self.indent {
            self.text("    ");
        }
    }

    /// Writes `{`, and indents the lines after it.
    pub(crate) fn open(&mut self) {
        self.text("{");
        self.indent += 1;
    }

    /// Writes `}` on a line of its own, one level out.
    pub(crate) fn close(&mut self) {
        self.indent -= 1;
        self.line();
        self.text("}");
    }

    /// Writes into `piece` from now on, and returns what was written so far.
    pub(crate) fn swap(&mut self, piece: Piece) -> Piece {
        std::mem::replace(&mut self.out, piece)
    }

    /// Writes `piece`, which was written elsewhere, into both copies.
    pub(crate) fn paste(&mut self, piece: &Piece) {
        self.out.program.push_str(&piece.program);
        self.out.rewritten.push_str(&piece.rewritten);
    }

    /// Where the rewritten copy ends.
    pub(crate) fn at(&self) -> usize {
        self.out.rewritten.len()
    }

    /// Where the next expression starts.
    pub(crate) fn mark(&self) -> Mark {
        Mark {
            program: self.out.program.len(),
            rewritten: self.out.rewritten.len(),
            lets: self.pending().map_or(0, Vec::len),
            consts: self.consts.len(),
            #[cfg(test)]
            made: self.made.len(),
        }
    }

    /// The `let`s of the statement being written, unless a block was opened since it began.
    fn pending(&self) -> Option<&Vec<String>> {
        self.lets.last().and_then(Option::as_ref)
    }

    /// Whether a `let` can go before the statement being written.
    pub(crate) fn can_bind(&self) -> bool {
        self.pending().is_some()
    }

    fn fresh(&mut self, prefix: &str) -> String {
        fresh_name(&mut self.names, prefix)
    }

    #[cfg_attr(
        not(test),
        expect(
            clippy::unused_self,
            reason = "only the tests record the rewrites made, in `self.made`"
        ),
        expect(
            clippy::needless_pass_by_ref_mut,
            reason = "only the tests push to `self.made`, which needs `&mut self`"
        )
    )]
    fn made(&mut self, rewrite: Rewrite) {
        #[cfg(test)]
        self.made.push(rewrite);
        #[cfg(not(test))]
        let _ = rewrite;
    }

    /// Opens a block, before whose statements their own `let`s go.
    pub(crate) fn open_block(&mut self) {
        self.lets.push(None);
    }

    pub(crate) fn close_block(&mut self) {
        let block = self.lets.pop();
        debug_assert!(
            matches!(block, Some(None)),
            "a block should close as it opened"
        );
    }

    /// Begins a statement at the current point.
    pub(crate) fn begin(&mut self) -> Mark {
        self.lets.push(Some(Vec::new()));
        self.mark()
    }

    /// Ends the statement begun at `start`: in the rewritten copy, writes before it its `let`s,
    /// and, if `dead`, first `if false` and a copy of the statement as the program has it.
    pub(crate) fn end(&mut self, start: Mark, dead: bool) {
        let lets = self
            .lets
            .pop()
            .flatten()
            .expect("a statement should end as it began");
        let line = format!("\n{}", "    ".repeat(self.indent));
        let mut before = String::new();
        if dead {
            // No string literal holds a line break, so each one starts a line.
            let copy = self.out.program[start.program..].replace('\n', "\n    ");
            before = format!("if false {{{line}    {copy}{line}}}{line}");
            self.made(Rewrite::DeadIf);
        }
        for binding in lets {
            before += &binding;
            before += &line;
        }
        self.out.rewritten.insert_str(start.rewritten, &before);
    }

    /// Writes the expression from `start` on in the rewritten copy as `({ e })`.
    pub(crate) fn block(&mut self, start: Mark) {
        self.out.rewritten.insert_str(start.rewritten, "({ ");
        self.out.rewritten.push_str(" })");
        self.made(Rewrite::Block);
    }

    /// Binds the expression from `start` on in the rewritten copy to a fresh `let` of type
    /// `ty` before its statement.
    pub(crate) fn bind(&mut self, start: Mark, ty: &Ty) {
        let name = self.fresh("r");
        let value = self.out.rewritten.split_off(start.rewritten);
        self.out.rewritten.push_str(&name);
        self.lets
            .last_mut()
            .and_then(Option::as_mut)
            .expect("a `let` should go before a statement")
            .push(format!("let {name}: {ty} = {value}"));
        self.made(Rewrite::Let);
    }

    /// Moves the expression from `start` on into a fresh `const` of type `ty`, written as the
    /// program has it: what the rewrites inside it added goes with it.
    pub(crate) fn constant(&mut self, start: Mark, ty: &Ty) {
        let name = self.fresh("C");
        if let Some(Some(lets)) = self.lets.last_mut() {
            lets.truncate(start.lets);
        }
        self.consts.truncate(start.consts);
        #[cfg(test)]
        self.made.truncate(start.made);
        let value = &self.out.program[start.program..];
        write!(self.consts, "\nconst {name}: {ty} = {value}")
            .expect("writing to a `String` should succeed");
        self.out.rewritten.truncate(start.rewritten);
        self.out.rewritten.push_str(&name);
        self.made(Rewrite::Const);
    }

    /// Writes `if c { a } else { b }`, from the condition `cond` on in the rewritten copy, as
    /// `if !(c) { b } else { a }`, where `a` and `b` are the arms' text.
    pub(crate) fn swap_arms(&mut self, cond: Range<usize>, a: Range<usize>, b: Range<usize>) {
        let r = &self.out.rewritten;
        let swapped = format!(
            "!({}){}{}{}{}{}",
            &r[cond.clone()],
            &r[cond.end..a.start],
            &r[b.clone()],
            &r[a.end..b.start],
            &r[a],
            &r[b.end..],
        );
        self.out.rewritten.truncate(cond.start);
        self.out.rewritten.push_str(&swapped);
        self.made(Rewrite::Swap);
    }
}
