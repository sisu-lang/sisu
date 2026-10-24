//! Unused names (W0012, W0013). The checker records each use of an item or a member as an edge
//! from the body that holds it. After a check with no error, `unused` walks the edges out from
//! the roots, `main` and each `_` function, method and constant, and reports each item that no
//! live code uses, each field that it does not read and each variant that it does not build.

use rustc_hash::FxHashSet;

use super::{Checker, Declared};
use crate::diagnostic::{Definition, Diagnostic, DiagnosticKind};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, Type, TypeKind};

/// A body that can use items: a function's or a method's, or a constant's value.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Owner {
    Func(tir::FuncId),
    /// The constant at this place among the constants (`ConstInfo::order`).
    Const(usize),
}

/// What a use names.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Used {
    Func(tir::FuncId),
    Class(tir::ClassId),
    /// A variant, by its index in its enum.
    Variant(tir::EnumId, usize),
    /// A field read, by the field's index in its class.
    Field(tir::ClassId, usize),
    Const(usize),
    /// An `==`, `!=` or `contains` on values of this type, which reads every field of each class
    /// that the type holds.
    Equal(Type),
}

/// A use at `at`, the call, construction, variant path, field access, constant name, comparison
/// or `contains` call, of `to` by `from`.
pub(super) struct Use {
    from: Owner,
    to: Used,
    at: Span,
}

impl Owner {
    /// The owner's index among the owners: its `FuncId`, or for a constant, the number of
    /// functions plus its order.
    fn index(self, functions: usize) -> usize {
        match self {
            Owner::Func(f) => f.0,
            Owner::Const(c) => functions + c,
        }
    }
}

impl Used {
    /// The owner that this use names: a function, a method or a constant.
    fn owner(self) -> Option<Owner> {
        match self {
            Used::Func(f) => Some(Owner::Func(f)),
            Used::Const(c) => Some(Owner::Const(c)),
            Used::Class(_) | Used::Variant(..) | Used::Field(..) | Used::Equal(_) => None,
        }
    }
}

/// The class or enum of a method.
#[derive(Clone, Copy)]
enum Holder {
    Class(tir::ClassId),
    Enum(tir::EnumId),
}

/// What the walk from the roots reaches.
struct Reached {
    /// Each owner that live code uses, by its index (`Owner::index`).
    live: Vec<bool>,
    /// Each class that live code builds.
    built: Vec<bool>,
    /// Each enum one of whose variants live code builds.
    enum_built: Vec<bool>,
    /// Each variant that live code builds, by enum.
    variants: Vec<Vec<bool>>,
    /// Each field that live code reads, by class.
    read: Vec<Vec<bool>>,
    /// Each class whose fields a comparison in live code reads.
    compared: Vec<bool>,
}

/// Where each item's and member's list of cited uses is, past the owners: the classes, the
/// enums, then each class's fields and each enum's variants.
struct Slots {
    classes: usize,
    enums: usize,
    /// The first slot of each class's fields.
    fields: Vec<usize>,
    /// The first slot of each enum's variants.
    variants: Vec<usize>,
    len: usize,
}

