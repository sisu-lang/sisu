//! Exhaustiveness and reachability of `match`: Maranget's usefulness algorithm ("Warnings for
//! pattern matching", 2007), run once over a matrix of every arm's row plus a last wildcard row,
//! the sentinel. A value reaches the sentinel exactly when it reaches no arm.
//!
//! Specializing a column puts each row in the bucket of its head constructor; a wildcard row goes
//! into every bucket and into the default matrix, which stands for every constructor no row
//! names. The constructors of an enum are its variants, those of `bool` its two values, and
//! those of an optional `None` and `Some`, which holds the payload as its one field. An
//! integer or `char` column is split into disjoint intervals at every bound its ranges use, and
//! each interval is a constructor. A `string` column has a constructor for each of its literals,
//! and the default stands for every other string, so only a wildcard covers a `string`. An `|`
//! row is expanded only when it reaches the head column, into one row per alternative in its
//! place.
//!
//! A matrix with no column left is a leaf: its first row takes its values, and the rows after it
//! are not reached there. A guarded row is reached but takes nothing, so a leaf reaches every
//! row up to its first unguarded one. A matrix whose first row has only wildcards left, and no
//! guard, is cut short the same way. Rows are hash-consed stacks of (pattern, column type) on a
//! bottom that says whether the arm is guarded, so a matrix is its rows' stack ids, and a matrix
//! met twice is solved once. Each `|` is interned apart, by its span, so a matrix met twice holds
//! the same alternatives, and reaching one is recorded once for the whole check. A solved matrix
//! whose rows took an alternative's values, or whose children's did, keeps those takers by its own
//! rows. Once the solve ends, one pass from the top renames each such matrix's rows to the arms
//! they come from on every path to it, so the arm that covers an alternative comes out of the one
//! solve.
//!
//! shortcut: usefulness can meet exponentially many distinct matrices when payload patterns are
//! correlated across fields (arm i needs fields `x_i` and `y_i`, with every x before every y);
//! deciding exhaustiveness is co-NP-complete in general, so no algorithm avoids it; revisit only
//! if a real program hits it.

use std::rc::Rc;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::source::Span;
use crate::tir::{self, Type, TypeArgs, TypeKind};

#[cfg(test)]
thread_local! {
    /// How many rows, stack nodes and fields the solver has read or built on this thread, counted
    /// where it does so, but not the intervals a range row is put in: that is `INTERVAL_STEPS`.
    pub(super) static STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many intervals the solver has put a range row in on this thread, which costs the rows
    /// times the intervals they cover, as `frame` says.
    pub(super) static INTERVAL_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Adds `work` to `STEPS`.
#[cfg(test)]
fn step(work: usize) {
    STEPS.set(STEPS.get() + work);
}

/// One arm's pattern: a row of the matrix.
pub(super) struct Row<'a> {
    pub(super) pattern: &'a tir::Pattern,
    /// Whether the arm has a guard, which may be false: the row then takes no value.
    pub(super) guarded: bool,
}

/// What `check` found.
pub(super) struct Report {
    /// A value no row matches, when there is one.
    pub(super) missing: Option<Missing>,
    /// Each row, and each alternative of a reached row, that no value reaches, in row order.
    pub(super) unreachable: Vec<Unreachable>,
}

/// A value no row matches.
pub(super) struct Missing {
    /// The value, written as a pattern: `Mul(_, _)`, `10..=255`.
    pub(super) pattern: String,
    /// The first row with the value's variant at the top (an `|` by its first alternative with
    /// it): that pattern's span, the pattern, and its payload, which is all of that variant it
    /// covers.
    pub(super) partly: Option<(Span, String, String)>,
    /// Whether a guarded row may not match the value.
    pub(super) guarded: bool,
    /// Whether the value is a gap in an integer, `char` or `string` type, which `_` closes.
    pub(super) gap: bool,
}

/// A row, or an alternative of a reached row, that no value reaches.
pub(super) struct Unreachable {
    /// The row's pattern, or the alternative.
    pub(super) at: Span,
    /// The pattern of the row that takes every value of this one, when one other row takes them
    /// all.
    pub(super) covered_by: Option<Span>,
    /// For an alternative, the alternative and the space to the alternative next to it, which
    /// holds the `|` that joins them: the one before it, or after it when it is first.
    pub(super) alternative: Option<Span>,
}

