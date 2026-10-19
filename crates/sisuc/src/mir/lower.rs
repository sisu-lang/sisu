//! `tir` to mir, for every node but the patterns of `IfSome` and `Match`.
//!
//! Operands are evaluated in source order, as codegen evaluates them today. Each helper returns
//! `None` once the code it lowered cannot fall through (a `return`, `break`, `continue` or
//! `panic`); lowering then stops emitting into that block, and a block is created only when an
//! edge reaches it, so every block is reachable from the entry.
//!
//! The scope stack holds the counted locals that `tir` declares and that are in scope, oldest
//! first. A block, a loop's body and the function each mark its height; an edge that leaves
//! one scope-releases (`release?`) the locals above the mark, newest first. Lowering's own
//! temporaries never join it: the checker's `spill` binds every counted operand before a later
//! operand that can exit to a `let` local, so no temporary is owned across an exit.

use std::ptr;

use rustc_hash::FxHashMap;

use super::{
    Block as MirBlock, BlockId, Const, Function, Local, Loop, Operand, Place as MirPlace, Program,
    Projection, Rvalue, Statement, Terminator,
};
use crate::tir::{self, Block, CompareOp, Expr, ExprKind, LocalId, Place, Stmt, Type};

/// The mir of `program`, before any pass: `Use` operands and borrow sites are still undecided.
pub(crate) fn lower(program: &tir::Program) -> Program {
    Program {
        functions: program
            .functions
            .iter()
            .map(|f| Lowering::function(program, f))
            .collect(),
    }
}

/// What lowering an expression gave: `None` when it cannot fall through, and otherwise the
/// local that holds its value, `None` for `unit`.
type Lowered = Option<Option<LocalId>>;

/// A written element place's root, its indexes from the root outwards with their operands, and
/// the temporary that holds a field root's base.
type WrittenRoot<'p> = (MirPlace, Vec<(&'p Expr, Operand)>, Option<LocalId>);

struct Lowering<'a> {
    program: &'a tir::Program,
    locals: Vec<Local>,
    blocks: Vec<MirBlock>,
    loops: Vec<Loop>,
    /// The block statements go to; `None` after an expression that cannot fall through.
    current: Option<BlockId>,
    /// The counted `tir` locals in scope, oldest first.
    scope: Vec<LocalId>,
    /// The loops around the code being lowered, innermost last.
    frames: Vec<Frame>,
    /// Where the function's body mentions each local, which the pin rule asks.
    mentions: Mentions,
}

/// A loop being lowered: where `continue` goes, where `break` goes once one has, and the
/// height of the scope stack outside it.
struct Frame {
    header: BlockId,
    exit: Option<BlockId>,
    mark: usize,
}

/// Whether a local of type `ty` holds anything: `unit` and `never` do not.
fn valued(ty: Type) -> bool {
    !matches!(ty, Type::UNIT | Type::NEVER)
}

/// `local` with no projections, not a borrow site.
fn whole(local: LocalId) -> MirPlace {
    MirPlace {
        local,
        projections: Vec::new(),
        borrow: false,
    }
}

/// The constant `e` is, if it is one.
fn constant(e: &Expr) -> Option<Const> {
    if let ExprKind::Int(n) = e.kind {
        return Some(Const::Int(n, e.ty));
    }
    if let ExprKind::Float(x) = e.kind {
        return Some(Const::Float(x));
    }
    if let ExprKind::Bool(b) = e.kind {
        return Some(Const::Bool(b));
    }
    if let ExprKind::Char(c) = e.kind {
        return Some(Const::Char(c));
    }
    if let ExprKind::None = e.kind {
        return Some(Const::None(e.ty));
    }
    None
}

/// The local that `e` reads whole. `Move` reads like `Local`: mir decides moves itself.
fn local_of(e: &Expr) -> Option<LocalId> {
    if let ExprKind::Local(local) | ExprKind::Move(local) = e.kind {
        Some(local)
    } else {
        None
    }
}

impl<'a> Lowering<'a> {
    fn function(program: &'a tir::Program, f: &tir::Function) -> Function {
        let locals = f
            .locals
            .iter()
            .map(|local| Local {
                name: local.name.clone(),
                ty: local.ty,
                counted: local.ty.is_counted(&program.enums),
                scope: BlockId(0),
            })
            .collect();
        let mut lowering = Lowering {
            program,
            locals,
            blocks: Vec::new(),
            loops: Vec::new(),
            current: None,
            scope: Vec::new(),
            frames: Vec::new(),
            mentions: Mentions::of(&f.body, f.locals.len()),
        };
        lowering.current = Some(lowering.new_block());
        for (i, param) in f.params.iter().enumerate() {
            assert_eq!(param.0, i, "the checker should number the parameters first");
            if lowering.locals[i].counted {
                lowering.scope.push(*param);
            }
        }
        if let Some(value) = lowering.block(&f.body, None) {
            // A `unit` function discards any value its body has.
            let value = if valued(f.ret) {
                Some(value.expect(
                    "the checker should give a body that falls through a value of the function's return type",
                ))
            } else {
                lowering.discard(value);
                None
            };
            lowering.release_scope(0);
            let value = value.map(|value| lowering.owned(value));
            lowering.terminate(Terminator::Return(value));
        }
        Function {
            name: f.name.clone(),
            params: f.params.len(),
            ret: f.ret,
            locals: lowering.locals,
            blocks: lowering.blocks,
            loops: lowering.loops,
            owners: FxHashMap::default(),
        }
    }

