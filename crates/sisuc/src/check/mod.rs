//! The checker: signatures, scopes, names and types. It lowers the program to `tir` as it
//! checks, and records every error instead of stopping at the first.

mod array;
mod branch;
mod call;
mod constant;
mod enums;
mod exhaust;
mod expr;
mod generic;
mod member;
mod number;
mod operator;
mod pattern;
mod place;
mod prelude;
mod stmt;
mod string;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::ast;
use crate::diagnostic::{
    BuiltIn, Conversion, Definition, Diagnostic, DiagnosticKind, Earlier, MainForm, MismatchFix,
    MismatchSource, Severity, Unbuilt, suggestion,
};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, Type, TypeArgs, TypeKind};
use constant::{ConstInfo, ConstantSite};
use enums::EnumInfo;
use prelude::PRELUDE;

/// An expression whose rule failed; its diagnostic is recorded. `ty` is its type when
/// still known (an `if` with one failed branch takes the other's), else it is the poison
/// type `Error`, which matches every type and draws no further diagnostic.
///
/// Poison spreads to sibling operands by one rule. For a homogeneous operator (`+ - * / %
/// && ||`, comparison chains), once an operand is `Error` the later operands are still
/// checked for their own errors but not against the operator's type. Call and constructor
/// arguments are each checked against their own parameter, so one bad argument does not
/// excuse another.
struct Poisoned {
    ty: Option<Type>,
    /// Whether the expected type passed down was rejected (a `None` that is not optional, or
    /// a block or `if` whose value is one): the rule that passed it failed, so its expression
    /// is `Error` too, where an `Error` operand would excuse it.
    mismatch: bool,
}

impl Poisoned {
    /// `Error`.
    fn error() -> Self {
        Self::of(None)
    }

    /// A failed expression whose type is still known.
    fn typed(ty: Type) -> Self {
        Self::of(Some(ty))
    }

    fn of(ty: Option<Type>) -> Self {
        Self {
            ty,
            mismatch: false,
        }
    }
}

type Checked = Result<tir::Expr, Poisoned>;
type CheckedBlock = Result<tir::Block, Poisoned>;

/// The expected type a position passes down when its own type is `Error`. No position expects
/// `never`, so it stands for `Error` there: a `None` it reaches is `Error`, with no diagnostic.
static UNKNOWN: Type = Type::NEVER;

/// A type of `None` is `Error`: its error is reported where the signature names it.
struct Signature {
    /// `f`, or `P.m` for a method.
    name: String,
    /// The type parameters in scope: a method's owner's, then its own.
    type_params: Vec<Symbol>,
    /// Without a method's `self`.
    params: Vec<Option<Type>>,
    ret: Option<Type>,
    /// The name's span.
    span: Span,
}

/// What a function, class, constant or method name stands for: the first intact item of that
/// name, or a broken one, which holds a syntax error. A broken name wins over an intact one, and
/// every use of it is poisoned without a diagnostic: its item's error is reported already.
#[derive(Clone, Copy)]
enum Declared<T> {
    Known(T),
    Broken,
}

struct ClassInfo {
    name: ast::Ident,
    type_params: Vec<Symbol>,
    /// The first field of each name, in declaration order.
    fields: Vec<FieldInfo>,
    /// The first method of each name, and every broken one.
    methods: FxHashMap<Symbol, Declared<tir::FuncId>>,
}

struct FieldInfo {
    name: ast::Ident,
    /// Where the field's type is written.
    ty_span: Span,
    /// `None` is `Error`.
    ty: Option<Type>,
    mutable: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum BindingKind {
    Let,
    Var,
    Param,
    /// Bound by an `if let` or a `while let`: the keyword.
    Unwrapped(&'static str),
}

struct Binding {
    local: tir::LocalId,
    /// `None` is `Error`.
    ty: Option<Type>,
    kind: BindingKind,
    span: Span,
    /// The `mut` of a `let mut`; `Some` only when `kind` is `Var`, which a repeated parameter
    /// is without one.
    keyword: Option<Span>,
    reassigned: bool,
    /// Whether the binding visible where it was declared, in any scope, is a `let mut`.
    shadows_mut: bool,
}

/// Which part of a loop the code being checked is in.
enum LoopContext {
    /// The body, where `break` and `continue` act on the loop.
    Body(LoopFrame),
    /// A `while` condition or a `for` source, which is not part of the body.
    Header {
        keyword: Span,
        label: Option<ast::Ident>,
    },
}

/// The kind of a loop: only a `loop` gives a value through `break`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LoopKind {
    Loop,
    While,
    WhileLet,
    For,
}

impl LoopKind {
    /// The keyword that starts the loop, as a diagnostic names it.
    fn keyword(self) -> &'static str {
        match self {
            LoopKind::Loop => "loop",
            LoopKind::While => "while",
            LoopKind::WhileLet => "while let",
            LoopKind::For => "for",
        }
    }
}

