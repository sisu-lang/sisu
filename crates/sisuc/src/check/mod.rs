//! The checker: signatures, scopes, names and types. It lowers the program to `tir` as it
//! checks, and records every error instead of stopping at the first.

mod alias;
mod any;
mod array;
mod assoc;
mod bound;
mod branch;
mod call;
mod conform;
mod constant;
mod cycle;
mod enums;
mod exhaust;
mod expectation;
mod expr;
mod extend;
mod generic;
mod interface;
mod member;
mod mut_args;
mod names;
mod number;
mod operator;
mod optional;
mod pattern;
mod place;
mod prelude;
mod range;
mod recursion;
mod reports;
mod stmt;
mod string;
mod structs;
mod unused_names;

use std::borrow::Cow;
use std::iter::Peekable;
use std::ops::Range;
use std::rc::Rc;
use std::slice;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::ast;
use crate::conformance::{self, StuckChoices};
use crate::diagnostic::{
    BuiltIn, Code, Conversion, Definition, Diagnostic, DiagnosticKind, Earlier, Expected, Literal,
    MainForm, MismatchFix, Severity, Unbuilt, UnknownForm, suggestion,
};
use crate::front_end::PreludeProgram;
use crate::payload_limit;
use crate::source::{self, Span};
use crate::symbol::Symbol;
use crate::tir::{self, Type, TypeArgs, TypeKind};
use crate::types::{Head, write_type};
use alias::{AliasInfo, Aliases, FieldType, UseCheck};
use assoc::{Choice, Choices, Named};
use constant::{ConstInfo, ConstantSite};
use enums::{EnumInfo, HoldsTable, reserved_name};
use expectation::{Expectation, Origin};
use extend::Extension;
use interface::{Bounds, InterfaceInfo, PendingBound, Required, Scope, Slot, Waiting};
use names::{ConstantId, ItemId, NameMaps, Position, Resolved};
use prelude::PRELUDE;
use reports::{Diagnostics, Reported};
pub(crate) use unused_names::is_judged;
use unused_names::{Owner, Pending, Use, Used};

/// An expression whose rule failed; its diagnostic is recorded. `ty` is its type when it is
/// still known: a `let` with a declared type keeps that type. Else the value is untyped: it
/// matches every type and draws no further diagnostic.
///
/// Poison spreads to sibling operands by one rule. Take a homogeneous operator (`+ - * / %
/// && ||`, comparison chains). Once an operand is untyped, the later operands are still
/// checked for their own errors, but not against the operator's type. Call and constructor
/// arguments are each checked against their own parameter, so one bad argument does not
/// excuse another.
///
/// A failed call keeps the result type that its callee fixes: a function, a method, a
/// constructor, a variant, a built-in method or a prelude function. The receiver's type
/// arguments and the written ones are put in the result (`Checker::fixed_result`).
///
/// A callee whose result names a parameter that only the arguments bind gives an untyped value,
/// as `dbg`, `get` and a conversion do. So does a call whose written type arguments fail. So
/// does a call that gives a bound to its own parameter, when that bound fails or no list is
/// written. The corrected call is untyped then (`Checker::counted_result`).
///
/// A construction of a generic class or variant keeps the type that its written list, or its
/// alias, fixes. It is untyped when that list fails, or fails a bound.
///
/// A generic call that cannot meet its context is untyped too, but it keeps the type it found
/// in `found`. A block, a branch and a safe call keep that field for the code that uses the value.
/// A value in which a generic call kept its E0050 has `kept` set. A block, a branch, unary `-`,
/// a number operator, and a call that fails on an unbound parameter, keep that field too.
///
/// A poisoned value is typed only when the source states its type apart from the failed part
/// (the form table of ADR 0049). Each reader reads a typed poisoned value as any value of that
/// type.
struct Poisoned {
    ty: Option<Type>,
    /// Whether the expected type passed down was rejected. It is rejected by a `None` that is not
    /// optional, a number that does not fit, or an array with such an element. A block or `if`
    /// whose value is one is rejected too. The rule that passed it failed, so its expression is
    /// untyped too, where an untyped operand would excuse it. A number that does not fit (E0040,
    /// E0042 or E0104) counts as a mismatch, as a value of another type does.
    mismatch: bool,
    /// What a generic call found for its result, which cannot meet the call's context. Only a
    /// report reads it. Every other check sees an untyped value. It stays apart from `ty`:
    /// `settled_by` and the joins of `coalesce` and `break_value` read it after the call
    /// returned, when no expected type went down to the call. S18 pins it.
    found: Option<Misfit>,
    /// A generic call in the value kept its E0050, which is its only report. The value is
    /// untyped, and an operator with such an operand adds no report for its default type. Three
    /// readers in `integers` and `binary` need the mark. An operand that waits after a kept one
    /// takes the default `i64` as a hint and keeps its own E0050 (S18). A leader that the
    /// operator does not take is held to the operator's type after a kept operand, so
    /// `id(mk2()) - b` with `b: bool` gives its E0032. And unary `-` and `binary` pass the mark
    /// on, so a `char` beside `-(id(mk2()) + 1)` is still rejected.
    kept: bool,
    /// A failed `break` or `continue` outside a loop makes the value `never`, and that jump has
    /// its own report. In a block, it is the statement that ends the block. In an `if` or a
    /// `match`, it is at least one branch (see `join`). The flow keeps the `never`, as a jump
    /// statement does; a place that reads the value takes it as untyped (`read_jump`).
    jumped: bool,
    /// The proof of the report, or of the promise, that made the value poisoned.
    reported: Reported,
}

/// The result type of a generic call that cannot meet its context (see `report_unbound`). The
/// type names the callee's unbound parameters, so it is never a value's type. The code that uses
/// the value reports the mismatch with it, as it does for a callee that is not generic. `entry`
/// is the place of the call's E0050 in `Body::misfits`, which that report cancels. It is not the
/// type of a poisoned value: a value with it is untyped, and only the readers of `found` see it.
#[derive(Clone, Copy)]
struct Misfit {
    ty: Type,
    entry: usize,
}

impl Poisoned {
    /// A failed expression whose type is not known, poisoned by the report `reported` proves.
    fn untyped(reported: Reported) -> Self {
        Self::new(None, reported)
    }

    /// A failed expression whose type is still known.
    fn typed(ty: Type, reported: Reported) -> Self {
        Self::new(Some(ty), reported)
    }

    /// A failed expression of type `ty`, when it has one.
    fn new(ty: Option<Type>, reported: Reported) -> Self {
        Self {
            ty,
            mismatch: false,
            found: None,
            kept: false,
            jumped: false,
            reported,
        }
    }

    /// An untyped value in which a generic call kept its E0050 (see `Poisoned::kept`).
    fn kept_error(reported: Reported) -> Self {
        Self {
            kept: true,
            ..Self::untyped(reported)
        }
    }

    /// The proof of the report that made the value poisoned.
    fn reported(&self) -> Reported {
        self.reported
    }
}

/// Whether a generic call in `checked` kept its E0050 (see `Poisoned::kept`).
fn is_kept<T>(checked: &Result<T, Poisoned>) -> bool {
    matches!(checked, Err(Poisoned { kept: true, .. }))
}

/// Whether a failed `break` or `continue` makes `checked` `never` (see `Poisoned::jumped`).
fn is_jumped<T>(checked: &Result<T, Poisoned>) -> bool {
    matches!(checked, Err(Poisoned { jumped: true, .. }))
}

type Checked = Result<tir::Expr, Poisoned>;
type CheckedBlock = Result<tir::Block, Poisoned>;

/// What holds methods: a class or an enum, or, for the prelude's `extend` blocks, every array
/// type together, or `i64`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Holder {
    Class(tir::ClassId),
    Struct(tir::StructId),
    Enum(tir::EnumId),
    Array,
    I64,
}

/// The type parameters of a declaration with the bounds of each.
type ParamBounds = (Vec<Symbol>, Vec<Bounds>);

/// A type of `None` is `Error`: its error is reported where the signature names it.
struct Signature {
    /// `f`, or `P.m` for a method.
    name: String,
    /// The type parameters in scope: a method's owner's, then its own.
    type_params: Vec<Symbol>,
    /// How many of `type_params` are the function's own, the last ones.
    own: usize,
    /// The bounds of each of the function's own type parameters.
    bounds: Vec<Bounds>,
    /// Without a method's `self`.
    params: Vec<Option<Type>>,
    /// Whether each of `params` is `mut`.
    muts: Vec<bool>,
    /// The name of each of `params`.
    param_names: Vec<ast::Ident>,
    ret: Option<Type>,
    /// The interface of each `any` type that the parameter and return types write, with the
    /// span as written: the function records them as uses.
    anys: Vec<(tir::InterfaceId, Span)>,
    /// Each parameter's type as written, when the function is not generic and the type names an
    /// alias: a mismatch shows it so.
    param_shown: Vec<Option<Rc<str>>>,
    /// The return type as written, under the same rule.
    ret_shown: Option<Rc<str>>,
    /// Where each of `params` writes its type: a mismatch with one points there.
    param_spans: Vec<Span>,
    /// Where the return type is written; none when the function writes none.
    ret_span: Option<Span>,
    /// The name's span.
    span: Span,
    /// `fn` through the end of the signature, before the body.
    header: Span,
    /// The interface, with its arguments, of the `extend` block that declares a method.
    conformance: Option<(tir::InterfaceId, TypeArgs)>,
    /// The place among `Checker.extensions` of the `extend` block that declares a method, if the
    /// checker takes the block: its target and bounds are the owner's.
    extension: Option<usize>,
}

/// What a function, class, constant or method name stands for: the first intact item of that
/// name, or a poisoned one: an item that holds a syntax error, or that the checker rejected. A
/// poisoned name wins over an intact one, and every use of it is poisoned without a diagnostic:
/// its item's error is reported already, and the token proves it.
#[derive(Clone, Copy)]
enum Declared<T> {
    Known(T),
    Poisoned(Reported),
}

/// The name of the top-level item `item`; an `extend` block has none of its own.
fn item_name(item: &ast::Item) -> Option<&ast::Ident> {
    match item {
        ast::Item::Class(class) => Some(&class.name),
        ast::Item::Struct(s) => Some(&s.name),
        ast::Item::Function(f) => Some(&f.name),
        ast::Item::Enum(e) => Some(&e.name),
        ast::Item::Const(c) => Some(&c.name),
        ast::Item::Interface(interface) => Some(&interface.name),
        ast::Item::Alias(alias) => Some(&alias.name),
        ast::Item::Extend(_) => None,
    }
}

/// The kind of definition that a broken item of `kind` is. An `extend` block defines no name.
fn broken_definition(kind: ast::BrokenKind) -> Option<Definition> {
    match kind {
        ast::BrokenKind::Function => Some(Definition::Function),
        ast::BrokenKind::Class => Some(Definition::Class),
        ast::BrokenKind::Struct => Some(Definition::Struct),
        ast::BrokenKind::Interface => Some(Definition::Interface),
        ast::BrokenKind::Enum => Some(Definition::Enum),
        ast::BrokenKind::Constant => Some(Definition::Constant),
        ast::BrokenKind::Alias => Some(Definition::Alias),
        ast::BrokenKind::Extend => None,
    }
}

struct ClassInfo {
    name: ast::Ident,
    type_params: Vec<Symbol>,
    /// Whether a type parameter is written with a bound.
    bounded: bool,
    /// The bounds of each of `type_params`; empty when `bounded` is false.
    bounds: Vec<Bounds>,
    /// The first field of each name, in declaration order.
    fields: Vec<FieldInfo>,
    /// Each field's index in `fields`, by name.
    index: FxHashMap<Symbol, usize>,
    /// The first method of each name, and every broken one.
    methods: FxHashMap<Symbol, Declared<tir::FuncId>>,
    /// Whether a field is named `to_string`.
    names_to_string: bool,
    /// The associated types the declaration's body chooses.
    assoc: Rc<Choices>,
}

impl ClassInfo {
    /// A class with no field, bound, method or choice of associated type yet.
    fn new(name: ast::Ident, type_params: Vec<Symbol>, bounded: bool) -> Self {
        Self {
            name,
            type_params,
            bounded,
            bounds: Vec::new(),
            fields: Vec::new(),
            index: FxHashMap::default(),
            methods: FxHashMap::default(),
            names_to_string: false,
            assoc: Rc::default(),
        }
    }

    /// Adds a field after the ones it has. The caller checks that no field has its name.
    fn add_field(&mut self, field: FieldInfo) {
        self.index.insert(field.name.name, self.fields.len());
        self.fields.push(field);
    }

    /// The index in `fields` of the field called `name`, if the class has one.
    fn field(&self, name: Symbol) -> Option<usize> {
        #[cfg(test)]
        FIELD_LOOKUPS.set(FIELD_LOOKUPS.get() + 1);
        self.index.get(&name).copied()
    }
}

