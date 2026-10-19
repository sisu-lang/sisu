//! ADR 0021's hoist on mir: a loop that writes the elements of a local array, and never
//! shares or replaces it, makes it unique once at its pre-header, so its writes skip the
//! count check at the root.
//!
//! Only a retain raises a block's count, and every retain in mir is a `copy`, so a read in
//! place never bars a local. That covers all six borrow positions, where `codegen/hoist.rs`
//! covered only an `Index` base and a `Len` operand.

use rustc_hash::FxHashSet;

use super::{Function, LocalId, Loop, Operand, Place, Rvalue, Statement, Terminator};

/// The locals that `lp` can make unique before it starts, sorted. A local `L` qualifies when
/// the loop's blocks, its nested loops' included, both:
///
/// - hold a level-0 `make_unique L`: the root of an element write to `L`;
/// - never copy `L` whole, move it, release it or assign it whole (a `let` or a binding
///   included). A read through a place rooted at `L`, a `take` and a store do not bar it, and
///   neither does a scope release, which only an edge leaving the loop runs for `L`.
pub(super) fn unique_roots(function: &Function, lp: &Loop) -> Vec<LocalId> {
    let mut walk = Walk::default();
    for block in &lp.blocks {
        let block = &function.blocks[block.0];
        for stmt in &block.stmts {
            walk.stmt(stmt);
        }
        walk.term(&block.term);
    }
    let mut roots: Vec<_> = walk.written.difference(&walk.barred).copied().collect();
    roots.sort_unstable_by_key(|local| local.0);
    roots
}

/// For each loop, innermost first, deletes the level-0 `make_unique` of each of its
/// `unique_roots` from its blocks and puts one at the end of its pre-header. An inner loop's
/// pre-header lies inside the outer loop, so the outer loop moves that one on in turn.
pub(super) fn hoist(function: &mut Function) {
    for i in (0..function.loops.len()).rev() {
        let roots = unique_roots(function, &function.loops[i]);
        if roots.is_empty() {
            continue;
        }
        let lp = &function.loops[i];
        for block in &lp.blocks {
            function.blocks[block.0]
                .stmts
                .retain(|stmt| level_0(stmt).is_none_or(|local| !roots.contains(&local)));
        }
        function.blocks[lp.pre_header.0]
            .stmts
            .extend(roots.into_iter().map(|local| {
                Statement::MakeUnique(Place {
                    local,
                    projections: Vec::new(),
                    borrow: false,
                    guard: false,
                })
            }));
    }
}

/// The local that `stmt` makes unique whole, if it is a `make_unique` with no projection.
fn level_0(stmt: &Statement) -> Option<LocalId> {
    match stmt {
        Statement::MakeUnique(place) if place.projections.is_empty() => Some(place.local),
        Statement::MakeUnique(_)
        | Statement::Assign { .. }
        | Statement::Take { .. }
        | Statement::Store { .. }
        | Statement::Release(_)
        | Statement::ScopeRelease(_)
        | Statement::Check(_)
        | Statement::End(_) => None,
    }
}

/// The locals a loop's blocks make unique at level 0, and those they bar.
#[derive(Default)]
struct Walk {
    written: FxHashSet<LocalId>,
    barred: FxHashSet<LocalId>,
}

impl Walk {
    fn stmt(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Assign { dest, value } => {
                self.barred.extend(*dest);
                self.rvalue(value);
            }
            Statement::Take { dest, .. } => {
                self.barred.insert(*dest);
            }
            Statement::Store { value, .. } => self.operand(value),
            Statement::Release(local) => {
                self.barred.insert(*local);
            }
            Statement::MakeUnique(_) => self.written.extend(level_0(stmt)),
            Statement::ScopeRelease(_) | Statement::Check(_) | Statement::End(_) => {}
        }
    }

    fn rvalue(&mut self, value: &Rvalue) {
        match value {
            Rvalue::Copy(place) if place.projections.is_empty() => {
                self.barred.insert(place.local);
            }
            Rvalue::Copy(_) | Rvalue::CopyStr(_) | Rvalue::Borrow(_) | Rvalue::ReadLine => {}
            Rvalue::Use(operand)
            | Rvalue::Unwrap(operand)
            | Rvalue::Wrap(operand)
            | Rvalue::Chars(operand)
            | Rvalue::Bytes(operand)
            | Rvalue::Words(operand)
            | Rvalue::FromChars(operand)
            | Rvalue::Len(operand)
            | Rvalue::LenWords(operand)
            | Rvalue::Sum { base: operand, .. }
            | Rvalue::Convert(operand)
            | Rvalue::Unary { operand, .. }
            | Rvalue::Print { operand, .. } => self.operand(operand),
            Rvalue::Call { args: operands, .. }
            | Rvalue::New { args: operands, .. }
            | Rvalue::Variant { args: operands, .. }
            | Rvalue::ArrayLit(operands)
            | Rvalue::Interpolate {
                parts: operands, ..
            }
            | Rvalue::Math { operands, .. } => {
                for operand in operands {
                    self.operand(operand);
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

    /// A move bars its local. `Use` and `Guarded` do too: the ownership pass makes each a move
    /// or a copy.
    fn operand(&mut self, operand: &Operand) {
        match operand {
            Operand::Move(local) | Operand::Use(local) | Operand::Guarded(local) => {
                self.barred.insert(*local);
            }
            Operand::Const(_) | Operand::Read(_) => {}
        }
    }

    fn term(&mut self, term: &Terminator) {
        match term {
            Terminator::Branch { cond: operand, .. }
            | Terminator::SwitchValue { operand, .. }
            | Terminator::Return(Some(operand))
            | Terminator::Panic {
                message: operand, ..
            } => self.operand(operand),
            Terminator::Goto(_)
            | Terminator::IfPresent { .. }
            | Terminator::SwitchTag { .. }
            | Terminator::Return(None)
            | Terminator::Unreachable => {}
        }
    }
}