/// A loop whose body is being checked, and what the `break`s that target it have given.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is a separate fact, set by a different jump or value"
)]
struct LoopFrame {
    kind: LoopKind,
    keyword: Span,
    label: Option<ast::Ident>,
    /// Whether a jump has named `label`.
    label_used: bool,
    /// Whether a `break` has targeted it.
    broken: bool,
    /// The type the position around a `loop` expects, each value's hint; `UNKNOWN` for an
    /// `Error` one.
    expected: Option<Type>,
    /// The type of the first value whose type is neither `never` nor `Error`, and its `break`'s
    /// span; under an expected `T?`, a `T?` value after `T` ones replaces it.
    first: Option<(Type, Span)>,
    /// Whether `first` went from a `T` to a `T?`, so the `T` values before need wrapping.
    widened: bool,
    /// Whether a `break` inside a nested loop has targeted it, so its values lie in one.
    crossed: bool,
    /// The first bare `break`.
    bare: Option<Span>,
    /// The first `break` whose value's type is not `never`.
    valued: Option<Span>,
    /// Whether the loop failed: a failed jump, loop label or `break` value in it. It then has
    /// no type.
    failed: bool,
}

impl LoopFrame {
    fn new(
        kind: LoopKind,
        keyword: Span,
        label: Option<ast::Ident>,
        expected: Option<Type>,
    ) -> Self {
        Self {
            kind,
            keyword,
            label,
            label_used: false,
            broken: false,
            expected,
            first: None,
            bare: None,
            valued: None,
            failed: false,
            widened: false,
            crossed: false,
        }
    }

    /// The type of a `loop` whose body has been checked: `never` with no `break`, or with only
    /// `never` values; `unit` with bare ones; else its values'. `None` is `Error`.
    fn loop_type(&self) -> Option<Type> {
        if self.failed {
            return None;
        }
        if self.valued.is_none() {
            return Some(if self.bare.is_some() {
                Type::UNIT
            } else {
                Type::NEVER
            });
        }
        // Under an `Error` expectation no value sets `first`.
        self.first.map(|(ty, _)| ty)
    }
}

/// The program's text and the positions the lexer and parser found in it; never changes.
struct Source<'src> {
    /// The text the program's spans index.
    text: &'src str,
    /// Where each comment starts.
    comments: &'src [usize],
    /// Where each string literal starts, in order.
    quotes: &'src [usize],
    /// Where each line break is, in order.
    breaks: &'src [usize],
}

#[cfg(test)]
thread_local! {
    /// How many class fields the class-cycle search has read on this thread.
    pub(super) static FIELD_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many nested loops the walk that wraps a widened loop's values has entered.
    pub(super) static WRAP_LOOPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

struct Checker<'src> {
    /// The program's text and the positions found in it.
    source: Source<'src>,
    /// The first function of each name, and every broken one.
    function_ids: FxHashMap<Symbol, Declared<tir::FuncId>>,
    /// Indexed by `FuncId`, so a duplicate function has a signature of its own.
    signatures: Vec<Signature>,
    /// Indexed by `ClassId`, so a duplicate class has fields and methods of its own.
    classes: Vec<ClassInfo>,
    /// The first class of each name, and every broken one.
    class_ids: FxHashMap<Symbol, Declared<tir::ClassId>>,
    /// Indexed by `EnumId`, so a duplicate enum has variants of its own.
    enums: Vec<EnumInfo>,
    /// The first enum of each name, and every broken one.
    enum_ids: FxHashMap<Symbol, Declared<tir::EnumId>>,
    /// The enums as `tir` holds them, with their boxes and countedness; bodies are checked
    /// after they are lowered.
    lowered_enums: Vec<tir::Enum>,
    /// The first constant of each name, and every broken one.
    constants: FxHashMap<Symbol, Declared<ConstInfo>>,
    diagnostics: Vec<Diagnostic>,
    /// The body being checked.
    body: Body,
}

/// What checking one body needs: a function's, a method's, or a constant's initialiser.
#[derive(Default)]
struct Body {
    /// The type parameters in scope, which `resolve` finds before any other type.
    type_params: Vec<Symbol>,
    /// Where each call of a value, `a<b, c>(d)`, that has been reported starts: comparisons
    /// misread as a call change the count of the arguments of the call they are an argument of,
    /// which is then not reported.
    misread: FxHashSet<usize>,
    scopes: Vec<FxHashMap<Symbol, Binding>>,
    locals: Vec<tir::Local>,
    /// `None` is `Error`.
    ret: Option<Type>,
    /// The type of the method being checked, a class or an enum, whose `self` is local 0.
    self_type: Option<Type>,
    /// Loops open around the code being checked, innermost last: what a `break` would act on.
    loops: Vec<LoopContext>,
    /// The value of the `break` being checked, and the earlier `break` whose value gave the
    /// loop the type that value expects.
    break_hint: Option<(Span, Span)>,
    /// Set by a `break` or `continue` outside a loop; `scoped_block` reads it per statement.
    invalid_jump: bool,
    /// Where each loop label is declared that a failed jump or a reused loop label names, which
    /// is not reported unused: its own error is.
    failed_labels: FxHashSet<Span>,
    /// The bare name right after an unlabeled `break`, which may be a misspelled loop label:
    /// only there does E0033 offer the open loop labels.
    label_typo: Option<Span>,
    /// Where the value of the unlabeled `break` being checked starts: the parser reads an open
    /// loop label there as the `break`'s label, so a fix must not write one there.
    break_start: Option<usize>,
    /// Whether the expression being checked is a link of an assigned field's base chain (field
    /// bases, method receivers, indexed bases), where a `?.` cannot stand.
    in_target: bool,
    /// The constant whose value is being checked, where interpolation is not known and a
    /// conversion that can fail is an error.
    in_constant: Option<ConstantSite>,
    /// Each `+` found to join a `string` under an expected `u8`, which a later check of it
    /// checks with no expected type.
    joins: FxHashSet<Span>,
    /// Each method receiver, by its span, checked ahead of its call to rank the call as an
    /// argument (see `needs_context`), which `member` takes in place of checking it again.
    receivers: FxHashMap<Span, Checked>,
}