struct FieldInfo {
    name: ast::Ident,
    /// Where the field's type is written.
    ty_span: Span,
    /// The class and enum types written in the field's type, each with its span.
    inner: Vec<(Type, Span)>,
    /// `None` is `Error`.
    ty: Option<Type>,
    /// The type as written, when the class is not generic and the type names an alias.
    shown: Option<Rc<str>>,
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

#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is a separate fact, set by a different statement or expression"
)]
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
    /// `Body::writes` at the last write rooted at the binding; 0 for none.
    written: u32,
    /// Whether W0011 names the binding, which then gets no W0002: its fix would keep the loop.
    loop_named: bool,
    /// Whether an expression reads the binding's value.
    read: bool,
    /// Whether the binding is a `mut` parameter, whose store-back reads it.
    mut_param: bool,
    /// A pattern made this local to hide a poisoned name: it is no value.
    hides_name: bool,
    /// Where each later alternative of an or-pattern binds the name again.
    also: Vec<Span>,
    /// The type of the annotation as written, when it names an alias.
    shown: Option<Rc<str>>,
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
    /// The type the position around a `loop` expects, each value's hint; `Poisoned` when that
    /// position failed. A `Hint` when the loop is hinted, which no value converts to.
    expected: Expectation,
    /// The type of the first value whose type is neither `never` nor `Error`, and its `break`'s
    /// span; under an expected `T?`, a `T?` value after `T` ones replaces it.
    first: Option<(Type, Span)>,
    /// The span of the value that first set `first`.
    first_value: Span,
    /// Whether `first` went from a `T` to a `T?`, so the `T` values before need wrapping.
    widened: bool,
    /// The values of objects that did not join the first one yet, by span, with their types. A
    /// later `any` value can join them, so the loop reports them when its body is checked.
    deferred: Vec<(Span, Type)>,
    /// Whether a `break` inside a nested loop has targeted it, so its values lie in one.
    crossed: bool,
    /// The first bare `break`.
    bare: Option<Span>,
    /// The first `break` whose value's type is not `never`.
    valued: Option<Span>,
    /// The token of the error that failed the loop: a failed jump, loop label or `break` value
    /// in it. It then has no type.
    failed: Option<Reported>,
}

impl LoopFrame {
    fn new(
        kind: LoopKind,
        keyword: Span,
        label: Option<ast::Ident>,
        expected: Expectation,
    ) -> Self {
        Self {
            kind,
            keyword,
            label,
            label_used: false,
            broken: false,
            expected,
            first: None,
            first_value: keyword,
            bare: None,
            valued: None,
            failed: None,
            widened: false,
            deferred: Vec::new(),
            crossed: false,
        }
    }

    /// The type of a `loop` whose body has been checked: `never` with no `break`, or with only
    /// `never` values; `unit` with bare ones; else its values'. `None` is `Error`.
    fn loop_type(&self) -> Option<Type> {
        if self.failed.is_some() {
            return None;
        }
        if self.valued.is_none() {
            return Some(if self.bare.is_some() {
                Type::UNIT
            } else {
                Type::NEVER
            });
        }
        // Under a `Poisoned` expectation no value sets `first`.
        self.first.map(|(ty, _)| ty)
    }
}

/// A held report of `Checker::interface_as_type`.
struct DeferredAsType {
    interface: tir::InterfaceId,
    name: String,
    /// The type as written.
    written: String,
    at: Span,
    /// The number of `?` after the name.
    layers: usize,
    /// Whether the name is in an alias body.
    alias: bool,
}

/// The program's text and the positions the lexer and parser found in it; never changes.
#[derive(Clone, Copy)]
struct Source<'src> {
    /// The text the user's spans index.
    user: source::Source<'src>,
    /// The text the prelude's spans index.
    prelude: source::Source<'src>,
    /// Where each comment starts. This list, `quotes`, `breaks` and `newlines` hold the user's
    /// file only, so for a span of the prelude they find no comment, no literal and one line.
    /// Only diagnostics read them.
    comments: &'src [usize],
    /// Where each string literal starts, in order.
    quotes: &'src [usize],
    /// Where each line break is, in order.
    breaks: &'src [usize],
    /// Where each line break that ends a statement is, in order.
    newlines: &'src [usize],
}

impl<'src> Source<'src> {
    /// The text that `span` covers.
    fn text(&self, span: Span) -> &'src str {
        self.holding(span.start()).text(span)
    }

    /// The type that `span` covers, on one line: see `source::Source::type_text`.
    fn type_text(&self, span: Span) -> Cow<'src, str> {
        self.holding(span.start()).type_text(span)
    }

    /// The text that `span` covers, on one line and with no comment: see
    /// `source::Source::line_text`.
    fn line_text(&self, span: Span) -> String {
        self.holding(span.start())
            .line_text(span, self.comments, self.newlines)
            .into_owned()
    }

    /// The text between the offsets of `range`, which lie in one text.
    fn slice(&self, range: Range<usize>) -> &'src str {
        self.holding(range.start).slice(range)
    }

    /// The text from offset `from` to the end of the text that holds it.
    fn rest(&self, from: usize) -> &'src str {
        self.holding(from).rest(from)
    }

    /// The text that holds offset `at`: the prelude's from `PRELUDE_BASE` on, else the user's.
    /// A span of the prelude must never index the user's text, or the other way round.
    fn holding(&self, at: usize) -> source::Source<'src> {
        if Span::new(at, at).in_prelude() {
            self.prelude
        } else {
            self.user
        }
    }
}

#[cfg(test)]
thread_local! {
    /// How many class fields the class-cycle search has read on this thread.
    pub(super) static FIELD_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many comment starts `comment_in` has read on this thread, in its search and in the
    /// check after it.
    pub(super) static COMMENT_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many expressions `Checker::expr` has checked on this thread.
    pub(super) static EXPR_CHECKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many unary `-` and operator expressions `int_literal` has walked on this thread, those
    /// it answers from `body.int_literals` not counted.
    pub(super) static INT_LITERAL_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many nested loops the walk that wraps a widened loop's values has entered.
    pub(crate) static WRAP_LOOPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many values `wrap_breaks` has wrapped in a walk that enters no nested loop, on this
    /// thread.
    pub(crate) static WIDENED_BREAKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many times `object_in_some` has been called on this thread.
    pub(crate) static OBJECT_IN_SOME_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many calls written `.push` have read a place of a type that is no array, on this
    /// thread.
    pub(crate) static OWN_PUSH_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many steps of a struct path `try_unread`, `is_path`, `read_pushed`, `struct_walk` and
    /// `first_safe` have walked on this thread.
    pub(super) static PATH_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many times `pushed_value` has been called on this thread.
    pub(crate) static PUSHED_VALUES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many type parameter names have been indexed or looked up on this thread.
    pub(super) static PARAM_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many users of an alias `unknown_alias` has visited on this thread.
    pub(super) static ALIAS_USER_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many alias steps `structs::extended` has taken on this thread.
    pub(super) static EXTENDED_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many times `binding` has walked the open scopes for a name on this thread.
    pub(super) static BINDING_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many inherited interfaces the check for a requirement that an interface declares
    /// again has visited on this thread.
    pub(super) static INHERIT_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many candidate results a call that picks among conformance blocks has read.
    pub(super) static PICK_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many interfaces the closures of a type's conformances have put in a visited set, read
    /// as an inherited one, or traced back to the interface that inherits them.
    pub(super) static CLOSURE_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many calls the lookups of a class's field by name have made. The tick counts calls of
    /// the one lookup helper, `ClassInfo::field`, and not the steps inside it.
    pub(super) static FIELD_LOOKUPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many methods of the prelude's blocks of built-in types a report of an unknown method
    /// has read on this thread.
    pub(super) static PRELUDE_NAME_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many times `conforms` has looked for the conformance of a class or an enum, not
    /// counting an answer it kept.
    pub(super) static CONFORMS_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The most steps `conforms` may count on this thread: the step that passes it panics, so a
    /// test of a query that would take too many steps fails at once.
    pub(super) static CONFORMS_BUDGET: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) };
    /// The depth at which `conforms` stops on this thread. A test lowers it: a query that goes
    /// `CONFORMS_DEPTH_LIMIT` deep takes about a second in a debug build.
    pub(super) static CONFORMS_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(interface::CONFORMS_DEPTH_LIMIT) };
    /// How many times `failing_leaf` has read a class or an enum for the part that fails, not
    /// counting an answer it kept.
    pub(super) static LEAF_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many steps `blocked_part` has taken, one for each written conformance it goes into.
    pub(super) static BLOCKED_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many conformances and choices the checks of the choices have read, or looked up by
    /// the name of an associated type, on this thread.
    pub(super) static CHOICE_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many times a bound that writes an associated type of a parameter has been tried.
    pub(super) static BOUND_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many declarations of associated types `names_of` has built for an interface, with the
    /// ones it inherits, on this thread.
    pub(super) static DECLARED_BUILT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many declarations of an interface `probe_associated` has read, on this thread.
    pub(super) static PROBE_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many types `nesting` has visited on this thread, including those it took from its memo.
    pub(super) static NESTING_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many scopes (type parameters with their bounds) have been kept on this thread.
    pub(super) static SCOPES_KEPT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many closure entries have been indexed, and how many entries of an interface have been
    /// read for the name of an associated type, on this thread.
    pub(super) static LOOKUP_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many dropped locals the rollback of a string join has visited on this thread.
    pub(super) static ROLLBACK_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The points of the checker that this thread has reached since `Walk::program` cleared them.
    pub(crate) static POINTS: std::cell::RefCell<FxHashSet<&'static str>> = std::cell::RefCell::default();
}

/// Records that the checker reached `point` on this thread.
#[cfg(test)]
pub(crate) fn reached(point: &'static str) {
    POINTS.with_borrow_mut(|points| {
        points.insert(point);
    });
}

