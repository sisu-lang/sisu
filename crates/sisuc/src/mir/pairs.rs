//! Pair removal: a copy that is only read within its block becomes a borrow, and its release
//! becomes `end`s, so the retain and the release it pairs with are gone.
//!
//! A candidate is `t = copy p` where `t` is counted and is not `p.local`, `p.local` is not a
//! `match` binding (it has no entry in `owners`), and `p.local` is not a member of a candidate
//! still open at that statement. Its members are `t` and each `u` that an exact `u = move m` of a
//! member `m` makes. It closes at a `release` of its last member in the same block, and is a pair
//! when all of these hold:
//!
//! 1. Every mention of every member, anywhere in the function, terminators included, falls
//!    between the copy and that release.
//! 2. Between them, a member is only read, moved on by `u = move m`, or written through by a
//!    `take`, a store or a `make_unique` when its type is a class. A write into an array or a
//!    string copy is copy-on-write of a value with a second holder; as a borrow it would write
//!    into `p`'s value in place.
//! 3. Between them, nothing consumes, assigns, releases or writes `p.local`.
//! 4. When `p` goes through a `Field` or an `Index`, nothing between them clobbers: a call, a
//!    `take` into a counted local, a store of a counted value, or a `make_unique`. A call can
//!    replace any field or element, a `take` or a counted store can free one, and a
//!    `make_unique` decides on the count that the copy would hold.
//!
//! The pass walks each block once, in order: each open member maps to its candidate, and each
//! candidate keeps the mentions of its members seen so far, and the root's kills and the block's
//! clobbers at its copy, which its release compares. The mentions and their effects come from the
//! certifier's own visitor, so the two passes classify every statement the same way, and the
//! certifier rejects a wrong removal in a debug build.
//!
//! The rewrite keeps the meaning. `t = borrow p` reads the value the copy read, without the
//! retain: by condition 3 `p.local` still holds it at the release, and by condition 4 no field or
//! element on the path was replaced, so the value lives as long as `t` is read. Later changes to
//! an index local do not matter: the borrow read the element once, as the copy did. Each
//! `u = move m` becomes `u = borrow m`, which holds the same pointer. The release becomes `end`
//! of each member, last first: it released the count the copy added, which is no longer there.
//! `owners` maps each member straight to `p.local`.

use rustc_hash::FxHashMap;

use super::certify::{self, Effect};
use super::{Function, LocalId, Operand, Place, Projection, Rvalue, Statement};
use crate::tir::TypeKind;

/// What one statement is to the pass.
enum Kind<'a> {
    /// `t = copy p`.
    Copy(LocalId, &'a Place),
    /// `u = move m`.
    Step(LocalId, LocalId),
    Release(LocalId),
    /// Condition 4's clobbers.
    Clobber,
    Other,
}

/// A copy and the members it has gathered so far.
struct Candidate {
    root: LocalId,
    /// Whether the copied place goes through a field or an element, so a clobber blocks it.
    unstable: bool,
    /// Each member and the statement that made it: the copy, then each `u = move m`.
    links: Vec<(usize, LocalId)>,
    /// The members' mentions seen since the copy, and over the whole function.
    seen: usize,
    total: usize,
    /// The root's kills and writes, and the block's clobbers, at the copy.
    kills: usize,
    clobbers: usize,
    /// Whether every mention so far keeps it a pair (condition 2).
    valid: bool,
}

impl Candidate {
    /// The candidate that `t = copy place`, statement `at` of its block, opens.
    fn new(
        at: usize,
        t: LocalId,
        place: &Place,
        mentions: &[usize],
        kills: &[usize],
        clobbers: usize,
    ) -> Self {
        Candidate {
            root: place.local,
            unstable: place.projections.iter().any(|projection| {
                matches!(
                    projection,
                    Projection::Field { .. } | Projection::Index { .. }
                )
            }),
            links: vec![(at, t)],
            // The copy's own assignment of `t`.
            seen: 1,
            total: mentions[t.0],
            kills: kills[place.local.0],
            clobbers,
            valid: true,
        }
    }

    fn current(&self) -> LocalId {
        self.links
            .last()
            .expect("a candidate should have its copy")
            .1
    }
}

/// What a pair does to one statement of its block.
enum Rewrite {
    /// The copy or a chain step becomes a borrow.
    Borrow,
    /// The release becomes an `end` of each of these members, last first.
    Ends(Vec<LocalId>),
}