    fn counted(&self, ty: Type) -> bool {
        ty.is_counted(&self.program.enums)
    }

    fn new_block(&mut self) -> BlockId {
        self.blocks.push(MirBlock {
            stmts: Vec::new(),
            term: Terminator::Unreachable,
        });
        BlockId(self.blocks.len() - 1)
    }

    fn here(&self) -> BlockId {
        self.current
            .expect("lowering should emit only into a block that falls through")
    }

    fn push(&mut self, stmt: Statement) {
        let block = self.here();
        self.blocks[block.0].stmts.push(stmt);
    }

    /// Ends the current block with `term`; nothing falls through after it.
    fn terminate(&mut self, term: Terminator) {
        let block = self.here();
        self.blocks[block.0].term = term;
        self.current = None;
    }

    fn goto(&mut self, to: BlockId) {
        self.terminate(Terminator::Goto(to));
    }

    /// A new temporary of type `ty`, whose scope is the current block.
    fn temp(&mut self, ty: Type) -> LocalId {
        let scope = self.here();
        self.locals.push(Local {
            name: None,
            ty,
            counted: self.counted(ty),
            scope,
        });
        LocalId(self.locals.len() - 1)
    }

    /// `dest = value`, into a new temporary when `dest` is `None`, or with no destination
    /// when `ty` has no value. Gives the local assigned.
    fn assign(&mut self, ty: Type, dest: Option<LocalId>, value: Rvalue) -> Option<LocalId> {
        let dest = valued(ty).then(|| dest.unwrap_or_else(|| self.temp(ty)));
        self.push(Statement::Assign { dest, value });
        dest
    }

    /// The operand that consumes the owned `local`: a move when it is counted, a read
    /// otherwise.
    fn owned(&self, local: LocalId) -> Operand {
        if self.locals[local.0].counted {
            Operand::Move(local)
        } else {
            Operand::Read(whole(local))
        }
    }

    /// A consuming read of the source local `local`: `Use` when it is counted, a read
    /// otherwise.
    fn read(&self, local: LocalId) -> Operand {
        if self.locals[local.0].counted {
            Operand::Use(local)
        } else {
            Operand::Read(whole(local))
        }
    }

    /// Releases `local`, a temporary that owns its value, when there is one.
    fn release(&mut self, local: Option<LocalId>) {
        if let Some(local) = local {
            self.push(Statement::Release(local));
        }
    }

    /// Releases a value nobody keeps: a counted one.
    fn discard(&mut self, value: Option<LocalId>) {
        if let Some(local) = value.filter(|local| self.locals[local.0].counted) {
            self.push(Statement::Release(local));
        }
    }

    /// Scope-releases the locals above `mark`, newest first, without popping them.
    fn release_scope(&mut self, mark: usize) {
        for i in (mark..self.scope.len()).rev() {
            self.push(Statement::ScopeRelease(self.scope[i]));
        }
    }

    /// The statements of `block`, then its value into `dest`, then the scope releases of the
    /// locals it declared.
    fn block(&mut self, block: &Block, dest: Option<LocalId>) -> Lowered {
        let mark = self.scope.len();
        let value = self.block_body(block, dest);
        if value.is_some() {
            self.release_scope(mark);
        }
        self.scope.truncate(mark);
        value
    }

    fn block_body(&mut self, block: &Block, dest: Option<LocalId>) -> Lowered {
        for stmt in &block.stmts {
            self.stmt(stmt)?;
        }
        match &block.value {
            Some(value) => self.into(value, dest),
            None => Some(None),
        }
    }

    fn stmt(&mut self, stmt: &Stmt) -> Option<()> {
        match stmt {
            Stmt::Let { local, init } => {
                self.locals[local.0].scope = self.here();
                let dest = valued(self.locals[local.0].ty).then_some(*local);
                self.into(init, dest)?;
                if self.locals[local.0].counted {
                    self.scope.push(*local);
                }
            }
            Stmt::Assign {
                place: Place::Local(local),
                value,
            } => {
                // The new value first, then the release of the old one.
                if self.locals[local.0].counted {
                    let value = self.temp_of(value)?;
                    self.push(Statement::ScopeRelease(*local));
                    self.push(Statement::Assign {
                        dest: Some(*local),
                        value: Rvalue::Use(Operand::Move(value)),
                    });
                } else {
                    let value = self.operand(value)?;
                    self.push(Statement::Assign {
                        dest: Some(*local),
                        value: Rvalue::Use(value),
                    });
                }
            }
            Stmt::Assign {
                place: Place::Field { base, index },
                value,
            } => self.field_store(base, *index, value)?,
            Stmt::Assign {
                place: place @ Place::Index { .. },
                value,
            } => self.element_write(place, value)?,
            Stmt::Append { place, value } => self.append(place, value)?,
            Stmt::Expr(e) => {
                let value = self.into(e, None)?;
                self.discard(value);
            }
        }
        Some(())
    }

