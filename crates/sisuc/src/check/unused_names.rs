//! Unused names (W0012, W0013, W0014). The checker records each use of an item or a member as an
//! edge from the body that holds it, and each binding of a function that nothing reads as a
//! pending finding. After a check with no error, `unused` walks the edges out from the roots,
//! `main` and each `_` function, method and constant, and reports each item that no live code
//! uses, each field that it does not read, each variant that it does not build, and the pending
//! findings of the live functions.

use rustc_hash::FxHashMap;

use super::extend::holder_of;
use super::{Checker, Declared, Holder};
use crate::ast;
use crate::diagnostic::{Code, Definition, Diagnostic, DiagnosticKind, UsedBy};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir;

/// Whether `d` is a warning that `Checker::unused` gives and `Checker::unjudged` does not: one
/// that judges whether code is live. A program with an error gets none, because a use in code
/// that fails may be missed. A program that `mono` stops after the check gets none either. There
/// a pending variable of a live function gets no warning, where the checker's own error path gives
/// it W0002.
pub(crate) fn is_judged(d: &Diagnostic) -> bool {
    matches!(
        d.code(),
        Code::UnusedItem | Code::UnusedMember | Code::UnusedVariable
    )
}

/// A body that can use items: a function's or a method's, or a constant's value, or the bounds of
/// an `extend` block.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Owner {
    Func(tir::FuncId),
    /// The `extend` block at this place among `Checker.extensions`. It uses the interfaces in the
    /// bounds of its type parameters, once, when it is live.
    Block(usize),
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
    /// An interface: a bound that names it, or a call of a method that meets one of its
    /// requirements, or of the requirement itself.
    Interface(tir::InterfaceId),
}

/// A use at `at`, the call, construction, variant path, field access or constant name, of `to`
/// by `from`.
pub(super) struct Use {
    from: Owner,
    to: Used,
    at: Span,
}

/// A binding of the function `func` that nothing read, a parameter when `param`, named `name`
/// and declared at `at`. `fix` holds each place that binds a `for` or pattern name, which `_`
/// can replace, and is empty for any other. `keyword` is the `mut` of a `let mut` that W0002
/// would warn about: it does when the pass does not judge the binding.
pub(super) struct Pending {
    pub(super) func: tir::FuncId,
    pub(super) param: bool,
    pub(super) name: String,
    pub(super) at: Span,
    pub(super) fix: Vec<Span>,
    pub(super) keyword: Option<Span>,
}

impl Pending {
    /// The W0014 of the binding when its function is judged; otherwise its W0002, if it is a
    /// `let mut` that W0002 warns about.
    fn judged(self, judged: bool) -> Option<Diagnostic> {
        let (name, at) = (self.name, self.at);
        if judged {
            let (param, fix) = (self.param, self.fix);
            return Some(
                DiagnosticKind::UnusedVariable {
                    param,
                    name,
                    at,
                    fix,
                }
                .into(),
            );
        }
        let keyword = self.keyword?;
        Some(DiagnosticKind::NeverReassigned { name, at, keyword }.into())
    }
}

impl Used {
    /// The owner that this use names: a function, a method or a constant.
    fn owner(self) -> Option<Owner> {
        match self {
            Used::Func(f) => Some(Owner::Func(f)),
            Used::Const(c) => Some(Owner::Const(c)),
            Used::Class(_) | Used::Variant(..) | Used::Field(..) | Used::Interface(_) => None,
        }
    }
}

/// Marks the interface `i` as used by live code, and queues it to be followed once.
fn use_interface(used: &mut [bool], queue: &mut Vec<usize>, i: usize) {
    if !std::mem::replace(&mut used[i], true) {
        queue.push(i);
    }
}

/// Marks the owner `o` as live, and queues it to be followed once.
fn make_live(live: &mut [bool], stack: &mut Vec<usize>, o: usize) {
    if !std::mem::replace(&mut live[o], true) {
        stack.push(o);
    }
}