/// Turns each pair of `function` into a borrow and `end`s.
pub(super) fn remove_pairs(function: &mut Function) {
    let (found, owners) = pairs(function);
    for (b, rewrites) in found {
        let block = &mut function.blocks[b];
        let stmts = std::mem::take(&mut block.stmts);
        for (stmt, rewrite) in stmts.into_iter().zip(rewrites) {
            match rewrite {
                None => block.stmts.push(stmt),
                Some(Rewrite::Borrow) => block.stmts.push(borrowed(stmt)),
                Some(Rewrite::Ends(members)) => {
                    block
                        .stmts
                        .extend(members.into_iter().rev().map(Statement::End));
                }
            }
        }
    }
    function.owners.extend(owners);
}

/// Each block with a pair, with what its pairs do to each of its statements, and each member
/// with its root.
#[expect(
    clippy::type_complexity,
    reason = "a block's rewrites and the owners are read once, by `remove_pairs`"
)]
fn pairs(function: &Function) -> (Vec<(usize, Vec<Option<Rewrite>>)>, Vec<(LocalId, LocalId)>) {
    let mut mentions = vec![0_usize; function.locals.len()];
    for block in &function.blocks {
        for stmt in &block.stmts {
            certify::statement(stmt, false, &mut |local, _, _| mentions[local.0] += 1);
        }
        certify::terminator(&block.term, false, &mut |local, _, _| {
            mentions[local.0] += 1;
        });
    }
    // Each local's kills and writes so far; a candidate compares the count at its copy.
    let mut kills = vec![0_usize; function.locals.len()];
    let mut found = Vec::new();
    let mut owners = Vec::new();
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut events = Vec::new();
    for (b, block) in function.blocks.iter().enumerate() {
        candidates.clear();
        // A fresh map per block: clearing a used one costs the capacity the widest block left.
        let mut open: FxHashMap<LocalId, usize> = FxHashMap::default();
        let mut clobbers = 0;
        let mut rewrites: Vec<Option<Rewrite>> = Vec::new();
        for (at, stmt) in block.stmts.iter().enumerate() {
            events.clear();
            certify::statement(stmt, false, &mut |local, effect, _| {
                events.push((local, effect));
            });
            let kind = kind(function, stmt);
            for &(local, effect) in &events {
                if let Some(&c) = open.get(&local) {
                    let candidate = &mut candidates[c];
                    candidate.seen += 1;
                    let current = candidate.current() == local;
                    candidate.valid &= allowed(function, &kind, &open, local, current, effect);
                }
                if effect.kills() || effect == Effect::Write {
                    kills[local.0] += 1;
                }
            }
            match kind {
                Kind::Copy(t, place)
                    if function.locals[t.0].counted
                        && t != place.local
                        && !function.owners.contains_key(&place.local)
                        && !open.contains_key(&place.local)
                        && !open.contains_key(&t) =>
                {
                    open.insert(t, candidates.len());
                    candidates.push(Candidate::new(at, t, place, &mentions, &kills, clobbers));
                }
                Kind::Step(u, m) => {
                    if let Some(&c) = open.get(&m)
                        && candidates[c].current() == m
                        && !open.contains_key(&u)
                    {
                        let candidate = &mut candidates[c];
                        candidate.links.push((at, u));
                        candidate.seen += 1;
                        candidate.total += mentions[u.0];
                        open.insert(u, c);
                    }
                }
                Kind::Release(x) => {
                    if let Some(&c) = open.get(&x)
                        && candidates[c].current() == x
                    {
                        let candidate = &mut candidates[c];
                        for (_, member) in &candidate.links {
                            open.remove(member);
                        }
                        if candidate.valid
                            && candidate.seen == candidate.total
                            && kills[candidate.root.0] == candidate.kills
                            && (!candidate.unstable || clobbers == candidate.clobbers)
                        {
                            if rewrites.is_empty() {
                                rewrites.resize_with(block.stmts.len(), || None);
                            }
                            let links = std::mem::take(&mut candidate.links);
                            for &(made, member) in &links {
                                rewrites[made] = Some(Rewrite::Borrow);
                                owners.push((member, candidate.root));
                            }
                            rewrites[at] = Some(Rewrite::Ends(
                                links.into_iter().map(|(_, member)| member).collect(),
                            ));
                        }
                    }
                }
                Kind::Clobber => clobbers += 1,
                Kind::Copy(..) | Kind::Other => {}
            }
        }
        if !rewrites.is_empty() {
            found.push((b, rewrites));
        }
    }
    (found, owners)
}

/// Condition 2: whether `member`, its candidate's last member when `current`, may have
/// `effect` in a statement of `kind` and its candidate stay a pair.
fn allowed(
    function: &Function,
    kind: &Kind<'_>,
    open: &FxHashMap<LocalId, usize>,
    member: LocalId,
    current: bool,
    effect: Effect,
) -> bool {
    match effect {
        Effect::Read | Effect::ReadInPlace => true,
        Effect::Write => matches!(function.locals[member.0].ty.kind(), TypeKind::Class(_)),
        Effect::Consume => {
            current && matches!(*kind, Kind::Step(u, m) if m == member && !open.contains_key(&u))
        }
        Effect::Release => current,
        Effect::ReadConsumed
        | Effect::ScopeRelease
        | Effect::Assign
        | Effect::AssignBorrowed
        | Effect::End => false,
    }
}