/// Which `rows`, the arms of a `match` on `ty` in order, no value reaches, and a value that
/// none matches. `enums` and `conformances` are the program's.
pub(super) fn check(
    enums: &[tir::Enum],
    conformances: &tir::Conformances,
    ty: Type,
    rows: &[Row<'_>],
) -> Report {
    let mut solver = Solver {
        enums,
        conformances,
        pats: vec![Pat::Wild],
        ors: vec![false],
        pat_ids: FxHashMap::default(),
        nodes: vec![
            Node {
                pat: WILD,
                col: Col::Other,
                tail: EMPTY,
                wild: true,
                ors: false,
            },
            Node {
                pat: WILD,
                col: Col::Other,
                tail: GUARDED,
                wild: false,
                ors: false,
            },
        ],
        node_ids: FxHashMap::default(),
        memo: FxHashMap::default(),
        defaults: FxHashMap::default(),
        reached: FxHashSet::default(),
        taken: Vec::new(),
    };
    let col = column(ty);
    let pats: Vec<_> = rows
        .iter()
        .map(|row| solver.intern(row.pattern, col))
        .collect();
    let mut stacks: Vec<_> = rows
        .iter()
        .zip(&pats)
        .map(|(row, &pat)| solver.push(pat, col, bottom(row.guarded)))
        .collect();
    stacks.push(solver.push(WILD, col, EMPTY));
    let result = solver.solve(stacks.into());
    let takers = solver.takers(result.taken, rows.len() + 1);
    let mut unreachable = Vec::new();
    for (k, (row, reach)) in rows.iter().zip(&result.reach).enumerate() {
        let covered_by = match reach {
            Reach::Reached => {
                let mut found = Vec::new();
                solver.unreached(row.pattern, pats[k], &mut found);
                for (alternative, at, remove) in found {
                    // The alternatives before it in its own arm cover none of it: a value they
                    // take would reach it, were it alone.
                    let covered_by = match takers.get(&alternative) {
                        Some(&(first, last)) if first == last && first != k => {
                            Some(rows[first].pattern.span)
                        }
                        _ => None,
                    };
                    unreachable.push(Unreachable {
                        at,
                        covered_by,
                        alternative: Some(remove),
                    });
                }
                continue;
            }
            &Reach::Taken(first, last) => (first == last).then(|| rows[first].pattern.span),
        };
        unreachable.push(Unreachable {
            at: row.pattern.span,
            covered_by,
            alternative: None,
        });
    }
    let missing = result.witness.map(|witness| {
        let top = solver.nodes[witness].pat;
        let partly = solver.partly(top, rows, &pats);
        let (pattern, gap) = match (col, result.gap) {
            (Col::I64 | Col::U8 | Col::Char, Some((lo, hi))) => {
                (print_range(lo, hi, col == Col::Char), true)
            }
            (Col::Str, _) => (solver.print(top), true),
            _ => (solver.print(top), false),
        };
        Missing {
            pattern,
            partly,
            guarded: result.guarded,
            gap,
        }
    });
    Report {
        missing,
        unreachable,
    }
}

/// An interned pattern, its bindings forgotten: an index into `Solver::pats`.
type PatId = usize;

/// An alternative: its `|` and its index there.
type Alternative = (PatId, usize);

/// The interned `_`, which a binding is too.
const WILD: PatId = 0;

/// A stack of (pattern, column type) pairs, a row's columns from the first, on a bottom: an
/// index into `Solver::nodes`.
type StackId = usize;

/// The bottom of an unguarded row's stack: the stack with no column.
const EMPTY: StackId = 0;

/// The bottom of a guarded row's stack, which is not `wild`, so the all-wildcard cut never
/// takes a guarded row, and a matrix differs from one whose rows have no guard.
const GUARDED: StackId = 1;

/// The bottom of a row's stack.
fn bottom(guarded: bool) -> StackId {
    if guarded { GUARDED } else { EMPTY }
}

/// The type of a column as coverage sees it: its enum with its type arguments, an optional with
/// its payload, a type whose literals or ranges a pattern names, or `Other`, a type whose values
/// no pattern names.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Col {
    Enum(usize, TypeArgs),
    Optional(Type),
    I64,
    U8,
    Char,
    Bool,
    Str,
    Other,
}