impl Checker<'_> {
    /// Records a use of `to` at `at` by the body being checked, when that is a function's, a
    /// method's or a constant's.
    pub(super) fn record(&mut self, to: Used, at: Span) {
        if let Some(from) = self.body.owner {
            self.uses.push(Use { from, to, at });
        }
    }

    /// The W0012 and W0013 warnings, in no order: each function, method, class, enum and
    /// constant that no live code uses, each field of a built class that it does not read, and
    /// each variant of a built enum that it does not build, except a `_` name. A method of a
    /// class or an enum that nothing builds is not reported: the class or enum is. Each cites up
    /// to three of its uses in dead code.
    pub(super) fn unused(&self) -> Vec<Diagnostic> {
        let functions = self.signatures.len();
        let constants = self.constant_names();
        let reached = self.reach(&constants);
        let holders = self.holders();
        let function_kind = |f: usize| match holders[f] {
            Some(_) => Definition::Method,
            None => Definition::Function,
        };
        let slots = self.slots(functions + constants.len());
        let mut cited = self.cited(&reached, &slots);
        let named = |o: Owner| match o {
            Owner::Func(f) => (function_kind(f.0), self.signatures[f.0].span),
            Owner::Const(c) => (
                Definition::Constant,
                constants[c].expect("a constant with a use should be the first of its name"),
            ),
        };
        let mut found = Vec::new();
        let mut report = |kind: Definition, at: Span, item: usize| {
            let name = self.text(at);
            if name.starts_with('_') {
                return;
            }
            let sites = std::mem::take(&mut cited[item]);
            let sites = sites.iter().map(|&i| {
                let u = &self.uses[i];
                let (kind, name) = named(u.from);
                (u.at, kind, self.text(name).to_string())
            });
            let (name, uses) = (name.to_string(), sites.collect());
            let d = match kind {
                Definition::Field | Definition::Variant => DiagnosticKind::UnusedMember {
                    kind,
                    name,
                    at,
                    uses,
                },
                _ => DiagnosticKind::UnusedItem {
                    kind,
                    name,
                    at,
                    uses,
                },
            };
            found.push(d.into());
        };
        for f in (0..functions).filter(|&f| !reached.live[f]) {
            let held = match holders[f] {
                Some(Holder::Class(c)) => reached.built[c.0],
                Some(Holder::Enum(e)) => reached.enum_built[e.0],
                None => true,
            };
            if held {
                report(function_kind(f), self.signatures[f].span, f);
            }
        }
        for (c, class) in self.classes.iter().enumerate() {
            if !reached.built[c] {
                report(Definition::Class, class.name.span, slots.classes + c);
            } else if !reached.compared[c] {
                for (f, field) in class.fields.iter().enumerate() {
                    if !reached.read[c][f] {
                        report(Definition::Field, field.name.span, slots.fields[c] + f);
                    }
                }
            }
        }
        for (e, info) in self.enums.iter().enumerate() {
            if !reached.enum_built[e] {
                report(Definition::Enum, info.name.span, slots.enums + e);
                continue;
            }
            for (v, variant) in info.variants.iter().enumerate() {
                if !reached.variants[e][v] {
                    report(
                        Definition::Variant,
                        variant.name.span,
                        slots.variants[e] + v,
                    );
                }
            }
        }
        for (c, span) in constants.iter().enumerate() {
            if let Some(span) = *span
                && !reached.live[Owner::Const(c).index(functions)]
            {
                report(Definition::Constant, span, Owner::Const(c).index(functions));
            }
        }
        found
    }

    /// The slots past the first `owners`.
    fn slots(&self, owners: usize) -> Slots {
        let classes = owners;
        let enums = classes + self.classes.len();
        let mut len = enums + self.enums.len();
        let mut next = |count: usize| {
            len += count;
            len - count
        };
        let fields = self.classes.iter().map(|c| next(c.fields.len())).collect();
        let variants = self.enums.iter().map(|e| next(e.variants.len())).collect();
        Slots {
            classes,
            enums,
            fields,
            variants,
            len,
        }
    }

    /// By slot (owners first, by `Owner::index`), the first three uses in dead code of each
    /// item or member, except an owner's own uses, in source order. A field's uses are its reads,
    /// then, while there are fewer than three, the first dead comparison that reads its class.
    /// An enum's uses are those of its variants.
    fn cited(&self, reached: &Reached, slots: &Slots) -> Vec<Vec<usize>> {
        let functions = self.signatures.len();
        let mut cited = vec![Vec::new(); slots.len];
        for (i, u) in self.uses.iter().enumerate() {
            if reached.live[u.from.index(functions)] || u.to.owner() == Some(u.from) {
                continue;
            }
            match u.to {
                Used::Func(f) => self.cite(&mut cited[f.0], i),
                Used::Const(c) => self.cite(&mut cited[Owner::Const(c).index(functions)], i),
                Used::Class(c) => self.cite(&mut cited[slots.classes + c.0], i),
                Used::Variant(e, v) => {
                    self.cite(&mut cited[slots.enums + e.0], i);
                    self.cite(&mut cited[slots.variants[e.0] + v], i);
                }
                Used::Field(c, f) => self.cite(&mut cited[slots.fields[c.0] + f], i),
                // Cited below, once the dead comparisons are walked.
                Used::Equal(_) => {}
            }
        }
        for (c, first) in self.dead_comparisons().into_iter().enumerate() {
            let Some(i) = first else { continue };
            let fields = slots.fields[c]..slots.fields[c] + self.classes[c].fields.len();
            for list in &mut cited[fields] {
                if list.len() < 3 {
                    self.cite(list, i);
                }
            }
        }
        cited
    }

    /// Puts the use `i` into `first`, a list of at most three uses in source order, and keeps
    /// the first three. The list never holds more than four, so this is constant work. A use at
    /// the same place as an earlier one goes after it.
    fn cite(&self, first: &mut Vec<usize>, i: usize) {
        let start = self.uses[i].at.start();
        let at = first.partition_point(|&j| self.uses[j].at.start() <= start);
        first.insert(at, i);
        first.truncate(3);
    }

    /// By class, the first comparison, in source order, that reads the class's fields. For a
    /// class that a field is reported in, no live comparison reads the fields, so that comparison
    /// is in dead code. The walks share one set of types, so the comparisons together cost time
    /// linear in their types.
    fn dead_comparisons(&self) -> Vec<Option<usize>> {
        let dead = self
            .uses
            .iter()
            .enumerate()
            .filter_map(|(i, u)| match u.to {
                Used::Equal(ty) => Some((u.at.start(), (i, ty))),
                _ => None,
            });
        let mut first = vec![None; self.classes.len()];
        let mut seen = FxHashSet::default();
        for (i, ty) in by_start(self.source.text.len(), dead.collect()) {
            self.held_classes(ty, &mut seen, |c| {
                first[c.0].get_or_insert(i);
            });
        }
        first
    }

    /// Calls `found` for each class that a comparison of values of type `ty` reads every field
    /// of: each class that `ty` holds, through arrays, optionals, field types and payload types,
    /// with the type arguments put in. A type in `seen` is not walked again, so each class of it
    /// was found before. An explicit stack, so a long chain of classes cannot overflow the stack.
    fn held_classes(
        &self,
        ty: Type,
        seen: &mut FxHashSet<Type>,
        mut found: impl FnMut(tir::ClassId),
    ) {
        let mut stack = vec![ty];
        while let Some(ty) = stack.pop() {
            if !seen.insert(ty) {
                continue;
            }
            match ty.kind() {
                TypeKind::Array(inner) | TypeKind::Optional(inner) => stack.push(inner),
                TypeKind::Class(c, _) => {
                    found(c);
                    let fields = 0..self.classes[c.0].fields.len();
                    stack.extend(fields.filter_map(|f| self.field_type(ty, f)));
                }
                TypeKind::Enum(e, args) => {
                    for variant in &self.enums[e.0].variants {
                        let payload = variant.fields.iter().flatten();
                        stack.extend(payload.map(|field| field.subst(args.get())));
                    }
                }
                _ => {}
            }
        }
    }

    /// The span of each constant's name, by its order; `None` for one its name does not hold.
    fn constant_names(&self) -> Vec<Option<Span>> {
        let mut constants = Vec::new();
        for info in self.constants.values() {
            if let Declared::Known(info) = info {
                if constants.len() <= info.order {
                    constants.resize(info.order + 1, None);
                }
                constants[info.order] = Some(info.span);
            }
        }
        constants
    }

    /// What the uses reach from the roots: `main`, and each function, method and constant whose
    /// name starts with `_`. Owners are indexed by `Owner::index`.
    fn reach(&self, constants: &[Option<Span>]) -> Reached {
        let functions = self.signatures.len();
        let owners = functions + constants.len();
        let mut edges = vec![Vec::new(); owners];
        for (i, u) in self.uses.iter().enumerate() {
            edges[u.from.index(functions)].push(i);
        }
        let main = match self.function_ids.get(&Symbol::intern("main")) {
            Some(&Declared::Known(id)) => Some(id.0),
            _ => None,
        };
        let names = (0..functions).map(|f| Some(self.signatures[f].span));
        let names = names.chain(constants.iter().copied());
        let mut live = vec![false; owners];
        let mut stack: Vec<usize> = (names.enumerate())
            .filter(|&(o, name)| {
                Some(o) == main || name.is_some_and(|name| self.text(name).starts_with('_'))
            })
            .map(|(o, _)| o)
            .collect();
        for &o in &stack {
            live[o] = true;
        }
        let mut built = vec![false; self.classes.len()];
        let mut variants: Vec<Vec<bool>> = (self.enums.iter())
            .map(|e| vec![false; e.variants.len()])
            .collect();
        let mut read: Vec<Vec<bool>> = (self.classes.iter())
            .map(|c| vec![false; c.fields.len()])
            .collect();
        let mut compared = vec![false; self.classes.len()];
        // One set for every comparison, so each type is walked once.
        let mut seen = FxHashSet::default();
        while let Some(o) = stack.pop() {
            for &i in &edges[o] {
                let reached = match self.uses[i].to {
                    Used::Func(f) => Owner::Func(f),
                    Used::Const(c) => Owner::Const(c),
                    Used::Class(c) => {
                        built[c.0] = true;
                        continue;
                    }
                    Used::Variant(e, v) => {
                        variants[e.0][v] = true;
                        continue;
                    }
                    Used::Field(c, f) => {
                        read[c.0][f] = true;
                        continue;
                    }
                    Used::Equal(ty) => {
                        self.held_classes(ty, &mut seen, |c| compared[c.0] = true);
                        continue;
                    }
                }
                .index(functions);
                if !std::mem::replace(&mut live[reached], true) {
                    stack.push(reached);
                }
            }
        }
        Reached {
            live,
            built,
            enum_built: variants.iter().map(|v| v.contains(&true)).collect(),
            variants,
            read,
            compared,
        }
    }

    /// The class or enum of each function that is a method, by its `FuncId`.
    fn holders(&self) -> Vec<Option<Holder>> {
        let mut holders = vec![None; self.signatures.len()];
        for (c, class) in self.classes.iter().enumerate() {
            for method in class.methods.values() {
                if let Declared::Known(f) = method {
                    holders[f.0] = Some(Holder::Class(tir::ClassId(c)));
                }
            }
        }
        for (e, info) in self.enums.iter().enumerate() {
            for method in info.methods.values() {
                if let Declared::Known(f) = method {
                    holders[f.0] = Some(Holder::Enum(tir::EnumId(e)));
                }
            }
        }
        holders
    }

    /// The source text at `span`.
    fn text(&self, span: Span) -> &str {
        &self.source.text[span.range()]
    }
}

/// The items of `starts`, each with the place where its use starts, ordered by that place, and at
/// one place in their own order: a counting sort, linear in the source's `len` and the items.
fn by_start<T: Copy>(len: usize, starts: Vec<(usize, T)>) -> Vec<T> {
    if starts.is_empty() {
        return Vec::new();
    }
    // First the count of items at each place, then the next index for an item there.
    let mut next = vec![0; len + 1];
    for &(start, _) in &starts {
        next[start] += 1;
    }
    let mut at = 0;
    for n in &mut next {
        let count = *n;
        *n = at;
        at += count;
    }
    let mut sorted = vec![None; starts.len()];
    for (start, item) in starts {
        sorted[next[start]] = Some(item);
        next[start] += 1;
    }
    sorted.into_iter().flatten().collect()
}