/// What `stmt` is to the pass. The clobbers are the certifier's (`certify::Clobbers`).
fn kind<'a>(function: &Function, stmt: &'a Statement) -> Kind<'a> {
    match stmt {
        Statement::Assign { dest, value } => match (value, *dest) {
            (Rvalue::Copy(place), Some(t)) => Kind::Copy(t, place),
            (Rvalue::Use(Operand::Move(m)), Some(u)) => Kind::Step(u, *m),
            (Rvalue::Call { .. }, _) => Kind::Clobber,
            (
                Rvalue::Use(_)
                | Rvalue::Copy(_)
                | Rvalue::CopyStr(_)
                | Rvalue::Borrow(_)
                | Rvalue::Unwrap(_)
                | Rvalue::New { .. }
                | Rvalue::Variant { .. }
                | Rvalue::ArrayLit(_)
                | Rvalue::ArrayFill { .. }
                | Rvalue::Wrap(_)
                | Rvalue::Concat { .. }
                | Rvalue::Interpolate { .. }
                | Rvalue::ReadLine
                | Rvalue::Chars(_)
                | Rvalue::Bytes(_)
                | Rvalue::Words(_)
                | Rvalue::FromChars(_)
                | Rvalue::Len(_)
                | Rvalue::LenWords(_)
                | Rvalue::Sum { .. }
                | Rvalue::Contains { .. }
                | Rvalue::Convert(_)
                | Rvalue::Math { .. }
                | Rvalue::Unary { .. }
                | Rvalue::Binary { .. }
                | Rvalue::Compare { .. }
                | Rvalue::Equal { .. }
                | Rvalue::Is { .. }
                | Rvalue::Print { .. },
                _,
            ) => Kind::Other,
        },
        Statement::Take { dest, .. } if function.locals[dest.0].counted => Kind::Clobber,
        Statement::Store { value, .. } if certify::counted_value(function, value) => Kind::Clobber,
        Statement::MakeUnique(_) => Kind::Clobber,
        Statement::Release(local) => Kind::Release(*local),
        Statement::Take { .. }
        | Statement::Store { .. }
        | Statement::ScopeRelease(_)
        | Statement::Check(_)
        | Statement::End(_) => Kind::Other,
    }
}

/// A pair's copy or chain step, as a borrow of the place it read.
fn borrowed(stmt: Statement) -> Statement {
    match stmt {
        Statement::Assign { dest, value } => {
            let place = match value {
                Rvalue::Copy(place) => place,
                Rvalue::Use(Operand::Move(local)) => Place {
                    local,
                    projections: Vec::new(),
                    borrow: false,
                    guard: false,
                },
                Rvalue::Use(_)
                | Rvalue::CopyStr(_)
                | Rvalue::Borrow(_)
                | Rvalue::Unwrap(_)
                | Rvalue::Call { .. }
                | Rvalue::New { .. }
                | Rvalue::Variant { .. }
                | Rvalue::ArrayLit(_)
                | Rvalue::ArrayFill { .. }
                | Rvalue::Wrap(_)
                | Rvalue::Concat { .. }
                | Rvalue::Interpolate { .. }
                | Rvalue::ReadLine
                | Rvalue::Chars(_)
                | Rvalue::Bytes(_)
                | Rvalue::Words(_)
                | Rvalue::FromChars(_)
                | Rvalue::Len(_)
                | Rvalue::LenWords(_)
                | Rvalue::Sum { .. }
                | Rvalue::Contains { .. }
                | Rvalue::Convert(_)
                | Rvalue::Math { .. }
                | Rvalue::Unary { .. }
                | Rvalue::Binary { .. }
                | Rvalue::Compare { .. }
                | Rvalue::Equal { .. }
                | Rvalue::Is { .. }
                | Rvalue::Print { .. } => {
                    panic!("a pair's copy or chain step should be a copy or a move")
                }
            };
            Statement::Assign {
                dest,
                value: Rvalue::Borrow(place),
            }
        }
        Statement::Take { .. }
        | Statement::Store { .. }
        | Statement::Release(_)
        | Statement::ScopeRelease(_)
        | Statement::MakeUnique(_)
        | Statement::Check(_)
        | Statement::End(_) => panic!("a pair's copy or chain step should be an assignment"),
    }
}