    /// The value of `e`, which the checker gives one, in a new temporary.
    fn temp_of(&mut self, e: &Expr) -> Option<LocalId> {
        Some(
            self.into(e, None)?
                .expect("the checker should give this expression a value"),
        )
    }

    /// `e` as an operand: a constant, a read of a source local, or a temporary.
    fn operand(&mut self, e: &Expr) -> Option<Operand> {
        if let Some(leaf) = self.leaf(e) {
            return Some(leaf);
        }
        let value = self.temp_of(e)?;
        Some(self.owned(value))
    }

    /// `e` as an operand when it is a constant or a read of a source local.
    fn leaf(&self, e: &Expr) -> Option<Operand> {
        constant(e)
            .map(Operand::Const)
            .or_else(|| local_of(e).map(|local| self.read(local)))
    }

    /// `op`, an operand of a statement whose later operands are `later`. A read of a whole
    /// source local that a later operand mentions is read into a temporary now, so the
    /// statement sees the value the local had here ("When an operand reads").
    fn pinned(&mut self, op: Operand, later: &[&Expr]) -> Operand {
        let local = match &op {
            Operand::Use(local) => *local,
            // An operand reads a whole local: `operand` never gives a projection.
            Operand::Read(place) => place.local,
            Operand::Const(_) | Operand::Move(_) => return op,
        };
        if !self.mentions.within(local, later) {
            return op;
        }
        let ty = self.locals[local.0].ty;
        let temp = self.assign(ty, None, Rvalue::Use(op));
        self.owned(temp.expect("a local read as an operand should have a value"))
    }

    /// The operands of one statement, in order, each pinned against the ones after it.
    fn operands(&mut self, es: &[&Expr]) -> Option<Vec<Operand>> {
        let mut ops = Vec::with_capacity(es.len());
        for (i, e) in es.iter().enumerate() {
            let op = self.operand(e)?;
            ops.push(self.pinned(op, &es[i + 1..]));
        }
        Some(ops)
    }

    /// The operands of a statement that reads them without consuming them. A counted one is
    /// read into a temporary that owns it, which the caller releases after the statement, as
    /// codegen releases these operands today.
    fn borrowed(&mut self, es: &[&Expr]) -> Option<(Vec<Operand>, Vec<LocalId>)> {
        let (mut ops, mut temps) = (Vec::with_capacity(es.len()), Vec::new());
        for (i, e) in es.iter().enumerate() {
            if self.counted(e.ty) {
                let temp = self.temp_of(e)?;
                temps.push(temp);
                ops.push(Operand::Read(whole(temp)));
            } else {
                let op = self.operand(e)?;
                ops.push(self.pinned(op, &es[i + 1..]));
            }
        }
        Some((ops, temps))
    }

    /// The place a read at a borrow position starts from. A source local is read in place, as
    /// a borrow site, when `borrow` holds; any other base is evaluated into a temporary that
    /// owns it, which the caller releases after the read.
    fn base(&mut self, base: &Expr, borrow: bool) -> Option<(MirPlace, Option<LocalId>)> {
        if borrow && let Some(local) = local_of(base) {
            let place = MirPlace {
                borrow: true,
                ..whole(local)
            };
            return Some((place, None));
        }
        let temp = self.temp_of(base)?;
        Some((whole(temp), Some(temp)))
    }

    /// A field or element read of `place` into `dest`: a copy of a counted value, a read of
    /// an uncounted one. Then the release of the base's temporary.
    fn read_place(
        &mut self,
        ty: Type,
        place: MirPlace,
        base: Option<LocalId>,
        dest: Option<LocalId>,
    ) -> Option<LocalId> {
        let value = if self.counted(ty) {
            Rvalue::Copy(place)
        } else {
            Rvalue::Use(Operand::Read(place))
        };
        let dest = self.assign(ty, dest, value);
        self.release(base);
        dest
    }

    /// A statement that reads `base` in place when it is a source local (B2, B3, B6), or
    /// evaluates it into a temporary released after.
    fn borrowing(
        &mut self,
        e: &Expr,
        base: &Expr,
        dest: Option<LocalId>,
        rvalue: fn(Operand) -> Rvalue,
    ) -> Lowered {
        let (place, temp) = self.base(base, true)?;
        let dest = self.assign(e.ty, dest, rvalue(Operand::Read(place)));
        self.release(temp);
        Some(dest)
    }