#[expect(
    clippy::zero_sized_map_values,
    reason = "the map holds the proof of a report for each key, and a set cannot hold it"
)]
struct Checker<'src> {
    /// The program's text and the positions found in it.
    source: Source<'src>,
    /// The top-level names, and the items that they stand for.
    names: NameMaps,
    /// The names of the prelude's free functions, which no item of the user's may take
    /// (`collect_names`).
    prelude_functions: FxHashSet<Symbol>,
    /// Indexed by `FuncId`, so a duplicate function has a signature of its own.
    signatures: Vec<Signature>,
    /// Indexed by `ClassId`, so a duplicate class has fields and methods of its own.
    classes: Vec<ClassInfo>,
    /// Indexed by `StructId`, so a duplicate struct has fields and methods of its own.
    structs: Vec<ClassInfo>,
    /// Indexed by `EnumId`, so a duplicate enum has variants of its own.
    enums: Vec<EnumInfo>,
    /// Indexed by `InterfaceId`, so a duplicate interface has requirements of its own.
    interfaces: Vec<InterfaceInfo>,
    /// For each method that meets a requirement, its interfaces: a call of it uses them.
    meets: FxHashMap<tir::FuncId, Vec<tir::InterfaceId>>,
    /// The methods that a conformance names for a requirement, also those that do not meet it. A
    /// `mut` parameter of one is the requirement's, or E0152 reports it: W0002 leaves it alone.
    named_for_requirement: FxHashSet<tir::FuncId>,
    /// For each method name, the interfaces that declare a requirement of it.
    declaring: FxHashMap<Symbol, Vec<tir::InterfaceId>>,
    /// For each name of an associated type, the interfaces that declare one of it, with its place
    /// among theirs.
    declaring_assoc: FxHashMap<Symbol, Vec<(tir::InterfaceId, usize)>>,
    /// The place of the associated type of a name among those of an interface, if it declares one.
    assoc_at: FxHashMap<(tir::InterfaceId, Symbol), usize>,
    /// The bound checks of types resolved before every conformance is known; `None` once they
    /// run, and later types are checked as they are resolved.
    pending_bounds: Option<Vec<PendingBound>>,
    /// The type parameters in scope where a pending check was queued.
    scopes: Vec<Scope>,
    /// The pending checks while `run_pending_bounds` runs them.
    waiting: Option<Waiting>,
    /// Where each bound is written whose closure went past the limit and was reported.
    wide_reported: FxHashSet<Span>,
    /// The interfaces written where a type belongs before every interface has its requirements:
    /// the report waits to say whether the interface can be `any`.
    deferred_as_type: Vec<DeferredAsType>,
    /// Why each interface cannot be `any`, `None` when it can (`why_not_any`).
    any_roots: FxHashMap<tir::InterfaceId, Option<any::AnyReason>>,
    /// The same for each interface with its arguments in the walk of one root (`entry_reason`).
    any_entries: FxHashMap<(tir::InterfaceId, TypeArgs), Option<any::AnyReason>>,
    /// The interfaces that each `any` type conforms to (`any_interfaces`).
    any_closures: FxHashMap<Type, any::AnyClosure>,
    /// Where each `any` type is first written in the user's program.
    any_written: FxHashMap<Type, Span>,
    /// A buffer for the walk of an interface closure, kept from one closure to the next.
    closure_seen: FxHashSet<(tir::InterfaceId, TypeArgs)>,
    /// Each `extend` block whose target is a class or an enum, in source order.
    extensions: Vec<Extension>,
    /// For each class or enum and method name, the blocks that hold a method of that name, and
    /// the method, so that a method call reads only the blocks that hold its name.
    extension_methods: FxHashMap<(Holder, Symbol), Vec<(usize, tir::FuncId)>>,
    /// The methods that the prelude's blocks give an array or an `i64`, each with its holder and
    /// block. A report of an unknown method reads these, not the methods of every block.
    builtin_methods: Vec<(Holder, Symbol, usize)>,
    /// The names of the types that a broken `extend` block, or a block of a conformance that
    /// failed, may have given methods to. Each name holds the proof of its report. A method no one
    /// finds on them is poisoned.
    broken_extensions: FxHashMap<Symbol, Reported>,
    /// The methods that a block of an intact target lost to a syntax error.
    broken_extension_methods: FxHashMap<(Holder, Symbol), Reported>,
    /// The names of the methods of each `extend` block whose target the checker rejected. Such a
    /// block may have given any type a method of its name, so a call of that name on any type is
    /// poisoned, with no report. The set holds names, not types. The target can be a type
    /// parameter, which stands for every type. It can also be a type that `Holder` does not
    /// name, such as `f64`, `string` or `bool`. The block's own error is in the program. So the
    /// set can hide a report on another type that has a member of the same name: a missing
    /// method (E0063), a field that is not a method (E0058), or a variant that is not a method
    /// (E0120). That costs nothing. Each name holds the proof of the block's report.
    rejected_methods: FxHashMap<Symbol, Reported>,
    /// The methods of the conformance blocks that cover the receiver of the call being found, with
    /// the block's interface and its arguments for the receiver: a buffer that `find_method` fills
    /// and clears per call.
    candidates: Vec<(tir::FuncId, tir::InterfaceId, TypeArgs)>,
    /// Every conformance of a class or an enum to an interface.
    conformances: tir::Conformances,
    /// The type parameters of each conformance that has bounded ones, with their bounds, by
    /// interface and place among the interface's: the class's or the block's.
    conformance_bounds: FxHashMap<(tir::InterfaceId, usize), ParamBounds>,
    /// Where each choice that writes an associated type or a type parameter is written, by
    /// conformance (interface and place among the interface's) and place of the associated type
    /// among the interface's.
    choice_ats: FxHashMap<tir::ChoicePlace, Span>,
    /// The choices that `normalize` found to need their own associated type, or too many steps,
    /// each with the first type found for it and why; reported once all bodies are checked (see
    /// `report_loops`).
    loops: std::cell::RefCell<StuckChoices>,
    /// The associated types that a bound of each interface gives its type, with the ones that the
    /// interfaces it inherits declare, indexed by name: built when first asked for, once every
    /// interface has its inheritance (`interfaces_resolved`), and kept for the program.
    declared: std::cell::RefCell<FxHashMap<tir::InterfaceId, Rc<interface::Names>>>,
    /// Whether pass 1b is done: every interface has its inheritance, so what an interface
    /// declares will not change.
    interfaces_resolved: bool,
    /// The types that `probe_associated` has probed for a bound, by interface and arguments: a
    /// type is probed once for a bound, however many calls ask.
    probed_types: FxHashSet<(Type, tir::InterfaceId, TypeArgs)>,
    /// The associated types that `probe_associated` has normalized: two bounds that inherit one
    /// interface give it once.
    probed_assoc: FxHashSet<Type>,
    /// How many associated types the probe of the bound being checked may still normalize.
    probe_budget: usize,
    /// The answers of `conforms` for a type with no type parameter, which no body changes.
    conformed: FxHashMap<(Type, tir::InterfaceId, TypeArgs), bool>,
    /// The part that makes a class or an enum with no type parameter fail an interface (`None`
    /// when it holds), as `failing_leaf` found it.
    leaves: LeafAnswers,
    /// The classes and enums whose queries went past `CONFORMS_DEPTH_LIMIT` and were reported.
    too_deep: FxHashSet<Holder>,
    /// The language interfaces, found by name once the prelude's names are collected.
    lang: tir::LangInterfaces,
    /// The state of the root query of `conforms` that is open, if one is.
    open: conformance::Open,
    /// By class and then enum, whether the declaration has the never-built error.
    unbuilt: Vec<bool>,
    /// By `EnumId`, whether the enum grows (`enum Nest<T> { End\n Next(Nest<[T]>) }`, E0140) or
    /// no value can be built of it (the never-built error): the generated rule answers `true`
    /// for it with no part query, so its own error is its only diagnostic.
    marked: Vec<bool>,
    /// By `ClassId`, whether the class grows (`class Nest<T> { next: Nest<[T]>? }`, E0140). A
    /// query still expands it, but a query that is too deep at it gets no second report.
    growing_classes: Vec<bool>,
    /// Whether the type of a field of a class or a payload of an enum holds an associated type:
    /// the choices of the associated types matter to the cycles only then. Set before the marks.
    holds_assoc: bool,
    /// The place among `extensions` of the first block that covers the receiver of the call being
    /// found and whose bounds fail, when `conforming_methods` finds no method for it, with that
    /// block's method of the called name.
    failed_block: Option<(usize, tir::FuncId)>,
    /// The enums as `tir` holds them, with their boxes and countedness; bodies are checked
    /// after they are lowered.
    lowered_enums: Vec<tir::Enum>,
    /// While a field's or payload's type resolves, the class and enum types in it and where each
    /// is written; `None` otherwise.
    written: Option<Vec<(Type, Span)>>,
    /// Indexed by `ConstantId`, which is the source order among the constants.
    const_infos: Vec<ConstInfo>,
    /// The type aliases, which share the namespace of the items above.
    aliases: Aliases,
    /// What a value of each enum and class may hold, for W0011.
    holds_table: HoldsTable,
    diagnostics: Diagnostics,
    /// The non-empty spans of the parser's errors, sorted by start.
    parse_errors: Vec<Span>,
    /// The body being checked.
    body: Body,
    /// While `signature` resolves the parameter and return types: the interface of each `any`
    /// type they write, with the span as written. `record` keeps a use here when the body has
    /// no owner, because a signature is resolved outside a body. No other body is resolved while
    /// it is `Some`: `resolve_aliases` resolves each alias body before `collect_signatures`.
    signature_anys: Option<Vec<(tir::InterfaceId, Span)>>,
    /// Each use of an item that a function, method or constant has, in check order.
    uses: Vec<Use>,
    /// Each binding of a function or a method that nothing reads, in check order.
    pending: Vec<Pending>,
    /// Whether a type argument was recorded: see `tir::Program::instantiates`.
    instantiates: bool,
    /// Each `dbg` call: the body that holds it, if any, the type of its operand, and its span.
    /// W0013 reads them: `dbg` shows a field, and that is not a read.
    dbg_shown: Vec<(Option<Owner>, Type, Span)>,
    /// How many classes, enums and functions the user's items declare.
    prelude: tir::ItemCounts,
    /// By the span of an expression, how the program wrote it when the typed tree does not say
    /// so (`Lowered`): the cycle check reads it for its labels.
    spellings: FxHashMap<Span, Lowered>,
}

/// What the typed tree forgets about an expression that the checker lowered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lowered {
    /// A method call (`x.eq(y)` or `x.to_string()`) on a type whose conformance the compiler
    /// writes, lowered to the node of the operator.
    Method,
    /// An operator, `==` or `<` for example, on a type with a written conformance, lowered to a
    /// call of its method. It is a call through the language interface given.
    Operator(tir::InterfaceId, &'static str),
}

/// The entries of the closure of a type parameter or an associated type, by interface.
#[derive(Default)]
struct ClosureIndex {
    /// How many entries of the closure are indexed.
    done: usize,
    /// For each interface, its entries with their places in the closure.
    by_interface: FxHashMap<tir::InterfaceId, Vec<(usize, TypeArgs)>>,
}

/// The part that makes a class or an enum fail an interface (`None` when it holds), as
/// `failing_leaf` found it, by the type, the interface and its arguments. Each answer is final:
/// any walk may use it.
type LeafKey = (Type, tir::InterfaceId, TypeArgs);
type LeafAnswers = FxHashMap<LeafKey, Option<Type>>;

/// The E0050 of a `None` or `[]` that a `let` initializes. A later use of the local can give the
/// type that the help of the E0050 writes.
struct Uninferred {
    /// Where the E0050 is in the diagnostics.
    at: usize,
    /// The span of the literal, which the E0050 reports.
    span: Span,
    literal: Literal,
    /// What the `let` writes before its type: `mut`, then the name.
    binding: String,
    /// The place in the source of the use that set the help, if one did (see
    /// `Checker::inferred_by_use`).
    settled: Option<usize>,
}

/// What `self` and `Self` mean in the code being checked.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
enum SelfType {
    /// Outside a method: `self` and `Self` are errors (E0041, E0149).
    #[default]
    Absent,
    /// In a block that failed (its target, its interface or its parameters): the block reports it,
    /// so `self` and `Self` give no error.
    Failed,
    Known(Type),
}

impl SelfType {
    /// The type, unless there is none or it failed.
    fn known(self) -> Option<Type> {
        match self {
            Self::Known(ty) => Some(ty),
            Self::Absent | Self::Failed => None,
        }
    }
}