/// Diagnostics sorted by span start (stable); the program only when none is an error.
pub(crate) fn check<'src>(
    program: &'src ast::Program,
    source: &'src str,
) -> (Option<tir::Program>, Vec<Diagnostic>) {
    let mut checker = Checker::new(program, source);
    checker.collect_names(program);
    let declared = checker.collect_signatures(program);
    checker.fold_constants(program);
    let cyclic = checker.never_constructed();
    checker.lowered_enums = checker.lower_enums(&cyclic);
    checker.check_main(&declared);
    // Every body is checked, so this collects into a `Vec` before it gives up on a `None`.
    let functions: Vec<_> = declared
        .into_iter()
        .enumerate()
        .map(|(i, (f, owner))| checker.function(f, tir::FuncId(i), owner))
        .collect();
    let classes = checker.classes.iter().map(lower_class).collect();
    let mut diagnostics = checker.diagnostics;
    diagnostics.sort_by_key(|d| d.span().start());
    let failed = diagnostics.iter().any(|d| d.severity() == Severity::Error);
    let broken = !program.broken.is_empty()
        || program.items.iter().any(|item| match item {
            ast::Item::Class(class) => !class.broken_methods.is_empty(),
            ast::Item::Enum(e) => !e.broken_methods.is_empty(),
            ast::Item::Function(_) | ast::Item::Const(_) => false,
        });
    let enums = checker.lowered_enums;
    let program = functions
        .into_iter()
        .collect::<Option<_>>()
        .zip(classes)
        .filter(|_| !failed)
        .map(|(functions, classes)| tir::Program {
            classes,
            enums,
            functions,
        });
    // A use of a broken item poisons without a diagnostic of its own.
    debug_assert!(
        program.is_some() || failed || broken,
        "a missing program implies an error diagnostic or a broken item or method"
    );
    (program, diagnostics)
}

/// The class, unless a field's type is `Error`.
fn lower_class(class: &ClassInfo) -> Option<tir::Class> {
    let fields = class
        .fields
        .iter()
        .map(|f| {
            Some(tir::Field {
                name: f.name.name.to_string(),
                ty: f.ty?,
                mutable: f.mutable,
            })
        })
        .collect::<Option<_>>()?;
    Some(tir::Class {
        name: class.name.name.to_string(),
        type_params: class.type_params.iter().map(Symbol::to_string).collect(),
        fields,
    })
}

/// The type of a checked expression; `None` is `Error`.
fn type_of(checked: &Checked) -> Option<Type> {
    match checked {
        Ok(e) => Some(e.ty),
        Err(p) => p.ty,
    }
}

/// The type of a checked block; `None` is `Error`.
fn block_type(checked: &CheckedBlock) -> Option<Type> {
    match checked {
        Ok(b) => Some(b.ty),
        Err(p) => p.ty,
    }
}

impl<'src> Checker<'src> {
    /// The one way to build a checker; `check` and the tests call it.
    fn new(program: &'src ast::Program, source: &'src str) -> Checker<'src> {
        Checker {
            source: Source {
                text: source,
                comments: &program.comments,
                quotes: &program.quotes,
                breaks: &program.breaks,
            },
            function_ids: FxHashMap::default(),
            signatures: Vec::new(),
            classes: Vec::new(),
            class_ids: FxHashMap::default(),
            enums: Vec::new(),
            enum_ids: FxHashMap::default(),
            lowered_enums: Vec::new(),
            constants: FxHashMap::default(),
            diagnostics: Vec::new(),
            body: Body::default(),
        }
    }

    /// Checks with `body` as the current body, then restores the one before it, and returns
    /// `f`'s result with the finished body (its `locals` become the function's).
    fn with_body<R>(&mut self, body: Body, f: impl FnOnce(&mut Self) -> R) -> (R, Body) {
        let outer = std::mem::replace(&mut self.body, body);
        let result = f(self);
        (result, std::mem::replace(&mut self.body, outer))
    }
}