    /// The value of `e` into `dest`, or into a new temporary when `dest` is `None`.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per node kind, so the lowering reads in one place"
    )]
    fn into(&mut self, e: &Expr, dest: Option<LocalId>) -> Lowered {
        let value = match &e.kind {
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Char(_)
            | ExprKind::None
            | ExprKind::Local(_)
            | ExprKind::Move(_) => Rvalue::Use(
                self.leaf(e)
                    .expect("a constant or a local should be a leaf operand"),
            ),
            ExprKind::Str(text) => Rvalue::CopyStr(text.clone()),
            ExprKind::Call { func, args } => Rvalue::Call {
                func: *func,
                args: self.operands(&args.iter().collect::<Vec<_>>())?,
            },
            ExprKind::New { class, args } => Rvalue::New {
                class: *class,
                args: self.operands(&args.iter().collect::<Vec<_>>())?,
            },
            ExprKind::Variant { id, variant, args } => Rvalue::Variant {
                id: *id,
                variant: *variant,
                args: self.operands(&args.iter().collect::<Vec<_>>())?,
            },
            ExprKind::Field { base, index } => {
                let (mut place, temp) = self.base(base, true)?;
                place.projections.push(Projection::Field(*index));
                return Some(self.read_place(e.ty, place, temp, dest));
            }
            ExprKind::Wrap(operand) => Rvalue::Wrap(self.operand(operand)?),
            ExprKind::Convert(operand) => Rvalue::Convert(self.operand(operand)?),
            ExprKind::Print { stream, operand } => {
                let (ops, temps) = self.borrowed(&[operand])?;
                let [operand] = <[Operand; 1]>::try_from(ops)
                    .expect("`borrowed` should give one operand for each expression");
                self.assign(
                    e.ty,
                    None,
                    Rvalue::Print {
                        stream: *stream,
                        operand,
                    },
                );
                for temp in temps {
                    self.release(Some(temp));
                }
                return Some(None);
            }
            ExprKind::Panic(message) => {
                let message = self.operand(message)?;
                self.terminate(Terminator::Panic {
                    message,
                    span: e.span,
                });
                return None;
            }
            ExprKind::ReadLine => Rvalue::ReadLine,
            ExprKind::ArrayLit(elements) => {
                Rvalue::ArrayLit(self.operands(&elements.iter().collect::<Vec<_>>())?)
            }
            ExprKind::ArrayFill { value, len } => {
                let [value_op, len_op] = self.pair(value, len)?;
                Rvalue::ArrayFill {
                    value: value_op,
                    len: len_op,
                    span: len.span,
                }
            }
            ExprKind::Index { base, index } => {
                let (mut place, temp) = self.base(base, leaves_locals(index))?;
                let at = self.operand(index)?;
                place.projections.push(Projection::Index {
                    index: at,
                    span: index.span,
                });
                return Some(self.read_place(e.ty, place, temp, dest));
            }
            ExprKind::Len(base) => {
                if let ExprKind::Words(string) = &base.kind {
                    return self.borrowing(e, string, dest, Rvalue::LenWords);
                }
                return self.borrowing(e, base, dest, Rvalue::Len);
            }
            ExprKind::Chars(base) => return self.borrowing(e, base, dest, Rvalue::Chars),
            ExprKind::Bytes(base) => return self.borrowing(e, base, dest, Rvalue::Bytes),
            ExprKind::Words(base) => return self.borrowing(e, base, dest, Rvalue::Words),
            ExprKind::FromChars(base) => return self.borrowing(e, base, dest, Rvalue::FromChars),
            ExprKind::Sum(base) => Rvalue::Sum {
                base: self.operand(base)?,
                span: e.span,
            },
            ExprKind::Contains { base, value } => {
                let [base, value] = self.pair(base, value)?;
                Rvalue::Contains { base, value }
            }
            ExprKind::Math { op, operands } => Rvalue::Math {
                op: *op,
                operands: self.operands(&operands.iter().collect::<Vec<_>>())?,
            },
            ExprKind::Unary { op, operand } => Rvalue::Unary {
                op: *op,
                operand: self.operand(operand)?,
                span: e.span,
            },
            ExprKind::Binary { op, lhs, rhs } => match op {
                tir::BinaryOp::And => return self.short_circuit(true, lhs, rhs, dest),
                tir::BinaryOp::Or => return self.short_circuit(false, lhs, rhs, dest),
                tir::BinaryOp::Add
                | tir::BinaryOp::AddUnchecked
                | tir::BinaryOp::Sub
                | tir::BinaryOp::Mul
                | tir::BinaryOp::Div
                | tir::BinaryOp::Rem => {
                    let [lhs, rhs] = self.pair(lhs, rhs)?;
                    Rvalue::Binary {
                        op: *op,
                        lhs,
                        rhs,
                        span: e.span,
                    }
                }
            },
            ExprKind::Interpolate { parts, formats } => Rvalue::Interpolate {
                parts: self.operands(&parts.iter().collect::<Vec<_>>())?,
                formats: formats.clone(),
            },
            ExprKind::Concat { lhs, rhs } => {
                let [lhs, rhs] = self.pair(lhs, rhs)?;
                Rvalue::Concat { lhs, rhs }
            }
            ExprKind::Compare { operands, ops } => return self.compare(operands, ops, dest),
            ExprKind::Equal { negated, lhs, rhs } => {
                let negated = *negated;
                return self.borrowed_pair(e.ty, lhs, rhs, dest, |lhs, rhs| Rvalue::Equal {
                    negated,
                    lhs,
                    rhs,
                });
            }
            ExprKind::Is { lhs, rhs } => {
                return self
                    .borrowed_pair(e.ty, lhs, rhs, dest, |lhs, rhs| Rvalue::Is { lhs, rhs });
            }
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => return self.if_expr(cond, then_block, else_block.as_ref(), e.ty, dest),
            ExprKind::IfSome { .. } => panic!("IfSome lands in Task 2"),
            ExprKind::Match { .. } => panic!("Match lands in Task 2"),
            ExprKind::Loop(body) => return self.loop_expr(body),
            ExprKind::Break => {
                let frame = self
                    .frames
                    .last()
                    .expect("the checker should reject `break` outside a loop");
                let (exit, mark) = (frame.exit, frame.mark);
                let exit = exit.unwrap_or_else(|| self.new_block());
                self.frames
                    .last_mut()
                    .expect("the loop's frame should still be there")
                    .exit = Some(exit);
                self.release_scope(mark);
                self.goto(exit);
                return None;
            }
            ExprKind::Continue => {
                let frame = self
                    .frames
                    .last()
                    .expect("the checker should reject `continue` outside a loop");
                let (header, mark) = (frame.header, frame.mark);
                self.release_scope(mark);
                self.goto(header);
                return None;
            }
            ExprKind::Return(value) => {
                let value = match value {
                    Some(value) => self.into(value, None)?.map(|value| self.owned(value)),
                    None => None,
                };
                self.release_scope(0);
                self.terminate(Terminator::Return(value));
                return None;
            }
            ExprKind::Block(block) => return self.block(block, dest),
        };
        Some(self.assign(e.ty, dest, value))
    }

    /// Two operands of one statement, in order.
    fn pair(&mut self, lhs: &Expr, rhs: &Expr) -> Option<[Operand; 2]> {
        let ops = self.operands(&[lhs, rhs])?;
        Some(
            <[Operand; 2]>::try_from(ops)
                .expect("`operands` should give one operand for each expression"),
        )
    }

    /// A statement that reads two operands without consuming them, then the releases of their
    /// temporaries.
    fn borrowed_pair(
        &mut self,
        ty: Type,
        lhs: &Expr,
        rhs: &Expr,
        dest: Option<LocalId>,
        rvalue: impl FnOnce(Operand, Operand) -> Rvalue,
    ) -> Lowered {
        let (ops, temps) = self.borrowed(&[lhs, rhs])?;
        let [lhs, rhs] = <[Operand; 2]>::try_from(ops)
            .expect("`borrowed` should give one operand for each expression");
        let dest = self.assign(ty, dest, rvalue(lhs, rhs));
        for temp in temps {
            self.release(Some(temp));
        }
        Some(dest)
    }

    /// `lhs && rhs` (`and`) or `lhs || rhs`: `rhs` runs only when `lhs` does not decide the
    /// result. Both go to a fresh `bool` temporary, which no operand mentions.
    fn short_circuit(
        &mut self,
        and: bool,
        lhs: &Expr,
        rhs: &Expr,
        dest: Option<LocalId>,
    ) -> Lowered {
        let result = self.temp(Type::BOOL);
        self.into(lhs, Some(result))?;
        let (rhs_block, join) = (self.new_block(), self.new_block());
        let (then, else_) = if and {
            (rhs_block, join)
        } else {
            (join, rhs_block)
        };
        self.terminate(Terminator::Branch {
            cond: Operand::Read(whole(result)),
            then,
            else_,
        });
        self.current = Some(rhs_block);
        if self.into(rhs, Some(result)).is_some() {
            self.goto(join);
        }
        self.current = Some(join);
        match dest {
            Some(dest) => Some(self.assign(
                Type::BOOL,
                Some(dest),
                Rvalue::Use(Operand::Read(whole(result))),
            )),
            None => Some(Some(result)),
        }
    }

    /// A comparison chain `a < b <= c`: each step goes to one `bool` temporary, and a step
    /// that fails skips the rest. Two strings compare as borrowed operands.
    fn compare(&mut self, operands: &[Expr], ops: &[CompareOp], dest: Option<LocalId>) -> Lowered {
        if operands[0].ty == Type::STRING {
            let ([lhs, rhs], [op]) = (operands, ops) else {
                panic!("the checker should split a comparison chain of strings into pairs")
            };
            let op = *op;
            return self.borrowed_pair(Type::BOOL, lhs, rhs, dest, |lhs, rhs| Rvalue::Compare {
                op,
                lhs,
                rhs,
            });
        }
        let es: Vec<&Expr> = operands.iter().collect();
        let result = dest.unwrap_or_else(|| self.temp(Type::BOOL));
        let first = self.operand(es[0])?;
        let mut lhs = self.pinned(first, &es[1..]);
        let mut join = None;
        for (i, op) in ops.iter().enumerate() {
            let next = self.operand(es[i + 1])?;
            let rhs = self.pinned(next, &es[i + 2..]);
            self.push(Statement::Assign {
                dest: Some(result),
                value: Rvalue::Compare {
                    op: *op,
                    lhs,
                    rhs: rhs.clone(),
                },
            });
            if i + 1 < ops.len() {
                let next = self.new_block();
                let end = *join.get_or_insert_with(|| self.new_block());
                self.terminate(Terminator::Branch {
                    cond: Operand::Read(whole(result)),
                    then: next,
                    else_: end,
                });
                self.current = Some(next);
            }
            lhs = rhs;
        }
        if let Some(end) = join {
            self.goto(end);
            self.current = Some(end);
        }
        Some(Some(result))
    }

    /// `if cond { then } else { else_ }`: a branch, each block's value into one destination
    /// when the `if` has a value, and a join that exists only if an edge reaches it. Without
    /// an `else`, the else edge goes straight to the join.
    fn if_expr(
        &mut self,
        cond: &Expr,
        then_block: &Block,
        else_block: Option<&Block>,
        ty: Type,
        dest: Option<LocalId>,
    ) -> Lowered {
        let cond = self.operand(cond)?;
        let dest = valued(ty).then(|| dest.unwrap_or_else(|| self.temp(ty)));
        let (then, else_) = (self.new_block(), self.new_block());
        self.terminate(Terminator::Branch { cond, then, else_ });
        self.current = Some(then);
        let mut join = else_block.is_none().then_some(else_);
        self.arm(then_block, dest, &mut join);
        if let Some(else_block) = else_block {
            self.current = Some(else_);
            self.arm(else_block, dest, &mut join);
        }
        self.current = join;
        join.map(|_| dest)
    }

    /// One block of an `if`: its value into `dest`, or discarded when the `if` has none, then
    /// a jump to the join, made on the first jump.
    fn arm(&mut self, block: &Block, dest: Option<LocalId>, join: &mut Option<BlockId>) {
        let Some(value) = self.block(block, dest) else {
            return;
        };
        if dest.is_none() {
            self.discard(value);
        }
        let to = *join.get_or_insert_with(|| self.new_block());
        self.goto(to);
    }

    /// `loop`: the current block enters the header, the body runs and jumps back to it, and
    /// `break` jumps to an exit block that exists only once a `break` does.
    fn loop_expr(&mut self, body: &Block) -> Lowered {
        let pre_header = self.here();
        let header = self.new_block();
        self.goto(header);
        self.current = Some(header);
        let index = self.loops.len();
        self.loops.push(Loop {
            pre_header,
            header,
            blocks: Vec::new(),
        });
        self.frames.push(Frame {
            header,
            exit: None,
            mark: self.scope.len(),
        });
        if let Some(value) = self.block(body, None) {
            self.discard(value);
            self.goto(header);
        }
        let exit = self
            .frames
            .pop()
            .expect("the loop's frame should still be there")
            .exit;
        self.loops[index].blocks = (header.0..self.blocks.len())
            .map(BlockId)
            .filter(|&block| Some(block) != exit)
            .collect();
        self.current = exit;
        exit.map(|_| None)
    }

    /// `base.index = value`. A source local base is a borrow site when the value leaves every
    /// local as it was (B5); any other base is read into a temporary first and released after
    /// the store.
    fn field_store(&mut self, base: &Expr, index: usize, value: &Expr) -> Option<()> {
        let (mut place, temp) = self.base(base, leaves_locals(value))?;
        place.projections.push(Projection::Field(index));
        let value_op = self.operand(value)?;
        self.store(place, value_op, value.ty);
        self.release(temp);
        Some(())
    }

    /// The store of `value` into `place`; for a counted type, the old value is taken out first
    /// and released after.
    fn store(&mut self, place: MirPlace, value: Operand, ty: Type) {
        if !self.counted(ty) {
            self.push(Statement::Store { place, value });
            return;
        }
        let old = self.temp(ty);
        self.push(Statement::Take {
            dest: old,
            place: place.clone(),
        });
        self.push(Statement::Store { place, value });
        self.push(Statement::Release(old));
    }

    /// The root of a written element place, and its index operands from the root outwards,
    /// each pinned against the indexes after it and `value`. A field root's base is read into
    /// a temporary, never borrowed, as `place_operands` evaluates it today; the caller releases
    /// it after the store.
    fn written_root<'p>(&mut self, place: &'p Place, value: &Expr) -> Option<WrittenRoot<'p>> {
        let mut indexes = Vec::new();
        let mut root = place;
        while let Place::Index { base, index } = root {
            indexes.push(index);
            root = base;
        }
        indexes.reverse();
        let (root, temp) = match root {
            Place::Local(local) => (whole(*local), None),
            Place::Field { base, index } => {
                let temp = self.temp_of(base)?;
                let mut root = whole(temp);
                root.projections.push(Projection::Field(*index));
                (root, Some(temp))
            }
            Place::Index { .. } => unreachable!("the loop above should stop at the root"),
        };
        let later: Vec<&Expr> = indexes.iter().copied().chain([value]).collect();
        let mut ops = Vec::with_capacity(indexes.len());
        for (i, index) in indexes.iter().enumerate() {
            let op = self.operand(index)?;
            ops.push((*index, self.pinned(op, &later[i + 1..])));
        }
        Some((root, ops, temp))
    }

    /// `root` with every index projection of `indexes`, with no statement.
    fn indexed(root: &MirPlace, indexes: &[(&Expr, Operand)]) -> MirPlace {
        let mut place = root.clone();
        for (index, op) in indexes {
            place.projections.push(Projection::Index {
                index: op.clone(),
                span: index.span,
            });
        }
        place
    }

    /// Per level of an element write, after its value: the bounds check of that level's
    /// index, then `make_unique` of the array it indexes. Gives the written place.
    fn walk(&mut self, root: MirPlace, indexes: &[(&Expr, Operand)]) -> MirPlace {
        let mut place = root;
        for (index, op) in indexes {
            let array = place.clone();
            place.projections.push(Projection::Index {
                index: op.clone(),
                span: index.span,
            });
            self.push(Statement::Check(place.clone()));
            self.push(Statement::MakeUnique(array));
        }
        place
    }

    /// `p[i]... = value`: a field root's base, the indexes, the value, then per level the
    /// check and `make_unique`, then the store, then the release of the base.
    fn element_write(&mut self, place: &Place, value: &Expr) -> Option<()> {
        let (root, indexes, temp) = self.written_root(place, value)?;
        let value_op = self.operand(value)?;
        let place = self.walk(root, &indexes);
        self.store(place, value_op, value.ty);
        self.release(temp);
        Some(())
    }

    /// `p += value` on a string field or element, in the spec's order: a copy of `p`, the
    /// value, the walk, the release of `p`'s own reference, the `concat`, the store, then the
    /// release of a field root's base.
    fn append(&mut self, place: &Place, value: &Expr) -> Option<()> {
        let (root, indexes, temp) = self.written_root(place, value)?;
        let read = Self::indexed(&root, &indexes);
        let old_text = self.assign(Type::STRING, None, Rvalue::Copy(read));
        let value_op = self.operand(value)?;
        let place = self.walk(root, &indexes);
        let old = self.temp(Type::STRING);
        self.push(Statement::Take {
            dest: old,
            place: place.clone(),
        });
        self.push(Statement::Release(old));
        let lhs = Operand::Move(old_text.expect("a string should have a value"));
        let joined = self.assign(Type::STRING, None, Rvalue::Concat { lhs, rhs: value_op });
        self.push(Statement::Store {
            place,
            value: Operand::Move(joined.expect("a string should have a value")),
        });
        self.release(temp);
        Some(())
    }
}

