//! Unused names (W0012). The checker records each use of an item as an edge from the body that
//! holds it. After a check with no error, `unused` walks the edges out from the roots, `main` and
//! each `_` function, method and constant, and reports each item that no live code uses.

use super::{Checker, Declared};
use crate::diagnostic::{Definition, Diagnostic, DiagnosticKind};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir;

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
    Const(usize),
}

/// A use at `at`, the call, construction, variant path or constant name, of `to` by `from`.
pub(super) struct Use {
    from: Owner,
    to: Used,
    at: Span,
}

/// The class or enum of a method.
#[derive(Clone, Copy)]
enum Holder {
    Class(tir::ClassId),
    Enum(tir::EnumId),
}

/// What the walk from the roots reaches.
struct Reached {
    /// Each owner that live code uses, by its index (`Checker::reach`).
    live: Vec<bool>,
    /// Each class that live code builds.
    built: Vec<bool>,
    /// Each enum one of whose variants live code builds.
    enum_built: Vec<bool>,
}

impl Checker<'_> {
    /// Records a use of `to` at `at` by the body being checked, when that is a function's, a
    /// method's or a constant's.
    pub(super) fn record(&mut self, to: Used, at: Span) {
        if let Some(from) = self.body.owner {
            self.uses.push(Use { from, to, at });
        }
    }

    /// The W0012 warnings, in no order: each function, method, class, enum and constant that no
    /// live code uses, except a `_` name. A method of a class or an enum that nothing builds is
    /// not reported: the class or enum is. Each cites up to three of its uses in dead code.
    pub(super) fn unused(&self) -> Vec<Diagnostic> {
        let functions = self.signatures.len();
        let constants = self.constant_names();
        let owners = functions + constants.len();
        let owner = |o: Owner| match o {
            Owner::Func(f) => f.0,
            Owner::Const(c) => functions + c,
        };
        let reached = self.reach(&constants);
        let holders = self.holders();
        let function_kind = |f: usize| match holders[f] {
            Some(_) => Definition::Method,
            None => Definition::Function,
        };

        // Each item, by an index past the owners: classes, then enums.
        let item = |to: Used| match to {
            Used::Func(f) => f.0,
            Used::Const(c) => functions + c,
            Used::Class(c) => owners + c.0,
            Used::Variant(e, _) => owners + self.classes.len() + e.0,
        };
        // The uses in dead code of each item, but an item's own.
        let mut cited = vec![Vec::new(); owners + self.classes.len() + self.enums.len()];
        for (i, u) in self.uses.iter().enumerate() {
            let own = match u.to {
                Used::Func(f) => Some(Owner::Func(f)),
                Used::Const(c) => Some(Owner::Const(c)),
                Used::Class(_) | Used::Variant(..) => None,
            };
            if !reached.live[owner(u.from)] && own != Some(u.from) {
                cited[item(u.to)].push(i);
            }
        }
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
            let mut sites = std::mem::take(&mut cited[item]);
            sites.sort_by_key(|&i| self.uses[i].at.start());
            let sites = sites.iter().take(3).map(|&i| {
                let u = &self.uses[i];
                let (kind, name) = named(u.from);
                (u.at, kind, self.text(name).to_string())
            });
            found.push(
                DiagnosticKind::UnusedItem {
                    kind,
                    name: name.to_string(),
                    at,
                    uses: sites.collect(),
                }
                .into(),
            );
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
        let enums = owners + self.classes.len();
        for (c, class) in self.classes.iter().enumerate() {
            if !reached.built[c] {
                report(Definition::Class, class.name.span, owners + c);
            }
        }
        for (e, info) in self.enums.iter().enumerate() {
            if !reached.enum_built[e] {
                report(Definition::Enum, info.name.span, enums + e);
            }
        }
        for (c, span) in constants.iter().enumerate() {
            if let Some(span) = *span
                && !reached.live[functions + c]
            {
                report(Definition::Constant, span, functions + c);
            }
        }
        found
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
    /// name starts with `_`. An owner's index is its `FuncId`, or for a constant, the number of
    /// functions plus its order.
    fn reach(&self, constants: &[Option<Span>]) -> Reached {
        let functions = self.signatures.len();
        let owners = functions + constants.len();
        let mut edges = vec![Vec::new(); owners];
        for (i, u) in self.uses.iter().enumerate() {
            let from = match u.from {
                Owner::Func(f) => f.0,
                Owner::Const(c) => functions + c,
            };
            edges[from].push(i);
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
        while let Some(o) = stack.pop() {
            for &i in &edges[o] {
                let reached = match self.uses[i].to {
                    Used::Func(f) => f.0,
                    Used::Const(c) => functions + c,
                    Used::Class(c) => {
                        built[c.0] = true;
                        continue;
                    }
                    Used::Variant(e, v) => {
                        variants[e.0][v] = true;
                        continue;
                    }
                };
                if !std::mem::replace(&mut live[reached], true) {
                    stack.push(reached);
                }
            }
        }
        Reached {
            live,
            built,
            enum_built: variants.iter().map(|v| v.contains(&true)).collect(),
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