/// What the walk from the roots reaches.
struct Reached {
    /// Each owner that live code uses, by its index (`Checker::owner_index`).
    live: Vec<bool>,
    /// Each class that live code builds.
    built: Vec<bool>,
    /// Each enum one of whose variants live code builds.
    enum_built: Vec<bool>,
    /// Each variant that live code builds, by enum.
    variants: Vec<Vec<bool>>,
    /// Each field that live code reads, by class.
    read: Vec<Vec<bool>>,
    /// Each interface that live code uses (see `reach`).
    interfaces: Vec<bool>,
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
    /// The first slot of the interfaces.
    interfaces: usize,
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

    /// Forgets the read of each field in the chain of fields and indexes `place` names, which the
    /// check of `place` as a value recorded. A write names a field without reading it, and so
    /// does a `push`. The reads sit at the end of the uses, so the search stops past the chain.
    pub(super) fn forget_field_reads(&mut self, mut place: &ast::Expr) {
        loop {
            match &place.kind {
                ast::ExprKind::Field { base, .. } => {
                    let at = place.span;
                    let found = self
                        .uses
                        .iter()
                        .rev()
                        .take_while(|u| u.at.start() >= at.start())
                        .position(|u| u.at == at && matches!(u.to, Used::Field(..)));
                    if let Some(back) = found {
                        self.uses.remove(self.uses.len() - 1 - back);
                    }
                    place = base;
                }
                ast::ExprKind::Index { base, .. } => place = base,
                _ => return,
            }
        }
    }

    /// Records a use of each interface in the bounds of the `extend` block `block`, by the block:
    /// one use for each bound, however many methods the block has.
    pub(super) fn record_block_bounds(&mut self, block: usize) {
        let bounds = self.extensions[block].bounds.iter().flatten();
        let from = Owner::Block(block);
        let uses = bounds.map(|b| Use {
            from,
            to: Used::Interface(b.interface),
            at: b.at,
        });
        self.uses.extend(uses);
    }

    /// The owner's index among the owners: its `FuncId`; for a block, the number of functions plus
    /// its place; for a constant, the number of functions and blocks plus its order.
    fn owner_index(&self, owner: Owner) -> usize {
        let functions = self.signatures.len();
        match owner {
            Owner::Func(f) => f.0,
            Owner::Block(b) => functions + b,
            Owner::Const(c) => functions + self.extensions.len() + c,
        }
    }