impl Checker<'_> {
    /// Records a coded diagnostic.
    fn report(&mut self, kind: DiagnosticKind) {
        self.diagnostics.push(kind.into());
    }

    /// Pass 1: gives every class a `ClassId` and every enum an `EnumId`, and records the first
    /// function, class, enum and constant of each name, so that types, calls and uses can name
    /// any of them. They share a namespace. `FuncId`s run in source order, a class's or an
    /// enum's methods at its place.
    fn collect_names(&mut self, program: &ast::Program) {
        let mut first = FxHashMap::default();
        let mut next_function = 0;
        // Source order among the constants.
        let mut order = 0;
        for item in &program.items {
            match item {
                ast::Item::Class(class) => {
                    let type_params = self.type_params(&[], &class.type_params);
                    let id = tir::ClassId(self.classes.len());
                    self.classes.push(ClassInfo {
                        name: class.name.clone(),
                        type_params,
                        fields: Vec::new(),
                        methods: FxHashMap::default(),
                    });
                    let name = &class.name;
                    if !self.built_in(name)
                        && self.first_callable(&mut first, Definition::Class, name)
                    {
                        self.class_ids.insert(name.name, Declared::Known(id));
                    }
                    next_function += class
                        .members
                        .iter()
                        .filter(|m| matches!(m, ast::Member::Method(_)))
                        .count();
                }
                ast::Item::Function(f) => {
                    if !self.built_in(&f.name)
                        && self.first_callable(&mut first, Definition::Function, &f.name)
                    {
                        self.function_ids
                            .insert(f.name.name, Declared::Known(tir::FuncId(next_function)));
                    }
                    next_function += 1;
                }
                ast::Item::Enum(e) => {
                    let type_params = self.type_params(&[], &e.type_params);
                    let id = tir::EnumId(self.enums.len());
                    self.enums.push(EnumInfo {
                        name: e.name.clone(),
                        type_params,
                        variants: Vec::new(),
                        index: FxHashMap::default(),
                        methods: FxHashMap::default(),
                    });
                    if !self.built_in(&e.name)
                        && self.first_callable(&mut first, Definition::Enum, &e.name)
                    {
                        self.enum_ids.insert(e.name.name, Declared::Known(id));
                    }
                    next_function += e.methods.len();
                }
                ast::Item::Const(c) => {
                    if !self.built_in(&c.name)
                        && self.first_callable(&mut first, Definition::Constant, &c.name)
                    {
                        self.constants.insert(
                            c.name.name,
                            Declared::Known(ConstInfo::new(c.name.span, order)),
                        );
                    }
                    order += 1;
                }
            }
        }
        for broken in &program.broken {
            let name = &broken.name.name;
            match broken.kind {
                ast::BrokenKind::Function => {
                    self.function_ids.insert(*name, Declared::Broken);
                }
                ast::BrokenKind::Class => {
                    self.class_ids.insert(*name, Declared::Broken);
                }
                ast::BrokenKind::Enum => {
                    self.enum_ids.insert(*name, Declared::Broken);
                }
                ast::BrokenKind::Constant => {
                    self.constants.insert(*name, Declared::Broken);
                }
            }
            // Functions, classes, enums and constants share a namespace, so an intact item of
            // another kind and this name is poisoned too.
            if let Some(class) = self.class_ids.get_mut(name) {
                *class = Declared::Broken;
            }
            if let Some(e) = self.enum_ids.get_mut(name) {
                *e = Declared::Broken;
            }
            if let Some(function) = self.function_ids.get_mut(name) {
                *function = Declared::Broken;
            }
            if let Some(constant) = self.constants.get_mut(name) {
                *constant = Declared::Broken;
            }
        }
    }

    /// Whether `name` is a built-in type's, which no class, function, enum or constant may
    /// take; reports it when it is. A name milestone 8's prelude takes is reported too, but the
    /// item still takes it, so that its uses do not cascade.
    fn built_in(&mut self, name: &ast::Ident) -> bool {
        let found = built_in_type(name.name.as_str()).is_some();
        if found {
            self.report(DiagnosticKind::BuiltInName {
                name: name.name.to_string(),
                at: name.span,
                kind: BuiltIn::Type,
            });
        } else {
            self.reserved(name);
        }
        found
    }

    /// Whether `name` is the first function, class, enum or constant of its name; reports it when
    /// not.
    fn first_callable(
        &mut self,
        first: &mut FxHashMap<Symbol, (Span, Definition)>,
        kind: Definition,
        name: &ast::Ident,
    ) -> bool {
        if PRELUDE.contains(&name.name.as_str()) {
            self.report(defined_twice(kind, name, Earlier::BuiltIn));
            return false;
        }
        self.first_definition(first, kind, None, name)
    }

    /// Whether `name` is the first of its name in `first`, which it joins with its `kind`;
    /// reports it when not. `owner` is the class whose members `first` holds.
    fn first_definition(
        &mut self,
        first: &mut FxHashMap<Symbol, (Span, Definition)>,
        kind: Definition,
        owner: Option<&str>,
        name: &ast::Ident,
    ) -> bool {
        if let Some(&(span, first_kind)) = first.get(&name.name) {
            let first = Earlier::At {
                span,
                kind: first_kind,
                owner: owner.map(str::to_string),
            };
            self.report(defined_twice(kind, name, first));
            return false;
        }
        first.insert(name.name, (name.span, kind));
        true
    }

    /// Pass 2: each class's fields, each enum's variants and every signature, in `FuncId`
    /// order. Returns each function with the type of a method's `self`. A class's fields and
    /// methods share a namespace.
    fn collect_signatures<'a>(
        &mut self,
        program: &'a ast::Program,
    ) -> Vec<(&'a ast::Function, Option<Type>)> {
        let mut declared = Vec::new();
        let mut next_class = 0;
        let mut next_enum = 0;
        for item in &program.items {
            let class = match item {
                ast::Item::Function(f) => {
                    self.signature(f, f.name.name.to_string(), &[]);
                    declared.push((f, None));
                    continue;
                }
                ast::Item::Class(class) => class,
                ast::Item::Enum(e) => {
                    self.enum_signatures(e, tir::EnumId(next_enum), &mut declared);
                    next_enum += 1;
                    continue;
                }
                ast::Item::Const(_) => continue,
            };
            let id = tir::ClassId(next_class);
            next_class += 1;
            let owner = self.classes[id.0].type_params.clone();
            let receiver = Type::intern(TypeKind::Class(id, own_args(&owner)));
            let mut first = FxHashMap::default();
            for member in &class.members {
                match member {
                    ast::Member::Field(field) => {
                        self.body.type_params.clone_from(&owner);
                        let ty = self.resolve(&field.ty);
                        self.body.type_params.clear();
                        if self.first_definition(
                            &mut first,
                            Definition::Field,
                            Some(class.name.name.as_str()),
                            &field.name,
                        ) {
                            self.classes[id.0].fields.push(FieldInfo {
                                name: field.name.clone(),
                                ty_span: field.ty.span,
                                ty,
                                mutable: field.mutable,
                            });
                        }
                    }
                    ast::Member::Method(method) => {
                        let func = tir::FuncId(self.signatures.len());
                        if self.first_definition(
                            &mut first,
                            Definition::Method,
                            Some(class.name.name.as_str()),
                            &method.name,
                        ) {
                            self.classes[id.0]
                                .methods
                                .insert(method.name.name, Declared::Known(func));
                        }
                        let name = format!("{}.{}", class.name.name, method.name.name);
                        self.signature(method, name, &class.type_params);
                        declared.push((method, Some(receiver)));
                    }
                }
            }
            for method in &class.broken_methods {
                self.classes[id.0]
                    .methods
                    .insert(method.name, Declared::Broken);
            }
        }
        declared
    }

    /// `f`'s signature, named `name`, where `owner` are the type parameters of a method's class
    /// or enum.
    fn signature(&mut self, f: &ast::Function, name: String, owner: &[ast::Ident]) {
        let type_params = self.type_params(owner, &f.type_params);
        self.body.type_params.clone_from(&type_params);
        let params = f.params.iter().map(|p| self.resolve(&p.ty)).collect();
        let ret = f
            .ret
            .as_ref()
            .map_or(Some(Type::UNIT), |ty| self.resolve(ty));
        self.body.type_params.clear();
        self.signatures.push(Signature {
            name,
            type_params,
            params,
            ret,
            span: f.name.span,
        });
    }

    /// Reports each strongly connected component of non-optional class fields once
    /// (overlapping cycles share one error), on the component's first class in source order,
    /// at its first field into the component: no first object of such a class can exist.
    /// Returns, by `ClassId`, whether each class is on such a cycle.
    fn never_constructed(&mut self) -> Vec<bool> {
        let edges: Vec<Vec<usize>> = self
            .classes
            .iter()
            .map(|class| {
                class
                    .fields
                    .iter()
                    .filter_map(|f| {
                        #[cfg(test)]
                        FIELD_READS.set(FIELD_READS.get() + 1);
                        match f.ty.map(Type::kind) {
                            Some(TypeKind::Class(d, _)) => Some(d.0),
                            _ => None,
                        }
                    })
                    .collect()
            })
            .collect();
        let component = crate::graph::components(&edges);
        let mut size = vec![0; edges.len()];
        for &k in &component {
            size[k] += 1;
        }
        let cyclic: Vec<bool> = (0..edges.len())
            .map(|c| size[component[c]] > 1 || edges[c].contains(&c))
            .collect();
        let mut reported = vec![false; edges.len()];
        let mut reports = Vec::new();
        for (c, class) in self.classes.iter().enumerate() {
            if !cyclic[c] || std::mem::replace(&mut reported[component[c]], true) {
                continue;
            }
            let (field, target) = class
                .fields
                .iter()
                .find_map(|f| match f.ty.map(Type::kind) {
                    Some(TypeKind::Class(d, _)) if component[d.0] == component[c] => Some((f, d)),
                    _ => None,
                })
                .expect(
                    "a class on a reference cycle should have a field that leads into the cycle",
                );
            reports.push(DiagnosticKind::NeverConstructed {
                name: class.name.name.to_string(),
                at: field.name.span,
                cause: Unbuilt::Field {
                    field: field.name.name.to_string(),
                    target: self.classes[target.0].name.name.to_string(),
                    ty: field.ty_span,
                },
            });
        }
        for report in reports {
            self.report(report);
        }
        cyclic
    }

    /// Reports a missing `main`, or one that takes anything but nothing or one `[string]`, or
    /// that returns a value. A `main` rejected as defined twice is not missing, and a type in
    /// its signature that did not resolve is not wrong: each error is reported already.
    fn check_main(&mut self, declared: &[(&ast::Function, Option<Type>)]) {
        match self.function_ids.get(&Symbol::intern("main")) {
            None if declared
                .iter()
                .any(|(f, owner)| owner.is_none() && f.name.name.as_str() == "main") => {}
            None => self.report(DiagnosticKind::NoMain),
            Some(Declared::Broken) => {}
            Some(Declared::Known(id)) => {
                let main = declared[id.0].0;
                if !main.type_params.is_empty() {
                    self.main_type_params(main);
                    return;
                }
                let Signature { params, ret, .. } = &self.signatures[id.0];
                if params.iter().any(Option::is_none) || ret.is_none() {
                    return;
                }
                let args = Some(Type::array(Type::STRING));
                let takes = params.is_empty() || *params == [args];
                if !takes || *ret != Some(Type::UNIT) {
                    self.report(DiagnosticKind::MainSignature {
                        at: main.name.span,
                        form: MainForm::Signature,
                    });
                }
            }
        }
    }

    /// `None`, reported at the name without its `?`, for an unknown type, and for type
    /// arguments that do not fit.
    fn resolve(&mut self, ty: &ast::TypeExpr) -> Option<Type> {
        let named = match &ty.kind {
            ast::TypeKind::Array(element) => Type::array(self.resolve(element)?),
            ast::TypeKind::Applied { name, args, angles } => {
                self.named_type(ty, *name, args, *angles)?
            }
            ast::TypeKind::Name(name) => self.named_type(ty, *name, &[], ty.span)?,
        };
        if !ty.optional {
            return Some(named);
        }
        Some(optional_of(named))
    }

    /// The type `name`, written in `ty` with the type arguments `args` in `angles`.
    fn named_type(
        &mut self,
        ty: &ast::TypeExpr,
        name: Symbol,
        args: &[ast::TypeExpr],
        angles: Span,
    ) -> Option<Type> {
        let ident = ast::Ident {
            name,
            span: Span::new(ty.span.start(), ty.span.start() + name.as_str().len()),
        };
        // A type parameter or a built-in type is not generic.
        let plain = |c: &mut Self, ty| c.type_args(&ident, &[], args, angles).map(|_| ty);
        if let Some(i) = self.body.type_params.iter().position(|&p| p == name) {
            let i = u32::try_from(i).expect("a declaration should have fewer than 2^32 parameters");
            return plain(self, Type::intern(TypeKind::Param(name, i)));
        }
        let declared = (
            self.class_ids.get(&name).copied(),
            self.enum_ids.get(&name).copied(),
        );
        // A broken name poisons every use, a built-in type's too.
        if let (Some(Declared::Broken), _) | (_, Some(Declared::Broken)) = declared {
            return None;
        }
        if let Some(named) = built_in_type(name.as_str()) {
            return plain(self, named);
        }
        match declared {
            (Some(Declared::Known(id)), _) => {
                let declared = self.classes[id.0].type_params.clone();
                let args = self.type_args(&ident, &declared, args, angles)?;
                Some(Type::intern(TypeKind::Class(id, args)))
            }
            (_, Some(Declared::Known(id))) => {
                let declared = self.enums[id.0].type_params.clone();
                let args = self.type_args(&ident, &declared, args, angles)?;
                Some(Type::intern(TypeKind::Enum(id, args)))
            }
            _ => {
                let types =
                    ["i64", "u8", "f64", "bool", "char", "string"].map(|ty| (ty, Definition::Type));
                let classes = self.class_names();
                let enums = self.enum_ids.keys().map(|e| (e.as_str(), Definition::Enum));
                let candidates = types.into_iter().chain(classes).chain(enums);
                self.report(DiagnosticKind::UnknownType {
                    name: name.to_string(),
                    at: ident.span,
                    suggestion: suggestion(name.as_str(), candidates),
                });
                None
            }
        }
    }

    /// Checks `f`'s body against its own signature, `id`. The function, unless its return type
    /// is `Error` or its body failed; an `Error` parameter still yields one, and `check` drops
    /// it for the error behind it.
    /// A method, of the class or enum `owner`, gets `self` as its first local.
    fn function(
        &mut self,
        f: &ast::Function,
        id: tir::FuncId,
        owner: Option<Type>,
    ) -> Option<tir::Function> {
        let sig = &self.signatures[id.0];
        let (name, param_types, ret) = (sig.name.clone(), sig.params.clone(), sig.ret);
        let owner_params = match owner.map(Type::kind) {
            Some(TypeKind::Class(class, _)) => self.classes[class.0].type_params.len(),
            Some(TypeKind::Enum(e, _)) => self.enums[e.0].type_params.len(),
            _ => 0,
        };
        let fresh = Body {
            type_params: sig.type_params.clone(),
            ret,
            self_type: owner,
            ..Body::default()
        };
        let ((params, body), state) = self.with_body(fresh, |c| {
            c.body.scopes.push(FxHashMap::default());
            // `self` is a keyword, so it needs no binding: `ExprKind::SelfValue` reads local 0.
            let receiver = owner.map(|ty| {
                c.body.locals.push(tir::Local {
                    name: Some("self".into()),
                    ty,
                    mutable: false,
                });
                tir::LocalId(0)
            });
            let params = receiver
                .into_iter()
                .chain(
                    f.params
                        .iter()
                        .zip(param_types)
                        .map(|(p, ty)| c.declare(&p.name, ty, BindingKind::Param, None)),
                )
                .collect();
            // A `unit` function discards its body's value, so it expects none.
            let expected = match ret {
                None => Some(UNKNOWN),
                Some(Type::UNIT) => None,
                Some(ret) => Some(ret),
            };
            let mut body = c.block(&f.body, expected);
            if let (Some(Type::UNIT), Ok(checked)) = (ret, &body) {
                c.discard_tail(&f.body, checked);
            }
            if let Some(want) = ret
                && let TypeKind::Optional(payload) = want.kind()
                && block_type(&body) == Some(payload)
            {
                wrap_block(&mut body, want);
            }
            c.pop_scope();
            (params, body)
        });
        let locals = state.locals;
        let type_params = state.type_params.iter().map(Symbol::to_string).collect();
        match (ret, block_type(&body)) {
            (Some(want), Some(got)) if want != Type::UNIT && got != Type::NEVER && got != want => {
                let d = if got == Type::UNIT {
                    let end = f.body.span.end();
                    DiagnosticKind::BodyHasNoValue {
                        function: f.name.name.to_string(),
                        want: self.type_name(want),
                        at: Span::new(end - 1, end),
                    }
                    .into()
                } else {
                    self.mismatch(value_span(&f.body), tail(&f.body), want, got)
                };
                self.diagnostics.push(d);
            }
            _ => {}
        }
        Some(tir::Function {
            name,
            type_params,
            owner_params,
            params,
            ret: ret?,
            locals,
            body: body.ok()?,
        })
    }

    /// The classes, as candidates for a name.
    fn class_names(&self) -> impl Iterator<Item = (&str, Definition)> {
        self.class_ids
            .keys()
            .map(|class| (class.as_str(), Definition::Class))
    }

    /// The variant `name`, at `span`, written without its enum: `None` when a binding or an item
    /// takes the name, or no enum has such a variant.
    fn bare_variant(&self, name: Symbol, span: Span) -> Option<Diagnostic> {
        let item = self
            .body
            .scopes
            .iter()
            .any(|scope| scope.contains_key(&name))
            || self.class_ids.contains_key(&name)
            || self.function_ids.contains_key(&name)
            || self.enum_ids.contains_key(&name)
            || self.constants.contains_key(&name);
        let enums = self.variant_of(name);
        let at_start = self.starts_break_value(span);
        let labelled = enums
            .iter()
            .filter(|e| at_start && self.is_open_label(e))
            .cloned()
            .collect();
        (!item && !enums.is_empty()).then(|| {
            DiagnosticKind::BareVariant {
                name: name.to_string(),
                enums,
                labelled,
                at: span,
            }
            .into()
        })
    }

    /// Whether `at` starts the value of an unlabeled `break`. A fix that writes a name from `at`
    /// on, a parenthesized name's span included, must not write an open loop label there.
    pub(super) fn starts_break_value(&self, at: Span) -> bool {
        self.body.break_start == Some(at.start())
    }

    /// Whether `name` is an open loop label. After an unlabeled `break`, the parser reads it as
    /// the `break`'s label, so a fix does not write it where the value starts.
    pub(super) fn is_open_label(&self, name: &str) -> bool {
        self.open_labels().any(|l| l.name.as_str() == name)
    }

    /// "cannot find binding `name`", offering the closest binding of any open scope, and of any
    /// open loop label right after an unlabeled `break`. A type's name, built in or a class's or
    /// an enum's, is in scope, but as a type: that is reported instead.
    fn not_found(&mut self, name: Symbol, span: Span) -> Diagnostic {
        if built_in_type(name.as_str()).is_some()
            || self.class_ids.contains_key(&name)
            || self.enum_ids.contains_key(&name)
        {
            return DiagnosticKind::TypeAsValue {
                name: name.to_string(),
                at: span,
            }
            .into();
        }
        let at_start = self.starts_break_value(span);
        let bindings = self
            .body
            .scopes
            .iter()
            .flat_map(|scope| scope.keys())
            .map(|binding| (binding.as_str(), Definition::Binding))
            .filter(|(binding, _)| !(at_start && self.is_open_label(binding)));
        let labels = self
            .open_labels()
            .filter(|_| self.body.label_typo == Some(span))
            .map(|label| (label.name.as_str(), Definition::LoopLabel));
        let suggestion = suggestion(name.as_str(), bindings.chain(labels));
        if let Some(found) = &suggestion
            && found.kind == Definition::LoopLabel
        {
            // Only a bare name after `break` offers a loop label: a misspelled loop label is one
            // mistake, which neither the loop label's warning nor its loop's type reports again.
            self.break_meant(&found.name);
        }
        DiagnosticKind::BindingNotFound {
            name: name.to_string(),
            at: span,
            suggestion,
        }
        .into()
    }

    fn type_name(&self, ty: Type) -> String {
        match ty.kind() {
            TypeKind::Class(id, args) => format!(
                "{}{}",
                self.classes[id.0].name.name,
                args.written(|ty| self.type_name(ty))
            ),
            TypeKind::Enum(id, args) => format!(
                "{}{}",
                self.enums[id.0].name.name,
                args.written(|ty| self.type_name(ty))
            ),
            TypeKind::Array(element) => format!("[{}]", self.type_name(element)),
            TypeKind::Optional(payload) => format!("{}?", self.type_name(payload)),
            _ => ty.to_string(),
        }
    }

    /// "expected `want`, found `got`" at `span`, where `source` is the expression found, when
    /// known. Between `u8`, `i64` and `char` it offers to convert `source`, unless that is an
    /// `if`: an `else if` is one, and `else u8(if ...)` does not parse. A conversion to `u8` or
    /// `char` gives an optional, so it is offered only where one is expected.
    fn mismatch(
        &self,
        span: Span,
        source: Option<&ast::Expr>,
        want: Type,
        got: Type,
    ) -> Diagnostic {
        self.mismatch_from(span, source, want, got, None)
    }

    /// Where the type that the value at `span` expects came from: an earlier `break`, when it
    /// is a `break` value checked against the loop's type.
    fn expected_from(&self, span: Span) -> Option<MismatchSource> {
        self.body
            .break_hint
            .filter(|&(at, _)| at == span)
            .map(|(_, first)| MismatchSource::Break(first))
    }

    /// `mismatch`, where `want` came from `from`.
    fn mismatch_from(
        &self,
        span: Span,
        source: Option<&ast::Expr>,
        want: Type,
        got: Type,
        from: Option<MismatchSource>,
    ) -> Diagnostic {
        let optional = matches!(want.kind(), TypeKind::Optional(_));
        // `u8` and `char` convert into each other through `i64`, and into `f64` through it.
        let (to, through) = match (payload(want), got) {
            (Type::I64, Type::U8 | Type::CHAR) => (Some("i64"), None),
            (Type::F64, Type::I64) => (Some("f64"), None),
            (Type::F64, Type::U8 | Type::CHAR) => (Some("f64"), Some("i64")),
            (Type::U8, Type::I64) if optional => (Some("u8"), None),
            (Type::CHAR, Type::I64) if optional => (Some("char"), None),
            (Type::U8, Type::CHAR) if optional => (Some("u8"), Some("i64")),
            (Type::CHAR, Type::U8) if optional => (Some("char"), Some("i64")),
            _ => (None, None),
        };
        let conversion = matches!(got.kind(), TypeKind::Optional(p) if p == want)
            && source.is_some_and(can_fail);
        DiagnosticKind::Mismatch {
            want: vec![self.type_name(want)],
            got: self.type_name(got),
            at: span,
            source: from,
            fix: to
                .zip(source.filter(|e| {
                    !matches!(
                        e.kind,
                        ast::ExprKind::If { .. }
                            | ast::ExprKind::IfLet { .. }
                            | ast::ExprKind::Match { .. }
                    )
                }))
                .map(|(to, e)| {
                    MismatchFix::Convert(Conversion {
                        to,
                        through,
                        operand: e.span,
                    })
                }),
            conversion,
        }
        .into()
    }
}

