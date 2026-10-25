//! The ownership pass: it turns each `use`, borrow site and guard mark that lowering left into
//! a move, a copy or an in-place read, by liveness after the statement.
//!
//! - `use x` moves `x` when `x` is not live after the statement, and otherwise reads a copy:
//!   `t = copy x` before the statement, which then moves `t`. In `l = use x` it becomes
//!   `l = move x` or `l = copy x`.
//! - A borrow site reads its root in place when the root is live after the statement. When it
//!   is not, `t = move x` comes before the statement, the place reads `t`, and `release t`
//!   follows: the statement owns the root's value, and releases it.
//! - A guard mark, on a `use` of a `match` binding or a borrow site rooted at one, is a read
//!   of a borrowed binding: a copy or an in-place read, whatever liveness says. A `use` of a
//!   borrowed parameter, or a borrow site rooted at one, is read the same way: the pass never
//!   moves a borrowed parameter.

use std::mem;

use super::liveness::{Liveness, liveness};
use super::{
    BlockId, Function, Local, LocalId, Operand, Place, Projection, Rvalue, Statement, Terminator,
};

/// Places the copies, moves and releases of `function`: no `use`, borrow site or guard mark is
/// left.
pub(super) fn place(function: &mut Function) {
    let live = liveness(function);
    let mut placer = Placer {
        live: &live,
        borrowed: &function.borrowed,
        locals: &mut function.locals,
        at: (BlockId(0), 0),
        out: Vec::new(),
        after: Vec::new(),
    };
    for (b, block) in function.blocks.iter_mut().enumerate() {
        // The terminator's position is the block's length as lowered.
        let len = block.stmts.len();
        let mut stmts = mem::take(&mut block.stmts)
            .into_iter()
            .enumerate()
            .peekable();
        placer.out = Vec::with_capacity(len);
        while let Some((i, mut stmt)) = stmts.next() {
            // A counted field store's `take` and store read one borrow site, lowered as two
            // copies of one place: the store decides it, and the `take` follows.
            let mut store = match (&stmt, stmts.peek()) {
                (Statement::Take { place, .. }, next) if place.borrow => {
                    debug_assert!(
                        matches!(next, Some((_, Statement::Store { place: to, .. })) if to == place),
                        "a borrowed take should be followed by the store to its place: lowering's \
                         store() emits them together"
                    );
                    stmts.next()
                }
                _ => None,
            };
            placer.at = (BlockId(b), store.as_ref().map_or(i, |(j, _)| *j));
            placer.statement(&mut stmt);
            if let Some((_, Statement::Store { place: to, .. })) = &mut store
                && let Statement::Take { place, .. } = &stmt
            {
                to.clone_from(place);
            }
            placer.out.push(stmt);
            if let Some((_, mut store)) = store {
                placer.statement(&mut store);
                placer.out.push(store);
            }
            placer.out.append(&mut placer.after);
        }
        placer.at = (BlockId(b), len);
        placer.terminator(&mut block.term);
        debug_assert!(
            placer.after.is_empty(),
            "a terminator should have no borrow site to release after"
        );
        block.stmts = mem::take(&mut placer.out);
    }
}

struct Placer<'a> {
    live: &'a Liveness,
    /// `Function::borrowed`.
    borrowed: &'a [bool],
    locals: &'a mut Vec<Local>,
    /// The statement being placed, as lowering left the block.
    at: (BlockId, usize),
    /// The block's statements so far.
    out: Vec<Statement>,
    /// What goes right after the statement: the releases of its moved borrow-site roots.
    after: Vec<Statement>,
}