/// Whether evaluating `expr` leaves every local as it was, so a local read before it still
/// holds the same block after it: codegen's `leaves_locals`, with `Move` read as `Local`, since
/// mir decides moves itself. The sibling condition of B1 and B5.
fn leaves_locals(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Int(_) | ExprKind::Local(_) | ExprKind::Move(_) => true,
        ExprKind::Unary { operand, .. }
        | ExprKind::Convert(operand)
        | ExprKind::Len(operand)
        | ExprKind::Field { base: operand, .. } => leaves_locals(operand),
        ExprKind::Binary { lhs, rhs, .. } => leaves_locals(lhs) && leaves_locals(rhs),
        ExprKind::Compare { operands, .. } => operands.iter().all(leaves_locals),
        ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Char(_)
        | ExprKind::Str(_)
        | ExprKind::None
        | ExprKind::Call { .. }
        | ExprKind::New { .. }
        | ExprKind::Variant { .. }
        | ExprKind::Wrap(_)
        | ExprKind::Print { .. }
        | ExprKind::Panic(_)
        | ExprKind::ReadLine
        | ExprKind::ArrayLit(_)
        | ExprKind::ArrayFill { .. }
        | ExprKind::Index { .. }
        | ExprKind::Chars(_)
        | ExprKind::Bytes(_)
        | ExprKind::Words(_)
        | ExprKind::FromChars(_)
        | ExprKind::Sum(_)
        | ExprKind::Contains { .. }
        | ExprKind::Math { .. }
        | ExprKind::Interpolate { .. }
        | ExprKind::Concat { .. }
        | ExprKind::Equal { .. }
        | ExprKind::Is { .. }
        | ExprKind::If { .. }
        | ExprKind::IfSome { .. }
        | ExprKind::Match { .. }
        | ExprKind::Loop(_)
        | ExprKind::Break
        | ExprKind::Continue
        | ExprKind::Return(_)
        | ExprKind::Block(_) => false,
    }
}