/// Whether `e` is a call of a conversion that can fail: `u8(...)`, `char(...)`, or `i64(...)`,
/// which can fail only from an `f64`, the one argument that makes it an optional.
fn can_fail(e: &ast::Expr) -> bool {
    matches!(&e.kind, ast::ExprKind::Call { callee, .. } if matches!(callee.name.as_str(), "u8" | "char" | "i64"))
}

/// `params` as the arguments of their own declaration: `<T, U>` as `Param`s.
fn own_args(params: &[Symbol]) -> TypeArgs {
    let list: Vec<Type> = (0..)
        .zip(params)
        .map(|(i, &name)| Type::intern(TypeKind::Param(name, i)))
        .collect();
    TypeArgs::intern(&list)
}

/// The type a built-in type name names.
fn built_in_type(name: &str) -> Option<Type> {
    match name {
        "i64" => Some(Type::I64),
        "u8" => Some(Type::U8),
        "f64" => Some(Type::F64),
        "bool" => Some(Type::BOOL),
        "char" => Some(Type::CHAR),
        "string" => Some(Type::STRING),
        _ => None,
    }
}

/// `T` for a `T?`, else `ty`.
fn payload(ty: Type) -> Type {
    match ty.kind() {
        TypeKind::Optional(payload) => payload,
        _ => ty,
    }
}

