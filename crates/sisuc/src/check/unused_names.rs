//! Unused names (W0012, W0013, W0014). The checker records each use of an item or a member as an
//! edge from the body that holds it, and each binding of a function that nothing reads as a
//! pending finding. After a check with no error, `unused` walks the edges out from the roots,
//! `main` and each `_` function, method and constant, and reports each item that no live code
//! uses, each field that it does not read, each variant that it does not build, and the pending
//! findings of the live functions.

use std::cell::OnceCell;

use rustc_hash::{FxHashMap, FxHashSet};

use super::cycle::inline_of;
use super::extend::holder_of;
use super::{Checker, Declared, Holder, Source};
use crate::conformance::{applies_written, has_written};
use crate::diagnostic::{
    Code, Definition, Diagnostic, DiagnosticKind, Immutable, NeverReassignedForm, UsedBy, copy_help,
};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir;
use crate::tir::{Type, TypeKind, head_of};
use crate::types::Head;

#[cfg(test)]
thread_local! {
    /// How many types `shown_by_dbg` has read on this thread.
    pub(super) static SHOWN_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many bytes of initializer text `quoted` has read on this thread.
    pub(super) static COPY_TEXT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Whether `d` is a warning that `Checker::unused` gives and `Checker::unjudged` does not: one
/// that judges whether code is live. A program with an error gets none, because a use in code
/// that fails may be missed. A program that `mono` stops after the check gets none either. There
/// a pending variable of a live function gets no warning, where the checker's own error path gives
/// it W0002.
pub(crate) fn is_judged(d: &Diagnostic) -> bool {
    matches!(
        d.code(),
        Code::UnusedItem | Code::UnusedMember | Code::UnusedVariable | Code::WriteToCopy
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
    /// The constant at this place among the constants (`ConstantId`).
    Const(usize),
}

/// What a use names.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Used {
    Func(tir::FuncId),
    Class(tir::ClassId),
    Struct(tir::StructId),
    /// A variant, by its index in its enum.
    Variant(tir::EnumId, usize),
    /// A field read, by the field's index in its class.
    Field(tir::ClassId, usize),
    /// A field read, by the field's index in its struct.
    StructField(tir::StructId, usize),
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

/// A `let mut` whose initializer is a place that crosses no class object. It records what W0016
/// needs: the first write to a part of the local, and whether any other use writes it. A read
/// of the local is not recorded here: it keeps the binding out of the pass (`Pending`). It holds
/// no text: the spans and the symbol are all that the `let` costs.
pub(super) struct Copied {
    /// The initializer.
    pub(super) init: Span,
    /// The name at the root of the place: `self` for the plain `self` of a struct method.
    pub(super) root: Symbol,
    /// Why the root cannot change; none for a root that can. A root keeps its kind between the
    /// `let` and each write, so this is found once.
    pub(super) cause: Option<Immutable>,
    /// The first write to a part of the local that stays in structs and arrays.
    pub(super) write: Option<Span>,
    /// Whether the initializer is a bare name or `self`: then the help does not repeat the root.
    pub(super) bare: bool,
    /// Whether the root has the name of the copy, so the copy hides it.
    pub(super) hidden: bool,
    /// Whether a use is a write of another form: the whole local, a `mut` argument, or a path
    /// that crosses a class object.
    pub(super) other: bool,
}

/// The text of the initializer at `span`, when it is on one line and holds no tab or carriage
/// return. A line break, a tab, a carriage return or a comment would change the message, so then
/// the message names only the root. A `//` inside a string literal is part of the text.
fn quoted(source: Source<'_>, span: Span) -> Option<&str> {
    let text = source.text(span);
    #[cfg(test)]
    COPY_TEXT.set(COPY_TEXT.get() + text.len());
    (source.one_line(span) && !text.contains(['\t', '\r'])).then_some(text)
}

impl Copied {
    /// The W0016 of the local declared at `copy`. Each use of it must be a write to a part, and
    /// its root must have a help to give.
    fn warning(self, copy: Span, source: Source<'_>) -> Option<DiagnosticKind> {
        let at = self.write.filter(|_| !self.other)?;
        let root = self.root.as_str();
        let place = quoted(source, self.init);
        // The help says that the place cannot change only when the place is more than the root.
        let named = place.filter(|_| !self.bare);
        let help = copy_help(self.cause.as_ref(), root, named, self.hidden)?;
        Some(DiagnosticKind::WriteToCopy {
            place: place.map(str::to_string),
            root: root.to_string(),
            at,
            copy,
            help,
        })
    }
}

/// A binding of the function `func` that nothing read, a parameter when `param`, named `name`
/// and declared at `at`. `fix` holds each place that binds a `for` or pattern name, which `_`
/// can replace, and is empty for any other. `remove` is what the fix of W0002 removes for a
/// `let mut` that W0002 would warn about: it does when the pass does not judge the binding.
/// `copy` is set for a `let mut` that copies a place.
pub(super) struct Pending {
    pub(super) func: tir::FuncId,
    pub(super) param: bool,
    pub(super) name: String,
    pub(super) at: Span,
    pub(super) fix: Vec<Span>,
    pub(super) remove: Option<Span>,
    pub(super) copy: Option<Copied>,
}

impl Pending {
    /// The W0016 or the W0014 of the binding when its function is judged; otherwise its W0002, if
    /// it is a `let mut` that W0002 warns about. A copy that only writes gets W0016, and not
    /// W0014.
    fn judged(self, judged: bool, source: Source<'_>) -> Option<Diagnostic> {
        let (name, at) = (self.name, self.at);
        if judged {
            if let Some(kind) = self.copy.and_then(|copy| copy.warning(at, source)) {
                return Some(kind.into());
            }
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
        let remove = self.remove?;
        let form = NeverReassignedForm::Let { remove };
        Some(DiagnosticKind::NeverReassigned { name, at, form }.into())
    }
}

impl Used {
    /// The owner that this use names: a function, a method or a constant.
    fn owner(self) -> Option<Owner> {
        match self {
            Used::Func(f) => Some(Owner::Func(f)),
            Used::Const(c) => Some(Owner::Const(c)),
            Used::Class(_)
            | Used::Struct(_)
            | Used::Variant(..)
            | Used::Field(..)
            | Used::StructField(..)
            | Used::Interface(_) => None,
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
    /// Each struct that live code builds.
    struct_built: Vec<bool>,
    /// Each field that live code reads by name, by struct.
    struct_read: Vec<Vec<bool>>,
    /// Each interface that live code uses (see `reach`).
    interfaces: Vec<bool>,
}

/// Where each item's and member's list of cited uses is, past the owners: the classes, the
/// enums, the structs, then each class's fields, each enum's variants and each struct's fields.
struct Slots {
    classes: usize,
    enums: usize,
    structs: usize,
    /// The first slot of each class's fields.
    fields: Vec<usize>,
    /// The first slot of each enum's variants.
    variants: Vec<usize>,
    /// The first slot of each struct's fields.
    struct_fields: Vec<usize>,
    /// The first slot of the interfaces.
    interfaces: usize,
    len: usize,
}

/// What live code shows of its classes and structs (see `Checker::shown_types`).
struct Shown {
    /// By class, the first live `dbg` that can show its fields.
    classes: Vec<Option<Span>>,
    /// By struct, the first live `dbg` that can show its fields.
    dbg_structs: Vec<Option<Span>>,
    /// By struct, whether live code reads each of its fields by showing it.
    read: Vec<bool>,
}

/// How a root of the walk of `shown_types` shows its type.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Mode {
    /// `dbg`: it labels a warning and reads nothing.
    Dbg,
    /// A generated `Display`: `print`, an interpolation, `any Display` or a `Display` bound.
    Display,
    /// A generated `Eq`: `==` or an `Eq` bound.
    Eq,
}

/// The rules of the walk of `shown_types` for one interface (`Checker::shown_rules`).
struct Rules {
    /// The types with a written conformance of the interface.
    written: FxHashSet<Head>,
    /// By inline declaration (`inline_of`), which of its arguments the walk shows.
    params: Vec<Vec<bool>>,
}

impl Checker<'_> {
    /// When `of_prelude` holds and `at` lies in the user's file, the user's file reaches an item
    /// of the prelude, which `mono` roots only for such a program: it needs that item in the
    /// program (see `tir::Program::instantiates`).
    pub(super) fn reaches_prelude(&mut self, of_prelude: bool, at: Span) {
        if of_prelude && !at.in_prelude() {
            self.instantiates = true;
        }
    }

    /// Records a use of `to` at `at` by the body being checked, when that is a function's, a
    /// method's or a constant's.
    ///
    /// A use from the user's file of a function, a class or an enum variant of the prelude
    /// needs that item in the program (see `reaches_prelude`).
    pub(super) fn record(&mut self, to: Used, at: Span) {
        let of_prelude = match to {
            Used::Func(func) => func.0 >= self.prelude.funcs,
            Used::Class(class) => class.0 >= self.prelude.classes,
            Used::Variant(id, _) => id.0 >= self.prelude.enums,
            Used::Struct(_)
            | Used::Field(..)
            | Used::StructField(..)
            | Used::Const(_)
            | Used::Interface(_) => false,
        };
        self.reaches_prelude(of_prelude, at);
        if let Some(from) = self.body.owner {
            self.uses.push(Use { from, to, at });
        } else if let (Some(anys), Used::Interface(interface)) = (&mut self.signature_anys, to) {
            anys.push((interface, at));
        }
    }

    /// Records a use of the interface of each `any` type in `ty`, written at `at`, by the body
    /// being checked.
    pub(super) fn record_anys(&mut self, ty: Type, at: Span) {
        for interface in ty.anys() {
            self.record(Used::Interface(interface), at);
        }
    }

    /// Records a use of each interface in the bounds of the `extend` block `block`, by the block:
    /// one use for each bound, however many methods the block has.
    pub(super) fn record_block_bounds(&mut self, block: usize) {
        let bounds = self.extensions[block].bounds.iter().flat_map(|b| &b.list);
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
    /// class, enum, struct and constant that no live code uses. W0013 is for each field of a built
    /// class or struct that live code does not read. It is also for each variant of a built enum
    /// that live code does not build. A `_` name gets no warning. A method of a class, an enum or
    /// a struct that nothing builds is not reported: the class, enum or struct is. An `extend`
    /// block is live when one of its methods is live, or when it conforms a type that live code
    /// builds. A dead block of a type that live code builds is reported, and its methods are not.
    /// Each cites up to three of its uses in dead code. Each pending finding of a live function
    /// is W0014. A pending finding of a dead function gives only the W0002 that it holds, if any.
    /// The function's warning tells you to delete all of it. An alias is not judged, and the
    /// interface that its body names as `S.Name` counts as used, also when the alias is dead.
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
            Holder::Struct(s) => reached.struct_built[s.0],
            Holder::Array | Holder::I64 => true,
        };
        let named = |o: Owner| match o {
            Owner::Func(f) => (
                UsedBy::Item(function_kind(f.0)),
                self.text(self.signatures[f.0].span).into(),
            ),
            Owner::Block(b) => {
                let block = &self.extensions[b];
                let built = built(block.holder);
                (
                    UsedBy::Block { built },
                    self.source.type_text(block.written),
                )
            }
            Owner::Const(c) => (
                UsedBy::Item(Definition::Constant),
                self.text(
                    constants[c].expect("a constant with a use should be the first of its name"),
                )
                .into(),
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
        let mut report = |kind: Definition, at: Span, item: usize, shown: Option<Span>| {
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
                    shown,
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
                report(function_kind(f), self.signatures[f].span, f, None);
            }
        }
        let mut shown = None;
        for (c, class) in self.classes.iter().enumerate() {
            if reached.built[c] {
                for (f, field) in class.fields.iter().enumerate() {
                    if !reached.read[c][f] {
                        let shown = shown.get_or_insert_with(|| self.shown_types(&reached.live));
                        let dbg = shown.classes[c];
                        report(Definition::Field, field.name.span, slots.fields[c] + f, dbg);
                    }
                }
            } else {
                report(Definition::Class, class.name.span, slots.classes + c, None);
            }
        }
        for (e, info) in self.enums.iter().enumerate() {
            if !reached.enum_built[e] {
                report(Definition::Enum, info.name.span, slots.enums + e, None);
                continue;
            }
            for (v, variant) in info.variants.iter().enumerate() {
                if !reached.variants[e][v] {
                    report(
                        Definition::Variant,
                        variant.name.span,
                        slots.variants[e] + v,
                        None,
                    );
                }
            }
        }
        for (s, info) in self.structs.iter().enumerate() {
            if !reached.struct_built[s] {
                report(Definition::Struct, info.name.span, slots.structs + s, None);
                continue;
            }
            for (f, field) in info.fields.iter().enumerate() {
                if reached.struct_read[s][f] {
                    continue;
                }
                let shown = shown.get_or_insert_with(|| self.shown_types(&reached.live));
                if !shown.read[s] {
                    let dbg = shown.dbg_structs[s];
                    report(
                        Definition::Field,
                        field.name.span,
                        slots.struct_fields[s] + f,
                        dbg,
                    );
                }
            }
        }
        for (i, info) in self.interfaces.iter().enumerate() {
            if !reached.interfaces[i] {
                report(
                    Definition::Interface,
                    info.name.span,
                    slots.interfaces + i,
                    None,
                );
            }
        }
        for (c, span) in constants.iter().enumerate() {
            let owner = self.owner_index(Owner::Const(c));
            if let Some(span) = *span
                && !reached.live[owner]
            {
                report(Definition::Constant, span, owner, None);
            }
        }
        for (b, block) in self.extensions.iter().enumerate() {
            let owner = self.owner_index(Owner::Block(b));
            if !reached.live[owner] && built(block.holder) && !block.header.in_prelude() {
                let name = self.source.type_text(block.written).into_owned();
                let at = block.header;
                let uses = describe(&std::mem::take(&mut cited[owner]));
                found.push(DiagnosticKind::UnusedBlock { name, at, uses }.into());
            }
        }
        found.extend(pending.into_iter().filter_map(|p| {
            let live = reached.live[p.func.0];
            p.judged(live, self.source)
        }));
        found
    }

    /// What live code shows of its classes and structs, without a read of a field. The walk reads
    /// types, not values: an empty array or a `None` shows no field.
    ///
    /// `dbg` shows the fields of a class or a struct. This holds when its operand is such a type,
    /// or holds one in an array, an optional, an enum payload or a struct. That is not a read, so
    /// the result holds, by class and by struct, the first live `dbg` that can show it. A class is
    /// written as `print` writes it, so the walk stops at a class.
    ///
    /// These read each field of each struct that their operand holds, in the same way:
    /// - a generated `print`, an interpolation, or a generated `==`;
    /// - a call of the generated `to_string` or `eq`;
    /// - a conversion to an `any` type;
    /// - a type argument of a call.
    ///
    /// The result holds, by struct, whether live code does one of these. A `Display` root and an
    /// `Eq` root are two walks. A written conformance of one interface does not stop the other.
    ///
    /// `dbg` skips a type with any written `Display` of its head, as `Checker::dbg_show` does. A
    /// read mode skips a type only when a written conformance of the interface applies to that
    /// instance. At each such stop, `dbg` included, every type argument becomes a read root of
    /// both modes. The written body can show or compare it with either interface.
    ///
    /// The walk follows the structure of each operand type. It reads the payloads of each enum and
    /// the fields of each struct once, as declared. It reads an argument of an enum or a struct
    /// only when `shown_params_with` says that it shows it. For `dbg` a head with a written
    /// `Display` shows none. So the cost is linear in the operand types and the declarations.
    ///
    /// shortcut: a `dbg` of a type parameter shows the instances that `mono` makes. The checker
    /// does not see them, so such a field gets no label. A class, an enum or a struct with a
    /// parameter and any written `Display` is skipped too: its text may apply to the instance that
    /// `dbg` prints, so a field behind it gets no label. A type that holds no type parameter, such
    /// as `Gate<string>`, is skipped too when it has a written `Display`. This holds also when a
    /// bound of that `Display` fails. Then `dbg` prints its fields, but they get no label:
    /// `extend<T: Mark> Gate<T>: Display` and `dbg(Gate(v: "one"))`. The same stop queues each
    /// type argument as a read root. So with `dbg(Gate(v: P(n: 1)))` the field `P.n` gets no
    /// W0013. This miss is allowed, because a false W0013 is worse.
    /// To give the label, check the bounds, as `record_holds` in `conformance.rs` does, when a
    /// program needs it.
    ///
    /// A false W0013 is worse than a missed one, so the roots cover more than they must. Each type
    /// argument of a call is a root in both modes, whatever its bound: the body of the callee may
    /// show it. So is the source type of each conversion to an `any` type of any interface. The
    /// walk does not follow which type parameters a generic body shows.
    ///
    /// A `Display` or `Eq` root stops only at a type that a written conformance always covers
    /// (`applies_written`): `extend B<i64>: Display` stops `B<i64>` and not `B<P>`. Each type
    /// argument of a type that it stops becomes a root of both modes, because the written body can
    /// show them.
    fn shown_types(&self, live: &[bool]) -> Shown {
        let is_live = |owner: Option<Owner>| owner.is_some_and(|o| live[self.owner_index(o)]);
        let mut dbgs: Vec<(Span, Type)> = (self.dbg_shown.iter())
            .filter(|&&(owner, ..)| is_live(owner))
            .map(|&(_, ty, span)| (span, ty))
            .collect();
        dbgs.sort_unstable_by_key(|(span, _)| span.start());
        let reads = (self.read_shown.iter()).filter(|&&(owner, ..)| is_live(owner));
        let mut roots: Vec<(Mode, Option<Span>, Type)> = dbgs
            .into_iter()
            .map(|(span, ty)| (Mode::Dbg, Some(span), ty))
            .collect();
        roots.extend(reads.map(|&(_, ty, interface)| {
            let mode = if interface == self.lang.eq {
                Mode::Eq
            } else {
                Mode::Display
            };
            (mode, None, ty)
        }));
        let mut shown = Shown {
            classes: vec![None; self.classes.len()],
            dbg_structs: vec![None; self.structs.len()],
            read: vec![false; self.structs.len()],
        };
        if roots.is_empty() {
            return shown;
        }
        let inline = self.enums.len();
        let (dbg, display, eq) = (OnceCell::new(), OnceCell::new(), OnceCell::new());
        let (mut seen, mut declared) = (FxHashSet::default(), FxHashSet::default());
        let mut next = 0;
        while let Some(&(mode, call, root)) = roots.get(next) {
            next += 1;
            let (interface, rules) = match mode {
                Mode::Dbg => (
                    self.lang.display,
                    dbg.get_or_init(|| self.shown_rules(self.lang.display, true)),
                ),
                Mode::Display => (
                    self.lang.display,
                    display.get_or_init(|| self.shown_rules(self.lang.display, false)),
                ),
                Mode::Eq => (
                    self.lang.eq,
                    eq.get_or_init(|| self.shown_rules(self.lang.eq, false)),
                ),
            };
            let mut walk = vec![root];
            let mut later = Vec::new();
            while let Some(ty) = walk.pop() {
                if !seen.insert((ty, mode)) {
                    continue;
                }
                if self.shown_stops(mode, interface, rules, ty) {
                    // The written body may show or compare the type arguments, with either
                    // interface, so each is a read root of both modes.
                    if let Some((_, args)) = inline_of(ty, inline) {
                        for &a in args.get() {
                            later.extend([(Mode::Display, None, a), (Mode::Eq, None, a)]);
                        }
                    }
                    continue;
                }
                #[cfg(test)]
                SHOWN_VISITS.set(SHOWN_VISITS.get() + 1);
                match ty.kind() {
                    // A class shows its name, and `dbg` shows its fields.
                    TypeKind::Class(class, _) => {
                        if let Some(call) = call {
                            shown.classes[class.0].get_or_insert(call);
                        }
                    }
                    TypeKind::Array(part) | TypeKind::Optional(part) => walk.push(part),
                    TypeKind::Enum(..) | TypeKind::Struct(..) => {
                        if let TypeKind::Struct(id, _) = ty.kind() {
                            match call {
                                Some(call) => {
                                    shown.dbg_structs[id.0].get_or_insert(call);
                                }
                                None => shown.read[id.0] = true,
                            }
                        }
                        let (v, args) = inline_of(ty, inline)
                            .expect("an enum or a struct should be an inline declaration");
                        let on = rules.params[v].iter();
                        let args = args.get().iter().zip(on);
                        walk.extend(args.filter(|(_, on)| **on).map(|(&a, _)| a));
                        if declared.insert((v, mode)) {
                            #[cfg(test)]
                            let before = walk.len();
                            walk.extend(self.inline_parts(v).filter_map(|(ty, ..)| ty));
                            #[cfg(test)]
                            SHOWN_VISITS.set(SHOWN_VISITS.get() + walk.len() - before);
                        }
                    }
                    _ => {}
                }
            }
            roots.extend(later);
        }
        shown
    }

    /// Whether the walk of `shown_types` stops at `ty`, which a written conformance of `interface`
    /// shows. For `dbg` any written conformance of the head counts. In a read mode the
    /// conformance must apply to the instance (`applies_written`).
    fn shown_stops(
        &self,
        mode: Mode,
        interface: tir::InterfaceId,
        rules: &Rules,
        ty: Type,
    ) -> bool {
        if mode != Mode::Dbg {
            applies_written(&self.conformances, interface, self.lang.display, ty)
        } else if ty.has_param() {
            head_of(ty).is_some_and(|head| rules.written.contains(&head))
        } else {
            has_written(&self.conformances, interface, ty)
        }
    }

    /// The rules of the walk of `shown_types` for `interface`. They name, by declaration, the
    /// arguments that it shows. With `by_head`, a declaration with any written conformance of the
    /// head hides its arguments, and `written` names those heads. This is the rule of `dbg`. A
    /// read mode hides none, because it walks the arguments of a type that it skips.
    fn shown_rules(&self, interface: tir::InterfaceId, by_head: bool) -> Rules {
        let written: FxHashSet<Head> = (self.conformances.iter())
            .filter(|&(i, _)| by_head && i == interface)
            .filter_map(|(_, c)| head_of(c.target))
            .collect();
        let hidden: Vec<bool> = if by_head {
            (0..self.enums.len())
                .map(|e| written.contains(&Head::Enum(tir::EnumId(e))))
                .chain(
                    (0..self.structs.len())
                        .map(|s| written.contains(&Head::Struct(tir::StructId(s)))),
                )
                .collect()
        } else {
            vec![false; self.inline_count()]
        };
        Rules {
            params: self.shown_params_with(&hidden),
            written,
        }
    }

    /// Records that generated code shows a value of type `ty` through `interface`, in the body
    /// being checked. Only `Display` and `Eq` count (see `shown_types`).
    pub(super) fn record_shown(&mut self, ty: Type, interface: tir::InterfaceId) {
        let counts = interface == self.lang.display || interface == self.lang.eq;
        if counts && self.body.owner.is_some() {
            self.read_shown.push((self.body.owner, ty, interface));
        }
    }

    /// Records that code of unknown reach, in the body being checked, may show a value of type
    /// `ty` through `Display` or `Eq`. A type that cannot hold a struct is left out.
    pub(super) fn record_both(&mut self, ty: Type) {
        let holds = matches!(
            ty.kind(),
            TypeKind::Array(_) | TypeKind::Optional(_) | TypeKind::Enum(..) | TypeKind::Struct(..)
        );
        if holds {
            self.record_shown(ty, self.lang.display);
            self.record_shown(ty, self.lang.eq);
        }
    }

    /// `record_both` for each type argument of a call: the callee may show them.
    pub(super) fn record_type_args(&mut self, types: tir::TypeArgs) {
        for &ty in types.get() {
            self.record_both(ty);
        }
    }

    /// `record_both` for a value of type `got` that converts to `want`: an `any` type of any
    /// interface, or its optional. A value of any other type is not converted.
    pub(super) fn record_shown_any(&mut self, got: Type, want: Type) {
        let target = match want.kind() {
            TypeKind::Optional(payload) => payload,
            _ => want,
        };
        if matches!(target.kind(), TypeKind::Any(..)) {
            self.record_both(got);
        }
    }

    /// The W0002 of each pending finding that holds one, for a program that the pass does not
    /// judge.
    pub(super) fn unjudged(&mut self) -> Vec<Diagnostic> {
        let pending = std::mem::take(&mut self.pending);
        pending
            .into_iter()
            .filter_map(|p| p.judged(false, self.source))
            .collect()
    }

    /// The slots past the first `owners`.
    fn slots(&self, owners: usize) -> Slots {
        let classes = owners;
        let enums = classes + self.classes.len();
        let structs = enums + self.enums.len();
        let mut len = structs + self.structs.len();
        let mut next = |count: usize| {
            len += count;
            len - count
        };
        let fields = self.classes.iter().map(|c| next(c.fields.len())).collect();
        let variants = self.enums.iter().map(|e| next(e.variants.len())).collect();
        let struct_fields = self.structs.iter().map(|s| next(s.fields.len())).collect();
        let interfaces = next(self.interfaces.len());
        Slots {
            classes,
            enums,
            structs,
            fields,
            variants,
            struct_fields,
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
                Used::Struct(s) => self.cite(&mut cited[slots.structs + s.0], i),
                Used::StructField(s, f) => {
                    self.cite(&mut cited[slots.struct_fields[s.0] + f], i);
                }
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
        for (_, id) in self.names.known_constants() {
            if constants.len() <= id.0 {
                constants.resize(id.0 + 1, None);
            }
            constants[id.0] = Some(self.const_infos[id.0].span);
        }
        constants
    }

    /// What the uses reach from the roots: `main`, and each function, method and constant whose
    /// name starts with `_`. Owners are indexed by `Checker::owner_index`.
    #[expect(
        clippy::too_many_lines,
        reason = "one walk with an arm for each kind of use, so the reach reads in one place"
    )]
    fn reach(&self, constants: &[Option<Span>]) -> Reached {
        let functions = self.signatures.len();
        let blocks = self.extensions.len();
        let owners = functions + blocks + constants.len();
        let mut edges = vec![Vec::new(); owners];
        for (i, u) in self.uses.iter().enumerate() {
            edges[self.owner_index(u.from)].push(i);
        }
        let main = match self.names.function(Symbol::intern("main")) {
            Some(Declared::Known(id)) => Some(id.0),
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
        let chosen = self.chosen_types();
        let mut struct_read: Vec<Vec<bool>> = (self.structs.iter())
            .map(|s| vec![false; s.fields.len()])
            .collect();
        let mut enum_started = vec![false; self.enums.len()];
        let mut struct_started = vec![false; self.structs.len()];
        // The interfaces live code uses, and those not yet followed to the ones they name.
        let mut interfaces = vec![false; self.interfaces.len()];
        let mut to_follow = Vec::new();
        // An alias body that names `S.Name` has no owner, so no use records its interface.
        for interface in &self.aliases.named_interfaces {
            use_interface(&mut interfaces, &mut to_follow, interface.0);
        }
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
                    Used::Struct(s) => Some((
                        Holder::Struct(s),
                        !std::mem::replace(&mut struct_started[s.0], true),
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
                    // A class or an enum that is built names the interfaces it holds.
                    for interface in self.held_interfaces(holder, &chosen) {
                        use_interface(&mut interfaces, &mut to_follow, interface.0);
                    }
                }
                match to {
                    Used::Variant(e, v) => variants[e.0][v] = true,
                    Used::Field(c, f) => read[c.0][f] = true,
                    Used::StructField(s, f) => struct_read[s.0][f] = true,
                    Used::Interface(interface) => {
                        use_interface(&mut interfaces, &mut to_follow, interface.0);
                    }
                    // A class or a struct is built above, and an owner is reached next.
                    Used::Class(_) | Used::Func(_) | Used::Const(_) | Used::Struct(_) => {}
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
            struct_built: struct_started,
            struct_read,
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
            let requirements =
                (info.requirements.iter()).flat_map(|r| r.bounds.iter().flat_map(|b| &b.list));
            // `Self`'s bound, first, is the interface itself.
            let own = info.bounds.iter().skip(1).flat_map(|b| &b.list);
            let assoc = info.assoc.iter().flat_map(|a| &a.bounds.list);
            for bound in inherited.chain(requirements).chain(own).chain(assoc) {
                use_interface(used, &mut to_follow, bound.interface.0);
            }
            // The `any` types in the types of its requirements.
            let written = (info.requirements.iter())
                .flat_map(|r| r.params.iter().copied().chain([r.ret]).flatten());
            for interface in written.flat_map(Type::anys) {
                use_interface(used, &mut to_follow, interface.0);
            }
        }
    }

    /// The associated types that the conformances of each class and enum choose, when one holds an
    /// `any` type.
    fn chosen_types(&self) -> FxHashMap<Holder, Vec<Type>> {
        let mut chosen: FxHashMap<Holder, Vec<Type>> = FxHashMap::default();
        for (_, conformance) in self.conformances.iter() {
            let holder = holder_of(conformance.target)
                .expect("the target of a conformance should be a class or an enum");
            let types = conformance
                .assoc
                .iter()
                .flatten()
                .filter(|ty| ty.holds_any());
            chosen.entry(holder).or_default().extend(types);
        }
        chosen
    }

    /// The interfaces that a class or an enum names: in its bounds, and in the `any` types it
    /// holds, in the type of a field, in the type of a variant's payload, and in an associated
    /// type that `chosen` gives for it.
    fn held_interfaces(
        &self,
        holder: Holder,
        chosen: &FxHashMap<Holder, Vec<Type>>,
    ) -> impl Iterator<Item = tir::InterfaceId> {
        let (bounds, declared): (_, Box<dyn Iterator<Item = Type>>) = match holder {
            Holder::Class(c) => (
                &self.classes[c.0].bounds,
                Box::new((self.classes[c.0].fields.iter()).filter_map(|f| f.ty)),
            ),
            Holder::Enum(e) => (
                &self.enums[e.0].bounds,
                Box::new(
                    (self.enums[e.0].variants.iter())
                        .flat_map(|v| v.fields.iter().flatten().copied()),
                ),
            ),
            Holder::Struct(s) => (
                &self.structs[s.0].bounds,
                Box::new((self.structs[s.0].fields.iter()).filter_map(|f| f.ty)),
            ),
            Holder::Array | Holder::I64 => {
                unreachable!("only a class, a struct or an enum is started by a use")
            }
        };
        let bounds = bounds
            .iter()
            .flat_map(|b| &b.list)
            .map(|bound| bound.interface);
        let chosen = chosen.get(&holder).into_iter().flatten().copied();
        bounds.chain(declared.chain(chosen).flat_map(Type::anys))
    }

    /// The class, enum or struct of each function that is a method, by its `FuncId`.
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
        for (s, info) in self.structs.iter().enumerate() {
            for method in info.methods.values() {
                if let Declared::Known(f) = method {
                    holders[f.0] = Some(Holder::Struct(tir::StructId(s)));
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
        self.source.text(span)
    }
}