/// What checking one body needs: a function's, a method's, or a constant's initialiser.
#[derive(Default)]
#[expect(
    clippy::zero_sized_map_values,
    reason = "the map holds the proof of a report for each key, and a set cannot hold it"
)]
struct Body {
    /// The function, method or constant whose body this is; `None` for none, such as a signature.
    owner: Option<Owner>,
    /// The type parameters in scope, which `resolve` finds before any other type.
    type_params: Vec<Symbol>,
    /// Where in `type_params` each name first is, so a lookup does not scan them.
    type_param_at: FxHashMap<Symbol, usize>,
    /// The bounds of the type parameters in scope, by place; a place past the end has none.
    bounds: Vec<Bounds>,
    /// The places of the type parameters that are unknown in this body: a closure past
    /// `BOUND_CLOSURE_LIMIT` (reported as E0157), or a bound that failed (reported where it is
    /// written). They conform to every interface, and a method call on them is untyped.
    unknown: Vec<usize>,
    /// The interfaces each type parameter conforms to, by place, once found (`param_interfaces`).
    conforming: Vec<Option<Vec<(tir::InterfaceId, TypeArgs)>>>,
    /// The interfaces each associated type of a type parameter conforms to through the bounds of
    /// its declaration, once found (`assoc_interfaces`). The bounds of the type parameters in scope
    /// do not change them, so a body keeps them as it goes. The signature of an interface forgets
    /// them after its inheritance list.
    assoc_conforming: FxHashMap<Type, Vec<(tir::InterfaceId, TypeArgs)>>,
    /// The associated types that are unknown in this body, for the same two causes as `unknown`.
    unknown_assoc: FxHashSet<Type>,
    /// The requirements of the bounds of each type parameter or associated type by name, once
    /// indexed (`required`). All these caches change with the bounds, and a body's are cleared
    /// with it.
    required: FxHashMap<Type, FxHashMap<Symbol, Vec<Required>>>,
    /// For each type parameter or associated type, how many entries of its closure are indexed,
    /// and the entries by interface, each with its place in the closure (`index_closure`).
    named: FxHashMap<Type, ClosureIndex>,
    /// The associated types that a bare name stands for: the interface's own in an interface,
    /// the conformance's choices in a class, an enum or an `extend` block. `None` is `Error`.
    assoc: Rc<Choices>,
    /// The names of the associated types whose choices are being resolved: a choice cannot name
    /// one of them.
    choosing: Vec<Symbol>,
    /// The checks that the body of the alias being resolved leaves to each use of the alias, in
    /// order; `None` outside an alias body, where a type parameter is an alias's parameter, with
    /// no name.
    alias: Option<Vec<UseCheck>>,
    /// The type that each expression is checked against as its declaration writes it, by the
    /// expression's span, when the type names an alias: a mismatch shows the alias.
    shown: FxHashMap<Span, (Type, Rc<str>)>,
    /// The return type as the function writes it, under the same rule.
    ret_shown: Option<Rc<str>>,
    /// The answers of `conforms` for a type that holds a type parameter: they change with the
    /// bounds, and a body's are cleared with them.
    conformed: FxHashMap<(Type, tir::InterfaceId, TypeArgs), bool>,
    /// The answers of `failing_leaf` for a type that holds a type parameter, as `conformed` keeps
    /// the answers of `conforms`: they change with the bounds, and are cleared with them.
    leaves: LeafAnswers,
    /// Where in `Checker.scopes` the type parameters now in scope are, once a check waits.
    scope: Option<usize>,
    /// Where each call of a value, `a<b, c>(d)`, that has been reported starts, with the token of
    /// the report: comparisons misread as a call change the count of the arguments of the call
    /// they are an argument of, which is then not reported.
    misread: FxHashMap<usize, Reported>,
    scopes: Vec<FxHashMap<Symbol, Binding>>,
    locals: Vec<tir::Local>,
    /// `None` is `Error`.
    ret: Option<Type>,
    /// What `self` is in the code being checked (see `SelfType`); when known, `self` is local 0.
    self_type: SelfType,
    /// Loops open around the code being checked, innermost last: what a `break` would act on.
    loops: Vec<LoopContext>,
    /// The waiting generic call that the outer call is checking as an argument.
    context_hint: Option<generic::ContextHint>,
    /// The E0050 of each generic call whose result cannot meet its context (see `Misfit`). A
    /// report that uses the result takes its entry. Each entry left is pushed when the body ends.
    misfits: Vec<Option<Diagnostic>>,
    /// The safe method call being checked: `receiver?.method(...)` at this span.
    safe_call: Option<Span>,
    /// Set by a `break` or `continue` outside a loop; `scoped_block` reads it per statement.
    invalid_jump: bool,
    /// The last string literal that is `never` because a part is: its span, and the span of the
    /// first such part, which E0043, E0138 and E0061 underline (see `Checker::never_at`).
    never_part: Option<(Span, Span)>,
    /// Where each loop label is declared that a failed jump or a reused loop label names, which
    /// is not reported unused: its own error is.
    failed_labels: FxHashSet<Span>,
    /// The bare name right after an unlabeled `break`, which may be a misspelled loop label, and
    /// the byte where that `break` starts: only there does E0033 offer the open loop labels.
    label_typo: Option<(Span, usize)>,
    /// Where the value of the unlabeled `break` being checked starts: the parser reads an open
    /// loop label there as the `break`'s label, so a fix must not write one there.
    break_start: Option<usize>,
    /// Where the initializer of a `let` is, whose annotation failed: a constructor there does not
    /// check its bounds, as its errors would follow from the annotation's.
    failed_let: Option<Span>,
    /// Whether an index follows the expression being checked in its chain (field bases, method
    /// receivers, indexed bases). A `?.` that fixes a `.` there leads to an index on an optional.
    indexed: bool,
    /// The span and the change of the write whose chain holds the expression being checked: its
    /// field bases and indexed bases up to the written place. A `?.` on a struct value there
    /// leads to a write that part 1 cannot make.
    write_chain: Option<(Span, place::Change)>,
    /// The initializer of the `let` being checked, and what that `let` writes before its type
    /// (`mut`, then the name): E0050 uses it in its example when the initializer is the whole
    /// `None` or `[]`.
    let_init: Option<(Span, Symbol, bool)>,
    /// The E0050 of each local whose initializer is such a literal, until a later use gives the
    /// local a type (see `Checker::inferred_by_use`).
    uninferred: FxHashMap<tir::LocalId, Uninferred>,
    /// The constant whose value is being checked, where interpolation is not known and a
    /// conversion that can fail is an error.
    in_constant: Option<ConstantSite>,
    /// Each `+` found to join a `string` under an expected `u8`, which a later check of it
    /// checks with no expected type.
    joins: FxHashSet<Span>,
    /// How many writes the body has checked; a binding's `written` is this count at its last.
    writes: u32,
    /// `writes` at the last write rooted at `self`; 0 for none.
    self_written: u32,
    /// How many `break`, `continue`, `return`, `panic` and `todo` the body has checked.
    exits: u32,
    /// How many W0005 (a comparison with `f64.nan`) the body has reported.
    nans: u32,
    /// The method names, by span, of the calls that `call_of_rejected_method` answered. Such a
    /// call has no report, but its member is unknown: a list of type arguments on it is not
    /// reported (`method_call_expr`).
    rejected_calls: FxHashSet<Span>,
    /// The span of the method name of each lookup that failed and was reported at the name.
    /// The method is missing, or it is a field or a variant (`report_lookup_failure`). The
    /// check of a `.` on an optional reads it, to report E0062 only when the member is found.
    failed_lookups: FxHashSet<Span>,
    /// The span of each `Some` that gave an error: it is never `None`, whatever type the error
    /// gives it, so printing it gets no W0015 (`warn_if_optional`).
    failed_some_spans: FxHashSet<Span>,
    /// The span of each argument value that is written with `mut`, or is at a `mut` parameter of
    /// a callee that the call knows. `expr` checks such a value as the base of an index
    /// (`indexed`). The `?.` fix of an E0062 in it leads to a path that is no place (E0170).
    mut_values: FxHashSet<Span>,
    /// The target of the compound assignment being checked, whose reads are not reads: they
    /// only give the value back to the target.
    quiet: Option<Span>,
    /// The receiver of a `push` that `push_receiver` checked as a place, at its span, until the
    /// method call takes it.
    pushed: Option<(Span, place::CheckedPlace)>,
    /// Whether the body has a `todo`, which keeps its parameters from W0014.
    todo: bool,
    /// Work ranking arguments did ahead of its turn, innermost last, by the span it was done
    /// for and whether that is a block's; `None` once used. Then whether the work made a `break`
    /// or `continue` outside a loop: using the work sets `invalid_jump` again.
    ahead: Vec<((Span, bool), Option<generic::Ahead>, bool)>,
    /// Where in `ahead` the last work kept for each span and kind is.
    ahead_at: FxHashMap<(Span, bool), usize>,
    /// What `takes_context` answered for each array literal and `Some(...)`, with the context,
    /// by the span of the expression.
    taken: FxHashMap<(Span, Type), bool>,
    /// What `rank_value` answered for each expression, by its span, and whether ranking it
    /// cleared `fit`. A span is checked in one scope only (there are no closures yet), so the
    /// answer holds for every call that ranks it.
    ranked: FxHashMap<Span, (generic::Ranked, bool)>,
    /// What `int_literal` answered for each unary `-` and operator, by its span. The answer
    /// depends only on the node, and no two operator nodes have one span.
    int_literals: FxHashMap<Span, bool>,
}

impl Body {
    /// Puts `params` in scope, in place of the type parameters there.
    fn set_type_params(&mut self, params: &[Symbol]) {
        self.forget_bounds(0);
        self.type_params.clear();
        self.type_params.extend_from_slice(params);
        self.type_param_at.clear();
        // Backwards, so a repeated name keeps its first place, as the scan it replaces did.
        for (i, &name) in params.iter().enumerate().rev() {
            #[cfg(test)]
            PARAM_STEPS.set(PARAM_STEPS.get() + 1);
            self.type_param_at.insert(name, i);
        }
    }

    /// Puts `params` after the type parameters in scope; a name in scope keeps its place.
    fn push_type_params(&mut self, params: &[Symbol]) {
        self.forget_bounds(self.type_params.len());
        for &name in params {
            #[cfg(test)]
            PARAM_STEPS.set(PARAM_STEPS.get() + 1);
            self.type_param_at
                .entry(name)
                .or_insert(self.type_params.len());
            self.type_params.push(name);
        }
    }

    /// Drops the type parameters after the first `len`, which `push_type_params` added.
    fn truncate_type_params(&mut self, len: usize) {
        self.forget_bounds(len);
        for at in (len..self.type_params.len()).rev() {
            if self.type_param_at.get(&self.type_params[at]) == Some(&at) {
                self.type_param_at.remove(&self.type_params[at]);
            }
        }
        self.type_params.truncate(len);
    }

    /// Drops the bounds from the place `first` on, and what was found from them.
    fn forget_bounds(&mut self, first: usize) {
        self.bounds.truncate(first);
        self.conforming.clear();
        self.unknown.clear();
        self.required.clear();
        self.named.clear();
        self.conformed.clear();
        self.leaves.clear();
        self.scope = None;
    }

    /// Gives the type parameters from the place `first` on their `bounds`.
    fn set_bounds(&mut self, first: usize, bounds: Vec<Bounds>) {
        self.forget_bounds(first);
        self.bounds.resize(first, Bounds::default());
        self.bounds.extend(bounds);
    }
}

/// Whether the parser marked an item, or a method of one, as broken.
fn parser_marked(program: &ast::Program, items: &[&ast::Item]) -> bool {
    !program.broken.is_empty()
        || items.iter().any(|item| match item {
            ast::Item::Class(class) => !class.broken_methods.is_empty(),
            ast::Item::Struct(s) => !s.broken_methods.is_empty(),
            ast::Item::Enum(e) => !e.broken_methods.is_empty(),
            ast::Item::Extend(extend) => !extend.broken_methods.is_empty(),
            // The checker reads no requirement that an interface lost to a syntax error: a call of
            // one finds no method and gets an error of its own, so no promise rests on the mark.
            ast::Item::Function(_)
            | ast::Item::Const(_)
            | ast::Item::Alias(_)
            | ast::Item::Interface(_) => false,
        })
}

/// Diagnostics sorted by span start (stable); the program only when none is an error. `parsed`
/// holds the parser's diagnostics, sorted by span start: their errors count against a function too.
/// The items of `prelude` follow those of `program`, so that every id of a user's item is what it
/// would be without a prelude. The prelude has no broken item, and no diagnostic should point into
/// it: `front_end` takes one that does for an internal error. `source` is the user's text, and
/// `prelude` holds the prelude's. The prelude's `Option` is no item here: it defines the optional
/// type, which has no enum.
#[expect(
    clippy::too_many_lines,
    reason = "the passes run in order, and the order is the design"
)]
// `scripts/scaling.py` reads this function's instruction count.
#[inline(never)]
pub(crate) fn check<'src>(
    program: &'src ast::Program,
    prelude: &'src PreludeProgram,
    source: &'src str,
    parsed: &[Diagnostic],
) -> (Option<tir::Program>, Vec<Diagnostic>) {
    let mut checker = Checker::new(program, source, &prelude.text);
    checker.parse_errors = error_spans(parsed);
    let option =
        |item: &&ast::Item| matches!(item, ast::Item::Enum(e) if e.name.name == Symbol::OPTION);
    let items: Vec<&ast::Item> = (program.items.iter())
        .chain(prelude.program.items.iter().filter(|item| !option(item)))
        .collect();
    let declared = checker.declare_items(&items, program.items.len(), &program.broken);
    checker.fold_constants(&items);
    checker.lowered_enums = checker.lower_enums();
    for d in payload_limit::diagnostics(&checker.lowered_enums, &[]) {
        checker.diagnostics.error(d);
    }
    checker.holds_table = checker.holds_table();
    checker.check_main(&declared);
    // Every body is checked, so this collects into a `Vec` before it gives up on a `None`.
    let functions: Vec<_> = (declared.iter().enumerate())
        .map(|(i, &(f, owner))| checker.method_or_function(f, tir::FuncId(i), owner))
        .collect();
    let unbuilt = std::mem::take(&mut checker.unbuilt);
    checker.instantiation_cycles(&functions, &unbuilt);
    checker.report_loops();
    let classes = checker.classes.iter().map(lower_class).collect();
    let structs = checker.structs.iter().map(structs::lower_struct).collect();
    let broken = parser_marked(program, &items);
    debug_assert!(
        checker.diagnostics.promises_kept(broken),
        "a promise of an error was not kept"
    );
    let mut diagnostics = std::mem::take(&mut checker.diagnostics).finish();
    diagnostics.sort_by_key(order);
    let errors = error_spans(&diagnostics);
    // A function with an error of its own is skipped, so that its errors stand alone.
    let warnings: Vec<Diagnostic> = functions
        .iter()
        .enumerate()
        .filter_map(|(i, function)| {
            let (f, owner) = declared[i];
            let function = function.as_ref()?;
            if starts_in(&errors, f.span) || starts_in(&checker.parse_errors, f.span) {
                return None;
            }
            let calls = recursion::unconditional(function, tir::FuncId(i))?;
            let kind = if owner == SelfType::Absent {
                Definition::Function
            } else {
                Definition::Method
            };
            Some(
                DiagnosticKind::UnconditionalRecursion {
                    kind,
                    name: f.name.name.to_string(),
                    at: f.name.span,
                    calls,
                }
                .into(),
            )
        })
        .collect();
    diagnostics.extend(warnings);
    // A stopgap until structs lower: a struct that the checker takes must not reach the mir.
    let erred = |ds: &[Diagnostic]| ds.iter().any(|d| d.severity() == Severity::Error);
    if !erred(&diagnostics)
        && !erred(parsed)
        && let Some(at) = program.items.iter().find_map(|item| match item {
            ast::Item::Struct(s) => Some(s.keyword),
            _ => None,
        })
    {
        diagnostics.push(DiagnosticKind::ReservedWord { word: "struct", at }.into());
    }
    diagnostics.sort_by_key(order);
    let failed = diagnostics.iter().any(|d| d.severity() == Severity::Error);
    // A use in a failed expression may not be recorded, so an error stops the pass. Only the
    // parser sees an item that fails to parse.
    if !failed && !parsed.iter().any(|d| d.severity() == Severity::Error) {
        diagnostics.extend(checker.unused());
    } else {
        diagnostics.extend(checker.unjudged());
    }
    diagnostics.sort_by_key(order);
    let interfaces = checker.lowered_interfaces();
    let enums = checker.lowered_enums;
    let (instantiates, prelude, lang) = (checker.instantiates, checker.prelude, checker.lang);
    let (conformances, choice_ats) = (checker.conformances, checker.choice_ats);
    let program = functions
        .into_iter()
        .collect::<Option<_>>()
        .zip(classes)
        .zip(structs)
        .filter(|_| !failed)
        .map(|((functions, classes), structs)| tir::Program {
            classes,
            structs,
            enums,
            functions,
            instantiates,
            fast_path: false,
            prelude,
            interfaces,
            conformances,
            choice_ats,
            lang,
            any_layouts: Vec::new(),
            any_tables: Vec::new(),
        });
    #[cfg(debug_assertions)]
    program.iter().for_each(tir::assert_exit_flags);
    // A use of a broken item poisons without a diagnostic of its own.
    debug_assert!(
        program.is_some() || failed || broken,
        "a missing program implies an error diagnostic or a broken item or method"
    );
    (program, diagnostics)
}

