//! The writer of decision 1: every generator rule writes its text once, and the writer puts it
//! into both the program and its rewritten copy. Only a rewrite point writes different text into
//! each (decision 2).

use std::fmt::Write as _;
use std::ops::Range;

use super::mistake::MistakeSite;
use super::scope::{Ty, fresh_name};

/// Text written into both copies.
#[derive(Default)]
pub(crate) struct Piece {
    pub(crate) program: String,
    pub(crate) rewritten: String,
    /// The sites of the single-mistake check in `program`, in the order they were recorded.
    pub(crate) sites: Vec<MistakeSite>,
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
    /// A statement that passes a local as a `mut` argument runs on a copy of it: `({ let mut w<n>
    /// = x; f(mut w<n>); x = w<n> })`.
    MutCopy,
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

/// Whether `rewrite` keeps the meaning at `site`: the condition of each rewrite that the
/// rewrite check needs, from the spec.
pub(crate) fn allowed(rewrite: Rewrite, site: &Site) -> bool {
    match rewrite {
        // A block in a `const` does not fold (E0086), and a block is no place.
        Rewrite::Block => !site.in_const && !site.place,
        // After a block's last expression, it would take the block's value (E0031).
        Rewrite::DeadIf => !site.last,
        // The condition of `MutCopy` is on the call, not on a point: see `mut_rewrite_allowed`.
        Rewrite::Swap | Rewrite::MutCopy => true,
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

    /// Records `site`, which `choose` may turn into a mistake.
    pub(crate) fn record(&mut self, site: MistakeSite) {
        self.out.sites.push(site);
    }

    /// Writes into `piece` from now on, and returns what was written so far.
    pub(crate) fn swap(&mut self, piece: Piece) -> Piece {
        std::mem::replace(&mut self.out, piece)
    }

    /// Writes `piece`, which was written elsewhere, into both copies.
    pub(crate) fn paste(&mut self, piece: &Piece) {
        let base = self.out.program.len();
        self.out.program.push_str(&piece.program);
        self.out.rewritten.push_str(&piece.rewritten);
        // The sites of the piece move up by the program it was pasted onto.
        self.out
            .sites
            .extend(piece.sites.iter().map(|site| MistakeSite {
                at: site.at.start + base..site.at.end + base,
                error: site.error + base,
                ..site.clone()
            }));
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

    /// Writes the statement from `start` on, in the rewritten copy, on copies of the `locals` it
    /// passes as `mut` arguments, if `copy` and `clean`: each local, with where the rewritten copy
    /// holds its text, goes into a fresh `w<n>` before the call and comes back after it. The
    /// generator says `clean` when no other argument can change a local (S31).
    pub(crate) fn mut_block(
        &mut self,
        start: Mark,
        locals: &[(String, usize)],
        copy: bool,
        clean: bool,
    ) {
        if !copy || !clean || locals.is_empty() {
            return;
        }
        let copies: Vec<String> = locals.iter().map(|_| self.fresh("w")).collect();
        // From the last, so that the offsets before it still hold.
        for ((local, at), copy) in locals.iter().zip(&copies).rev() {
            self.out
                .rewritten
                .replace_range(*at..*at + local.len(), copy);
        }
        let (mut before, mut after) = (String::from("({ "), String::new());
        for ((local, _), copy) in locals.iter().zip(&copies) {
            write!(before, "let mut {copy} = {local}; ")
                .expect("writing to a `String` should succeed");
            write!(after, "; {local} = {copy}").expect("writing to a `String` should succeed");
        }
        self.out.rewritten.insert_str(start.rewritten, &before);
        self.out.rewritten.push_str(&after);
        self.out.rewritten.push_str(" })");
        self.made(Rewrite::MutCopy);
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

#[cfg(test)]
mod tests {
    use super::super::mistake::{Kind, MistakeSite};
    use super::{Piece, Writer};

    /// GIVEN a site at `0..1` recorded in a side piece, which the writer then pastes after `ab`
    /// WHEN the paste is done
    /// THEN the site is at `2..3`, and its error at 2
    #[test]
    fn paste_should_move_the_sites_of_its_piece() {
        let mut w = Writer::default();
        w.text("ab");
        let outer = w.swap(Piece::default());
        w.text("c");
        let site = MistakeSite {
            kind: Kind::Local,
            at: 0..1,
            error: 0,
            function: None,
        };
        w.record(site.clone());
        let piece = w.swap(outer);
        w.paste(&piece);
        assert_eq!(w.out.program, "abc");
        let moved = MistakeSite {
            at: 2..3,
            error: 2,
            ..site
        };
        assert_eq!(w.out.sites, [moved]);
    }

    /// GIVEN S31's `f(mut xs, g(mut xs))`, whose other argument can change `xs`, and
    /// `f(mut xs, 1)`, whose other argument cannot
    /// WHEN the writer is told which, and asked to copy
    /// THEN it leaves the first call as it is, and rewrites the second
    #[test]
    fn mut_rewrite_should_skip_a_call_whose_arguments_can_change_the_place() {
        for (call, clean) in [("f(mut xs, g(mut xs))", false), ("f(mut xs, 1)", true)] {
            let mut w = Writer::default();
            let start = w.mark();
            w.text("f(mut ");
            let at = w.at();
            w.text(&call["f(mut ".len()..]);
            w.mut_block(start, &[("xs".to_string(), at)], true, clean);
            assert_eq!(w.out.program, call);
            assert_eq!(w.out.rewritten != call, clean, "{call}");
        }
    }

    /// GIVEN a call `f(mut x, mut o.g, 1)` of which two arguments are `mut`, and `x` is a local
    /// WHEN the writer rewrites it
    /// THEN the rewritten copy runs the call on a copy of `x`, and the program is as it was
    #[test]
    fn mut_block_should_copy_each_local_in_and_out() {
        let mut w = Writer::default();
        let start = w.mark();
        w.text("f(mut ");
        let first = w.at();
        w.text("x");
        w.text(", mut o.g, mut ");
        let second = w.at();
        w.text("y)");
        let locals = [("x".to_string(), first), ("y".to_string(), second)];
        w.mut_block(start, &locals, true, true);
        assert_eq!(w.out.program, "f(mut x, mut o.g, mut y)");
        assert_eq!(
            w.out.rewritten,
            "({ let mut w1 = x; let mut w2 = y; f(mut w1, mut o.g, mut w2); x = w1; y = w2 })"
        );
    }

    /// GIVEN a clean call whose text holds the words `mut` and `mutable`, in a string and a name
    /// WHEN the writer rewrites it
    /// THEN it copies the local: the generator's word decides, not the text
    #[test]
    fn mut_block_should_not_read_the_text_of_the_call() {
        let mut w = Writer::default();
        let start = w.mark();
        w.text("f(mut ");
        let at = w.at();
        w.text("x, \"mut\", mutable)");
        w.mut_block(start, &[("x".to_string(), at)], true, true);
        assert_eq!(
            w.out.rewritten,
            "({ let mut w1 = x; f(mut w1, \"mut\", mutable); x = w1 })"
        );
    }

    /// GIVEN a call that the writer may rewrite
    /// WHEN the choice is not to copy, or the call holds no local
    /// THEN the rewritten copy is as the program is
    #[test]
    fn mut_block_should_leave_a_call_that_is_not_copied() {
        for (copy, locals) in [(false, true), (true, false)] {
            let mut w = Writer::default();
            let start = w.mark();
            w.text("f(mut ");
            let at = w.at();
            w.text("x)");
            let locals = if locals {
                vec![("x".to_string(), at)]
            } else {
                Vec::new()
            };
            w.mut_block(start, &locals, copy, true);
            assert_eq!(w.out.rewritten, "f(mut x)");
        }
    }
}