impl Placer<'_> {
    fn statement(&mut self, stmt: &mut Statement) {
        match stmt {
            Statement::Assign { value, .. } => self.rvalue(value),
            Statement::Take { place, .. }
            | Statement::MakeUnique(place)
            | Statement::Check(place) => self.place(place),
            Statement::Store { place, value } => {
                self.place(place);
                self.operand(value);
            }
            // A take-apart's places are its scrutinee's own, read where it consumes it.
            Statement::Release(_)
            | Statement::ScopeRelease(_)
            | Statement::End(_)
            | Statement::TakeApart { .. }
            | Statement::Free(_) => {}
        }
    }

    fn rvalue(&mut self, value: &mut Rvalue) {
        match value {
            // `l = use x` needs no temporary.
            Rvalue::Use(Operand::Use(local) | Operand::Guarded(local)) => {
                let local = *local;
                let guarded = matches!(value, Rvalue::Use(Operand::Guarded(_)));
                *value = if self.keeps(local, guarded) {
                    Rvalue::Copy(whole(local))
                } else {
                    Rvalue::Use(Operand::Move(local))
                };
            }
            Rvalue::Use(op)
            | Rvalue::Unwrap(op)
            | Rvalue::Wrap(op)
            | Rvalue::Chars(op)
            | Rvalue::Bytes(op)
            | Rvalue::Words(op)
            | Rvalue::FromChars(op)
            | Rvalue::Len(op)
            | Rvalue::LenWords(op)
            | Rvalue::Sum { base: op, .. }
            | Rvalue::Convert(op)
            | Rvalue::Unary { operand: op, .. }
            | Rvalue::Print { operand: op, .. } => self.operand(op),
            Rvalue::Copy(place) | Rvalue::Borrow(place) => self.place(place),
            Rvalue::CopyStr(_) | Rvalue::ReadLine => {}
            Rvalue::Call { args, .. }
            | Rvalue::New { args, .. }
            | Rvalue::Variant { args, .. }
            | Rvalue::ArrayLit(args)
            | Rvalue::Interpolate { parts: args, .. }
            | Rvalue::Math { operands: args, .. } => {
                for op in args {
                    self.operand(op);
                }
            }
            Rvalue::ArrayFill {
                value: lhs,
                len: rhs,
                ..
            }
            | Rvalue::Concat { lhs, rhs }
            | Rvalue::Contains {
                base: lhs,
                value: rhs,
            }
            | Rvalue::Binary { lhs, rhs, .. }
            | Rvalue::Compare { lhs, rhs, .. }
            | Rvalue::Equal { lhs, rhs, .. }
            | Rvalue::Is { lhs, rhs } => {
                self.operand(lhs);
                self.operand(rhs);
            }
        }
    }

    fn terminator(&mut self, term: &mut Terminator) {
        match term {
            Terminator::Branch { cond: op, .. }
            | Terminator::SwitchValue { operand: op, .. }
            | Terminator::Return(Some(op))
            | Terminator::Panic { message: op, .. } => self.operand(op),
            Terminator::SwitchTag { place, .. } | Terminator::TestPresent { place, .. } => {
                self.place(place);
            }
            Terminator::Goto(_)
            | Terminator::IfPresent { .. }
            | Terminator::Return(None)
            | Terminator::Unreachable => {}
        }
    }

    /// Whether `local` stays put: a `use` of it reads a copy, and a borrow site rooted at it
    /// reads it in place. Always for a guard binding or a borrowed parameter, otherwise when
    /// `local` is live after the statement.
    fn keeps(&self, local: LocalId, guarded: bool) -> bool {
        guarded || self.borrowed.get(local.0) == Some(&true) || self.live.live_after(local, self.at)
    }

    /// A `use` becomes a move of its local, or of a copy made just before the statement.
    fn operand(&mut self, op: &mut Operand) {
        match *op {
            Operand::Use(local) | Operand::Guarded(local) => {
                *op = if self.keeps(local, matches!(op, Operand::Guarded(_))) {
                    let copy = self.temp(local);
                    self.out.push(Statement::Assign {
                        dest: Some(copy),
                        value: Rvalue::Copy(whole(local)),
                    });
                    Operand::Move(copy)
                } else {
                    Operand::Move(local)
                };
            }
            Operand::Read(ref mut place) => self.place(place),
            Operand::Const(_) | Operand::Move(_) => {}
        }
    }

    /// Clears a borrow site: its root stays in place, or moves into a temporary released
    /// after the statement when the statement is the root's last use.
    fn place(&mut self, place: &mut Place) {
        for projection in &mut place.projections {
            if let Projection::Index { index, .. } = projection {
                self.operand(index);
            }
        }
        let guard = mem::take(&mut place.guard);
        if !mem::take(&mut place.borrow) || self.keeps(place.local, guard) {
            return;
        }
        let root = self.temp(place.local);
        self.out.push(Statement::Assign {
            dest: Some(root),
            value: Rvalue::Use(Operand::Move(place.local)),
        });
        place.local = root;
        self.after.push(Statement::Release(root));
    }

    /// A new counted temporary of `local`'s type, scoped to the block being placed.
    fn temp(&mut self, local: LocalId) -> LocalId {
        let ty = self.locals[local.0].ty;
        self.locals.push(Local {
            name: None,
            ty,
            counted: true,
            scope: self.at.0,
            token: false,
        });
        LocalId(self.locals.len() - 1)
    }
}

/// `local` with no projections.
fn whole(local: LocalId) -> Place {
    Place {
        local,
        projections: Vec::new(),
        borrow: false,
        guard: false,
    }
}