/// Each local's mentions in a function body, in the order `block_mentions` walks it, and the
/// stretch of that order each expression covers. Whether any of a run of sibling expressions
/// mentions a local is then a binary search, not a walk: the body is walked once.
pub(super) struct Mentions {
    /// The local of each mention, in walk order.
    found: Vec<usize>,
    /// Each expression's mentions: `found[start..end]`.
    spans: FxHashMap<*const Expr, (usize, usize)>,
    /// Each local's positions in `found`, ascending.
    at: Vec<Vec<usize>>,
}

impl Mentions {
    /// The mentions of `body`, whose function has `locals` locals.
    pub(super) fn of(body: &Block, locals: usize) -> Mentions {
        let mut mentions = Mentions {
            found: Vec::new(),
            spans: FxHashMap::default(),
            at: vec![Vec::new(); locals],
        };
        block_mentions(body, &mut mentions);
        for (position, &local) in mentions.found.iter().enumerate() {
            mentions.at[local].push(position);
        }
        mentions
    }

    /// Whether any of `siblings`, adjacent expressions in walk order, mentions `local`.
    fn within(&self, local: LocalId, siblings: &[&Expr]) -> bool {
        let (Some(first), Some(last)) = (siblings.first(), siblings.last()) else {
            return false;
        };
        let (start, end) = (self.span(first).0, self.span(last).1);
        // A lowering temporary is past the `tir` locals, and no expression mentions it.
        let Some(at) = self.at.get(local.0) else {
            return false;
        };
        at.get(at.partition_point(|&position| position < start))
            .is_some_and(|&position| position < end)
    }