/// `check` over a program with no prelude but the language interfaces, for the tests of what
/// follows the checker: the prelude's templates would stay in a program that `mono` has not run
/// on.
#[cfg(test)]
pub(crate) fn check_alone(
    program: &ast::Program,
    source: &str,
    parsed: &[Diagnostic],
) -> (Option<tir::Program>, Vec<Diagnostic>) {
    check(
        program,
        &crate::front_end::prelude_program(""),
        source,
        parsed,
    )
}

/// The key that orders diagnostics: where each starts, and at one start each E0062 before the
/// rest, whichever check ran first.
fn order(diagnostic: &Diagnostic) -> (usize, bool) {
    (
        diagnostic.span().start(),
        diagnostic.code() != Code::MayBeNone,
    )
}

/// The non-empty spans of the errors in `diagnostics`, in order. `NoMain`'s empty span at offset
/// 0 is then not inside the function that starts the file.
fn error_spans(diagnostics: &[Diagnostic]) -> Vec<Span> {
    let errors = diagnostics
        .iter()
        .filter(|d| d.severity() == Severity::Error);
    let spans = errors.map(Diagnostic::span);
    spans.filter(|span| span.start() < span.end()).collect()
}

/// Whether a span of `spans`, which is sorted by start, starts inside `span`.
fn starts_in(spans: &[Span], span: Span) -> bool {
    let first = spans.partition_point(|s| s.start() < span.start());
    spans.get(first).is_some_and(|s| s.start() < span.end())
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
        span: class.name.span,
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

/// The type of `checked`, or the proof of the report that left it untyped.
fn known_type(checked: &Checked) -> Result<Type, Reported> {
    match checked {
        Ok(e) => Ok(e.ty),
        Err(p) => p.ty.ok_or(p.reported),
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
    #[expect(
        clippy::zero_sized_map_values,
        reason = "the map holds the proof of a report for each key, and a set cannot hold it"
    )]
    fn new(
        program: &'src ast::Program,
        source: &'src str,
        prelude_text: &'src str,
    ) -> Checker<'src> {
        // `comment_in` searches this list by position, so it must stay in source order. The check
        // is here, once, because it reads the whole list.
        debug_assert!(
            program.comments.is_sorted(),
            "the comment starts are not in order"
        );
        Checker {
            source: Source {
                user: source::Source::new(source, 0),
                prelude: source::Source::new(prelude_text, source::PRELUDE_BASE as usize),
                comments: &program.comments,
                quotes: &program.quotes,
                breaks: &program.breaks,
                newlines: &program.newlines,
            },
            names: NameMaps::default(),
            prelude_functions: FxHashSet::default(),
            signatures: Vec::new(),
            classes: Vec::new(),
            structs: Vec::new(),
            enums: Vec::new(),
            interfaces: Vec::new(),
            meets: FxHashMap::default(),
            named_for_requirement: FxHashSet::default(),
            declaring: FxHashMap::default(),
            declaring_assoc: FxHashMap::default(),
            assoc_at: FxHashMap::default(),
            pending_bounds: Some(Vec::new()),
            scopes: Vec::new(),
            waiting: None,
            wide_reported: FxHashSet::default(),
            deferred_as_type: Vec::new(),
            any_roots: FxHashMap::default(),
            any_entries: FxHashMap::default(),
            any_closures: FxHashMap::default(),
            any_written: FxHashMap::default(),
            closure_seen: FxHashSet::default(),
            extensions: Vec::new(),
            extension_methods: FxHashMap::default(),
            builtin_methods: Vec::new(),
            broken_extensions: FxHashMap::default(),
            broken_extension_methods: FxHashMap::default(),
            rejected_methods: FxHashMap::default(),
            candidates: Vec::new(),
            conformances: tir::Conformances::default(),
            conformance_bounds: FxHashMap::default(),
            choice_ats: FxHashMap::default(),
            loops: std::cell::RefCell::default(),
            declared: std::cell::RefCell::default(),
            interfaces_resolved: false,
            probed_types: FxHashSet::default(),
            probed_assoc: FxHashSet::default(),
            probe_budget: 0,
            conformed: FxHashMap::default(),
            leaves: FxHashMap::default(),
            too_deep: FxHashSet::default(),
            lang: tir::LangInterfaces::default(),
            open: conformance::Open::default(),
            unbuilt: Vec::new(),
            marked: Vec::new(),
            growing_classes: Vec::new(),
            holds_assoc: false,
            failed_block: None,
            lowered_enums: Vec::new(),
            spellings: FxHashMap::default(),
            written: None,
            const_infos: Vec::new(),
            aliases: Aliases::default(),
            holds_table: HoldsTable::default(),
            diagnostics: Diagnostics::default(),
            parse_errors: Vec::new(),
            body: Body::default(),
            signature_anys: None,
            uses: Vec::new(),
            pending: Vec::new(),
            instantiates: false,
            dbg_shown: Vec::new(),
            prelude: tir::ItemCounts::default(),
        }
    }

    /// Checks with `body` as the current body, then restores the one before it, and returns
    /// `f`'s result with the finished body (its `locals` become the function's).
    /// A misfit that no report used gives its E0050 then.
    fn with_body<R>(&mut self, body: Body, f: impl FnOnce(&mut Self) -> R) -> (R, Body) {
        let outer = std::mem::replace(&mut self.body, body);
        let result = f(self);
        let mut body = std::mem::replace(&mut self.body, outer);
        for d in body.misfits.drain(..).flatten() {
            self.diagnostics.error(d);
        }
        (result, body)
    }

    /// The type that `checked`, a failed generic call, found for its result and could not meet
    /// its context with (see `Misfit`), while no report has used it. A block gives its value's.
    fn misfit_type<T>(&self, checked: &Result<T, Poisoned>) -> Option<Type> {
        let misfit = checked.as_ref().err()?.found?;
        let open = self
            .body
            .misfits
            .get(misfit.entry)
            .is_some_and(Option::is_some);
        open.then_some(misfit.ty)
    }

    /// Marks the misfit of `checked`, if any, as reported: its E0050 is dropped.
    fn settle_misfit<T>(&mut self, checked: &Result<T, Poisoned>) {
        if let Err(Poisoned {
            found: Some(misfit),
            ..
        }) = checked
            && let Some(entry) = self.body.misfits.get_mut(misfit.entry)
        {
            *entry = None;
        }
    }
}