fn column(ty: Type) -> Col {
    match ty.kind() {
        TypeKind::Enum(id, args) => Col::Enum(id.0, args),
        TypeKind::Optional(payload) => Col::Optional(payload),
        TypeKind::I64 => Col::I64,
        TypeKind::U8 => Col::U8,
        TypeKind::Char => Col::Char,
        TypeKind::Bool => Col::Bool,
        TypeKind::String => Col::Str,
        _ => Col::Other,
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Pat {
    Wild,
    Variant {
        enum_id: usize,
        variant: usize,
        fields: Box<[PatId]>,
    },
    Bool(bool),
    /// `lo..=hi`, of code points when `char`.
    Range {
        lo: i64,
        hi: i64,
        char: bool,
    },
    Str(String),
    /// An `|` written at `at`: each one is a pattern of its own.
    Or {
        at: Span,
        alts: Box<[PatId]>,
    },
    /// The `None` of an optional, its constructor 0.
    None,
    /// A value of an optional, its constructor 1, whose one field is the payload.
    Some(PatId),
}

/// `lo..=hi` as a pattern, of code points when `char`: one value alone, else a range.
fn print_range(lo: i64, hi: i64, char: bool) -> String {
    let one = |value: i64| {
        if !char {
            return value.to_string();
        }
        match u32::try_from(value).ok().and_then(char::from_u32) {
            Some(c) if c == ' ' || (c.is_ascii_graphic() && c != '\'' && c != '\\') => {
                format!("'{c}'")
            }
            _ => format!("'\\u{{{value:X}}}'"),
        }
    };
    if lo == hi {
        one(lo)
    } else {
        format!("{}..={}", one(lo), one(hi))
    }
}

/// A stack's top: its first column's pattern and type, and the stack below. `wild` is whether
/// every pattern in the stack is `_` and the bottom has no guard; `ors` whether one holds an `|`.
#[derive(Clone, Copy)]
struct Node {
    pat: PatId,
    col: Col,
    tail: StackId,
    wild: bool,
    ors: bool,
}

/// Where one row of a matrix stands, from the leaves below it.
#[derive(Clone, Copy)]
enum Reach {
    /// A leaf reached it.
    Reached,
    /// No leaf reached it, and the earlier rows that took the leaves' values run from the first
    /// to the last of these, equal when one row took them all. The two ends are enough: renaming
    /// a child's rows to the matrix above keeps their order, so the ends stay the ends.
    Taken(usize, usize),
}

/// What the leaves under one row say so far, `None` before the first.
#[derive(Default)]
struct Takers(Option<Reach>);

impl Takers {
    /// Adds what `found` says, its takers renamed by `rename`, which keeps their order.
    fn add(&mut self, found: Reach, rename: impl Fn(usize) -> usize) {
        #[cfg(test)]
        step(1);
        let found = match found {
            Reach::Reached => Reach::Reached,
            Reach::Taken(first, last) => Reach::Taken(rename(first), rename(last)),
        };
        self.0 = Some(match (self.0, found) {
            (None, found) => found,
            (Some(Reach::Reached), _) | (_, Reach::Reached) => Reach::Reached,
            (Some(Reach::Taken(first, last)), Reach::Taken(other_first, other_last)) => {
                Reach::Taken(first.min(other_first), last.max(other_last))
            }
        });
    }

    /// What every leaf added says, once at least one was.
    fn reach(self) -> Reach {
        self.0.expect("a row should land in a child matrix")
    }
}

/// What a matrix's leaves say, by the matrix's own row indices.
struct Solved {
    reach: Vec<Reach>,
    /// A value that only the last row, the sentinel, matches: a stack of one pattern per
    /// column, shared with the matrices that found it.
    witness: Option<StackId>,
    /// Whether a guarded row reaches the sentinel in a leaf of the witness: of its first
    /// column's gap, for an integer or `char` column.
    guarded: bool,
    /// For an integer or `char` first column some row names a range of, the witness's value
    /// there: the lowest interval that reaches the sentinel, joined with each next adjacent one
    /// that does too.
    gap: Option<(i64, i64)>,
    /// The index in `Solver::taken` of this matrix, when its rows or its children's took an
    /// alternative's values.
    taken: Option<usize>,
}

/// A matrix whose rows took the values of alternatives, or one of whose children's rows did.
struct Taken {
    /// Each alternative its rows took values of, and the first and last row that took them.
    here: Vec<(Alternative, (usize, usize))>,
    /// Each child that is a `Taken` too: its index, and for each of its rows, the row of this
    /// matrix it comes from.
    below: Vec<(usize, Vec<usize>)>,
}

/// A row of a matrix with each `|` at its head expanded: the head and the stack below it, the
/// matrix row it comes from, and its `|` and alternative when it is one.
#[derive(Clone, Copy)]
struct Expanded {
    head: PatId,
    tail: StackId,
    origin: usize,
    alternative: Option<Alternative>,
}

/// A matrix that specializing a column makes: the bucket of the constructor `order`, or the
/// default, which stands for the constructor `order` (the first one no row names) and every
/// other constructor no row names, or for `_` when no row names one.
struct Child {
    order: usize,
    bucket: bool,
    /// The parent's expanded rows in it, in order.
    members: Vec<usize>,
}

/// A matrix being solved: its first column split into `children`, of which the first `next`
/// are solved.
struct Frame {
    rows: Box<[StackId]>,
    expanded: Vec<Expanded>,
    col: Col,
    /// For an integer or `char` column, its intervals, which are its constructors in order; the
    /// ones no row names; and whether a row names one.
    intervals: Vec<(i64, i64)>,
    unnamed: Vec<usize>,
    split: bool,
    children: Vec<Child>,
    next: usize,
    /// What the solved children say of each expanded row.
    reach: Vec<Takers>,
    /// The children that are a `Taken`, as `Taken::below`.
    below: Vec<(usize, Vec<usize>)>,
    /// The witness first in constructor order so far: its child's order, the child, the
    /// child's witness, and its `guarded`.
    best: Option<(usize, usize, StackId, bool)>,
    /// The intervals that reach the sentinel, with the `guarded` of their child.
    gaps: Vec<(i64, i64, bool)>,
}

impl Frame {
    /// Takes in what the child `next` found, and moves on to the next child.
    fn feed(&mut self, solved: &Solved) {
        let child = &self.children[self.next];
        // `Reach::Taken` keeps only the ends: renaming keeps the order, since a child's members
        // and the expanded rows' origins both ascend.
        debug_assert!(child.members.is_sorted(), "a child's members should ascend");
        // A row another takes is named by its matrix row, so two alternatives of one row that
        // take a row's values are one row.
        for (&e, &found) in child.members.iter().zip(&solved.reach) {
            #[cfg(test)]
            step(1);
            let expanded = &self.expanded;
            self.reach[e].add(found, |by| expanded[child.members[by]].origin);
        }
        if let Some(taken) = solved.taken {
            let origins = child.members.iter().map(|&e| self.expanded[e].origin);
            self.below.push((taken, origins.collect()));
        }
        if let Some(witness) = solved.witness {
            if self.best.is_none_or(|(order, ..)| child.order < order) {
                self.best = Some((child.order, self.next, witness, solved.guarded));
            }
            if child.bucket && !self.intervals.is_empty() {
                let (lo, hi) = self.intervals[child.order];
                self.gaps.push((lo, hi, solved.guarded));
            } else {
                for &k in &self.unnamed {
                    let (lo, hi) = self.intervals[k];
                    self.gaps.push((lo, hi, solved.guarded));
                }
            }
        }
        self.next += 1;
    }
}

struct Solver<'a> {
    enums: &'a [tir::Enum],
    conformances: &'a tir::Conformances,
    pats: Vec<Pat>,
    /// Whether each pattern holds an `|`.
    ors: Vec<bool>,
    pat_ids: FxHashMap<Pat, PatId>,
    nodes: Vec<Node>,
    node_ids: FxHashMap<(PatId, Col, StackId), StackId>,
    /// Each matrix solved so far, by its rows.
    memo: FxHashMap<Box<[StackId]>, Rc<Solved>>,
    /// The all-`_` pattern of each (enum, variant) a default has stood for, so a default head
    /// costs its payload's width only once.
    defaults: FxHashMap<(usize, usize), PatId>,
    /// Each alternative a leaf reached, over the whole check.
    reached: FxHashSet<Alternative>,
    /// Each matrix solved so far whose rows or children's rows took an alternative's values, in
    /// the order they were solved, so a matrix comes after every child.
    taken: Vec<Taken>,
}

/// Puts the expanded row `e` into the bucket of the constructor `key`, which comes `order`th,
/// made with the wildcard rows `wild` above it when this is its first row.
fn join(
    bucket: &mut FxHashMap<usize, usize>,
    children: &mut Vec<Child>,
    wild: &[usize],
    (key, order): (usize, usize),
    e: usize,
) {
    let b = *bucket.entry(key).or_insert_with(|| {
        children.push(Child {
            order,
            bucket: true,
            members: wild.to_vec(),
        });
        children.len() - 1
    });
    children[b].members.push(e);
}

impl Solver<'_> {
    /// `ty` with `args` for its type parameters and each associated type of a known type
    /// replaced (see `Checker::subst`). A type that does not normalize keeps its form.
    fn subst(&self, ty: Type, args: &[Type]) -> Type {
        let ty = ty.subst(args);
        crate::conformance::normalize(self.conformances, ty).unwrap_or(ty)
    }

    fn pat(&mut self, pat: Pat) -> PatId {
        // Interning hashes the whole pattern.
        #[cfg(test)]
        step(match &pat {
            Pat::Variant { fields, .. } => fields.len(),
            Pat::Or { alts, .. } => alts.len(),
            _ => 1,
        });
        let (pats, ors) = (&mut self.pats, &mut self.ors);
        *self.pat_ids.entry(pat).or_insert_with_key(|pat| {
            ors.push(match pat {
                Pat::Or { .. } => true,
                Pat::Variant { fields, .. } => fields.iter().any(|&field| ors[field]),
                &Pat::Some(field) => ors[field],
                _ => false,
            });
            pats.push(pat.clone());
            pats.len() - 1
        })
    }

    /// `p`, at a column of type `col`.
    fn intern(&mut self, p: &tir::Pattern, col: Col) -> PatId {
        match &p.kind {
            tir::PatternKind::Wild | tir::PatternKind::Bind(_) => WILD,
            tir::PatternKind::Variant { variant, fields } => {
                let Col::Enum(enum_id, args) = col else {
                    panic!("a variant pattern should sit at an enum")
                };
                let enums = self.enums;
                let payload = &enums[enum_id].variants[*variant].fields;
                let fields = fields
                    .iter()
                    .zip(payload)
                    .map(|(field, payload)| {
                        let ty = self.subst(payload.ty, args.get());
                        self.intern(field, column(ty))
                    })
                    .collect();
                self.pat(Pat::Variant {
                    enum_id,
                    variant: *variant,
                    fields,
                })
            }
            tir::PatternKind::Bool(value) => self.pat(Pat::Bool(*value)),
            tir::PatternKind::Range { lo, hi } => self.pat(Pat::Range {
                lo: *lo,
                hi: *hi,
                char: col == Col::Char,
            }),
            tir::PatternKind::Str(text) => self.pat(Pat::Str(text.clone())),
            tir::PatternKind::Or(alts) => {
                let alts = alts.iter().map(|alt| self.intern(alt, col)).collect();
                self.pat(Pat::Or { at: p.span, alts })
            }
            tir::PatternKind::Some(inner) => {
                let Col::Optional(payload) = col else {
                    panic!("a `Some` pattern should sit at an optional")
                };
                let inner = self.intern(inner, column(payload));
                self.pat(Pat::Some(inner))
            }
            tir::PatternKind::None => self.pat(Pat::None),
        }
    }

    /// The stack of `pat`, of type `col`, on `tail`.
    fn push(&mut self, pat: PatId, col: Col, tail: StackId) -> StackId {
        #[cfg(test)]
        step(1);
        let wild = pat == WILD && self.nodes[tail].wild;
        let ors = self.ors[pat] || self.nodes[tail].ors;
        let nodes = &mut self.nodes;
        *self.node_ids.entry((pat, col, tail)).or_insert_with(|| {
            nodes.push(Node {
                pat,
                col,
                tail,
                wild,
                ors,
            });
            nodes.len() - 1
        })
    }

    /// Adds to `found` each alternative in `p`, interned as `pat`, that no leaf reached, but not
    /// the alternatives inside it: the alternative, its span, and the span the fix removes.
    fn unreached(&self, p: &tir::Pattern, pat: PatId, found: &mut Vec<(Alternative, Span, Span)>) {
        match (&p.kind, &self.pats[pat]) {
            (tir::PatternKind::Variant { fields, .. }, Pat::Variant { fields: ids, .. }) => {
                for (field, &id) in fields.iter().zip(ids.iter()) {
                    self.unreached(field, id, found);
                }
            }
            (tir::PatternKind::Some(inner), &Pat::Some(id)) => self.unreached(inner, id, found),
            (tir::PatternKind::Or(alts), Pat::Or { alts: ids, .. }) => {
                for (a, (alt, &id)) in alts.iter().zip(ids.iter()).enumerate() {
                    if self.reached.contains(&(pat, a)) {
                        self.unreached(alt, id, found);
                    } else {
                        let remove = match a.checked_sub(1) {
                            Some(before) => Span::new(alts[before].span.end(), alt.span.end()),
                            None => Span::new(alt.span.start(), alts[1].span.start()),
                        };
                        found.push(((pat, a), alt.span, remove));
                    }
                }
            }
            _ => {}
        }
    }

    /// Adds to `found` each alternative in `pat`, inside one another too, as taken by the first
    /// row.
    fn alternatives_in(&self, pat: PatId, found: &mut Vec<(Alternative, (usize, usize))>) {
        match &self.pats[pat] {
            Pat::Variant { fields, .. } => {
                for &field in fields.iter().filter(|&&field| self.ors[field]) {
                    self.alternatives_in(field, found);
                }
            }
            &Pat::Some(field) => self.alternatives_in(field, found),
            Pat::Or { alts, .. } => {
                for (a, &alt) in alts.iter().enumerate() {
                    found.push(((pat, a), (0, 0)));
                    self.alternatives_in(alt, found);
                }
            }
            _ => {}
        }
    }

    /// The first and last of the `rows` rows of the matrix whose `taken` is `top` that took each
    /// alternative's values: each `Taken`'s rows are renamed to the top rows they come from on
    /// every path, and the ends are enough, as in `Reach::Taken`.
    fn takers(&self, top: Option<usize>, rows: usize) -> FxHashMap<Alternative, (usize, usize)> {
        let mut takers: FxHashMap<Alternative, (usize, usize)> = FxHashMap::default();
        let Some(top) = top else {
            return takers;
        };
        // Each `Taken`'s rows as top rows, filled by its parents, which come after it.
        let mut renamed: Vec<Option<Vec<(usize, usize)>>> = vec![None; top + 1];
        renamed[top] = Some((0..rows).map(|row| (row, row)).collect());
        for id in (0..=top).rev() {
            let names = renamed[id]
                .take()
                .expect("a taken matrix should lie below the top, so a parent named its rows");
            let Taken { here, below } = &self.taken[id];
            let join = |(first, last): &mut (usize, usize), (lo, hi): (usize, usize)| {
                *first = (*first).min(lo);
                *last = (*last).max(hi);
            };
            for &(alternative, (first, last)) in here {
                let found = (names[first].0, names[last].1);
                join(takers.entry(alternative).or_insert(found), found);
            }
            for (child, origins) in below {
                let child =
                    renamed[*child].get_or_insert_with(|| vec![(usize::MAX, 0); origins.len()]);
                for (name, &origin) in child.iter_mut().zip(origins) {
                    join(name, names[origin]);
                }
            }
        }
        takers
    }

    /// The first unguarded row of `rows`, interned as `pats`, with the variant of `top`, a missing
    /// value, at the top, or an `|` by its first alternative with it: that pattern's span, the
    /// pattern, and its payload. With a value missing, no row is a wildcard: one would match every
    /// value.
    fn partly(
        &self,
        top: PatId,
        rows: &[Row<'_>],
        pats: &[PatId],
    ) -> Option<(Span, String, String)> {
        let (missing, _) = self.variant(top)?;
        // A guarded row covers nothing, so it covers no part of the variant either.
        rows.iter()
            .zip(pats)
            .filter(|(row, _)| !row.guarded)
            .find_map(|(row, &pat)| {
                let alternatives = match (&row.pattern.kind, &self.pats[pat]) {
                    (tir::PatternKind::Or(written), Pat::Or { alts, .. }) => {
                        written.iter().zip(alts.iter()).collect()
                    }
                    _ => vec![(row.pattern, &pat)],
                };
                alternatives
                    .into_iter()
                    .find_map(|(written, &alt)| match self.variant(alt) {
                        Some((variant, fields)) if variant == missing => {
                            Some((written.span, self.print(alt), self.print_all(fields)))
                        }
                        _ => None,
                    })
            })
    }

    /// The variant `id` names and its fields, unless it is no variant pattern.
    fn variant(&self, id: PatId) -> Option<(usize, &[PatId])> {
        match &self.pats[id] {
            Pat::Variant {
                variant, fields, ..
            } => Some((*variant, fields)),
            _ => None,
        }
    }

    fn print(&self, id: PatId) -> String {
        match &self.pats[id] {
            Pat::Wild => "_".to_string(),
            Pat::Variant {
                enum_id,
                variant,
                fields,
            } => {
                let name = &self.enums[*enum_id].variants[*variant].name;
                if fields.is_empty() {
                    name.clone()
                } else {
                    format!("{name}({})", self.print_all(fields))
                }
            }
            Pat::Bool(value) => value.to_string(),
            &Pat::Range { lo, hi, char } => print_range(lo, hi, char),
            Pat::Str(text) => format!("{text:?}"),
            Pat::Or { alts, .. } => {
                let printed: Vec<_> = alts.iter().map(|&alt| self.print(alt)).collect();
                printed.join(" | ")
            }
            Pat::None => "None".to_string(),
            // As the user writes it: a pattern of the value inside stands alone.
            &Pat::Some(inner) if self.inside(inner) => self.print(inner),
            &Pat::Some(inner) => format!("Some({})", self.print(inner)),
        }
    }

    /// Whether `id`, the pattern of an optional's value, matches the value inside as it stands
    /// at the optional's position: a variant, literal or range pattern, inside every layer.
    fn inside(&self, id: PatId) -> bool {
        match &self.pats[id] {
            Pat::Variant { .. } | Pat::Bool(_) | Pat::Range { .. } | Pat::Str(_) => true,
            &Pat::Some(inner) => self.inside(inner),
            Pat::Or { alts, .. } => alts.iter().all(|&alt| self.inside(alt)),
            Pat::Wild | Pat::None => false,
        }
    }

    fn print_all(&self, ids: &[PatId]) -> String {
        let printed: Vec<_> = ids.iter().map(|&id| self.print(id)).collect();
        printed.join(", ")
    }

    /// Field `k` of `pat`, a variant pattern, a `Some` or `_`.
    fn field(&self, pat: PatId, k: usize) -> PatId {
        match &self.pats[pat] {
            Pat::Variant { fields, .. } => fields[k],
            &Pat::Some(payload) => payload,
            _ => WILD,
        }
    }

    /// The matrix of `rows`, whose last row has only wildcards, solved. A matrix in progress
    /// waits on a stack of frames, not on the Rust stack, so a payload thousands of fields wide
    /// does not overflow it.
    fn solve(&mut self, rows: Box<[StackId]>) -> Rc<Solved> {
        let mut frames = Vec::new();
        let mut done = self.start(rows, &mut frames);
        loop {
            if let Some(solved) = done.take() {
                let Some(top) = frames.last_mut() else {
                    return solved;
                };
                top.feed(&solved);
            }
            let top = frames.last().expect("a frame should wait for its children");
            if top.next == top.children.len() {
                let frame = frames.pop().expect("a frame should wait for its children");
                done = Some(self.finish(frame));
            } else {
                let rows = self.child_rows(top);
                done = self.start(rows, &mut frames);
            }
        }
    }

    /// The matrix of `rows` solved, when the cut, a leaf or the memo solves it at once;
    /// otherwise `None`, with a frame for it pushed onto `frames`.
    fn start(&mut self, rows: Box<[StackId]>, frames: &mut Vec<Frame>) -> Option<Rc<Solved>> {
        // The cut, the memo's hash and the leaf's `reach` each read every row.
        #[cfg(test)]
        step(rows.len());
        // The all-wildcard row cut: the first row takes every value the matrix stands for.
        if self.nodes[rows[0]].wild {
            let mut reach = vec![Reach::Taken(0, 0); rows.len()];
            reach[0] = Reach::Reached;
            // Alone, the first row is the sentinel, already a `_` for each column.
            let witness = (rows.len() == 1).then_some(rows[0]);
            // The first row takes the values of every alternative below it, too. A matrix met
            // again records them once.
            if !rows[1..].iter().any(|&row| self.nodes[row].ors) {
                return Some(Rc::new(Solved {
                    reach,
                    witness,
                    guarded: false,
                    gap: None,
                    taken: None,
                }));
            }
            if let Some(solved) = self.memo.get(&rows) {
                return Some(Rc::clone(solved));
            }
            let mut here = Vec::new();
            for &row in &rows[1..] {
                let mut stack = row;
                while self.nodes[stack].ors {
                    let node = self.nodes[stack];
                    self.alternatives_in(node.pat, &mut here);
                    stack = node.tail;
                }
            }
            self.taken.push(Taken {
                here,
                below: Vec::new(),
            });
            let solved = Rc::new(Solved {
                reach,
                witness,
                guarded: false,
                gap: None,
                taken: Some(self.taken.len() - 1),
            });
            self.memo.insert(rows, Rc::clone(&solved));
            return Some(solved);
        }
        // A leaf whose first row is guarded: every row is a bottom, and the guarded rows up to
        // the first unguarded one are reached, without taking its values.
        if rows[0] == GUARDED {
            let first = rows
                .iter()
                .position(|&row| row == EMPTY)
                .expect("the sentinel should have no guard");
            let mut reach = vec![Reach::Taken(first, first); rows.len()];
            reach[..=first].fill(Reach::Reached);
            let witness = (first == rows.len() - 1).then_some(EMPTY);
            return Some(Rc::new(Solved {
                reach,
                witness,
                guarded: witness.is_some(),
                gap: None,
                taken: None,
            }));
        }
        if let Some(solved) = self.memo.get(&rows) {
            return Some(Rc::clone(solved));
        }
        frames.push(self.frame(rows));
        None
    }

    /// The frame that specializes the first column of `rows`, each `|` at its head expanded:
    /// one bucket per constructor a row names, in the order the rows name them, then the
    /// default when a constructor is left.
    fn frame(&self, rows: Box<[StackId]>) -> Frame {
        let col = self.nodes[rows[0]].col;
        let mut expanded = Vec::with_capacity(rows.len());
        for (origin, &row) in rows.iter().enumerate() {
            let Node { pat, tail, .. } = self.nodes[row];
            if let Pat::Or { alts, .. } = &self.pats[pat] {
                expanded.extend(alts.iter().enumerate().map(|(a, &head)| Expanded {
                    head,
                    tail,
                    origin,
                    alternative: Some((pat, a)),
                }));
            } else {
                expanded.push(Expanded {
                    head: pat,
                    tail,
                    origin,
                    alternative: None,
                });
            }
        }
        let intervals = self.intervals(col, &expanded);
        let mut bucket = FxHashMap::default();
        let mut children: Vec<Child> = Vec::new();
        let mut wild = Vec::new();
        for (e, row) in expanded.iter().enumerate() {
            #[cfg(test)]
            step(1);
            match self.pats[row.head] {
                Pat::Variant { variant, .. } => {
                    join(&mut bucket, &mut children, &wild, (variant, variant), e);
                }
                Pat::Bool(value) => {
                    let value = usize::from(value);
                    join(&mut bucket, &mut children, &wild, (value, value), e);
                }
                // The default comes first, so it gives the witness, a `_`.
                Pat::Str(_) => join(&mut bucket, &mut children, &wild, (row.head, 1), e),
                Pat::Range { lo, hi, .. } => {
                    // shortcut: a range row is bucketed once per interval it covers, so many
                    // wide overlapping ranges cost rows × intervals; index the intervals when a
                    // program needs it.
                    let first = intervals.partition_point(|&(start, _)| start < lo);
                    for k in (first..intervals.len()).take_while(|&k| intervals[k].0 <= hi) {
                        #[cfg(test)]
                        INTERVAL_STEPS.set(INTERVAL_STEPS.get() + 1);
                        join(&mut bucket, &mut children, &wild, (k, k), e);
                    }
                }
                Pat::Wild => {
                    // shortcut: a wildcard row joins every bucket, so n named constructors above
                    // n wildcard rows cost n²; share the wildcard rows between buckets if a real
                    // program hits it.
                    for child in &mut children {
                        #[cfg(test)]
                        step(1);
                        child.members.push(e);
                    }
                    wild.push(e);
                }
                Pat::None => join(&mut bucket, &mut children, &wild, (0, 0), e),
                Pat::Some(_) => join(&mut bucket, &mut children, &wild, (1, 1), e),
                Pat::Or { .. } => panic!("an alternative should not be an `|`"),
            }
        }
        #[cfg(test)]
        step(intervals.len());
        let unnamed: Vec<_> = (0..intervals.len())
            .filter(|k| !bucket.contains_key(k))
            .collect();
        let constructors = match col {
            Col::Enum(e, _) => Some(self.enums[e].variants.len()),
            Col::Bool | Col::Optional(_) => Some(2),
            Col::I64 | Col::U8 | Col::Char => Some(intervals.len()),
            Col::Str | Col::Other => None,
        };
        if children.is_empty() || constructors.is_none_or(|count| children.len() < count) {
            // For an interval column the first unnamed interval is the first index no bucket
            // holds, which the last arm finds as well.
            let order = match col {
                Col::Str => 0,
                _ => (0..children.len())
                    .find(|v| !bucket.contains_key(v))
                    .unwrap_or(children.len()),
            };
            children.push(Child {
                order,
                bucket: false,
                members: wild,
            });
        }
        Frame {
            reach: expanded.iter().map(|_| Takers::default()).collect(),
            below: Vec::new(),
            rows,
            expanded,
            col,
            split: !intervals.is_empty() && !bucket.is_empty(),
            intervals,
            unnamed,
            children,
            next: 0,
            best: None,
            gaps: Vec::new(),
        }
    }

    /// The disjoint intervals of a column of type `col` that hold its values, split at every
    /// bound of the ranges at the heads of `rows`, in order; none for a type that has no ranges.
    fn intervals(&self, col: Col, rows: &[Expanded]) -> Vec<(i64, i64)> {
        let domain: &[(i64, i64)] = match col {
            Col::I64 => &[(i64::MIN, i64::MAX)],
            Col::U8 => &[(0, 255)],
            // A `char` is never a surrogate.
            Col::Char => &[(0, 0xD7FF), (0xE000, 0x10_FFFF)],
            _ => return Vec::new(),
        };
        // Each point starts an interval; `i128`, since one ends `i64::MAX`.
        let mut points: Vec<i128> = domain
            .iter()
            .flat_map(|&(lo, hi)| [i128::from(lo), i128::from(hi) + 1])
            .collect();
        for row in rows {
            #[cfg(test)]
            step(1);
            if let Pat::Range { lo, hi, .. } = self.pats[row.head] {
                points.extend([i128::from(lo), i128::from(hi) + 1]);
            }
        }
        points.sort_unstable();
        points.dedup();
        points
            .windows(2)
            .map(|pair| {
                let fits = "an interval in a domain should fit an `i64`";
                (
                    i64::try_from(pair[0]).expect(fits),
                    i64::try_from(pair[1] - 1).expect(fits),
                )
            })
            .filter(|&(lo, hi)| domain.iter().any(|&(start, end)| start <= lo && hi <= end))
            .collect()
    }

    /// The rows of the child `next` of `frame`: an enum bucket's rows with the variant's fields
    /// in place of their first column, `Some`'s with the payload, any other child's without it.
    fn child_rows(&mut self, frame: &Frame) -> Box<[StackId]> {
        let child = &frame.children[frame.next];
        let heads: Vec<_> = child
            .members
            .iter()
            .map(|&e| {
                #[cfg(test)]
                step(1);
                frame.expanded[e]
            })
            .collect();
        // `Some`'s bucket: the payload in place of the optional.
        if let Col::Optional(payload) = frame.col
            && child.bucket
            && child.order == 1
        {
            let col = column(payload);
            return (heads.into_iter())
                .map(|head| self.push(self.field(head.head, 0), col, head.tail))
                .collect();
        }
        let Col::Enum(enum_id, args) = frame.col else {
            return heads.into_iter().map(|head| head.tail).collect();
        };
        if !child.bucket {
            return heads.into_iter().map(|head| head.tail).collect();
        }
        let enums = self.enums;
        let payload = &enums[enum_id].variants[child.order].fields;
        heads
            .into_iter()
            .map(|head| {
                payload
                    .iter()
                    .enumerate()
                    .rev()
                    .fold(head.tail, |tail, (k, field)| {
                        let pat = self.field(head.head, k);
                        let ty = self.subst(field.ty, args.get());
                        self.push(pat, column(ty), tail)
                    })
            })
            .collect()
    }

    /// What `frame`, all of whose children are solved, says; also kept for its rows. Each
    /// alternative its expanded rows reached is recorded.
    fn finish(&mut self, frame: Frame) -> Rc<Solved> {
        let witness = frame.best.map(|(order, child, witness, _)| {
            let head = match frame.col {
                // A bucket's witness starts with the variant's fields: they become its payload.
                Col::Enum(enum_id, _) if frame.children[child].bucket => {
                    let count = self.enums[enum_id].variants[order].fields.len();
                    let mut fields = Vec::with_capacity(count);
                    let mut rest = witness;
                    for _ in 0..count {
                        let node = self.nodes[rest];
                        fields.push(node.pat);
                        rest = node.tail;
                    }
                    let head = self.pat(Pat::Variant {
                        enum_id,
                        variant: order,
                        fields: fields.into(),
                    });
                    return self.push(head, frame.col, rest);
                }
                // The default stands for a variant when a row names another.
                Col::Enum(enum_id, _) if frame.children.len() > 1 => {
                    if let Some(&head) = self.defaults.get(&(enum_id, order)) {
                        head
                    } else {
                        let count = self.enums[enum_id].variants[order].fields.len();
                        let head = self.pat(Pat::Variant {
                            enum_id,
                            variant: order,
                            fields: vec![WILD; count].into(),
                        });
                        self.defaults.insert((enum_id, order), head);
                        head
                    }
                }
                Col::Bool if frame.children[child].bucket || frame.children.len() > 1 => {
                    self.pat(Pat::Bool(order == 1))
                }
                // `Some`'s witness starts with the payload's.
                Col::Optional(_) if frame.children[child].bucket && order == 1 => {
                    let node = self.nodes[witness];
                    let head = self.pat(Pat::Some(node.pat));
                    return self.push(head, frame.col, node.tail);
                }
                Col::Optional(_) if frame.children[child].bucket || frame.children.len() > 1 => {
                    let head = if order == 0 {
                        Pat::None
                    } else {
                        Pat::Some(WILD)
                    };
                    self.pat(head)
                }
                _ => WILD,
            };
            self.push(head, frame.col, witness)
        });
        // The gap: the lowest interval that reaches the sentinel, joined with each next one
        // adjacent to it, their `guarded`s together.
        let mut gaps = frame.gaps;
        gaps.sort_unstable_by_key(|&(lo, ..)| lo);
        let (gap, guarded) = match gaps.split_first() {
            Some((&(lo, mut hi, mut guarded), rest)) => {
                for &(next, end, next_guarded) in rest {
                    if hi.checked_add(1) != Some(next) {
                        break;
                    }
                    hi = end;
                    guarded |= next_guarded;
                }
                (frame.split.then_some((lo, hi)), guarded)
            }
            None => (None, frame.best.is_some_and(|(.., guarded)| guarded)),
        };
        let mut reach: Vec<Takers> = frame.rows.iter().map(|_| Takers::default()).collect();
        let mut here = Vec::new();
        for (row, found) in frame.expanded.iter().zip(frame.reach) {
            let found = found.reach();
            match (found, row.alternative) {
                (Reach::Reached, Some(alternative)) => {
                    self.reached.insert(alternative);
                }
                (Reach::Taken(first, last), Some(alternative)) => {
                    here.push((alternative, (first, last)));
                }
                (_, None) => {}
            }
            reach[row.origin].add(found, |by| by);
        }
        let taken = (!here.is_empty() || !frame.below.is_empty()).then(|| {
            self.taken.push(Taken {
                here,
                below: frame.below,
            });
            self.taken.len() - 1
        });
        let solved = Rc::new(Solved {
            reach: reach.into_iter().map(Takers::reach).collect(),
            witness,
            guarded,
            gap,
            taken,
        });
        self.memo.insert(frame.rows, Rc::clone(&solved));
        solved
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::tests::checked;

    /// The report on the `match` that function `t` of `src` ends in, without its last arm;
    /// `src` checks clean.
    fn report(src: &str) -> Report {
        let (program, ds) = checked(src);
        assert!(ds.is_empty(), "{ds:?}");
        let program = program.expect("the source should check clean");
        let t = program
            .functions
            .iter()
            .find(|f| f.name == "t")
            .expect("the source should define `t`");
        let Some(tir::ExprKind::Match {
            scrutinee, arms, ..
        }) = t.body.value.as_ref().map(|value| &value.kind)
        else {
            panic!("`t` should end in a `match`");
        };
        let (_, arms) = arms
            .split_last()
            .expect("the `match` should end in `_ => 0`");
        let rows: Vec<_> = arms
            .iter()
            .map(|arm| Row {
                pattern: &arm.pattern,
                guarded: arm.guard.is_some(),
            })
            .collect();
        check(&program.enums, &program.conformances, scrutinee.ty, &rows)
    }

    /// The texts of `Missing.partly`: the arm's pattern and its payload.
    type Texts<'a> = Option<(&'a str, &'a str)>;

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one row per witness, so the table reads in one place"
    )]
    fn witnesses_should_name_the_missing_pattern_of_each_row() {
        let decls = "enum Expr {\n    Num(i64)\n    Add(Expr, Expr)\n    Mul(Expr, Expr)\n}\nenum Name {\n    Named(string)\n    Anon\n}\nenum Shape {\n    Circle(f64)\n    Square(f64)\n}\nenum Holder {\n    V(Shape??)\n    W(i64?, bool)\n}\nfn main() {}";
        // Each row: the scrutinee, the arms before `_ => 0`, the missing pattern, the texts of
        // the arm that covers part of its variant, and whether a guarded arm may not match.
        let rows: [(&str, &[&str], &str, Texts<'_>, bool); 27] = [
            (
                "e",
                &["Num(_) => 1", "Add(_, _) => 2"],
                "Mul(_, _)",
                None,
                false,
            ),
            (
                "e",
                &["Num(_) => 1", "Add(Num(_), _) => 2", "Mul(_, _) => 3"],
                "Add(Add(_, _), _)",
                Some(("Add(Num(_), _)", "Num(_), _")),
                false,
            ),
            ("i", &[], "_", None, false),
            // A default before a bucket in variant order wins over it, and one after loses.
            ("e", &["Add(Num(_), _) => 1"], "Num(_)", None, false),
            (
                "e",
                &["Num(_) => 1", "Add(Num(_), _) => 2"],
                "Add(Add(_, _), _)",
                Some(("Add(Num(_), _)", "Num(_), _")),
                false,
            ),
            // A column no arm names a variant in prints as `_`.
            (
                "e",
                &["Num(_) => 1", "Add(_, Num(_)) => 2", "Mul(_, _) => 3"],
                "Add(_, Add(_, _))",
                Some(("Add(_, Num(_))", "_, Num(_)")),
                false,
            ),
            // The sentinel alone under two columns gives a `_` for each.
            (
                "e",
                &[
                    "Num(_) => 1",
                    "Add(Num(_), _) => 2",
                    "Add(Add(Num(_), _), _) => 3",
                    "Mul(_, _) => 4",
                ],
                "Add(Add(Add(_, _), _), _)",
                Some(("Add(Num(_), _)", "Num(_), _")),
                false,
            ),
            ("u", &["0..=9 => 1"], "10..=255", None, false),
            (
                "c",
                &["'\\u{0}'..='\\u{D7FF}' => 1"],
                "'\\u{E000}'..='\\u{10FFFF}'",
                None,
                false,
            ),
            ("b", &["true => 1"], "false", None, false),
            ("s", &["\"a\" => 1"], "_", None, false),
            // A bound only a guarded arm uses does not split the gap.
            ("u", &["0..=9 => 0", "15 if b => 1"], "10..=255", None, true),
            (
                "e",
                &["Num(0) => 1", "Add(_, _) => 2", "Mul(_, _) => 3"],
                "Num(_)",
                Some(("Num(0)", "0")),
                false,
            ),
            // A guarded arm takes no value, and covers no part of its variant.
            (
                "e",
                &["Num(n) if b => 1", "Add(_, _) => 2", "Mul(_, _) => 3"],
                "Num(_)",
                None,
                true,
            ),
            // Rows that differ only in a guard are not one row: the second does not take the
            // values the first leaves.
            (
                "e",
                &[
                    "Num(_) => 0",
                    "Add(Mul(_, _), _) => 2",
                    "Add(Num(_), _) if b => 1",
                    "Mul(_, _) => 3",
                ],
                "Add(Num(_), _)",
                Some(("Add(Mul(_, _), _)", "Mul(_, _), _")),
                true,
            ),
            // An alternative names the variant for `partly`.
            (
                "e",
                &["Add(_, _) | Num(0) => 1", "Mul(_, _) => 3"],
                "Num(_)",
                Some(("Num(0)", "0")),
                false,
            ),
            // A `char` prints as itself unless it is a quote, a backslash or not visible.
            (
                "c",
                &["'\\u{0}'..='&' => 1", "'c'..='\\u{10FFFF}' => 2"],
                "'\\u{27}'..='b'",
                None,
                false,
            ),
            // A column no arm names a value in prints as `_`, guarded or not.
            ("b", &["_ if b => 1"], "_", None, true),
            // The arm that covers part of a variant prints with its `|` and its strings.
            (
                "e",
                &[
                    "Num(_) => 1",
                    "Add(Num(0) | Num(1), _) => 2",
                    "Mul(_, _) => 3",
                ],
                "Add(Num(_), _)",
                Some(("Add(Num(0) | Num(1), _)", "Num(0) | Num(1), _")),
                false,
            ),
            (
                "n",
                &["Named(\"a\") => 1", "Anon => 2"],
                "Named(_)",
                Some(("Named(\"a\")", "\"a\"")),
                false,
            ),
            // An optional misses its `None`, a value, a variant of its payload as the arm
            // would name it, or the `None` of a middle layer.
            ("o", &["Some(m) => m"], "None", None, false),
            ("o", &["None => 1"], "Some(_)", None, false),
            (
                "sh",
                &["Circle(r) => 1", "None => 2"],
                "Square(_)",
                None,
                false,
            ),
            (
                "oo",
                &["Some(Some(m)) => m", "None => 1"],
                "Some(None)",
                None,
                false,
            ),
            // A guarded `None` may not match; a `Some` arm prints as the user writes it, its
            // `|` too; an optional field no arm names stays `_`.
            ("o", &["None if b => 1", "Some(_) => 2"], "None", None, true),
            (
                "h",
                &["V(Some(Circle(_) | Square(_))) => 1", "W(_, _) => 2"],
                "V(None)",
                Some(("V(Circle(_) | Square(_))", "Circle(_) | Square(_)")),
                false,
            ),
            (
                "h",
                &["V(_) => 1", "W(_, true) => 2"],
                "W(_, false)",
                Some(("W(_, true)", "_, true")),
                false,
            ),
        ];
        for (scrutinee, arms, pattern, partly, guarded) in rows {
            let arms = arms
                .iter()
                .map(|arm| format!("        {arm}\n"))
                .collect::<Vec<_>>()
                .concat();
            let src = format!(
                "{decls}\nfn t(e: Expr, i: i64, u: u8, c: char, b: bool, s: string, n: Name, o: i64?, sh: Shape?, oo: i64??, h: Holder) -> i64 {{\n    match {scrutinee} {{\n{arms}        _ => 0\n    }}\n}}"
            );
            let missing = report(&src).missing.expect("the arms should miss a value");
            assert_eq!(missing.pattern, pattern, "{src}");
            let texts = missing
                .partly
                .as_ref()
                .map(|(_, pattern, payload)| (pattern.as_str(), payload.as_str()));
            assert_eq!(texts, partly, "{src}");
            assert_eq!(missing.guarded, guarded, "{src}");
        }
    }
}