    fn span(&self, e: &Expr) -> (usize, usize) {
        *self
            .spans
            .get(&ptr::from_ref(e))
            .expect("the walk should have reached every expression of the body")
    }

    /// Whether the body mentions `local` at all.
    #[cfg(test)]
    pub(super) fn mentions(&self, local: usize) -> bool {
        !self.at[local].is_empty()
    }
}

/// Adds to `found` each local that `e` reads or assigns, and those its blocks define, and
/// records the stretch of `found` that `e` covers.
fn mentions(e: &Expr, found: &mut Mentions) {
    #[cfg(test)]
    WALKED.set(WALKED.get() + 1);
    let start = found.found.len();
    expr_mentions(e, found);
    found
        .spans
        .insert(ptr::from_ref(e), (start, found.found.len()));
}

#[cfg(test)]
thread_local! {
    /// How many expressions `mentions` has walked on this thread.
    pub(super) static WALKED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// `mentions` of `e`'s own node and its children.
fn expr_mentions(e: &Expr, found: &mut Mentions) {
    match &e.kind {
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Char(_)
        | ExprKind::Str(_)
        | ExprKind::None
        | ExprKind::ReadLine
        | ExprKind::Break
        | ExprKind::Continue
        | ExprKind::Return(None) => {}
        ExprKind::Local(local) | ExprKind::Move(local) => found.found.push(local.0),
        ExprKind::Call { args, .. }
        | ExprKind::New { args, .. }
        | ExprKind::Variant { args, .. }
        | ExprKind::ArrayLit(args)
        | ExprKind::Interpolate { parts: args, .. }
        | ExprKind::Math { operands: args, .. }
        | ExprKind::Compare { operands: args, .. } => {
            for arg in args {
                mentions(arg, found);
            }
        }
        ExprKind::Field { base: operand, .. }
        | ExprKind::Len(operand)
        | ExprKind::Chars(operand)
        | ExprKind::Bytes(operand)
        | ExprKind::Words(operand)
        | ExprKind::FromChars(operand)
        | ExprKind::Sum(operand)
        | ExprKind::Wrap(operand)
        | ExprKind::Convert(operand)
        | ExprKind::Print { operand, .. }
        | ExprKind::Unary { operand, .. }
        | ExprKind::Panic(operand)
        | ExprKind::Return(Some(operand)) => mentions(operand, found),
        ExprKind::Binary { lhs, rhs, .. }
        | ExprKind::Concat { lhs, rhs }
        | ExprKind::Equal { lhs, rhs, .. }
        | ExprKind::Is { lhs, rhs }
        | ExprKind::ArrayFill {
            value: lhs,
            len: rhs,
        }
        | ExprKind::Index {
            base: lhs,
            index: rhs,
        }
        | ExprKind::Contains {
            base: lhs,
            value: rhs,
        } => {
            mentions(lhs, found);
            mentions(rhs, found);
        }
        ExprKind::If {
            cond: scrutinee,
            then_block,
            else_block,
        }
        | ExprKind::IfSome {
            scrutinee,
            then_block,
            else_block,
            ..
        } => {
            mentions(scrutinee, found);
            block_mentions(then_block, found);
            if let Some(else_block) = else_block {
                block_mentions(else_block, found);
            }
        }
        ExprKind::Match {
            scrutinee, arms, ..
        } => {
            mentions(scrutinee, found);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    mentions(guard, found);
                }
                block_mentions(&arm.body, found);
            }
        }
        ExprKind::Loop(block) | ExprKind::Block(block) => block_mentions(block, found),
    }
}

/// Adds to `found` each local that `block` reads or assigns.
fn block_mentions(block: &Block, found: &mut Mentions) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Let { local, init } => {
                found.found.push(local.0);
                mentions(init, found);
            }
            Stmt::Assign { place, value } | Stmt::Append { place, value } => {
                place_mentions(place, found);
                mentions(value, found);
            }
            Stmt::Expr(e) => mentions(e, found),
        }
    }
    if let Some(value) = &block.value {
        mentions(value, found);
    }
}

/// Adds to `found` each local that `place` names or reads.
fn place_mentions(place: &Place, found: &mut Mentions) {
    match place {
        Place::Local(local) => found.found.push(local.0),
        Place::Field { base, .. } => mentions(base, found),
        Place::Index { base, index } => {
            place_mentions(base, found);
            mentions(index, found);
        }
    }
}