/// The expression a block ends in, if it ends in one: its value.
fn tail(block: &ast::Block) -> Option<&ast::Expr> {
    match block.stmts.last() {
        Some(ast::Stmt {
            kind: ast::StmtKind::Expr(e),
            ..
        }) => Some(e),
        _ => None,
    }
}

/// Where a block's value comes from: its last statement, or the block itself when empty.
fn value_span(block: &ast::Block) -> Span {
    block.stmts.last().map_or(block.span, |s| s.span)
}

/// A `Block` expression of `stmts` then `value`, of type `ty`.
fn block_expr(stmts: Vec<tir::Stmt>, value: Option<tir::Expr>, ty: Type, span: Span) -> tir::Expr {
    let block = tir::Block {
        stmts,
        value: value.map(Box::new),
        ty,
    };
    tir::Expr {
        kind: tir::ExprKind::Block(block),
        ty,
        span,
    }
}

/// `ty?`; an optional is its own, as optionals do not nest.
fn optional_of(ty: Type) -> Type {
    match ty.kind() {
        TypeKind::Optional(_) => ty,
        _ => Type::optional(ty),
    }
}

/// `e`, of type `T`, where its optional `ty` is expected.
fn wrap(e: tir::Expr, ty: Type) -> tir::Expr {
    tir::Expr {
        span: e.span,
        kind: tir::ExprKind::Wrap(Box::new(e)),
        ty,
    }
}

/// Makes a block of type `T` one of the optional `ty` by wrapping its value.
fn wrap_block(block: &mut CheckedBlock, ty: Type) {
    match block {
        Ok(block) => {
            let value = block
                .value
                .take()
                .expect("a block checked as a value type should end in its value");
            block.value = Some(Box::new(wrap(*value, ty)));
            block.ty = ty;
        }
        Err(poisoned) => poisoned.ty = Some(ty),
    }
}

/// `kind` is the second definition's; `first` is the earlier one.
fn defined_twice(kind: Definition, name: &ast::Ident, first: Earlier) -> DiagnosticKind {
    DiagnosticKind::DefinedTwice {
        kind,
        name: name.name.to_string(),
        at: name.span,
        first,
    }
}

#[cfg(test)]
mod tests;