impl Checker<'_> {
    /// Passes 1 to 2b: the names, the signatures, the declarations that a query for a generated
    /// conformance must not expand, and every conformance. Returns each function with the type of
    /// a method's `self`.
    fn declare_items<'a>(
        &mut self,
        items: &[&'a ast::Item],
        user_items: usize,
        broken: &[ast::Broken],
    ) -> Vec<(&'a ast::Function, SelfType)> {
        self.collect_names(items, user_items, broken);
        self.resolve_aliases();
        self.lang = self.language_interfaces();
        self.interface_signatures(items);
        let declared = self.collect_signatures(items);
        // The first queries for a generated conformance come before the bodies, in
        // `conformances`, so the enums that a query must not expand are known first: those that
        // grow, and those no value can be built of. Each has its own error, which stands alone.
        // The enums that grow are marked in `conformances`, once the choices of the associated
        // types are recorded, which a growth through an associated type needs.
        let cyclic = self.never_constructed();
        self.unbuilt = self.never_built(&cyclic);
        self.marked = self.unbuilt[self.classes.len()..].to_vec();
        self.growing_classes = vec![false; self.classes.len()];
        self.read_declared_bounds();
        self.conformances(items);
        self.normalize_declarations();
        declared
    }

    /// The language interfaces: the prelude's `Display`, `Eq`, `Ord` and `AnyObject`. A prelude
    /// that lacks one is the compiler's fault: `prelude_program` puts them in every prelude.
    fn language_interfaces(&self) -> tir::LangInterfaces {
        let find = |name: &str| {
            let at = (self.interfaces.iter())
                .position(|info| info.name.span.in_prelude() && info.name.name.as_str() == name);
            tir::InterfaceId(at.expect("the prelude should declare the language interfaces"))
        };
        tir::LangInterfaces {
            display: find("Display"),
            eq: find("Eq"),
            ord: find("Ord"),
            any_object: find("AnyObject"),
        }
    }

    /// The interfaces as `tir` names them.
    fn lowered_interfaces(&self) -> Vec<tir::Interface> {
        (self.interfaces.iter())
            .map(|info| tir::Interface {
                name: info.name.name.to_string(),
                methods: (info.requirements.iter())
                    .map(|requirement| requirement.name.name.to_string())
                    .collect(),
                assoc: (info.assoc.iter())
                    .map(|assoc| assoc.name.name.to_string())
                    .collect(),
                inherits: (info.inherits.iter())
                    .map(|bound| (bound.interface, bound.args))
                    .collect(),
            })
            .collect()
    }

    /// Records a coded error.
    fn error(&mut self, kind: DiagnosticKind) -> Reported {
        self.diagnostics.error(kind)
    }

    /// Records a coded warning.
    fn warn(&mut self, kind: DiagnosticKind) {
        self.diagnostics.warn(kind);
    }

    /// Records a promise made at `at`. An error in the list keeps it, wherever the error is, and
    /// so does a parser mark (see `Diagnostics::promises_kept`).
    fn proof(&mut self, at: Span) -> Reported {
        self.diagnostics.proof(at)
    }

    /// The token of a use at `at` of a binding with no type. Its error was reported where it was
    /// bound: its initializer's, its declared type's, or an E0050. No binding holds that token.
    fn untyped_binding(&mut self, at: Span) -> Reported {
        self.proof(at) // proof: binding
    }

    /// The token of a value at `at` that a `Poisoned` expectation gives no type. The reader of
    /// the value stays quiet. The error of the context was reported before, or the caller
    /// reports it after: `checked_alone` gives no error before, and in `g(mut [])` its caller
    /// reports E0169.
    fn poisoned_context(&mut self, at: Span) -> Reported {
        self.proof(at) // proof: quiet reader
    }

    /// Pass 1: gives every class a `ClassId` and every enum an `EnumId`, and records the first
    /// function, class, enum and constant of each name, so that types, calls and uses can name
    /// any of them. They share a namespace. `FuncId`s run in source order, a class's or an
    /// enum's methods at its place.
    /// The first `user_items` of `items` are the user's: `self.prelude` is where the rest start.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per kind of item, so the names of all of them are collected in one place"
    )]
    fn collect_names(&mut self, items: &[&ast::Item], user_items: usize, broken: &[ast::Broken]) {
        self.prelude_functions = prelude_function_names(items[user_items..].iter().copied());
        let mut first = FxHashMap::default();
        // The top-level items that a prelude function's name rejected, and the items that
        // redeclare a name of the program.
        let mut rejected = Vec::new();
        let mut next_function = 0;
        let extended = structs::extended(items);
        let mut waiting = broken.iter().peekable();
        for (at, item) in items.iter().enumerate() {
            if at == user_items {
                // No broken item is in the prelude, and its items must see their names in `first`.
                self.take_broken(&mut waiting, None, &mut first);
                self.prelude = self.item_counts(next_function);
            } else if let Some(name) = item_name(item) {
                self.take_broken(&mut waiting, Some(name.span.start()), &mut first);
            }
            match item {
                ast::Item::Class(class) => {
                    let type_params = self.type_params(&[], &class.type_params);
                    let id = tir::ClassId(self.classes.len());
                    let bounded = class.type_params.iter().any(|p| !p.bounds.is_empty());
                    self.classes
                        .push(ClassInfo::new(class.name.clone(), type_params, bounded));
                    let name = &class.name;
                    if !self.built_in(name)
                        && self.first_callable(&mut first, &mut rejected, Definition::Class, name)
                    {
                        self.names.add_class(name.name, id);
                    }
                    next_function += class
                        .members
                        .iter()
                        .filter(|m| matches!(m, ast::Member::Method(_)))
                        .count();
                }
                ast::Item::Struct(s) => {
                    next_function += self.collect_struct(s, &extended, &mut first, &mut rejected);
                }
                ast::Item::Function(f) => {
                    if !self.built_in(&f.name)
                        && self.first_callable(
                            &mut first,
                            &mut rejected,
                            Definition::Function,
                            &f.name,
                        )
                    {
                        self.names
                            .add_function(f.name.name, tir::FuncId(next_function));
                    }
                    next_function += 1;
                }
                ast::Item::Enum(e) => {
                    let type_params = self.type_params(&[], &e.type_params);
                    let id = tir::EnumId(self.enums.len());
                    self.enums.push(EnumInfo {
                        name: e.name.clone(),
                        type_params,
                        bounded: e.type_params.iter().any(|p| !p.bounds.is_empty()),
                        bounds: Vec::new(),
                        variants: Vec::new(),
                        index: FxHashMap::default(),
                        methods: FxHashMap::default(),
                        assoc: Rc::default(),
                    });
                    if !self.built_in(&e.name)
                        && self.first_callable(&mut first, &mut rejected, Definition::Enum, &e.name)
                    {
                        self.names.add_enum(e.name.name, id);
                    }
                    next_function += e.methods.len();
                }
                ast::Item::Const(c) => {
                    let id = ConstantId(self.const_infos.len());
                    self.const_infos.push(ConstInfo::new(c.name.span));
                    if !self.built_in(&c.name)
                        && self.first_callable(
                            &mut first,
                            &mut rejected,
                            Definition::Constant,
                            &c.name,
                        )
                    {
                        self.names.add_constant(c.name.name, id);
                    }
                }
                ast::Item::Interface(interface) => {
                    let mut type_params = self.type_params(&[], &interface.type_params);
                    type_params.insert(0, Symbol::intern("Self"));
                    let id = tir::InterfaceId(self.interfaces.len());
                    self.interfaces.push(InterfaceInfo {
                        name: interface.name.clone(),
                        type_params,
                        inherits: Vec::new(),
                        inherits_at: Vec::new(),
                        inherits_failed: false,
                        requirements: Vec::new(),
                        bounds: Vec::new(),
                        assoc: Vec::new(),
                    });
                    self.index_assoc(interface, id);
                    let name = &interface.name;
                    if !self.built_in(name)
                        && self.first_callable(
                            &mut first,
                            &mut rejected,
                            Definition::Interface,
                            name,
                        )
                    {
                        self.names.add_interface(name.name, id);
                    }
                }
                ast::Item::Alias(alias) => self.declare_alias(alias, &mut first, &mut rejected),
                ast::Item::Extend(extend) => next_function += extend.methods.len(),
            }
        }
        if user_items == items.len() {
            self.prelude = self.item_counts(next_function);
        }
        // After the loop: the prelude's own items, which come last, would take the names back.
        for (kind, name, reported) in rejected {
            self.names.poison_language(name, reported);
            self.names.poison(kind, name, reported);
        }
        self.poison_broken(broken);
    }

    /// Records the alias `alias`, and its name as the first function, class, enum, constant or
    /// alias of that name, which `first` holds. `rejected` holds the names that a prelude function
    /// takes and the names that the program declares twice.
    fn declare_alias(
        &mut self,
        alias: &ast::AliasDecl,
        first: &mut FxHashMap<Symbol, (Span, Definition)>,
        rejected: &mut Vec<(Definition, Symbol, Reported)>,
    ) {
        let id = self.aliases.infos.len();
        self.aliases.infos.push(AliasInfo::new(alias));
        // Its parameters are names of types, as a function's are.
        let mut params = FxHashMap::default();
        for param in &alias.params {
            if !self.built_in(param) {
                self.first_definition(&mut params, Definition::Type, None, param);
            }
        }
        if !self.built_in(&alias.name)
            && self.first_callable(first, rejected, Definition::Alias, &alias.name)
        {
            self.names.add_alias(alias.name.name, id);
        }
    }

    /// Records, for each name, the interfaces that declare an associated type of it, and its place
    /// among theirs: the first associated type of each name that `interface`, whose id is `id`,
    /// declares. They are known before any type resolves, as a type alias may name one.
    fn index_assoc(&mut self, interface: &ast::Interface, id: tir::InterfaceId) {
        let mut named = FxHashSet::default();
        for assoc in &interface.assoc {
            if named.insert(assoc.name.name) {
                let k = named.len() - 1;
                (self.declaring_assoc.entry(assoc.name.name).or_default()).push((id, k));
                self.assoc_at.insert((id, assoc.name.name), k);
            }
        }
    }

    /// Gives each broken item in `waiting` whose name starts before `before` (all of them when it
    /// is `None`) the checks of an intact item: a name that the language or the prelude takes, and
    /// a name that another item has. `first` holds the first item of each name. The broken items
    /// come in source order, and `collect_names` takes each one just before the first intact item
    /// that starts after it, so that a duplicate is reported against the first item of its name.
    fn take_broken(
        &mut self,
        waiting: &mut Peekable<slice::Iter<'_, ast::Broken>>,
        before: Option<usize>,
        first: &mut FxHashMap<Symbol, (Span, Definition)>,
    ) {
        while let Some(broken) =
            waiting.next_if(|b| before.is_none_or(|at| b.name.span.start() < at))
        {
            let Some(kind) = broken_definition(broken.kind) else {
                // Its target is intact, and its parse error already fails the program. A method
                // call on the target that finds no method is poisoned.
                let reported = self.proof(broken.name.span); // proof: parser mark
                self.broken_extensions.insert(broken.name.name, reported);
                continue;
            };
            if !(self.built_in(&broken.name) || self.prelude_taken(kind, &broken.name).is_some()) {
                if redeclares(first, &broken.name)
                    && let Some(reported) = self.report_redeclared(first, kind, None, &broken.name)
                {
                    self.names.poison_language(broken.name.name, reported);
                } else {
                    self.first_definition(first, kind, None, &broken.name);
                }
            }
        }
    }

    /// Poisons the name of each broken item.
    fn poison_broken(&mut self, broken: &[ast::Broken]) {
        for broken in broken {
            if let Some(kind) = broken_definition(broken.kind) {
                let reported = self.proof(broken.name.span); // proof: parser mark
                self.names.poison(kind, broken.name.name, reported);
            }
        }
    }

    /// Whether `name` is a built-in type's, which no class, function, enum, constant or interface
    /// may take; reports it when it is. A built-in type keeps its name, also when the prelude
    /// declares it, as `Option` and `Result`. A language interface's name is reported too, and so
    /// is a name of milestone 8's constructors (`Some`, `Ok`, `Err`), but the item still takes
    /// it, so that its uses do not cascade. The prelude's own declarations take the language
    /// interfaces' names as well, in the interface names.
    fn built_in(&mut self, name: &ast::Ident) -> bool {
        let kind = if built_in_type(name.name.as_str()).is_some() {
            Some(BuiltIn::Type)
        } else if !name.span.in_prelude() && is_language_interface(name.name.as_str()) {
            Some(BuiltIn::Interface)
        } else {
            None
        };
        if let Some(kind) = kind {
            self.error(DiagnosticKind::BuiltInName {
                name: name.name.to_string(),
                at: name.span,
                kind,
            });
        } else if self.reserved(name) {
            return matches!(reserved_name(name.name), Some(BuiltIn::Type));
        }
        matches!(kind, Some(BuiltIn::Type))
    }

    /// Whether `name` is the first function, class, enum, interface, constant or alias of its name;
    /// reports it when not. A name that a prelude function takes, and a name that another item of
    /// the program has, is added to `rejected`, for `collect_names` to poison.
    fn first_callable(
        &mut self,
        first: &mut FxHashMap<Symbol, (Span, Definition)>,
        rejected: &mut Vec<(Definition, Symbol, Reported)>,
        kind: Definition,
        name: &ast::Ident,
    ) -> bool {
        if let Some(reported) = self.prelude_taken(kind, name) {
            rejected.push((kind, name.name, reported));
            return false;
        }
        if redeclares(first, name)
            && let Some(reported) = self.report_redeclared(first, kind, None, name)
        {
            rejected.push((kind, name.name, reported));
            return false;
        }
        self.first_definition(first, kind, None, name)
    }

    /// Whether `name` is a prelude function's, which no top-level item of `kind` may take;
    /// reports it when it is, and gives the token of that report.
    fn prelude_taken(&mut self, kind: Definition, name: &ast::Ident) -> Option<Reported> {
        // A function the prelude's code writes is taken too, but not in the prelude's own code.
        let written = !name.span.in_prelude() && self.prelude_functions.contains(&name.name);
        let taken = written || PRELUDE.contains(&name.name.as_str());
        taken.then(|| self.error(defined_twice(kind, name, Earlier::BuiltIn)))
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
        if let Some(&(span, _)) = first.get(&name.name) {
            // A user's item took the prelude's name first. For a constructor name, `BuiltInName`
            // has said so. A language interface still takes its name, which only the prelude's
            // code reads. A built-in type never gets here: `built_in` keeps a user item of its
            // name out of `first`, as `first_callable` does for a prelude function.
            if name.span.in_prelude() && !span.in_prelude() {
                return is_language_interface(name.name.as_str());
            }
            self.report_redeclared(first, kind, owner, name);
            return false;
        }
        first.insert(name.name, (name.span, kind));
        true
    }

    /// Reports `name` as defined twice when `first` holds an item of its name, and gives the
    /// token of that report. `owner` is the class whose members `first` holds.
    fn report_redeclared(
        &mut self,
        first: &FxHashMap<Symbol, (Span, Definition)>,
        kind: Definition,
        owner: Option<&str>,
        name: &ast::Ident,
    ) -> Option<Reported> {
        let &(span, first_kind) = first.get(&name.name)?;
        let first = Earlier::At {
            span,
            kind: first_kind,
            owner: owner.map(str::to_string),
        };
        Some(self.error(defined_twice(kind, name, first)))
    }

    /// How many classes, enums and functions are declared so far, `functions` being the count of
    /// functions.
    fn item_counts(&self, functions: usize) -> tir::ItemCounts {
        tir::ItemCounts {
            classes: self.classes.len(),
            enums: self.enums.len(),
            funcs: functions,
        }
    }

    /// Pass 2: each class's fields, each enum's variants and every signature, in `FuncId`
    /// order. Returns each function with the type of a method's `self`. A class's fields and
    /// methods share a namespace.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per kind of item, so the signatures of all of them are collected in one place"
    )]
    fn collect_signatures<'a>(
        &mut self,
        items: &[&'a ast::Item],
    ) -> Vec<(&'a ast::Function, SelfType)> {
        let mut declared = Vec::new();
        let mut next_class = 0;
        let mut next_enum = 0;
        let mut next_struct = 0;
        for item in items {
            let class = match item {
                ast::Item::Function(f) => {
                    self.signature(f, f.name.name.to_string(), &[], Vec::new());
                    declared.push((f, SelfType::Absent));
                    continue;
                }
                ast::Item::Class(class) => class,
                ast::Item::Enum(e) => {
                    self.enum_signatures(e, tir::EnumId(next_enum), &mut declared);
                    next_enum += 1;
                    continue;
                }
                ast::Item::Extend(extend) => {
                    self.extend_signatures(extend, &mut declared);
                    continue;
                }
                ast::Item::Struct(s) => {
                    next_struct +=
                        usize::from(self.struct_signatures(s, next_struct, &mut declared));
                    continue;
                }
                ast::Item::Const(_) | ast::Item::Alias(_) | ast::Item::Interface(_) => continue,
            };
            let id = tir::ClassId(next_class);
            next_class += 1;
            let owner = self.classes[id.0].type_params.clone();
            let receiver = Type::intern(TypeKind::Class(id, own_args(&owner)));
            self.body.self_type = SelfType::Known(receiver);
            self.body.set_type_params(&owner);
            let bounds = self.declare_bounds(0, Vec::new(), &class.type_params);
            if self.classes[id.0].bounded {
                self.classes[id.0].bounds = bounds;
            }
            // The choices come before the fields and the signatures, which may name them.
            self.classes[id.0].assoc = self.declare_choices(&class.assoc, class.name.name.as_str());
            self.body.set_type_params(&[]);
            let mut first = FxHashMap::default();
            let mut scoped = false;
            for member in &class.members {
                match member {
                    ast::Member::Field(field) => {
                        // The class's parameters stay in scope from one field to the next, and a
                        // method takes them out.
                        if !scoped {
                            self.body.set_type_params(&owner);
                            self.body.set_bounds(0, self.classes[id.0].bounds.clone());
                            scoped = true;
                        }
                        let from = self.queued();
                        let FieldType { ty, inner, shown } =
                            self.resolve_field(&field.ty, owner.is_empty());
                        let queued = from..self.queued();
                        if self.first_definition(
                            &mut first,
                            Definition::Field,
                            Some(class.name.name.as_str()),
                            &field.name,
                        ) {
                            self.keep_at(queued, Slot::Field(id, self.classes[id.0].fields.len()));
                            // `names_to_string_field` reads this flag, not the fields.
                            if field.name.name.as_str() == "to_string" {
                                self.classes[id.0].names_to_string = true;
                            }
                            self.classes[id.0].add_field(FieldInfo {
                                name: field.name.clone(),
                                ty_span: field.ty.span,
                                inner,
                                ty,
                                shown,
                                mutable: field.mutable,
                            });
                        }
                    }
                    ast::Member::Method(method) => {
                        scoped = false;
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
                        let bounds = self.classes[id.0].bounds.clone();
                        self.signature(method, name, &class.type_params, bounds);
                        declared.push((method, SelfType::Known(receiver)));
                    }
                }
            }
            self.body.set_type_params(&[]);
            for method in &class.broken_methods {
                let reported = self.proof(method.span); // proof: parser mark
                self.classes[id.0]
                    .methods
                    .insert(method.name, Declared::Poisoned(reported));
            }
            self.body.self_type = SelfType::Absent;
            self.body.assoc = Rc::default();
        }
        declared
    }

    /// `f`'s signature, named `name`, where `owner` are the type parameters of a method's class
    /// or enum, with `owner_bounds` their bounds.
    fn signature(
        &mut self,
        f: &ast::Function,
        name: String,
        owner: &[ast::TypeParam],
        owner_bounds: Vec<Bounds>,
    ) {
        let type_params = self.type_params(owner, &f.type_params);
        self.body.set_type_params(&type_params);
        self.body.set_bounds(0, owner_bounds);
        let bounds = self.declare_bounds(owner.len(), Vec::new(), &f.type_params);
        let func = tir::FuncId(self.signatures.len());
        // A mismatch shows an alias as written only where the type is the declaration's own.
        let plain = type_params.is_empty();
        self.signature_anys = Some(Vec::new());
        let (params, param_shown): (Vec<_>, Vec<_>) = (f.params.iter().enumerate())
            .map(|(i, p)| {
                let from = self.queued();
                let uses = self.aliases.uses;
                let ty = self.resolve(&p.ty);
                self.keep_at(from..self.queued(), Slot::Param(func, i));
                (ty, self.shown_text(&p.ty, uses).filter(|_| plain))
            })
            .unzip();
        let from = self.queued();
        let uses = self.aliases.uses;
        let ret = f
            .ret
            .as_ref()
            .map_or(Some(Type::UNIT), |ty| self.resolve(ty));
        let ret_shown = (f.ret.as_ref())
            .and_then(|ty| self.shown_text(ty, uses))
            .filter(|_| plain);
        self.keep_at(from..self.queued(), Slot::Ret(func));
        let anys = self.signature_anys.take().unwrap_or_default();
        self.body.set_type_params(&[]);
        self.signatures.push(Signature {
            name,
            type_params,
            own: f.type_params.len(),
            bounds,
            params,
            muts: f.params.iter().map(|p| p.mutable).collect(),
            param_names: f.params.iter().map(|p| p.name.clone()).collect(),
            ret,
            anys,
            param_shown,
            ret_shown,
            param_spans: f.params.iter().map(|p| p.ty.span).collect(),
            ret_span: f.ret.as_ref().map(|ty| ty.span),
            span: f.name.span,
            header: Span::new(f.span.start(), f.body.span.start()),
            conformance: None,
            extension: None,
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
            self.error(report);
        }
        cyclic
    }

    /// Reports a missing `main`, or one that takes anything but nothing or one `[string]`, or
    /// that returns a value. A `main` rejected as defined twice is not missing, and a type in
    /// its signature that did not resolve is not wrong: each error is reported already.
    fn check_main(&mut self, declared: &[(&ast::Function, SelfType)]) {
        match self.names.function(Symbol::intern("main")) {
            None => {
                self.error(DiagnosticKind::NoMain);
            }
            Some(Declared::Poisoned(_)) => {}
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
                    self.error(DiagnosticKind::MainSignature {
                        at: main.name.span,
                        form: MainForm::Signature,
                    });
                }
            }
        }
    }

    /// `resolve`, and the applied names written in `ty`, each with its span.
    fn resolve_written(&mut self, ty: &ast::TypeExpr) -> (Option<Type>, Vec<(Type, Span)>) {
        self.written = Some(Vec::new());
        let resolved = self.resolve(ty);
        (resolved, self.written.take().unwrap_or_default())
    }

    /// `None`, reported at the name without its `?`, for an unknown type, and for type
    /// arguments that do not fit. The aliases in `ty` are replaced, within the limits of an alias:
    /// the depth of the type, and the size of the work that the outermost written type causes.
    fn resolve(&mut self, ty: &ast::TypeExpr) -> Option<Type> {
        let outermost = self.begin_written();
        let uses = self.aliases.uses;
        let resolved = self.resolve_type(ty);
        let resolved = self.alias_limits(ty, resolved, uses);
        if outermost {
            return self.end_written(ty, resolved);
        }
        resolved
    }

    /// `resolve` without the limits of aliases.
    fn resolve_type(&mut self, ty: &ast::TypeExpr) -> Option<Type> {
        let named = match &ty.kind {
            ast::TypeKind::Array(element) => Type::array(self.resolve(element)?),
            ast::TypeKind::Applied(applied) => {
                self.named_type(ty, applied.name, &applied.args, applied.angles)?
            }
            ast::TypeKind::Name(name) => self.named_type(ty, *name, &[], ty.span)?,
            ast::TypeKind::SelfType => match self.body.self_type {
                SelfType::Known(declared) => declared,
                SelfType::Failed => return None,
                SelfType::Absent => {
                    self.error(DiagnosticKind::SelfOutside { at: ty.bare });
                    return None;
                }
            },
            ast::TypeKind::Member { base, name } => self.member_type(ty, base, name)?,
            ast::TypeKind::Any(interface) => self.resolve_any(ty, interface)?,
        };
        // Only an applied name can be an instance that grows: a plain name has no arguments.
        // Arguments on a parameter or a built-in type are E0137, and no instance has that type.
        if let (Some(written), ast::TypeKind::Applied(applied)) = (&mut self.written, &ty.kind) {
            // Without the `?`: the type that grows is the instance, not the optional of it.
            written.push((named, Span::new(ty.span.start(), applied.angles.end())));
        }
        Some((0..ty.optional).fold(named, |ty, _| optional_of(ty)))
    }

    /// The type `name`, written in `ty` with the type arguments `args` in `angles`.
    fn named_type(
        &mut self,
        ty: &ast::TypeExpr,
        name: Symbol,
        args: &[ast::TypeExpr],
        angles: Span,
    ) -> Option<Type> {
        // Built only for `type_args`, which reads it when `args` do not fit: most names have
        // none, and their declarations take none.
        let ident = || ast::Ident {
            name,
            span: Span::new(ty.span.start(), ty.span.start() + name.as_str().len()),
        };
        // A type parameter or a built-in type is not generic.
        let plain = |c: &mut Self, ty| {
            if args.is_empty() {
                return Some(ty);
            }
            c.type_args(&ident(), &[], args, angles).ok().map(|_| ty)
        };
        match self.meaning(name, names::Site::new(Position::Type, false)) {
            Resolved::TypeParam(i) => {
                let i =
                    u32::try_from(i).expect("a declaration should have fewer than 2^32 parameters");
                // The body of an alias names its parameters by place alone.
                let name = if self.body.alias.is_some() {
                    Symbol::intern("")
                } else {
                    name
                };
                plain(self, Type::intern(TypeKind::Param(name, i)))
            }
            Resolved::AssocType => match self.assoc_named(name, ident().span) {
                Named::Type(ty) => plain(self, ty),
                Named::Failed | Named::Unknown => None,
            },
            // A broken name poisons every use, a built-in type's too. Its own error is reported,
            // and so are the errors of the arguments that the use writes.
            Resolved::Poisoned(..) => {
                self.resolve_args(args);
                None
            }
            Resolved::Item(ItemId::Alias(id)) => self.alias(id, ty, name, args, angles),
            Resolved::Item(ItemId::Interface(id)) => {
                self.interface_as_type(id, name.to_string(), ty, ident().span);
                None
            }
            Resolved::BuiltinType(named) => plain(self, named),
            Resolved::Item(ItemId::Class(id)) => {
                self.reaches_prelude(id.0 >= self.prelude.classes, ident().span);
                let list = if args.is_empty() && self.classes[id.0].type_params.is_empty() {
                    TypeArgs::NONE
                } else {
                    let declared = self.classes[id.0].type_params.clone();
                    self.type_args(&ident(), &declared, args, angles).ok()?
                };
                let ty = Type::intern(TypeKind::Class(id, list));
                self.bounded(self.classes[id.0].bounded, ty, args)
                    .then_some(ty)
            }
            Resolved::Item(ItemId::Struct(id)) => {
                let list = if args.is_empty() {
                    TypeArgs::NONE
                } else {
                    self.type_args(&ident(), &[], args, angles).ok()?
                };
                Some(Type::intern(TypeKind::Struct(id, list)))
            }
            Resolved::Item(ItemId::Enum(id)) => {
                self.reaches_prelude(id.0 >= self.prelude.enums, ident().span);
                let list = if args.is_empty() && self.enums[id.0].type_params.is_empty() {
                    TypeArgs::NONE
                } else {
                    let declared = self.enums[id.0].type_params.clone();
                    self.type_args(&ident(), &declared, args, angles).ok()?
                };
                let ty = Type::intern(TypeKind::Enum(id, list));
                self.bounded(self.enums[id.0].bounded, ty, args)
                    .then_some(ty)
            }
            // `Option<T>` is `T?`: the prelude's declaration of it makes no enum.
            Resolved::OptionType => {
                let args = (self.type_args(&ident(), &[Symbol::intern("T")], args, angles)).ok()?;
                Some(optional_of(args.get()[0]))
            }
            _ => {
                let types =
                    ["i64", "u8", "f64", "bool", "char", "string"].map(|ty| (ty, Definition::Type));
                let classes = self.class_names();
                let enums = (self.names.enum_names()).map(|e| (e.as_str(), Definition::Enum));
                let interfaces =
                    (self.names.interface_names()).map(|i| (i.as_str(), Definition::Interface));
                let aliases = self.alias_candidates();
                let candidates = (types.into_iter().chain(classes).chain(enums))
                    .chain(interfaces)
                    .chain(aliases);
                self.error(DiagnosticKind::UnknownType {
                    name: name.to_string(),
                    at: ident().span,
                    form: UnknownForm::Name {
                        suggestion: suggestion(name.as_str(), candidates),
                    },
                });
                None
            }
        }
    }

    /// Whether the applied type `ty` meets the bounds of its class or enum, which is `bounded` when
    /// its declaration writes one, and `written` are its arguments as written.
    fn bounded(&mut self, bounded: bool, ty: Type, written: &[ast::TypeExpr]) -> bool {
        if !bounded {
            return true;
        }
        // In an alias body, a type that names a parameter waits for each use of the alias.
        if self.body.alias.is_some() && ty.has_param() {
            return self
                .make_check(UseCheck::Applied(ty), Span::new(0, 0))
                .is_ok();
        }
        (self.check_applied(ty, written.iter().map(|arg| arg.span).collect())).is_ok()
    }

    /// `function`, except that a method of a block that failed has a failed `self` and is not
    /// checked: the block's own error says what is wrong.
    fn method_or_function(
        &mut self,
        f: &ast::Function,
        id: tir::FuncId,
        owner: SelfType,
    ) -> Option<tir::Function> {
        if owner == SelfType::Failed {
            return None;
        }
        self.function(f, id, owner.known())
    }

    /// Checks `f`'s body against its own signature, `id`. The function, unless its return type
    /// is `Error` or its body failed; an `Error` parameter still yields one, and `check` drops
    /// it for the error behind it.
    /// A method, of the class or enum `owner`, gets `self` as its first local.
    #[expect(
        clippy::too_many_lines,
        reason = "one pass over a function: its signature, locals, body and result"
    )]
    fn function(
        &mut self,
        f: &ast::Function,
        id: tir::FuncId,
        owner: Option<Type>,
    ) -> Option<tir::Function> {
        let sig = &self.signatures[id.0];
        let (name, param_types, ret) = (sig.name.clone(), sig.params.clone(), sig.ret);
        let anys = sig.anys.clone();
        let (conformance, prelude) = (sig.conformance, sig.span.in_prelude());
        // The parameters of a method's owner: its class's, or its `extend` block's.
        let owner_params = sig.type_params.len() - sig.own;
        let ret_shown = sig.ret_shown.clone();
        let mut fresh = Body {
            owner: Some(Owner::Func(id)),
            ret,
            ret_shown,
            self_type: owner.map_or(SelfType::Absent, SelfType::Known),
            ..Body::default()
        };
        fresh.set_type_params(&sig.type_params);
        fresh.assoc = self.assoc_scope(id, owner);
        // The method's own bounds come after the places of its owner's parameters, which have none
        // when the class or enum is not bounded.
        let mut bounds = match sig.extension {
            Some(at) => self.extensions[at].bounds.clone(),
            None => self.owner_bounds(owner),
        };
        bounds.resize(owner_params, Bounds::default());
        bounds.extend(sig.bounds.iter().cloned());
        fresh.set_bounds(0, bounds);
        let ((params, body, misfit), state) = self.with_body(fresh, |c| {
            // The signature's bounds name interfaces for this function. The bounds of a block's
            // parameters are named once, by the block (`record_block_bounds`).
            for place in owner_params..c.body.bounds.len() {
                for bound in c.body.bounds[place].list.clone() {
                    c.record(Used::Interface(bound.interface), bound.at);
                }
            }
            // So do the `any` types in its parameter and return types, each as written.
            for (interface, at) in anys {
                c.record(Used::Interface(interface), at);
            }
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
                .chain(f.params.iter().zip(param_types).map(|(p, ty)| {
                    if p.mutable {
                        c.declare_mut_param(&p.name, ty)
                    } else {
                        c.declare(&p.name, ty, BindingKind::Param, None)
                    }
                }))
                .collect();
            // A `unit` function discards its body's value, so it expects none.
            let expected = match ret {
                None => Expectation::Poisoned,
                Some(Type::UNIT) => Expectation::None,
                Some(ret) => Expectation::Required(ret, c.returns()),
            };
            c.show_value(&f.body, ret);
            let mut body = c.block(&f.body, expected);
            if let (Some(Type::UNIT), Ok(checked)) = (ret, &body) {
                c.discard_tail(&f.body, checked);
            }
            if let Some(want) = ret {
                c.any_block(&mut body, want, value_span(&f.body));
            }
            if let Some(want) = ret
                && let TypeKind::Optional(payload) = want.kind()
                && block_type(&body) == Some(payload)
            {
                wrap_block(&mut body, want);
            }
            c.pop_scope();
            // The value's mismatch is reported once the body is gone, so a misfit is taken now.
            let misfit = (c.misfit_type(&body))
                .filter(|&found| ret.is_some_and(|ret| ret != Type::UNIT && ret != found));
            if misfit.is_some() {
                c.settle_misfit(&body);
            }
            (params, body, misfit)
        });
        let locals = state.locals;
        // The mismatch of the body's value is reported after its checks, as the body is gone. The
        // next body replaces it, so the map holds one function's entries.
        self.body.shown = state.shown;
        let type_params = state.type_params.iter().map(Symbol::to_string).collect();
        match (ret, block_type(&body).or(misfit)) {
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
                    let origin = Origin::Returns {
                        function: Origin::id(id.0),
                    };
                    self.mismatch(value_span(&f.body), tail(&f.body), want, got, origin)
                };
                self.diagnostics.error(d);
            }
            _ => {}
        }
        Some(tir::Function {
            name,
            span: f.name.span,
            owner,
            conformance,
            method: owner.map(|_| f.name.name.to_string()),
            type_params,
            owner_params,
            prelude,
            params,
            ret: ret?,
            locals,
            body: body.ok()?,
        })
    }

    /// The bounds of the type parameters of the class or enum `owner`, a method's `self`; empty
    /// for a function, and when none is bounded.
    fn owner_bounds(&self, owner: Option<Type>) -> Vec<Bounds> {
        match owner.map(Type::kind) {
            Some(TypeKind::Class(id, _)) => self.classes[id.0].bounds.clone(),
            Some(TypeKind::Enum(id, _)) => self.enums[id.0].bounds.clone(),
            _ => Vec::new(),
        }
    }

    /// The classes and the structs, as candidates for a name.
    fn class_names(&self) -> impl Iterator<Item = (&str, Definition)> {
        let classes = (self.names.class_names()).map(|class| (class.as_str(), Definition::Class));
        classes.chain((self.names.struct_names()).map(|s| (s.as_str(), Definition::Struct)))
    }

    /// The variant `name`, at `span`, written without its enum: `None` when a binding or an item
    /// takes the name, or no enum has such a variant.
    fn bare_variant(&self, name: Symbol, span: Span) -> Option<Diagnostic> {
        let item = self
            .body
            .scopes
            .iter()
            .any(|scope| scope.contains_key(&name))
            || self.names.class(name).is_some()
            || self.names.struct_id(name).is_some()
            || self.names.function(name).is_some()
            || self.names.enum_id(name).is_some()
            || self.names.interface(name).is_some()
            || self.names.alias(name).is_some()
            || self.names.constant(name).is_some();
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
    /// an enum's, is in scope, but as a type: that is reported instead. So is a function's name.
    /// Reports the error, and gives its token.
    fn not_found(&mut self, name: Symbol, span: Span) -> Reported {
        let site = names::Site::value(true);
        if let Some(wrong) = self.wrong_kind(name, span, site, self.meaning(name, site)) {
            return self.error(wrong);
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
            .filter(|_| self.body.label_typo.is_some_and(|(at, _)| at == span))
            .map(|label| (label.name.as_str(), Definition::LoopLabel));
        let suggestion = suggestion(name.as_str(), bindings.chain(labels));
        let meant = (suggestion.as_ref())
            .filter(|found| found.kind == Definition::LoopLabel)
            .map(|found| found.name.clone());
        let wrap = match (&meant, self.body.label_typo) {
            (Some(_), Some((_, start))) => self.wrap_break(span, start),
            _ => None,
        };
        let reported = self.error(DiagnosticKind::BindingNotFound {
            name: name.to_string(),
            at: span,
            suggestion,
            wrap,
        });
        if let Some(meant) = meant {
            // Only a bare name after `break` offers a loop label: a misspelled loop label is one
            // mistake, which neither the loop label's warning nor its loop's type reports again.
            self.break_meant(&meant, reported);
        }
        reported
    }

    /// `break_start`, the start of the `break` whose label is at `label`, when the label ends the
    /// head of an `if`, an `else if`, an `if let` or a `match`: a `{` follows it on its line. The
    /// parser then takes the `{` as the value of the `break`, so a fix of the label must wrap the
    /// `break` in parentheses.
    pub(super) fn wrap_break(&self, label: Span, break_start: usize) -> Option<usize> {
        // The lexer skips these characters between two tokens of a line.
        self.source
            .rest(label.end())
            .trim_start_matches([' ', '\t', '\r'])
            .starts_with('{')
            .then_some(break_start)
    }

    /// Whether `e` is a call of a conversion that can fail: `u8(...)`, `char(...)`, or `i64(...)`,
    /// which can fail only from an `f64`, the one argument that makes it an optional. A call of an
    /// alias of one of these types is one too.
    fn can_fail(&self, e: &ast::Expr) -> bool {
        let ast::ExprKind::Call { callee, .. } = &e.kind else {
            return false;
        };
        matches!(callee.name.as_str(), "u8" | "char" | "i64")
            || matches!(
                self.alias_type(callee.name),
                Some(Type::U8 | Type::CHAR | Type::I64)
            )
    }

    fn type_name(&self, ty: Type) -> String {
        let mut name = String::new();
        write_type(&mut name, ty, |head| self.head(head));
        name
    }

    /// The declared name of a class or an enum.
    fn head(&self, head: Head) -> &'static str {
        match head {
            Head::Class(id) => self.classes[id.0].name.name.as_str(),
            Head::Struct(id) => self.structs[id.0].name.name.as_str(),
            Head::Enum(id) => self.enums[id.0].name.name.as_str(),
            Head::Interface(id) => self.interfaces[id.0].name.name.as_str(),
            Head::Assoc(interface, k) => self.interfaces[interface.0].assoc[k as usize]
                .name
                .name
                .as_str(),
        }
    }

    /// "expected `want`, found `got`" at `span`, where `source` is the expression found, when
    /// known. Between `u8`, `i64` and `char` it offers to convert `source`, unless that is an
    /// `if`: an `else if` is one, and `else u8(if ...)` does not parse. A conversion to `u8` or
    /// `char` gives an optional, so it is offered only where one is expected. `origin` is where
    /// `want` comes from.
    fn mismatch(
        &self,
        span: Span,
        source: Option<&ast::Expr>,
        want: Type,
        got: Type,
        origin: Origin,
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
            && source.is_some_and(|e| self.can_fail(e));
        DiagnosticKind::Mismatch {
            want: Expected::Types(vec![self.want_name(span, want)]),
            got: self.type_name(got),
            at: span,
            origin: self.origin_fact(origin),
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
            layers: beyond_one_layer(want, got),
        }
        .into()
    }
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
pub(crate) fn built_in_type(name: &str) -> Option<Type> {
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

/// Whether `name` is one of `lang.sisu`'s interfaces, which `Checker::language_interfaces` finds
/// by name.
fn is_language_interface(name: &str) -> bool {
    matches!(name, "Display" | "Eq" | "Ord" | "AnyObject")
}

/// Whether `name` is the second top-level item of its name that the program writes. `first` holds
/// the first item of each name. The prelude's item of a name that the program took is no
/// redeclaration.
fn redeclares(first: &FxHashMap<Symbol, (Span, Definition)>, name: &ast::Ident) -> bool {
    first.contains_key(&name.name) && !name.span.in_prelude()
}

/// `T` for a `T?`, else `ty`.
fn payload(ty: Type) -> Type {
    match ty.kind() {
        TypeKind::Optional(payload) => payload,
        _ => ty,
    }
}

/// The expression a block ends in, if it ends in one: its value. A jump is no value.
fn tail(block: &ast::Block) -> Option<&ast::Expr> {
    match block.stmts.last() {
        Some(ast::Stmt {
            kind: ast::StmtKind::Expr(e),
            ..
        }) if !e.is_jump() => Some(e),
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
    tir::Expr::new(tir::ExprKind::Block(block), ty, span)
}

/// `ty?`: one more layer, also on an optional.
fn optional_of(ty: Type) -> Type {
    Type::optional(ty)
}

/// `ty` when it is an optional, else `ty?`: the type a `None` or a value beside `ty` takes.
fn as_optional(ty: Type) -> Type {
    match ty.kind() {
        TypeKind::Optional(_) => ty,
        _ => Type::optional(ty),
    }
}

/// Whether `ty` is `inner` with two or more `?` layers added: a conversion wraps one only.
fn beyond_one_layer(ty: Type, inner: Type) -> bool {
    let TypeKind::Optional(mut ty) = ty.kind() else {
        return false;
    };
    while let TypeKind::Optional(payload) = ty.kind() {
        if payload == inner {
            return true;
        }
        ty = payload;
    }
    false
}

/// `e`, of type `T`, where its optional `ty` is expected.
fn wrap(e: tir::Expr, ty: Type) -> tir::Expr {
    let span = e.span;
    tir::Expr::new(tir::ExprKind::Wrap(Box::new(e)), ty, span)
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

/// The names of the free functions among the prelude's `items`.
pub(crate) fn prelude_function_names<'a>(
    items: impl IntoIterator<Item = &'a ast::Item>,
) -> FxHashSet<Symbol> {
    items
        .into_iter()
        .filter_map(|item| match item {
            ast::Item::Function(f) => Some(f.name.name),
            _ => None,
        })
        .collect()
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
pub(crate) mod tests;