    /// The W0012, W0013 and W0014 warnings, in no order. W0012 is for each function, method,
    /// class, enum and constant that no live code uses. W0013 is for each field of a built class
    /// that live code does not read. It is also for each variant of a built enum that live code
    /// does not build. A `_` name gets no warning. A method of a class or an enum that nothing
    /// builds is not reported: the class or enum is. An `extend` block is live when one of its
    /// methods is live, or when it conforms a type that live code builds. A dead block of a type
    /// that live code builds is reported, and its methods are not. Each cites up to three of its
    /// uses in dead code. Each pending finding of a live function is W0014. A pending finding of a
    /// dead function gives only the W0002 that it holds, if any. The function's warning tells you
    /// to delete all of it.
    #[expect(
        clippy::too_many_lines,
        reason = "one report loop for each kind of item, so the warnings read in one place"
    )]
    pub(super) fn unused(&mut self) -> Vec<Diagnostic> {
        let pending = std::mem::take(&mut self.pending);
        let functions = self.signatures.len();
        let constants = self.constant_names();
        let reached = self.reach(&constants);
        let holders = self.holders();
        let function_kind = |f: usize| match holders[f] {
            Some(_) => Definition::Method,
            None => Definition::Function,
        };
        let slots = self.slots(functions + self.extensions.len() + constants.len());
        let mut cited = self.cited(&reached, &slots);
        let built = |holder: Holder| match holder {
            Holder::Class(c) => reached.built[c.0],
            Holder::Enum(e) => reached.enum_built[e.0],
            Holder::Array | Holder::I64 => true,
        };
        let named = |o: Owner| match o {
            Owner::Func(f) => (
                UsedBy::Item(function_kind(f.0)),
                self.text(self.signatures[f.0].span),
            ),
            Owner::Block(b) => {
                let block = &self.extensions[b];
                let built = built(block.holder);
                (UsedBy::Block { built }, self.text(block.written))
            }
            Owner::Const(c) => (
                UsedBy::Item(Definition::Constant),
                self.text(
                    constants[c].expect("a constant with a use should be the first of its name"),
                ),
            ),
        };
        let describe = |sites: &[usize]| -> Vec<(Span, UsedBy, String)> {
            sites
                .iter()
                .map(|&i| {
                    let u = &self.uses[i];
                    let (by, name) = named(u.from);
                    (u.at, by, name.to_string())
                })
                .collect()
        };
        let mut found = Vec::new();
        let mut report = |kind: Definition, at: Span, item: usize| {
            // The prelude is out of the user's sight, so nothing in it is unused.
            if at.in_prelude() {
                return;
            }
            let name = self.text(at);
            if name.starts_with('_') {
                return;
            }
            let uses = describe(&std::mem::take(&mut cited[item]));
            let name = name.to_string();
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
            let held = holders[f].is_none_or(built);
            // A dead block is reported itself, or its type is.
            let dead_block = (self.signatures[f].extension)
                .is_some_and(|b| !reached.live[self.owner_index(Owner::Block(b))]);
            if held && !dead_block {
                report(function_kind(f), self.signatures[f].span, f);
            }
        }
        for (c, class) in self.classes.iter().enumerate() {
            if reached.built[c] {
                for (f, field) in class.fields.iter().enumerate() {
                    if !reached.read[c][f] {
                        report(Definition::Field, field.name.span, slots.fields[c] + f);
                    }
                }
            } else {
                report(Definition::Class, class.name.span, slots.classes + c);
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
        for (i, info) in self.interfaces.iter().enumerate() {
            if !reached.interfaces[i] {
                report(Definition::Interface, info.name.span, slots.interfaces + i);
            }
        }
        for (c, span) in constants.iter().enumerate() {
            let owner = self.owner_index(Owner::Const(c));
            if let Some(span) = *span
                && !reached.live[owner]
            {
                report(Definition::Constant, span, owner);
            }
        }
        for (b, block) in self.extensions.iter().enumerate() {
            let owner = self.owner_index(Owner::Block(b));
            if !reached.live[owner] && built(block.holder) && !block.header.in_prelude() {
                let name = self.text(block.written).to_string();
                let at = block.header;
                let uses = describe(&std::mem::take(&mut cited[owner]));
                found.push(DiagnosticKind::UnusedBlock { name, at, uses }.into());
            }
        }
        found.extend(pending.into_iter().filter_map(|p| {
            let live = reached.live[p.func.0];
            p.judged(live)
        }));
        found
    }

    /// The W0002 of each pending finding that holds one, for a program that the pass does not
    /// judge.
    pub(super) fn unjudged(&mut self) -> Vec<Diagnostic> {
        let pending = std::mem::take(&mut self.pending);
        pending
            .into_iter()
            .filter_map(|p| p.judged(false))
            .collect()
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
        let interfaces = next(self.interfaces.len());
        Slots {
            classes,
            enums,
            fields,
            variants,
            interfaces,
            len,
        }
    }

    /// By slot (owners first, by `Checker::owner_index`), the first three uses in dead code of each
    /// item or member, except an owner's own uses, in source order. A field's uses are its reads.
    /// An enum's uses are those of its variants.
    fn cited(&self, reached: &Reached, slots: &Slots) -> Vec<Vec<usize>> {
        let mut cited = vec![Vec::new(); slots.len];
        for (i, u) in self.uses.iter().enumerate() {
            if reached.live[self.owner_index(u.from)] || u.to.owner() == Some(u.from) {
                continue;
            }
            match u.to {
                Used::Class(c) => self.cite(&mut cited[slots.classes + c.0], i),
                Used::Variant(e, v) => {
                    self.cite(&mut cited[slots.enums + e.0], i);
                    self.cite(&mut cited[slots.variants[e.0] + v], i);
                }
                Used::Field(c, f) => self.cite(&mut cited[slots.fields[c.0] + f], i),
                Used::Interface(interface) => {
                    self.cite(&mut cited[slots.interfaces + interface.0], i);
                }
                // An owner is cited next.
                Used::Func(_) | Used::Const(_) => {}
            }
            if let Some(owner) = u.to.owner() {
                self.cite(&mut cited[self.owner_index(owner)], i);
            }
            // A call of a method of a block is a use of the block, unless a method of the block
            // makes it.
            if let Used::Func(f) = u.to
                && let Some(block) = self.signatures[f.0].extension
                && !self.in_block(u.from, block)
            {
                self.cite(&mut cited[self.owner_index(Owner::Block(block))], i);
            }
        }
        cited
    }

    /// Whether `owner` is a method of the `extend` block `block`.
    fn in_block(&self, owner: Owner, block: usize) -> bool {
        matches!(owner, Owner::Func(g) if self.signatures[g.0].extension == Some(block))
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
    /// name starts with `_`. Owners are indexed by `Checker::owner_index`.
    fn reach(&self, constants: &[Option<Span>]) -> Reached {
        let functions = self.signatures.len();
        let blocks = self.extensions.len();
        let owners = functions + blocks + constants.len();
        let mut edges = vec![Vec::new(); owners];
        for (i, u) in self.uses.iter().enumerate() {
            edges[self.owner_index(u.from)].push(i);
        }
        let main = match self.function_ids.get(&Symbol::intern("main")) {
            Some(&Declared::Known(id)) => Some(id.0),
            _ => None,
        };
        let names = (0..functions).map(|f| Some(self.signatures[f].span));
        // A block has no name, so nothing keeps it live.
        let names = names.chain(std::iter::repeat_n(None, blocks));
        let names = names.chain(constants.iter().copied());
        let mut live = vec![false; owners];
        let mut stack: Vec<usize> = (names.enumerate())
            .filter(|&(o, name)| {
                Some(o) == main
                    || name
                        .is_some_and(|name| !name.in_prelude() && self.text(name).starts_with('_'))
            })
            .map(|(o, _)| o)
            .inspect(|&o| live[o] = true)
            .collect();
        let mut built = vec![false; self.classes.len()];
        let mut variants: Vec<Vec<bool>> = (self.enums.iter())
            .map(|e| vec![false; e.variants.len()])
            .collect();
        let mut read: Vec<Vec<bool>> = (self.classes.iter())
            .map(|c| vec![false; c.fields.len()])
            .collect();
        let conforming = self.conformance_methods();
        let conforming_blocks = self.conformance_blocks();
        let mut enum_started = vec![false; self.enums.len()];
        // The interfaces live code uses, and those not yet followed to the ones they name.
        let mut interfaces = vec![false; self.interfaces.len()];
        let mut to_follow = Vec::new();
        while let Some(o) = stack.pop() {
            // A live method makes its `extend` block live: the block names the interfaces in its
            // bounds.
            if o < functions
                && let Some(b) = self.signatures[o].extension
            {
                make_live(&mut live, &mut stack, self.owner_index(Owner::Block(b)));
            }
            for &i in &edges[o] {
                let to = self.uses[i].to;
                let started = match to {
                    Used::Class(c) => {
                        Some((Holder::Class(c), !std::mem::replace(&mut built[c.0], true)))
                    }
                    Used::Variant(e, _) => Some((
                        Holder::Enum(e),
                        !std::mem::replace(&mut enum_started[e.0], true),
                    )),
                    _ => None,
                };
                if let Some((holder, true)) = started {
                    for &method in conforming.get(&holder).into_iter().flatten() {
                        make_live(&mut live, &mut stack, method);
                    }
                    // The blocks that conform it are live, as their methods are.
                    for &b in conforming_blocks.get(&holder).into_iter().flatten() {
                        make_live(&mut live, &mut stack, self.owner_index(Owner::Block(b)));
                    }
                    // A class or an enum that is built names the interfaces of its bounds.
                    let bounds = match holder {
                        Holder::Class(c) => &self.classes[c.0].bounds,
                        Holder::Enum(e) => &self.enums[e.0].bounds,
                        Holder::Array | Holder::I64 => {
                            unreachable!("only a class or an enum is started by a use")
                        }
                    };
                    let bounds = bounds.iter().flatten().map(|bound| bound.interface.0);
                    for interface in bounds {
                        use_interface(&mut interfaces, &mut to_follow, interface);
                    }
                }
                match to {
                    Used::Variant(e, v) => variants[e.0][v] = true,
                    Used::Field(c, f) => read[c.0][f] = true,
                    Used::Interface(interface) => {
                        use_interface(&mut interfaces, &mut to_follow, interface.0);
                    }
                    // A class is built above, and an owner is reached next.
                    Used::Class(_) | Used::Func(_) | Used::Const(_) => {}
                }
                if let Some(owner) = to.owner() {
                    make_live(&mut live, &mut stack, self.owner_index(owner));
                }
            }
        }
        self.follow_interfaces(&mut interfaces, to_follow);
        Reached {
            live,
            built,
            enum_built: variants.iter().map(|v| v.contains(&true)).collect(),
            variants,
            read,
            interfaces,
        }
    }

    /// The methods of the conformances of each class and enum. An interface call names a
    /// requirement, so no use reaches them: they are live when the type is built, as rustc
    /// reports no method of a trait impl of a type it builds.
    fn conformance_methods(&self) -> FxHashMap<Holder, Vec<usize>> {
        let mut conforming: FxHashMap<Holder, Vec<usize>> = FxHashMap::default();
        for (_, conformance) in self.conformances.iter() {
            let holder = holder_of(conformance.target)
                .expect("the target of a conformance should be a class or an enum");
            (conforming.entry(holder).or_default()).extend(conformance.methods.iter().map(|f| f.0));
        }
        conforming
    }

    /// The `extend` blocks of each class and enum that conform it, by their places. Such a block
    /// is live when the type is built, like the methods of its conformances: no call names it.
    fn conformance_blocks(&self) -> FxHashMap<Holder, Vec<usize>> {
        let mut conforming: FxHashMap<Holder, Vec<usize>> = FxHashMap::default();
        let blocks = self.extensions.iter().enumerate();
        for (b, block) in blocks.filter(|(_, b)| b.interface.is_some()) {
            conforming.entry(block.holder).or_default().push(b);
        }
        conforming
    }

    /// Marks as used the interfaces that the used ones in `to_follow` name: those they inherit,
    /// those in the bounds of their requirements and of their associated types and those in the
    /// bounds of their own type parameters.
    fn follow_interfaces(&self, used: &mut [bool], mut to_follow: Vec<usize>) {
        while let Some(i) = to_follow.pop() {
            let info = &self.interfaces[i];
            let inherited = info.inherits.iter();
            let requirements = (info.requirements.iter()).flat_map(|r| r.bounds.iter().flatten());
            // `Self`'s bound, first, is the interface itself.
            let own = info.bounds.iter().skip(1).flatten();
            let assoc = info.assoc.iter().flat_map(|a| a.bounds.iter());
            for bound in inherited.chain(requirements).chain(own).chain(assoc) {
                use_interface(used, &mut to_follow, bound.interface.0);
            }
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
        for ext in &self.extensions {
            for f in ext.methods.values() {
                holders[f.0] = Some(ext.holder);
            }
        }
        holders
    }

    /// The source text at `span`.
    fn text(&self, span: Span) -> &str {
        self.source.file.text(span)
    }
}
