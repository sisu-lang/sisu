//! The program and its items: `const`s, functions, and `main`.

use std::fmt::Write as _;

use rustc_hash::FxHashMap;

use super::expr::{CHARS, OPS, arithmetic_value};
use super::scope::{
    Adds, Alias, Class, Const, Enum, Field, Func, MAX_MUT_LEN, Ty, Var, Variant, Wrap,
};
use super::stmt::{GROW, Jumps, MUT_FUEL};
use super::writer::Piece;
use super::{
    Answer, BoundSite, Cost, DEPTHS, Gen, Known, MAX_NESTING, MAX_SIZE, NO_BOUND, NOT_A_CLASS,
};

/// The most items before `main`.
const MAX_ITEMS: usize = 8;
/// The most parameters a function takes after `depth`.
const MAX_PARAMS: usize = 3;
/// The most functions with `mut` parameters.
const MAX_MUT_FUNCTIONS: usize = 2;
/// The most lines of stdin.
const MAX_LINES: usize = 4;
/// The sizes a function's `string` parameter or result may have.
const SIZES: [u64; 3] = [0, 16, 100];
/// The most fields of a class, and of a variant.
pub(crate) const MAX_FIELDS: usize = 3;
/// The most classes and enums a program declares.
const MAX_TYPES: usize = 3;
/// The most type aliases a program declares.
pub(crate) const MAX_ALIASES: usize = 3;
/// The most methods of a class.
const MAX_METHODS: usize = 2;
/// The most interfaces a program declares, and the most requirements of one.
pub(crate) const MAX_INTERFACES: usize = 3;
pub(crate) const MAX_REQUIREMENTS: usize = 2;
/// The most variants of an enum.
const MAX_VARIANTS: usize = 4;
/// The room a field of a size has beyond its floor.
const CAPS: [u64; 3] = [0, 8, 32];
/// The most size of a class object, or of an enum's first variant: a few of them still fit
/// an array.
pub(crate) const MOST: u64 = 100;

/// The most known sites of a program, and the most `let`s of a known function (decision 3).
pub(crate) const MAX_KNOWN: usize = 3;
const MAX_STEPS: usize = 3;

#[cfg(test)]
thread_local! {
    /// On this thread: how many requirement types were asked for, how many were built, how many
    /// of those were a declared type, and for how many asks `default` was called.
    pub(super) static REQUIREMENT_TYPES: std::cell::Cell<[usize; 4]> =
        const { std::cell::Cell::new([0; 4]) };
    /// How many times `default` has run on this thread, with its recursion.
    pub(super) static DEFAULT_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A known site (decision 3): a function `k<n>` of straight-line `i64` or `u8` arithmetic
/// over its two parameters, a call of it on literals, and what the call gives, worked out
/// here.
struct KnownSite {
    marker: String,
    function: String,
    call: String,
    answer: Result<i64, &'static str>,
}

/// The statements of `shapes`, and the bound sites in them.
#[derive(Default)]
struct Calls {
    lines: Vec<String>,
    /// The line each site is in, and the site, whose `at` is in that line.
    sites: Vec<(usize, BoundSite)>,
    /// The lines that the marked statements print when `shapes` runs to its end, in order.
    expected: Vec<String>,
    /// The lines that `gap_forms` wrote. They hold no bound site.
    gap: std::ops::Range<usize>,
    /// How many marked statements the interface calls have so far: the next is `@d<marks>`.
    marks: usize,
    /// The values of the methods that meet the requirements of the interfaces.
    values: Values,
}

impl Calls {
    fn push(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }

    fn extend(&mut self, lines: impl IntoIterator<Item = String>) {
        self.lines.extend(lines);
    }

    /// The statement or call that `shapes` runs next makes the marked statements `marks` print,
    /// with the results of the methods of `ty`.
    fn runs(&mut self, marks: &[Marked], ty: &str) {
        let Calls {
            expected, values, ..
        } = self;
        expected.extend(marks.iter().map(|mark| values.line(ty, mark)));
    }

    /// The statement `before`, `arg` and `after`, where `arg` is a bound site: the first argument
    /// of a call that no other argument precedes to name the parameter. `bound` is the bound of
    /// that parameter, and `broken` replaces `arg` to break it.
    fn site(&mut self, before: &str, arg: &str, after: &str, bound: &str, broken: &'static str) {
        let at = before.len()..before.len() + arg.len();
        self.sites.push((
            self.lines.len(),
            BoundSite {
                at,
                bound: bound.to_string(),
                broken,
            },
        ));
        self.push(format!("{before}{arg}{after}"));
    }
}

/// A declared interface, as `shapes` needs it.
struct Shape {
    name: String,
    /// Whether it takes a type parameter `T`.
    generic: bool,
    /// The interfaces it inherits, by place among the declared, and how each writes its argument.
    inherits: Vec<(usize, Inherited)>,
    requirements: Vec<Requirement>,
    /// The associated type it declares, if it does.
    assoc: Option<Assoc>,
}

/// Whether a requirement has a result, for the pick `pick` below 3: two in three do.
fn has_result(pick: usize) -> bool {
    pick != 0
}

/// Whether a draw below 2 comes up heads: the draw 0, which an input that ran out gives. The
/// first choice of a coin is what the program writes when the input is empty.
fn heads(draw: usize) -> bool {
    draw == 0
}

/// An associated type `A<n>` of an interface. A conformance chooses `plain` for it, or, when the
/// interface bounds it, the conforming type itself, which conforms to the interface.
struct Assoc {
    name: String,
    bounded: bool,
    /// The type a conformance chooses when the type is not bounded: `i64`, or the counted `string`.
    plain: &'static str,
}

impl Assoc {
    /// The associated type that `draw`, a pick below 18, gives interface `id`. The draw is in six
    /// parts of three: an associated type in the upper half, bounded by the interface itself in
    /// the last third, and chosen as `i64` rather than `string` in the first.
    fn drawn(id: usize, draw: usize) -> Option<Assoc> {
        let part = draw / 3;
        (part >= 3).then(|| Assoc {
            name: format!("A{id}"),
            bounded: part == 5,
            plain: if part == 3 { "i64" } else { "string" },
        })
    }

    /// The type chosen for it where `ty` conforms.
    fn chosen<'a>(&'a self, ty: &'a str) -> &'a str {
        if self.bounded { ty } else { self.plain }
    }

    /// The `type` line of a conformance of `ty`.
    fn choice(&self, ty: &str) -> String {
        format!("type {} = {}", self.name, self.chosen(ty))
    }

    /// A value of the chosen type, where `var` is a value of the type that conforms.
    fn value(&self, var: &str) -> String {
        match (self.bounded, self.plain) {
            (true, _) => var.to_string(),
            (false, "string") => "\"s\"".to_string(),
            (false, _) => "0".to_string(),
        }
    }
}

/// How the inheritance list of an interface writes the argument of an interface it inherits.
#[derive(Clone, Copy)]
enum Inherited {
    /// The inherited interface takes none: `I1`.
    Plain,
    /// The parameter of the inheriting interface: `I1<T>`.
    Own,
    /// A type that names no parameter: `I1<i64>`.
    Fixed,
}

/// An interface that a type conforms to, and the type that stands for its parameter `T` when it
/// takes one. A generic interface is conformed to with `i64`, and a class also with `string`.
#[derive(Clone, Copy, PartialEq)]
struct Conformed {
    interface: usize,
    arg: Option<&'static str>,
}

impl Conformed {
    /// The interface as a bound or a conformance writes it: `I1`, or `I1<i64>`.
    fn written(self, declared: &[Shape]) -> String {
        let name = &declared[self.interface].name;
        match self.arg {
            Some(arg) => format!("{name}<{arg}>"),
            None => name.clone(),
        }
    }
}

/// The result of a requirement that a call prints, and that each method gives its own value.
#[derive(Clone, Copy)]
enum Kind {
    Integer,
    Text,
}

/// The first value of a method with an `i64` result: the values are 10, 11 and so on.
const FIRST_VALUE: usize = 10;

/// The values of the methods that meet a requirement with an `i64` or a `string` result. Each
/// method of the program gets a value that no other method gets, so a call that reaches the
/// wrong method prints a different line.
#[derive(Default)]
struct Values {
    /// The number of each method with an `i64` result, by its type, its requirement and the
    /// argument of its interface.
    numbers: FxHashMap<(String, String, Option<&'static str>), usize>,
}

impl Values {
    /// What the method of `ty` for `requirement`, as the interface with `arg` for its parameter
    /// has it, returns and a call prints. A `string` names its type, requirement and argument,
    /// and holds no `@`. An `i64` is the next number of the table.
    fn value(
        &mut self,
        ty: &str,
        requirement: &str,
        arg: Option<&'static str>,
        kind: Kind,
    ) -> String {
        match kind {
            Kind::Text => match arg {
                Some(arg) => format!("{ty}.{requirement}.{arg}"),
                None => format!("{ty}.{requirement}"),
            },
            Kind::Integer => {
                let next = self.numbers.len();
                let key = (ty.to_string(), requirement.to_string(), arg);
                (FIRST_VALUE + *self.numbers.entry(key).or_insert(next)).to_string()
            }
        }
    }

    /// The line that the marked statement `mark` prints when it calls a method of `ty`.
    fn line(&mut self, ty: &str, mark: &Marked) -> String {
        let value = self.value(ty, &mark.requirement, mark.arg, mark.kind);
        format!("@d{} {value}", mark.n)
    }
}

/// A statement `print("@d<n> ...")` that prints the result of a call of a requirement, or the
/// `mut` parameter that the call wrote to, with the requirement (and the parameter) and the
/// argument that it calls through. The check of the output finds the line by its marker.
struct Marked {
    n: usize,
    requirement: String,
    arg: Option<&'static str>,
    kind: Kind,
}

/// A requirement `fn name(self, p: T, ...) -> R`. It may take one more parameter `mut p: i64`:
/// each method writes a value of its own to it, and each call passes a place and prints it.
struct Requirement {
    name: String,
    params: Vec<Param>,
    ret: Option<RequirementType>,
}

/// A parameter of a requirement. A `mut` parameter is an `i64`, and its `ty.value` is a
/// placeholder: a call passes a place to it, so nothing reads that value.
struct Param {
    name: String,
    ty: RequirementType,
    mutable: bool,
}

/// A type that a requirement names, as written, and a value of it where `$` stands for a value of
/// the type that conforms and `@` for a value of the interface's parameter.
struct RequirementType {
    written: String,
    value: String,
}

impl RequirementType {
    /// The type as a conformance to the interface with `arg` for its parameter writes it.
    fn written(&self, arg: Option<&str>) -> String {
        if self.written == "T" {
            arg.expect("an interface that names `T` should take a parameter")
                .to_string()
        } else {
            self.written.clone()
        }
    }

    /// A value of the type for a conformance with `arg`, where `$` stands for `var`.
    fn value(&self, arg: Option<&str>, var: &str) -> String {
        let smallest = if arg == Some("string") { "\"s\"" } else { "0" };
        self.value.replace('$', var).replace('@', smallest)
    }
}

/// Every interface that a type conforms to when it conforms to `roots`: the roots and, after
/// each, those it inherits, each interface with its argument once.
fn conformed_closure(declared: &[Shape], roots: &[Conformed]) -> Vec<Conformed> {
    let mut stack: Vec<Conformed> = roots.iter().rev().copied().collect();
    let mut all: Vec<Conformed> = Vec::new();
    while let Some(c) = stack.pop() {
        if all.contains(&c) {
            continue;
        }
        for &(m, how) in declared[c.interface].inherits.iter().rev() {
            let arg = match how {
                Inherited::Plain => None,
                Inherited::Own => c.arg,
                Inherited::Fixed => Some("i64"),
            };
            stack.push(Conformed { interface: m, arg });
        }
        all.push(c);
    }
    all
}

/// Every requirement of the interfaces `entries`, each with the argument of its interface.
fn requirements_of<'a>(
    declared: &'a [Shape],
    entries: &[Conformed],
) -> Vec<(&'a Requirement, Option<&'static str>)> {
    (entries.iter())
        .flat_map(|c| {
            declared[c.interface]
                .requirements
                .iter()
                .map(|r| (r, c.arg))
        })
        .collect()
}

/// Whether `entries` hold one interface twice, with two arguments.
fn repeats(entries: &[Conformed]) -> bool {
    (entries.iter().enumerate())
        .any(|(i, c)| entries[..i].iter().any(|o| o.interface == c.interface))
}

impl Requirement {
    /// A method of `ty` that meets the requirement of the interface with `arg` for its parameter:
    /// `fn r(self, _p: T) -> R { value }`. A result `i64` or `string` has the value of `values`
    /// for the method. A `mut` parameter `p` is written first, with a value of its own:
    /// `fn r(self, mut p: i64) -> R { p = 10; value }`.
    fn method(&self, arg: Option<&'static str>, ty: &str, values: &mut Values) -> String {
        let mut method = format!("fn {}(self", self.name);
        for param in &self.params {
            if param.mutable {
                write!(method, ", mut {}: i64", param.name)
            } else {
                write!(method, ", _{}: {}", param.name, param.ty.written(arg))
            }
            .expect("writing to a `String` should succeed");
        }
        method += ")";
        let write_back = self.mut_param().map(|param| {
            let number = values.value(ty, &self.written_key(param), arg, Kind::Integer);
            format!("{} = {number}", param.name)
        });
        let body = |value: String| match &write_back {
            Some(write) => format!("{write}; {value}"),
            None => value,
        };
        match (&self.ret, self.kind(arg)) {
            (Some(ret), Some(kind)) => {
                let value = values.value(ty, &self.name, arg, kind);
                let value = match kind {
                    Kind::Integer => value,
                    Kind::Text => format!("\"{value}\""),
                };
                write!(method, " -> {} {{ {} }}", ret.written(arg), body(value))
                    .expect("writing to a `String` should succeed");
            }
            (Some(ret), None) => {
                write!(
                    method,
                    " -> {} {{ {} }}",
                    ret.written(arg),
                    body(ret.value(arg, "self"))
                )
                .expect("writing to a `String` should succeed");
            }
            (None, _) => {
                let body = write_back.map_or_else(String::new, |write| format!(" {write} "));
                write!(method, " {{{body}}}").expect("writing to a `String` should succeed");
            }
        }
        method
    }

    /// The `mut` parameter, if the requirement has one.
    fn mut_param(&self) -> Option<&Param> {
        self.params.iter().find(|param| param.mutable)
    }

    /// The key of the number that a method writes to the `mut` parameter `param`: not that of the
    /// result, so each write-back has a number of its own.
    fn written_key(&self, param: &Param) -> String {
        format!("{}.{}", self.name, param.name)
    }

    /// The result that a call through the interface with `arg` prints, when it is an `i64` or a
    /// `string`.
    fn kind(&self, arg: Option<&str>) -> Option<Kind> {
        match self.ret.as_ref()?.written(arg).as_str() {
            "i64" => Some(Kind::Integer),
            "string" => Some(Kind::Text),
            _ => None,
        }
    }

    /// The call of the requirement on the value `var`, where `place` is the number `m` of the
    /// variable `m<m>` that the `mut` parameter takes.
    fn invocation(&self, var: &str, arg: Option<&str>, place: Option<usize>) -> String {
        let args: Vec<String> = (self.params.iter())
            .map(|param| match place {
                Some(m) if param.mutable => format!("mut m{m}"),
                _ => param.ty.value(arg, var),
            })
            .collect();
        format!("{var}.{}({})", self.name, args.join(", "))
    }

    /// The place for the `mut` parameter, if there is one: the number `m` of `m<m>`, which is
    /// `*next`, and the statement `let mut m<m> = 0`.
    fn place(&self, next: &mut usize) -> Option<(usize, String)> {
        self.mut_param().map(|_| {
            let m = take(next);
            (m, format!("let mut m{m} = 0"))
        })
    }

    /// The statement `print("@d<m> {m<m>}")` that prints the place of the `mut` parameter after a
    /// call, and the mark that it gives.
    fn written(&self, m: usize, arg: Option<&'static str>) -> (String, Marked) {
        let param = self
            .mut_param()
            .expect("a requirement with a place should have a `mut` parameter");
        let mark = Marked {
            n: m,
            requirement: self.written_key(param),
            arg,
            kind: Kind::Integer,
        };
        (format!("print(\"@d{m} {{m{m}}}\")"), mark)
    }

    /// The statements that call the requirement on the value `var`, and print an `i64` or a
    /// `string` result as `@d<n> {call}`; then, for a `mut` parameter, the one that prints its
    /// place as `@d<m> {m<m>}`. The statements come with their marks, in the order that they
    /// print. `n` is `*next`, and `m` is the next number after the one that the result takes,
    /// if any; the place `m<m>` is declared before the call.
    fn call(
        &self,
        var: &str,
        arg: Option<&'static str>,
        next: &mut usize,
    ) -> (Vec<String>, Vec<Marked>) {
        let printed = self.kind(arg).map(|kind| (take(next), kind));
        let place = self.place(next);
        let call = self.invocation(var, arg, place.as_ref().map(|&(m, _)| m));
        let (mut lines, mut marks) = (Vec::new(), Vec::new());
        lines.extend(place.as_ref().map(|(_, declaration)| declaration.clone()));
        match (&self.ret, printed) {
            (None, _) => lines.push(call),
            (Some(_), None) => lines.push(format!("let _ = {call}")),
            (Some(_), Some((n, kind))) => {
                lines.push(format!("print(\"@d{n} {{{call}}}\")"));
                marks.push(self.marked(n, arg, kind));
            }
        }
        if let Some((m, _)) = place {
            let (line, mark) = self.written(m, arg);
            lines.push(line);
            marks.push(mark);
        }
        (lines, marks)
    }

    fn marked(&self, n: usize, arg: Option<&'static str>, kind: Kind) -> Marked {
        Marked {
            n,
            requirement: self.name.clone(),
            arg,
            kind,
        }
    }

    /// A statement that calls the requirement on the value `var` of a type that conforms to its
    /// interface with other arguments too. Only the expected type of a result `T` picks the
    /// conformance, so the result is bound to a `let` of its type; a requirement with another
    /// result gives none.
    /// The result is printed as `call` prints it: the `let` is `d<n>`, and the `print` after it
    /// writes `@d<n> {d<n>}`. A `mut` parameter takes a place, and `call` shows the statements
    /// that declare and print it.
    fn picked_call(
        &self,
        var: &str,
        arg: Option<&'static str>,
        next: &mut usize,
    ) -> Option<(Vec<String>, Vec<Marked>)> {
        let ret = self.ret.as_ref().filter(|ty| ty.written == "T")?;
        let kind = self
            .kind(arg)
            .expect("a conformance that two arguments pick between should have `i64` or `string`");
        let n = take(next);
        let place = self.place(next);
        let call = self.invocation(var, arg, place.as_ref().map(|&(m, _)| m));
        let (mut lines, mut marks) = (Vec::new(), Vec::new());
        lines.extend(place.as_ref().map(|(_, declaration)| declaration.clone()));
        lines.push(format!("let d{n}: {} = {call}", ret.written(arg)));
        lines.push(format!("print(\"@d{n} {{d{n}}}\")"));
        marks.push(self.marked(n, arg, kind));
        if let Some((m, _)) = place {
            let (line, mark) = self.written(m, arg);
            lines.push(line);
            marks.push(mark);
        }
        Some((lines, marks))
    }
}

/// `*next`, and then the number after it.
fn take(next: &mut usize) -> usize {
    *next += 1;
    *next - 1
}

/// What a constant expression is known to be: an integer's or a `char`'s value, and a
/// `string`'s length.
#[derive(Clone, Copy)]
pub(crate) struct Folded {
    value: i64,
    size: u64,
}

/// The declarations of the generic forms of `gap_forms`. Their names start with `Gap` or `gap_`.
/// The text is fixed and makes no draw. `gap_forms` writes the statements of `shapes` that use it.
const GENERIC_GAP_ITEMS: &str = r"interface GapSource { type Item; fn next(self) -> Item }
interface GapConvert<T> { fn convert(self) -> T }
interface GapHolds<Y> { type Item; fn gets(self) -> Item }
class GapCount: GapSource { type Item = i64; n: i64; fn next(self) -> Item { self.n } }
class GapTwice: GapConvert<i64> { k: i64; fn convert(self) -> i64 { self.k * 2 } }
class GapPair: GapHolds<i64> { type Item = i64; k: i64; fn gets(self) -> Item { self.k } }
fn gap_chain<S: GapHolds<T.Item>, U: GapConvert<S.Item>, T: GapSource>(s: S, _t: T, u: U) -> [S.Item] { [s.gets(), u.convert()] }
class GapKept { d: i64 }
class GapLeft { d: i64 }
interface GapPick<T> { fn pick<U>(self, x: U) -> T }
class GapPicker {}
extend GapPicker: GapPick<GapKept> { fn pick<U>(self, x: U) -> GapKept { GapKept(d: [x].len()) } }
extend GapPicker: GapPick<GapLeft> { fn pick<U>(self, x: U) -> GapLeft { GapLeft(d: [x].len() + 1) } }
extend GapPicker: GapConvert<GapKept> { fn convert(self) -> GapKept { GapKept(d: 3) } }
extend GapPicker: GapConvert<GapLeft> { fn convert(self) -> GapLeft { GapLeft(d: 4) } }
fn gap_take<T>(a: T, b: T) -> [T] { [a, b] }
fn gap_two<T: GapConvert<GapKept> & GapConvert<GapLeft>>(x: T) -> i64 {
    let k: GapKept = x.convert()
    let l: GapLeft = x.convert()
    k.d + l.d
}
class GapCell<T> { value: T }
extend<T> GapCell<T>: GapConvert<T> { fn convert(self) -> T { self.value } }
extend<U> GapCell<U>: GapConvert<[U]> { fn convert(self) -> [U] { [self.value] } }
type GapLists<T> = [T]?
fn gap_lists(m: [i64]?) -> i64 {
    match m {
        GapLists.Some(xs) => xs.len()
        GapLists.None => 0
    }
}
interface GapStep { type Next: GapStep; type Item }
class GapEnd: GapStep { type Next = GapEnd; type Item = i64 }
class GapOn<T: GapStep>: GapStep { type Next = GapOn<T.Next>; type Item = i64 }
class GapAt<T: GapStep> { v: T.Item }
interface GapSame { fn same(self, o: Self) -> bool }
interface GapLess: GapSame { fn less(self, o: Self) -> bool }
class GapRack<T: GapLess>: GapLess { v: T; fn less(self, o: Self) -> bool { self.v.less(o.v) } }
extend<T: GapLess> GapRack<T>: GapSame { fn same(self, o: Self) -> bool { self.v.same(o.v) } }
class GapNum: GapLess {
    n: i64
    fn same(self, o: GapNum) -> bool { self.n == o.n }
    fn less(self, o: GapNum) -> bool { self.n < o.n }
}
fn gap_same<T: GapSame>(a: T) -> bool { a.same(a) }
class GapTwo<A, B> { a: [A]; b: B }
fn gap_result<E>() -> Result<i64, E> { Ok(7) }
fn gap_empty<X>() -> GapTwo<X, bool> { GapTwo(a: [], b: true) }
fn gap_wait<T>(_c: GapTwo<T, bool>, _d: Result<T, string>) -> i64 { 1 }
fn gap_wide<T>(a: T, _b: T?) -> T { a }
interface GapShown { fn shown(self) -> i64 }
class GapMade: GapShown { fn shown(self) -> i64 { 1 } }
class GapBound<T: GapShown> { value: T }
type GapAlias<T> = GapBound<T>
interface GapOf<T> { fn of(self) -> T }
class GapOne: GapOf<i64> { fn of(self) -> i64 { 1 } }
type GapAnyOf<T> = any GapOf<T>
interface GapNamed { type Named }
class GapName: GapNamed {
    type Named = i64
}
type GapItem<S> = [S.Named]";

impl Gen<'_> {
    /// Writes the program, and returns its cost (decision 5), its known sites (decision 3) and the
    /// lines that the marked statements of `shapes` print (`Generated::expected`).
    pub(crate) fn program(&mut self) -> (u64, Vec<Known>, Vec<String>) {
        // Half the fuel is `main`'s, which the items before it would otherwise often spend.
        let reserve = self.fuel / 2;
        self.fuel -= reserve;
        // The types come first, so the functions after them may use them; three quarters of
        // what is left are the functions', which the methods would otherwise often spend.
        let functions = self.fuel * 3 / 4;
        self.fuel -= functions;
        for _ in 0..self.pick(MAX_TYPES + 1) {
            if self.pick(2) == 0 {
                self.class();
            } else {
                self.enumeration();
            }
        }
        self.generics();
        self.aliases();
        self.fuel += functions;
        let mut items = 0;
        while items < MAX_ITEMS && self.pick(4) != 0 && self.spend(1) {
            if self.pick(2) == 0 {
                self.constant();
            } else {
                self.function(None, false);
                self.w.line();
            }
            items += 1;
        }
        self.mut_functions();
        self.fuel += reserve;
        self.w.text("fn main() ");
        self.open_body();
        let sites: Vec<KnownSite> = (0..self.rewrite_pick(MAX_KNOWN + 1))
            .map(|n| self.known_site(n))
            .collect();
        let (values, panics): (Vec<KnownSite>, Vec<KnownSite>) =
            sites.into_iter().partition(|site| site.answer.is_ok());
        // Every value site comes first, so nothing panics before it.
        for site in &values {
            self.known_text(|g| {
                g.w.line();
                g.w.text(&format!("print(\"@{} {{{}}}\")", site.marker, site.call));
            });
        }
        self.main_stmts();
        self.mut_epilogue();
        // Whether `main` calls the function of the conformances that follows it. The choice is
        // made after the statements, so it moves none of theirs.
        let called = heads(self.pick(2));
        if called {
            self.w.line();
            self.cost.statement();
            self.w.text("shapes()");
        }
        // One panic site at most, the last statement, which a `return` would skip; and seldom,
        // since a panic ends the run before `--rc-checks` looks for leaks.
        let panic = panics
            .into_iter()
            .next()
            .filter(|_| !self.returns && self.rewrite_pick(8) == 0);
        if let Some(site) = &panic {
            self.known_text(|g| {
                g.w.line();
                g.w.text(&format!("print(\"{}\")", super::panic_line(&site.marker)));
                g.w.line();
                g.w.text(&format!("print({})", site.call));
            });
        }
        self.w.close();
        self.w.line();
        let sites: Vec<KnownSite> = values.into_iter().chain(panic).collect();
        for site in &sites {
            self.known_text(|g| {
                g.w.text(&site.function);
                g.w.line();
            });
        }
        // After `main`, so that the interfaces move no choice of the items before them. They are
        // written only when `main` calls `shapes`, which uses each one.
        let expected = if called {
            let declared = self.interfaces();
            self.shapes(&declared)
        } else {
            Vec::new()
        };
        // The stdin comes last, so the first bytes choose the program's shape.
        for _ in 0..self.pick(MAX_LINES + 1) {
            let line = self.stdin_line();
            self.stdin += &line;
            self.stdin.push('\n');
        }
        let costs: Vec<[u64; DEPTHS]> = self.scope.funcs.iter().map(|f| f.cost).collect();
        let known = sites
            .into_iter()
            .map(|site| Known {
                marker: site.marker,
                answer: match site.answer {
                    Ok(value) => Answer::Value(value.to_string()),
                    Err(message) => Answer::Panic(message),
                },
            })
            .collect();
        (self.cost.of_main(&costs), known, expected)
    }

    /// `type Al<n> = T` or `type Al<n><T> = [T]`: another name for a type, or for a type over a
    /// parameter, that later types are now and then written as, and objects and variants built
    /// through.
    fn aliases(&mut self) {
        for id in 0..self.alias_pick(MAX_ALIASES + 1) {
            let name = format!("Al{id}");
            self.w.text(&format!("type {name}"));
            if heads(self.alias_pick(3)) {
                let wrap = [Wrap::Array, Wrap::Optional, Wrap::Nested][self.alias_pick(3)];
                self.w.text(&format!("<T> = {}", wrap.body()));
                self.scope.aliases.push(Alias {
                    name,
                    plain: None,
                    wrap: Some(wrap),
                });
            } else {
                let ty = self.alias_type();
                self.w.text(&format!(" = {ty}"));
                self.scope.aliases.push(Alias {
                    name,
                    plain: Some(ty),
                    wrap: None,
                });
            }
            self.w.line();
        }
    }

    /// The type of a plain alias: a value type, or a class or an enum declared so far, alone or
    /// in an array or an optional.
    fn alias_type(&mut self) -> Ty {
        let (types, generic) = (self.scope.types.len(), self.scope.generic_types.len());
        let pick = self.alias_pick(Ty::VALUES.len() + types + generic);
        let leaf = if pick < Ty::VALUES.len() {
            Ty::VALUES[pick].clone()
        } else if pick < Ty::VALUES.len() + types {
            self.scope.types[pick - Ty::VALUES.len()].clone()
        } else {
            self.scope.generic_types[pick - Ty::VALUES.len() - types].clone()
        };
        match self.alias_pick(4) {
            1 => Ty::Array(Box::new(leaf)),
            2 => Ty::Optional(Box::new(leaf)),
            3 => Ty::Array(Box::new(Ty::Array(Box::new(leaf)))),
            _ => leaf,
        }
    }

    /// Writes the text of a known site or of the forms of `gap_forms` by `write`, which the
    /// coverage test leaves out.
    fn known_text(&mut self, write: impl FnOnce(&mut Self)) {
        #[cfg(test)]
        let start = self.w.out.program.len();
        write(self);
        #[cfg(test)]
        self.known_text.push(start..self.w.out.program.len());
    }

    /// The known site `k<n>`: its function, of `let`s each an operator over two of the
    /// parameters and the `let`s before, a call of it, and its result.
    fn known_site(&mut self, n: usize) -> KnownSite {
        let ty = if self.rewrite_pick(2) == 0 {
            Ty::I64
        } else {
            Ty::U8
        };
        let (first, second) = (self.known_arg(&ty), self.known_arg(&ty));
        let marker = format!("k{n}");
        let mut function = format!("fn {marker}(a: {ty}, b: {ty}) -> {ty} {{");
        let mut names = vec!["a".to_string(), "b".to_string()];
        let mut values = vec![Ok(first.1), Ok(second.1)];
        for _ in 0..=self.rewrite_pick(MAX_STEPS) {
            let (left, right) = (
                self.rewrite_pick(names.len()),
                self.rewrite_pick(names.len()),
            );
            let op = self.rewrite_pick(OPS.len());
            let name = format!("t{}", names.len() - 1);
            let (l, r) = (&names[left], &names[right]);
            write!(function, "\n    let {name} = ({l}{}{r})", OPS[op])
                .expect("writing to a `String` should succeed");
            // A step after one that panicked never runs.
            let value = values[left]
                .and_then(|l| values[right].and_then(|r| arithmetic_value(&ty, op, l, r)));
            names.push(name);
            values.push(value);
        }
        let last = names.last().expect("a known function should have a `let`");
        write!(function, "\n    {last}\n}}").expect("writing to a `String` should succeed");
        let answer = values[2..]
            .iter()
            .find(|value| value.is_err())
            .unwrap_or(&values[values.len() - 1]);
        KnownSite {
            call: format!("{marker}({}, {})", first.0, second.0),
            marker,
            function,
            answer: *answer,
        }
    }

    /// An argument of a known site: a literal of `ty`, `i64` or `u8`, often at an end of its
    /// range, and its value.
    fn known_arg(&mut self, ty: &Ty) -> (String, i64) {
        let n = if *ty == Ty::U8 {
            let any = self.ru.int_in_range(0..=255).unwrap_or(0);
            [0, 1, 2, 255, any][self.rewrite_pick(5)]
        } else {
            let any = self.ru.int_in_range(-1_000..=1_000).unwrap_or(0);
            [0, 1, -1, 7, i64::MAX, i64::MIN, any][self.rewrite_pick(7)]
        };
        let text = if n < 0 {
            format!("(-{})", n.unsigned_abs())
        } else {
            n.to_string()
        };
        (text, n)
    }

    /// Opens the body of a function or `main`, which counts its own cost.
    fn open_body(&mut self) {
        self.cost = Cost::default();
        self.jumps = Jumps::default();
        self.jumps.ret = true;
        self.w.open();
    }

    /// `fn f(depth: i64, params) -> R { if depth <= 0 { return default } ... value }`, or a
    /// method `fn m(self, depth: i64, ...)` of the class `owner`. With `mut_params`, either has
    /// `mut` parameters.
    fn function(&mut self, owner: Option<Ty>, mut_params: bool) {
        let name = self.scope.fresh(if owner.is_some() { "m" } else { "f" });
        let mut params = Vec::new();
        let count = if mut_params {
            1 + self.pick(MAX_PARAMS)
        } else {
            self.pick(MAX_PARAMS + 1)
        };
        for _ in 0..count {
            let ty = self.any_type();
            // The first is, so that the function has one.
            let mutable = mut_params && (params.is_empty() || self.pick(2) == 0);
            // A `mut` parameter takes the largest place that a local of a mutable value may hold.
            let size = self.size_for(&ty, if mutable { &[SIZES[2]] } else { &SIZES });
            let adds = mutable.then(|| self.mut_adds(&ty));
            params.push((self.scope.fresh("v"), ty, size, adds));
        }
        let ret = if self.pick(Ty::VALUES.len() + 1) == 0 {
            Ty::Unit
        } else {
            self.any_type()
        };
        let ret_size = self.size_for(&ret, &SIZES);
        let receiver = if owner.is_some() { "self, " } else { "" };
        self.w.text(&format!("fn {name}({receiver}depth: i64"));
        for (param, ty, _, adds) in &params {
            let written = self.spelled_in(ty, owner.as_ref());
            let keyword = if adds.is_some() { "mut " } else { "" };
            self.w.text(&format!(", {keyword}{param}: {written}"));
        }
        self.w.text(")");
        if ret != Ty::Unit {
            let written = self.spelled_in(&ret, owner.as_ref());
            self.w.text(&format!(" -> {written}"));
        }
        self.w.text(" ");
        #[cfg(test)]
        self.mut_lens.extend(
            (params.iter())
                .filter_map(|(name, _, _, adds)| Some((name.clone(), adds.as_ref()?.len))),
        );
        self.scope.funcs.push(Func {
            name,
            owner: owner.clone(),
            params: params
                .iter()
                .map(|(_, ty, size, _)| (ty.clone(), *size))
                .collect(),
            muts: params.iter().map(|&(.., adds)| adds).collect(),
            ret: ret.clone(),
            ret_size,
            cost: [1; DEPTHS],
        });
        self.scope.func = Some(self.scope.funcs.len() - 1);
        self.scope.open();
        if let Some(class) = owner {
            let size = class.floor();
            self.scope.vars.push(param("self".to_string(), class, size));
        }
        self.scope.vars.push(param("depth".to_string(), Ty::I64, 0));
        for (name, ty, size, adds) in params {
            let var = match adds {
                Some(adds) => mut_param(name, ty, size, adds),
                None => param(name, ty, size),
            };
            self.scope.vars.push(var);
        }
        self.open_body();
        self.w.line();
        self.w.text("if depth <= 0 ");
        self.w.open();
        self.w.line();
        self.w.text("return");
        if ret != Ty::Unit {
            let value = self.default(&ret);
            self.w.text(&format!(" {value}"));
        }
        self.w.close();
        self.mut_uses_first();
        self.stmts(false);
        if ret != Ty::Unit {
            self.w.line();
            self.expr(&ret, ret_size);
        }
        self.w.close();
        self.scope.close();
        let own = self
            .scope
            .func
            .take()
            .expect("a function's body should be open");
        let costs: Vec<[u64; DEPTHS]> = self.scope.funcs.iter().map(|f| f.cost).collect();
        let cost = self.cost.of_function(&costs, own);
        self.scope
            .funcs
            .last_mut()
            .expect("the function should have been pushed")
            .cost = cost;
    }

    /// Now and then, up to two functions with `mut` parameters, after the others. Each is a
    /// function, or, as often, a method in an `extend K<n>` block of a class that is no instance of
    /// a generic class. They are written from the spare streams with fuel of their own, so no
    /// other function or item moves for them, and no ordinary call names them (`callees`): only a
    /// `mut` call does.
    fn mut_functions(&mut self) {
        for _ in 0..self.mut_pick(MAX_MUT_FUNCTIONS + 1) {
            let fuel = std::mem::replace(&mut self.fuel, MUT_FUEL);
            self.swap_streams();
            self.mut_fn = true;
            let plain: Vec<usize> = (0..self.scope.classes.len())
                .filter(|&id| self.scope.classes[id].applied.is_none())
                .collect();
            if self.pick(2) == 1 && !plain.is_empty() {
                let id = plain[self.pick(plain.len())];
                self.w.text(&format!("extend K{id} "));
                self.w.open();
                self.w.line();
                self.function(Some(self.scope.class_ty(id)), true);
                self.w.close();
            } else {
                self.function(None, true);
            }
            self.mut_fn = false;
            self.w.line();
            self.swap_streams();
            self.fuel = fuel;
        }
    }

    /// A `mut` function changes what it gets: each parameter that adds something has a statement
    /// for each thing it may do, before the others.
    fn mut_uses_first(&mut self) {
        for v in 0..self.scope.vars.len() {
            let uses = if self.scope.vars[v].assignable {
                self.mut_uses(v)
            } else {
                Vec::new()
            };
            for usage in uses {
                // A statement before this one may have taken what it needs.
                if !self.mut_uses(v).contains(&usage) {
                    continue;
                }
                self.w.line();
                self.cost.statement();
                self.statement(|g| g.mut_use(v, usage));
            }
        }
    }

    /// What a `mut` parameter of `ty` may add to its place: a `string` takes room for `+=`, and
    /// an array a length from which an element may be written, growth for `push`, and, when its
    /// elements are strings, room. A type that holds a class object adds nothing (decision 4).
    fn mut_adds(&mut self, ty: &Ty) -> Adds {
        if self.scope.contains_class(ty) {
            return Adds::default();
        }
        match ty {
            Ty::String => Adds {
                room: [0, 16][self.pick(2)],
                ..Adds::default()
            },
            Ty::Array(element) => Adds {
                room: if **element == Ty::String {
                    [0, 16][self.pick(2)]
                } else {
                    0
                },
                grow: [0, GROW][self.pick(2)],
                len: self.upto(MAX_MUT_LEN),
            },
            _ => Adds::default(),
        }
    }

    /// `spell`, except that the class `owner` of the method being written is now and then
    /// `Self`.
    fn spelled_in(&mut self, ty: &Ty, owner: Option<&Ty>) -> String {
        if owner == Some(ty) && heads(self.pick(2)) {
            return "Self".to_string();
        }
        self.spell(ty)
    }

    /// `interface I<n>[<T>][: I<m>[<A>], ...] { fn r(self, ...) ... }`: requirements over `Self`,
    /// `i64`, `string`, a declared type and the interface's parameter, and some earlier interfaces
    /// inherited, each with an argument that is the parameter alone or names none. Gives what
    /// `shapes` needs of each.
    fn interfaces(&mut self) -> Vec<Shape> {
        let mut shapes: Vec<Shape> = Vec::new();
        for id in 0..self.pick(MAX_INTERFACES + 1) {
            // One byte decides whether it takes a parameter, as before, and which associated type
            // it declares, so that the choices after it stay where they were: 18 is a multiple of
            // 3.
            let draw = self.pick(18);
            let takes = draw.is_multiple_of(3);
            let mut inherits: Vec<(usize, Inherited)> = Vec::new();
            if id > 0 {
                for _ in 0..self.pick(3) {
                    let earlier = self.pick(id);
                    if inherits.iter().all(|&(m, _)| m != earlier) {
                        let how = match (shapes[earlier].generic, takes) {
                            (false, _) => Inherited::Plain,
                            (true, true) => Inherited::Own,
                            (true, false) => Inherited::Fixed,
                        };
                        inherits.push((earlier, how));
                    }
                }
            }
            let param = if takes { "<T>" } else { "" };
            let assoc = Assoc::drawn(id, draw);
            self.w.text(&format!("interface I{id}{param}"));
            if !inherits.is_empty() {
                let list: Vec<String> = inherits
                    .iter()
                    .map(|&(m, how)| match how {
                        Inherited::Plain => format!("I{m}"),
                        Inherited::Own => format!("I{m}<T>"),
                        Inherited::Fixed => format!("I{m}<i64>"),
                    })
                    .collect();
                self.w.text(&format!(": {}", list.join(", ")));
            }
            self.w.text(" ");
            self.w.open();
            if let Some(assoc) = &assoc {
                self.w.line();
                let bound = if assoc.bounded {
                    format!(": I{id}{}", if takes { "<T>" } else { "" })
                } else {
                    String::new()
                };
                self.w.text(&format!("type {}{bound}", assoc.name));
            }
            let mut requirements = Vec::new();
            for _ in 0..self.pick(MAX_REQUIREMENTS + 1) {
                let name = self.scope.fresh("r");
                let mut signature = format!("fn {name}(self");
                let mut params = Vec::new();
                for _ in 0..self.pick(3) {
                    let name = self.scope.fresh("p");
                    let ty = self.requirement_type(takes, None);
                    params.push(Param {
                        name,
                        ty,
                        mutable: false,
                    });
                }
                // The spare stream draws the `mut` parameter, so no other choice moves.
                if self.mut_pick(2) == 1 {
                    let at = self.mut_pick(params.len() + 1);
                    params.insert(
                        at,
                        Param {
                            name: self.scope.fresh("p"),
                            ty: RequirementType {
                                written: "i64".to_string(),
                                value: "0".to_string(),
                            },
                            mutable: true,
                        },
                    );
                }
                for param in &params {
                    let mutable = if param.mutable { "mut " } else { "" };
                    write!(signature, ", {mutable}{}: {}", param.name, param.ty.written)
                        .expect("writing to a `String` should succeed");
                }
                signature += ")";
                let ret =
                    has_result(self.pick(3)).then(|| self.requirement_type(takes, assoc.as_ref()));
                if let Some(ty) = &ret {
                    write!(signature, " -> {}", ty.written)
                        .expect("writing to a `String` should succeed");
                }
                self.w.line();
                self.w.text(&signature);
                requirements.push(Requirement { name, params, ret });
            }
            self.w.close();
            self.w.line();
            shapes.push(Shape {
                name: format!("I{id}"),
                generic: takes,
                inherits,
                requirements,
                assoc,
            });
        }
        shapes
    }

    /// A type a requirement of an interface may name: `Self`, `T` when it has a parameter, its
    /// associated type `assoc` (as a result only: a caller has no value of it), or one that names
    /// none.
    fn requirement_type(&mut self, takes: bool, assoc: Option<&Assoc>) -> RequirementType {
        #[cfg(test)]
        let defaults_before = {
            REQUIREMENT_TYPES.with(|c| c.set([c.get()[0] + 1, c.get()[1], c.get()[2], c.get()[3]]));
            DEFAULT_CALLS.get()
        };
        let plain = |written: &str, value: &str| {
            #[cfg(test)]
            REQUIREMENT_TYPES.with(|c| c.set([c.get()[0], c.get()[1] + 1, c.get()[2], c.get()[3]]));
            RequirementType {
                written: written.to_string(),
                value: value.to_string(),
            }
        };
        // The choices, in order: the fixed ones, `T` when the interface has a parameter, and a
        // declared type when there is one. Only the pick is built. The fifth is the associated
        // type, when there is one, in place of `Self?`, so that the picks of an interface without
        // one stay as they were.
        let fixed = [
            ("i64", "0"),
            ("string", "\"s\""),
            ("Self", "$"),
            ("[Self]", "[$]"),
        ];
        let declared = (!self.scope.types.is_empty()).then(|| self.pick(self.scope.types.len()));
        let choice = self.pick(5 + usize::from(takes) + usize::from(declared.is_some()));
        let ty = match (choice, assoc, declared) {
            (0..=3, ..) => plain(fixed[choice].0, fixed[choice].1),
            (4, Some(assoc), _) => plain(&assoc.name, &assoc.value("$")),
            (4, None, _) => plain("Self?", "None"),
            (5, ..) if takes => plain("T", "@"),
            (_, _, Some(at)) => {
                #[cfg(test)]
                REQUIREMENT_TYPES
                    .with(|c| c.set([c.get()[0], c.get()[1], c.get()[2] + 1, c.get()[3]]));
                let declared = self.scope.types[at].clone();
                plain(&declared.to_string(), &self.default(&declared))
            }
            _ => unreachable!("a pick should be one of the choices"),
        };
        #[cfg(test)]
        if DEFAULT_CALLS.get() > defaults_before {
            REQUIREMENT_TYPES.with(|c| c.set([c.get()[0], c.get()[1], c.get()[2], c.get()[3] + 1]));
        }
        ty
    }

    /// For each interface, a class and an enum that conform to it, each in its declaration list or
    /// in an `extend` block (a block for every interface it holds, when the class conforms to a
    /// generic interface twice), with a plain `extend` block; a generic function `via<n>` whose
    /// bound is the interface and that calls every requirement through it, including those of the
    /// interfaces it inherits; a generic class `H<n>` with the interface as the bound of its
    /// parameter; a generic function `later<n>`, with the interface as its bound, which takes `T`
    /// bare and inside other types (`Gen::later`), and the helpers `same`, `none_of`, `pair` and
    /// the class `Two`, which its call uses; and, when two interfaces are declared, a class that
    /// conforms to both and a function with both as bounds. A generic interface is conformed to
    /// with `i64`, and the class with `string` as well. And `fn shapes()`, which calls each method
    /// on a value of each type and the generic functions with both types, and builds the class. A
    /// call on a type with two conformances to one interface is picked by the type of its result.
    /// A method with an `i64` or `string` result holds a value of its own (`Values`), and each call
    /// that prints a result writes `@d<n> <value>`: the calls in `shapes`, and in the generic
    /// functions it calls. A requirement with a `mut` parameter writes a value of its own to it,
    /// and each call of it passes a place and prints the place after the call as `@d<n> <value>`
    /// as well. Gives the lines that `shapes` prints (property 5). The other methods
    /// hold the smallest value of their result. Each interface is used, so none is reported.
    /// Records the bound sites of its calls (property 6).
    fn shapes(&mut self, declared: &[Shape]) -> Vec<String> {
        let mut calls = Calls::default();
        let once = |k: usize| Conformed {
            interface: k,
            arg: declared[k].generic.then_some("i64"),
        };
        if !declared.is_empty() {
            // The helpers whose calls `later` uses in its arguments.
            for item in [
                "fn same<X>(x: X) -> X { x }",
                "fn none_of<X>() -> X? { None }",
                "class Two<A, B> {\n    a: A?\n    b: B?\n}",
                "fn pair<X>() -> Two<X, X> { Two(a: None, b: None) }",
            ] {
                self.w.text(item);
                self.w.line();
            }
        }
        for (k, shape) in declared.iter().enumerate() {
            let single = [once(k)];
            let all = requirements_of(declared, &conformed_closure(declared, &single));
            let mut values = Vec::new();
            let mut types = Vec::new();
            // A class and an enum.
            for class in [true, false] {
                let (ty, value, var) = if class {
                    (format!("Q{k}"), format!("Q{k}(v: 1)"), format!("q{k}"))
                } else {
                    (format!("N{k}"), format!("N{k}.A"), format!("n{k}"))
                };
                // The class also conforms to a generic interface with `string`.
                let roots: Vec<Conformed> = if class && shape.generic {
                    let string = Conformed {
                        interface: k,
                        arg: Some("string"),
                    };
                    vec![single[0], string]
                } else {
                    single.to_vec()
                };
                let listed = heads(self.pick(2));
                let entries =
                    self.conforming_type(class, &ty, &roots, declared, listed, &mut calls);
                calls.push(format!("let {var} = {value}"));
                // The requirements of the interface, and of those it inherits.
                Self::direct_calls(declared, &entries, &var, &ty, &mut calls);
                calls.push(format!("print({var}.plain())"));
                values.push(var);
                types.push(ty);
            }
            let bound = once(k).written(declared);
            let marks = self.via(&format!("via{k}"), &bound, &all, &mut calls);
            for (var, ty) in values.iter().zip(&types) {
                calls.site(&format!("via{k}("), var, ")", &bound, NO_BOUND);
                calls.runs(&marks, ty);
            }
            if let Some(assoc) = &shape.assoc {
                let marks = self.via_assoc(k, declared, &mut calls);
                for (var, ty) in values.iter().zip(&types) {
                    let (before, after) = (
                        format!("let _ = pick{k}("),
                        format!(", {})", assoc.value(var)),
                    );
                    calls.site(&before, var, &after, &bound, NO_BOUND);
                    calls.runs(&marks, ty);
                }
            }
            self.w.text(&format!("class H{k}<T: {bound}> "));
            self.w.open();
            self.w.line();
            self.w.text("v: T");
            self.w.close();
            self.w.line();
            calls.site(
                &format!("let h{k} = H{k}(v: "),
                &values[0],
                ")",
                &bound,
                NO_BOUND,
            );
            calls.push(format!("print(h{k}.v.plain())"));
            self.generic_extension(k, declared, &values[0], &marks, &mut calls);
            self.later(k, &bound, &values, &mut calls);
        }
        // Two interfaces in one bound: a class that conforms to both.
        if declared.len() >= 2 {
            let roots = [once(0), once(1)];
            let (a, b) = (roots[0].written(declared), roots[1].written(declared));
            let all = requirements_of(declared, &conformed_closure(declared, &roots));
            self.conforming_type(true, "QBoth", &roots, declared, true, &mut calls);
            let marks = self.via("both", &format!("{a} & {b}"), &all, &mut calls);
            calls.push("let qboth = QBoth(v: 1)");
            calls.site("both(", "qboth", ")", &format!("{a} & {b}"), NO_BOUND);
            calls.runs(&marks, "QBoth");
        }
        self.language_bounds(!declared.is_empty(), &mut calls);
        self.any_values(&mut calls);
        self.gap_forms(&mut calls);
        calls.extend(self.associated_cycle());
        let expected = std::mem::take(&mut calls.expected);
        self.write_shapes(calls);
        expected
    }

    /// The statements of `shapes` that call each requirement of `entries`, the interfaces that the
    /// value `var` of the type `ty` conforms to, with the lines they print. A type with two
    /// conformances to one interface picks a call by the type of its result.
    fn direct_calls(
        declared: &[Shape],
        entries: &[Conformed],
        var: &str,
        ty: &str,
        calls: &mut Calls,
    ) {
        for entry in entries {
            let again = entries
                .iter()
                .filter(|e| e.interface == entry.interface)
                .count()
                > 1;
            for requirement in &declared[entry.interface].requirements {
                if again {
                    let picked = requirement.picked_call(var, entry.arg, &mut calls.marks);
                    if let Some((lines, marks)) = picked {
                        calls.extend(lines);
                        calls.runs(&marks, ty);
                    }
                } else {
                    let (lines, marks) = requirement.call(var, entry.arg, &mut calls.marks);
                    calls.extend(lines);
                    calls.runs(&marks, ty);
                }
            }
        }
    }

    /// `fn shapes()`, which runs `calls`. Records where `program` holds each bound site. The sites
    /// of `calls` come in the order of their lines.
    fn write_shapes(&mut self, calls: Calls) {
        let Calls {
            lines, sites, gap, ..
        } = calls;
        let mut sites = sites.into_iter().peekable();
        self.w.text("fn shapes() ");
        self.w.open();
        // The lines of `gap_forms` are one block of known text between the other lines.
        self.shape_lines(&lines, 0..gap.start, &mut sites);
        let (before, after) = (gap.start, gap.end);
        self.known_text(|g| g.shape_lines(&lines, before..after, &mut sites));
        self.shape_lines(&lines, after..lines.len(), &mut sites);
        self.w.close();
        self.w.line();
    }

    /// The lines `range` of `lines`, each on a line of its own in `shapes`.
    fn shape_lines(
        &mut self,
        lines: &[String],
        range: std::ops::Range<usize>,
        sites: &mut std::iter::Peekable<std::vec::IntoIter<(usize, BoundSite)>>,
    ) {
        for line in range {
            self.w.line();
            let start = self.w.out.program.len();
            self.w.text(&lines[line]);
            while let Some((_, site)) = sites.next_if(|(at, _)| *at == line) {
                self.bound_sites.push(BoundSite {
                    at: start + site.at.start..start + site.at.end,
                    ..site
                });
            }
        }
    }

    /// `fn later<k><T: bound, U>(_a: T, ...)`, whose other seven parameters are `[T]`, `T?`,
    /// `H<k><T>`, `[T?]`, `T?`, `[T?]` and `Two<..>`, in an order that the draws choose. Gives the
    /// statement of `shapes` that calls it with `value`, one of `values`, for `_a`, which is the
    /// bound site. The other arguments name `T` in the ways that #283 broke, each level of the
    /// fix for it in turn:
    /// - a value or a bare hole: `[same(value)]`, `H<k>(v: value)`, `none_of()` and `[none_of()]`;
    /// - a call whose type comes only from its context, in another call: `same(none_of())`;
    /// - the same in a context that holds `T` in a type: `same([none_of()])`;
    /// - a call, `pair()`, whose one type parameter meets `T` and the free `U`. A draw puts them
    ///   in either order: `Two<[T], [U]>` or `Two<[U], [T]>`.
    ///
    /// When `value` breaks the bound, they must report nothing more.
    fn later(&mut self, k: usize, bound: &str, values: &[String], calls: &mut Calls) {
        let value = &values[self.pick(values.len())];
        let pair = if self.pick(2) == 0 {
            "Two<[T], [U]>"
        } else {
            "Two<[U], [T]>"
        };
        let mut rest = vec![
            ("_b: [T]".to_string(), format!("[same({value})]")),
            ("_c: T?".to_string(), "none_of()".to_string()),
            (format!("_d: H{k}<T>"), format!("H{k}(v: {value})")),
            ("_e: [T?]".to_string(), "[none_of()]".to_string()),
            ("_f: T?".to_string(), "same(none_of())".to_string()),
            ("_g: [T?]".to_string(), "same([none_of()])".to_string()),
            (format!("_h: {pair}"), "pair()".to_string()),
        ];
        for at in 0..rest.len() {
            let swap = at + self.pick(rest.len() - at);
            rest.swap(at, swap);
        }
        let (params, args): (Vec<String>, Vec<String>) = rest.into_iter().unzip();
        self.w.text(&format!(
            "fn later{k}<T: {bound}, U>(_a: T, {}) -> i64 {{ 1 }}",
            params.join(", ")
        ));
        self.w.line();
        let after = format!(", {})", args.join(", "));
        calls.site(
            &format!("let _ = later{k}("),
            value,
            &after,
            bound,
            NO_BOUND,
        );
    }

    /// The generic functions `lang_eq<T: Eq>`, `lang_ord<T: Ord>` and `lang_display<T: Display>`,
    /// whose bodies use the operators that the bounds give and call nothing else, and the
    /// statements of `shapes` that call each with types that conform: the compiler writes those
    /// conformances. With `enum_value`, `shapes` also calls `lang_eq` on an enum, which has `Eq`
    /// when its payloads do. The class `Ranked` writes its `Eq` and `Ord`, and `shapes` compares
    /// it directly and inside an array, an optional and an enum payload, and calls the generated
    /// `eq` and `less` of a number. `is` runs in `lang_same<T: AnyObject>`, on a `T?` in
    /// `lang_held`, and through `Pinned`, an interface that inherits `AnyObject`, in `lang_pinned`.
    /// `lang_both<T: Display & Eq>` prints and compares one `T` in one body. None of it takes a
    /// byte.
    /// Writes the statements into `calls`. Each statement holds at most one bound site.
    fn language_bounds(&mut self, enum_value: bool, calls: &mut Calls) {
        for (name, bound, body) in [
            (
                "eq",
                "Eq",
                "if x == x && !(x != x) && x.eq(x) { 1 } else { 1 }",
            ),
            (
                "ord",
                "Ord",
                "if x < x || x >= x || x.less(x) { 2 } else { 2 }",
            ),
            (
                "display",
                "Display",
                "if x.to_string() == \"\" || \"{x:#}\" != \"\" { 3 } else { 3 }",
            ),
        ] {
            self.w.text(&format!(
                "fn lang_{name}<T: {bound}>(x: T) -> i64 {{ {body} }}"
            ));
            self.w.line();
        }
        self.w.text(
            "class Ranked: Eq, Ord {\n    r: i64\n    fn eq(self, other: Self) -> bool { self.r == other.r }\n    fn less(self, other: Self) -> bool { self.r < other.r }\n}\nenum Choice {\n    One(Ranked)\n    Nothing\n}\ninterface Pinned: AnyObject {}\nclass Anchor: Pinned {}\nfn lang_same<T: AnyObject>(a: T, b: T) -> bool { a is b }\nfn lang_held<T: AnyObject>(a: T?, b: T) -> bool { a is b }\nfn lang_pinned<T: Pinned>(a: T, b: T) -> bool { a is b }\nfn lang_both<T: Display & Eq>(x: T) -> i64 {\n    print(x)\n    if x == x { 4 } else { 4 }\n}",
        );
        self.w.line();
        for (function, arg, bound) in [
            ("lang_eq", "[1, 2]", "Eq"),
            ("lang_eq", "\"a\"", "Eq"),
            ("lang_ord", "1.5", "Ord"),
            ("lang_ord", "'c'", "Ord"),
        ] {
            calls.site(&format!("let _ = {function}("), arg, ")", bound, NO_BOUND);
        }
        // `Display` alone has no site: every type conforms to it.
        for line in [
            "let _ = lang_display(true)",
            "let _ = lang_display([\"a\"])",
            "let _ = lang_display(Choice.Nothing)",
            "let _ = 5.to_string() == \"5\" && \"{[1, 2]}\" == [1, 2].to_string()",
            "let _ = \"{[1, 2]:#}\" == [1, 2].to_string()",
            "let rank_a = Ranked(r: 1)",
            "let rank_b = Ranked(r: 2)",
            "let rank_m: Ranked? = rank_b",
            "let _ = rank_a == rank_b || rank_a != rank_b",
            "let _ = rank_a < rank_b && rank_a >= rank_b",
            "let _ = [rank_a] == [rank_b]",
            "let _ = rank_m == rank_m",
            "let _ = rank_m == None",
            "let _ = Choice.One(rank_a) == Choice.One(rank_b)",
            "let _ = [rank_a].contains(rank_b)",
            "let _ = rank_a.eq(rank_b) || rank_a.less(rank_b)",
        ] {
            calls.push(line);
        }
        // Joined calls hold one site each: the first call. The second call stays as it is.
        calls.site(
            "let _ = lang_eq(",
            "rank_a",
            ") + lang_ord(rank_b)",
            "Eq",
            NO_BOUND,
        );
        calls.site(
            "let _ = lang_both(",
            "rank_a",
            ") + lang_both([1, 2])",
            "Display & Eq",
            NO_BOUND,
        );
        calls.push("let anchor = Anchor()");
        // `lang_held` has no site: its first argument, a `T?`, names `T` before its second.
        calls.site(
            "let _ = lang_same(",
            "rank_a",
            ", rank_b) || lang_held(rank_m, rank_a)",
            "AnyObject",
            NOT_A_CLASS,
        );
        calls.site(
            "let _ = lang_pinned(",
            "anchor",
            ", anchor)",
            "Pinned",
            NOT_A_CLASS,
        );
        calls.push("let _ = 5.eq(5) || 1.5.less(2.5)");
        if enum_value {
            calls.site("let _ = lang_eq(", "N0.A", ")", "Eq", NO_BOUND);
        }
    }

    /// Declares the interfaces `Seen` and `Heard`, which inherits `AnyObject`, the classes `Eye`
    /// and `Ear` that conform to them and the enum `Beam` that conforms to `Seen`, so that `any
    /// Seen` and `any Heard` hold objects: the functions `any_see`, `any_make` and `any_hear`
    /// take, return and compare interface values. The statements of `shapes` convert an object in
    /// each place a conversion is made, call through an array element, an optional, a parameter
    /// and a call result, and print and compare the values. An enum, a number, a `string` and a
    /// `char` convert too, each into a box, to `any Seen` and to `any Display`. The interfaces
    /// have no `Self` and no generic method, so they can be `any`. `any_never<S: Seen>() -> S`
    /// takes `S` only from its context, `any Seen` and `(any Seen)?`, in a branch that does not
    /// run: the call meets the bound with the payload of the optional. `Seen` has a requirement
    /// `put` with a `mut` parameter, and `shapes` calls it through an array element, through `??`,
    /// through `any_fill<S: Seen>` and, with `exchange`, through `any_swap<S: Seen>`, each with a
    /// class and an enum; each call prints the value that it wrote. None of it takes a byte.
    fn any_values(&mut self, calls: &mut Calls) {
        self.w.text(
            "interface Seen {\n    fn seen(self) -> i64\n    fn put(self, mut n: i64)\n}\ninterface Heard: AnyObject {\n    fn echo(self, n: i64) -> i64\n}\nclass Eye: Seen {\n    v: i64\n    fn seen(self) -> i64 { self.v }\n    fn put(self, mut n: i64) { n = self.v }\n}\nclass Ear: Heard {\n    fn echo(self, n: i64) -> i64 { n }\n}\nenum Beam: Seen {\n    Wide(i64)\n    fn seen(self) -> i64 {\n        match self {\n            Beam.Wide(n) => n\n        }\n    }\n    fn put(self, mut n: i64) {\n        match self {\n            Beam.Wide(k) => { n = k }\n        }\n    }\n}\nfn any_see(x: any Seen) -> i64 { x.seen() }\nfn any_make() -> any Seen { Eye(v: 5) }\nfn any_put(x: any Seen, mut n: i64) { x.put(mut n) }\nfn any_fill<S: Seen>(s: S, mut n: i64) { s.put(mut n) }\nfn any_swap<S: Seen>(mut s: S, u: S) -> i64 {\n    let _ = exchange(mut s, u)\n    s.seen()\n}\nfn any_never<S: Seen>() -> S { panic(\"any_never runs only in a branch that is not taken\") }\nfn any_hear(x: any Heard, y: any Heard) -> i64 {\n    if x is y { x.echo(1) } else { x.echo(2) }\n}",
        );
        self.w.line();
        for line in [
            "let eyes: [any Seen] = [Eye(v: 1), Eye(v: 2)]",
            "let _ = eyes[0].seen() + any_see(eyes[1]) + any_see(Eye(v: 3))",
            "let eye: (any Seen)? = Eye(v: 4)",
            "let _ = (eye ?? any_make()).seen()",
            "if eyes.len() > 2 {\n        let _lost: (any Seen)? = any_never()\n        let _gone: any Seen = any_never()\n    }",
            "let ear: any Heard = Ear()",
            "let _ = any_hear(ear, Ear()) + any_hear(ear, ear) + ear.echo(0)",
            "print(eyes[0])",
            "let beam: any Seen = Beam.Wide(6)",
            "let _ = beam.seen() + any_see(Beam.Wide(7))",
            "let beams: [any Seen] = [Eye(v: 8), Beam.Wide(9)]",
            "let _ = beams[1].seen() + any_see(beams[0])",
            "let shown: [any Display] = [\"s\", 'c', 5, Beam.Wide(1)]",
            "print(shown)",
        ] {
            calls.push(line);
        }
        // `put` writes the value of its receiver. A printed text comes with the value it prints:
        // the objects that `beams`, `eye`, `any_fill` and `any_swap` reach.
        for (line, printed) in [
            ("let mut put0 = 0", None),
            ("beams[1].put(mut put0)", None),
            ("{put0}", Some("9")),
            ("any_put(eye ?? any_make(), mut put0)", None),
            ("{put0}", Some("4")),
            ("any_fill(Beam.Wide(12), mut put0)", None),
            ("{put0}", Some("12")),
            ("any_fill(Eye(v: 13), mut put0)", None),
            ("{put0}", Some("13")),
            ("let mut swapped = Eye(v: 14)", None),
            ("{any_swap(mut swapped, Eye(v: 15))}", Some("15")),
            ("{swapped.seen()}", Some("15")),
            ("let mut wide = Beam.Wide(16)", None),
            ("{any_swap(mut wide, Beam.Wide(17))}", Some("17")),
            ("{wide.seen()}", Some("17")),
        ] {
            match printed {
                Some(value) => {
                    let n = take(&mut calls.marks);
                    calls.push(format!("print(\"@d{n} {line}\")"));
                    calls.expected.push(format!("@d{n} {value}"));
                }
                None => calls.push(line),
            }
        }
    }

    /// Declares the classes `PushTally`, `PushHolder` and `AppendField`, the enum `PushKind` and
    /// the function `twice_set`. It writes the statements of `shapes` that reach the checks no
    /// other rule reaches (`known_gaps.txt` lists the rest). Each form has a fixed text and makes
    /// no draw, so it moves nothing that a later seed writes. `Generated::plain` leaves out the
    /// declarations and the statements, so the older keys still show that the random rules run.
    /// The forms are:
    /// - `push` of a class, called on a local, a local that holds an optional, an element and a
    ///   field;
    /// - `push` of a class through `?.`, on a field that holds an optional;
    /// - `push` of an enum variant, a method of the enum. The receiver is a value, because
    ///   `rooted` gives false for it. No key marks this line: `enum_method_named_push` in
    ///   `tests/run` holds the rule, and the stdout that `a_failure_should_print_the_program`
    ///   pins holds the line;
    /// - a `loop` that gives a `u8?` and then a literal, in a `let` with no written type;
    /// - a `loop` whose `break` of `T` widens to the `T?` of the loop;
    /// - a `loop` where a `break` in an inner loop gives the outer loop `None`;
    /// - a call with two `mut` arguments that are elements of one array;
    /// - a `string` field that appends to itself with `+=` and with `a.f = a.f + e`;
    /// - a `match` with an arm that no value reaches (W0006), and a statement with an
    ///   unnecessary `;` (W0007). The checker accepts a warning in a generated program;
    /// - the generic forms of `GENERIC_GAP_ITEMS`, each for the points of the checker in
    ///   `GAP_POINTS` (`coverage.rs`):
    ///   - `gap_chain`: a bound that waits for a later parameter, and one that names an earlier;
    ///   - `GapPicker`, with `gap_take`: a method with written type arguments, and two
    ///     conformances that answer it;
    ///   - `gap_two`: two bounds that answer one method;
    ///   - `GapCell`: two `extend` blocks that the overlap test resolves together;
    ///   - `GapLists`: an optional alias as a path and as a pattern;
    ///   - `GapStep`, `GapOn` and `GapAt`: an associated type bound in a recursive class;
    ///   - `GapRack`: a list that inherits through an `extend` block;
    ///   - `gap_wait` and `gap_wide`: a parameter that waits for an unbound type, and one that
    ///     is optional under a hint;
    ///   - `GapAlias`, `GapAnyOf` and `GapItem`: an alias body that keeps a check.
    ///
    /// Each loop counts down from one and a `break` ends it, in at most two passes.
    /// An index is a literal.
    fn gap_forms(&mut self, calls: &mut Calls) {
        self.known_text(|g| {
            g.w.text(
                "class PushTally {\n    mut total: i64\n    fn push(self, x: i64) -> i64 {\n        self.total += x\n        self.total\n    }\n}\nclass PushHolder {\n    mut held: PushTally?\n    kept: PushTally\n}\nclass AppendField {\n    mut text: string\n}\nenum PushKind {\n    One\n    fn push(self, x: i64) -> i64 { x }\n}\nfn twice_set(mut a: i64, _b: i64, mut c: i64) {\n    a += 10\n    c += 20\n}",
            );
            g.w.line();
            g.w.text(GENERIC_GAP_ITEMS);
            g.w.line();
        });
        let first = calls.lines.len();
        for line in [
            "let tally = PushTally(total: 3)",
            "print(tally.push(1))",
            "let maybe: PushTally? = tally",
            "print(maybe?.push(1) ?? 9)",
            "let tallies = [tally]",
            "print(tallies[0].push(1))",
            "let holder = PushHolder(held: PushTally(total: 0), kept: PushTally(total: 0))",
            "print(holder.kept.push(1))",
            "print(holder.held?.push(1) ?? 9)",
            "print(PushKind.One.push(2))",
            "let opt_byte: u8? = 200",
            "let joined = ({\n        let mut left = 1\n        loop {\n            if left == 0 { break opt_byte }\n            left -= 1\n            break 6\n        }\n    })",
            "print(joined ?? 0)",
            "let widened: i64? = ({\n        let mut left = 1\n        loop {\n            if left == 0 { break 3 }\n            left -= 1\n            break None\n        }\n    })",
            "print(widened ?? 4)",
            "let crossed: string? = ({\n        let mut left = 1\n        outer: loop {\n            if left == 0 { break \"a\" }\n            left -= 1\n            let mut inner = 1\n            loop {\n                if inner == 0 { break }\n                inner -= 1\n                break outer None\n            }\n        }\n    })",
            "print(crossed ?? \"b\")",
            "let mut slots = [1, 2]",
            "twice_set(mut slots[0], 0, mut slots[1])",
            "print(slots)",
            "let appended = AppendField(text: \"a\")",
            "appended.text += \"b\"",
            "appended.text = appended.text + \"c\"",
            "print(appended.text)",
            "let arm = slots.len()",
            "let dead = match arm {\n        _ => 1\n        2 => 2\n    }",
            "print(dead);",
            "print(gap_chain(GapPair(k: 5), GapCount(n: 6), GapTwice(k: 7)))",
            "let picker = GapPicker()",
            "print(gap_take(picker.pick<i64>(1), GapKept(d: 9))[0].d)",
            "print(gap_two(picker))",
            "let held: GapCell<i64> = GapCell(value: 7)",
            "let number: i64 = held.convert()",
            "print(number)",
            "print(gap_lists(GapLists<i64>.Some([4])))",
            "print(GapAt<GapOn<GapEnd>>(v: 1).v)",
            "print(gap_same(GapRack(v: GapNum(n: 1))))",
            "print(gap_wait(gap_empty(), gap_result()))",
            "let wide: u8? = gap_wide(1, Some(Some(2)))",
            "print(wide ?? 0)",
            "let bound: GapAlias<GapMade> = GapBound(value: GapMade())",
            "print(bound.value.shown())",
            "let any_of: GapAnyOf<i64> = GapOne()",
            "print(any_of.of())",
            "let items: GapItem<GapName> = [1]",
            "print(items)",
        ] {
            calls.push(line);
        }
        calls.gap = first..calls.lines.len();
    }

    /// Declares the interface `Nest`, the class `NestW<T>` and the enum `NestE<T: Nest>`. The
    /// associated type `Item` of `NestW<T>` is `NestE<NestW<T>>`, and the payload of `NestE` is
    /// `T.Item` or `T.Item?`. So the payload of `NestE<NestW<i64>>` is the enum itself, a cycle
    /// that no declared name shows. Declares `nest_depth`, which takes a value apart, and `nest`.
    /// `nest` builds a value of two to four levels, compares it with `==`, prints it and drops
    /// it. Gives the statements of `shapes` that use them. One draw chooses the form of the
    /// payload, and another chooses the depth.
    fn associated_cycle(&mut self) -> Vec<String> {
        let optional = self.pick(2) == 1;
        let depth = 2 + self.pick(3);
        let ty = "NestE<NestW<i64>>";
        let (payload, arm) = if optional {
            (
                "T.Item?",
                "match x {\n            None => 1\n            Some(y) => 1 + nest_depth(y)\n        }",
            )
        } else {
            ("T.Item", "1 + nest_depth(x)")
        };
        self.w.text(&format!(
            "interface Nest {{ type Item }}\nclass NestW<T>: Nest {{ type Item = NestE<NestW<T>> }}\nenum NestE<T: Nest> {{\n    End\n    A({payload})\n}}\nfn nest_depth(e: {ty}) -> i64 {{\n    match e {{\n        NestE.End => 0\n        NestE.A(x) => {arm}\n    }}\n}}"
        ));
        self.w.line();
        let mut body = format!("let v0: {ty} = NestE.End");
        for level in 1..=depth {
            let below = level - 1;
            write!(body, "\n    let v{level}: {ty} = NestE.A(v{below})")
                .expect("writing to a `String` should succeed");
        }
        let below = depth - 1;
        write!(
            body,
            "\n    print(v{depth})\n    print(v{depth} == v{depth})\n    print(v{depth} == v{below})"
        )
        .expect("writing to a `String` should succeed");
        if optional {
            write!(
                body,
                "\n    let none: {ty} = NestE.A(None)\n    print(none == v1)\n    print(none == none)"
            )
            .expect("writing to a `String` should succeed");
        }
        self.w.text(&format!(
            "fn nest() -> i64 {{\n    {body}\n    nest_depth(v{depth})\n}}"
        ));
        self.w.line();
        vec![
            "let _nest = NestW<i64>()".to_string(),
            "print(nest())".to_string(),
        ]
    }

    /// A generic class `R<k><T>` that conforms to the interface `k`, and to those it inherits,
    /// when `T` does, each in a block with the bound `T: I<k>`; a block for every `T` with a
    /// method that reads the field; and a block for `R<k><i64>` alone. Gives the statements of
    /// `shapes` that use them, with `value`, a value of a type that conforms to `k`: the
    /// conformance holds through the bound of `via<k>`, whose marked statements are `marks`. Its
    /// methods call nothing, so no cycle goes through them, and each interface has one block, so no
    /// two conformances overlap.
    fn generic_extension(
        &mut self,
        k: usize,
        declared: &[Shape],
        value: &str,
        marks: &[Marked],
        calls: &mut Calls,
    ) {
        let class = format!("R{k}");
        self.w.text(&format!("class {class}<T> "));
        self.w.open();
        self.w.line();
        self.w.text("v: T");
        self.w.close();
        self.w.line();
        let bound = Conformed {
            interface: k,
            arg: declared[k].generic.then_some("i64"),
        };
        for entry in conformed_closure(declared, &[bound]) {
            self.w.text(&format!(
                "extend<T: {}> {class}<T>: {} ",
                bound.written(declared),
                entry.written(declared)
            ));
            self.w.open();
            if let Some(assoc) = &declared[entry.interface].assoc {
                self.w.line();
                self.w.text(&assoc.choice(&format!("{class}<T>")));
            }
            for requirement in &declared[entry.interface].requirements {
                self.w.line();
                self.w
                    .text(&requirement.method(entry.arg, &class, &mut calls.values));
            }
            self.w.close();
            self.w.line();
        }
        self.w.text(&format!("extend<T> {class}<T> "));
        self.w.open();
        self.w.line();
        self.w.text("fn top(self) -> T { self.v }");
        self.w.close();
        self.w.line();
        self.w.text(&format!("extend {class}<i64> "));
        self.w.open();
        self.w.line();
        self.w.text("fn solo(self) -> i64 { 1 }");
        self.w.close();
        self.w.line();
        calls.push(format!("let w{k} = {class}(v: {value})"));
        calls.site(
            &format!("via{k}("),
            &format!("w{k}"),
            ")",
            &bound.written(declared),
            NO_BOUND,
        );
        calls.runs(marks, &class);
        calls.push(format!("print(w{k}.top().plain())"));
        calls.push(format!("print({class}(v: 1).solo())"));
    }

    /// A class `ty` or an enum, if not `class`, that conforms to the interfaces `roots`: in its
    /// declaration list, which holds the methods, or in a block for each interface it conforms
    /// to, with a plain `extend` block. A type that conforms to one interface twice has the
    /// blocks, as a body holds one method of a name. Gives the interfaces it conforms to.
    fn conforming_type(
        &mut self,
        class: bool,
        ty: &str,
        roots: &[Conformed],
        declared: &[Shape],
        listed: bool,
        calls: &mut Calls,
    ) -> Vec<Conformed> {
        let entries = conformed_closure(declared, roots);
        let listed = listed && !repeats(&entries);
        let keyword = if class { "class" } else { "enum" };
        let conformance = if listed {
            let names: Vec<String> = roots.iter().map(|c| c.written(declared)).collect();
            format!(": {}", names.join(", "))
        } else {
            String::new()
        };
        self.w.text(&format!("{keyword} {ty}{conformance} "));
        self.w.open();
        if listed {
            // The associated types of every interface the list gives, listed or inherited.
            for entry in &entries {
                if let Some(assoc) = &declared[entry.interface].assoc {
                    self.w.line();
                    self.w.text(&assoc.choice(ty));
                }
            }
        }
        self.w.line();
        self.w.text(if class { "v: i64" } else { "A\n    B(i64)" });
        if listed {
            for (requirement, arg) in requirements_of(declared, &entries) {
                self.w.line();
                self.w.text(&requirement.method(arg, ty, &mut calls.values));
            }
        }
        self.w.close();
        self.w.line();
        if !listed {
            for entry in &entries {
                self.w
                    .text(&format!("extend {ty}: {} ", entry.written(declared)));
                self.w.open();
                if let Some(assoc) = &declared[entry.interface].assoc {
                    self.w.line();
                    self.w.text(&assoc.choice(ty));
                }
                for requirement in &declared[entry.interface].requirements {
                    self.w.line();
                    self.w
                        .text(&requirement.method(entry.arg, ty, &mut calls.values));
                }
                self.w.close();
                self.w.line();
            }
        }
        self.w.text(&format!("extend {ty} "));
        self.w.open();
        self.w.line();
        self.w.text("fn plain(self) -> i64 { 1 }");
        self.w.close();
        self.w.line();
        entries
    }

    /// `fn pick<n><T: I<n>>(_x: T, y: T.A<n>) -> T.A<n> { y }`: the associated type of the
    /// interface `k` as the type of a parameter and of a result, with a call of each requirement on
    /// `y` when the interface bounds the type by itself. Gives the marked statements of the calls.
    fn via_assoc(&mut self, k: usize, declared: &[Shape], calls: &mut Calls) -> Vec<Marked> {
        let assoc = declared[k]
            .assoc
            .as_ref()
            .expect("an interface with an associated type should be picked through");
        let bound = Conformed {
            interface: k,
            arg: declared[k].generic.then_some("i64"),
        };
        let name = &assoc.name;
        self.w.text(&format!(
            "fn pick{k}<T: {}>(_x: T, y: T.{name}) -> T.{name} ",
            bound.written(declared)
        ));
        self.w.open();
        let mut marks = Vec::new();
        if assoc.bounded {
            for requirement in &declared[k].requirements {
                let (lines, called) = requirement.call("y", bound.arg, &mut calls.marks);
                for line in lines {
                    self.w.line();
                    self.w.text(&line);
                }
                marks.extend(called);
            }
        }
        self.w.line();
        self.w.text("y");
        self.w.close();
        self.w.line();
        marks
    }

    /// `fn name<T: bounds>(x: T)`, which calls each of `requirements` on `x`. Gives the marked
    /// statements of the calls, in order: each call of `name` runs them all.
    fn via(
        &mut self,
        name: &str,
        bounds: &str,
        requirements: &[(&Requirement, Option<&'static str>)],
        calls: &mut Calls,
    ) -> Vec<Marked> {
        let param = if requirements.is_empty() { "_x" } else { "x" };
        self.w.text(&format!("fn {name}<T: {bounds}>({param}: T) "));
        self.w.open();
        let mut marks = Vec::new();
        for (requirement, arg) in requirements {
            let (lines, called) = requirement.call("x", *arg, &mut calls.marks);
            for line in lines {
                self.w.line();
                self.w.text(&line);
            }
            marks.extend(called);
        }
        self.w.close();
        self.w.line();
        marks
    }

    /// The most size for a value of `ty` (decision 5): a class object's own, else its floor
    /// and one of `sizes`, an optional's floor that of its payload. 0 for a type without one.
    pub(crate) fn size_for(&mut self, ty: &Ty, sizes: &[u64]) -> u64 {
        let floor = least_size(ty);
        if matches!(ty, Ty::Class { .. }) || !ty.sized() {
            return floor;
        }
        (floor + sizes[self.pick(sizes.len())]).min(MAX_SIZE)
    }

    /// The smallest value of `ty`, a value type, as a constant expression writes it: what a
    /// function returns at depth 0. Its size is `ty.floor()`.
    pub(crate) fn default(&self, ty: &Ty) -> String {
        #[cfg(test)]
        DEFAULT_CALLS.set(DEFAULT_CALLS.get() + 1);
        match ty {
            Ty::I64 | Ty::U8 => "0".to_string(),
            Ty::F64 => "0.0".to_string(),
            Ty::Bool => "false".to_string(),
            Ty::Char => "'a'".to_string(),
            Ty::String => "\"\"".to_string(),
            Ty::Array(_) => "[]".to_string(),
            Ty::Optional(_) => "None".to_string(),
            Ty::Class { id, .. } => {
                let fields: Vec<String> = self.scope.classes[*id]
                    .fields
                    .iter()
                    .map(|field| format!("{}: {}", field.name, self.default(&field.ty)))
                    .collect();
                format!("{ty}({})", fields.join(", "))
            }
            Ty::Enum { id, .. } => {
                let variant = &self.scope.enums[*id].variants[0];
                let values: Vec<String> = variant.fields.iter().map(|f| self.default(f)).collect();
                format!("{ty}.{}{}", variant.name, payload(&variant.labels, &values))
            }
            Ty::Unit => unreachable!("`unit` has no value"),
        }
    }

    /// A field's or a payload field's type: a type declared before, unless its floor would take
    /// the holder past `MOST`, where an `i64` takes its place.
    pub(crate) fn field_type(&mut self, size: u64) -> Ty {
        let ty = self.any_type();
        if size + ty.floor() > MOST {
            Ty::I64
        } else {
            ty
        }
    }

    /// `class K { [mut] f: T ... fn m(self, ...) ... }`: fields of types declared before, each
    /// with a cap on its size, then methods, which may take and give the class itself.
    pub(crate) fn class(&mut self) {
        let id = self.scope.classes.len();
        // One class in three lists `Display`, which the compiler writes, and one in three writes
        // a method `to_string`, which is its `Display`. Neither takes a byte.
        let listed = if id.is_multiple_of(3) {
            ": Display"
        } else {
            ""
        };
        self.w.text(&format!("class K{id}{listed} "));
        self.w.open();
        let mut fields = Vec::new();
        let mut size = 1;
        for _ in 0..self.pick(MAX_FIELDS + 1) {
            let mut ty = self.field_type(size);
            let mut cap = self.size_for(&ty, &CAPS);
            if size + cap > MOST {
                (ty, cap) = (Ty::I64, 0);
            }
            size += cap;
            let mutable = self.pick(2) == 1;
            let name = self.scope.fresh("g");
            self.w.line();
            let keyword = if mutable { "mut " } else { "" };
            let written = self.spell(&ty);
            self.w.text(&format!("{keyword}{name}: {written}"));
            fields.push(Field {
                name,
                ty,
                mutable,
                cap,
            });
        }
        let ty = Ty::class(id, size);
        self.scope.classes.push(Class {
            fields,
            size,
            applied: None,
            inferable: false,
        });
        self.scope.types.push(ty.clone());
        for _ in 0..self.pick(MAX_METHODS + 1) {
            if !self.spend(1) {
                break;
            }
            self.w.line();
            self.function(Some(ty.clone()), false);
        }
        if id % 3 == 1 {
            self.w.line();
            self.w.text("fn to_string(self) -> string { \"k\" }");
        }
        self.w.close();
        self.w.line();
    }

    /// `enum E { V ... fn m(self, _other: Self) -> bool { true } }`: variants with no payload,
    /// with fields, or with labeled fields. The first holds only types declared before; a later
    /// one may hold the enum itself, which a value built finite ends in the first.
    pub(crate) fn enumeration(&mut self) {
        let id = self.scope.enums.len();
        // The body goes into a piece, so the header can name what the compiler writes for it.
        let header = self.w.swap(Piece::default());
        self.w.open();
        let mut variants: Vec<Variant> = Vec::new();
        let mut least = 1;
        let mut has_class = false;
        // Two variants at least: a pattern of one is refutable, as `while let` needs (E0131).
        for v in 0..2 + self.pick(MAX_VARIANTS - 1) {
            let mut fields = Vec::new();
            let mut size = 1;
            for _ in 0..self.pick(MAX_FIELDS + 1) {
                // Each later variant holds the enum itself as often as not.
                let ty = if v > 0 && fields.is_empty() && self.pick(2) == 0 {
                    Ty::enumeration(id, least)
                } else {
                    self.field_type(size)
                };
                size += ty.floor();
                has_class |= !matches!(ty, Ty::Enum { id: own, .. } if own == id)
                    && self.scope.contains_class(&ty);
                fields.push(ty);
            }
            if v == 0 {
                least = size;
            }
            let labels: Vec<String> = if !fields.is_empty() && self.pick(3) == 0 {
                fields.iter().map(|_| self.scope.fresh("g")).collect()
            } else {
                Vec::new()
            };
            let name = self.scope.fresh("V");
            let types: Vec<String> = fields.iter().map(ToString::to_string).collect();
            self.w.line();
            self.w.text(&format!("{name}{}", payload(&labels, &types)));
            let variant = Variant {
                name,
                recursive: fields
                    .iter()
                    .any(|f| matches!(f, Ty::Enum { id: own, .. } if *own == id)),
                fields,
                labels,
                inferable: false,
            };
            self.scope.note(&variant);
            variants.push(variant);
        }
        // A method that takes the enum itself, written as `Self` in every other enum. It makes no
        // pick, so that it moves no choice of the items after it, and nothing calls it.
        let own = Ty::enumeration(id, least);
        let other = if id.is_multiple_of(2) {
            "Self".to_string()
        } else {
            self.spell(&own)
        };
        let name = self.scope.fresh("m");
        self.w.line();
        self.w.text(&format!(
            "fn {name}(self, _other: {other}) -> bool {{ true }}"
        ));
        self.scope.enums.push(Enum {
            variants,
            least,
            has_class,
            applied: None,
        });
        self.scope.types.push(Ty::enumeration(id, least));
        self.w.close();
        let body = self.w.swap(header);
        // An enum with no class in it lists `Eq`, which the compiler writes for it; every other
        // enum of three lists `Display`. Neither takes a byte.
        let listed = if !has_class && id.is_multiple_of(2) {
            ": Eq"
        } else if id % 3 == 1 {
            ": Display"
        } else {
            ""
        };
        self.w.text(&format!("enum E{id}{listed} "));
        self.w.paste(&body);
        self.w.line();
    }

    /// `const c[: T] = e`, where `e` folds without a panic (`check/constant.rs`).
    fn constant(&mut self) {
        let ty = self.value_type();
        let name = self.scope.fresh("c");
        self.w.text(&format!("const {name}"));
        if ty == Ty::U8 || self.pick(2) == 1 {
            self.w.text(&format!(": {ty}"));
        }
        self.w.text(" = ");
        let folded = self.const_expr(&ty, MAX_SIZE);
        self.w.line();
        self.scope.consts.push(Const {
            name,
            ty,
            value: folded.value,
            size: folded.size,
        });
    }

    /// A constant expression of type `ty`, at most `max` chars if a `string`: literals, earlier
    /// constants, operators and the conversions that cannot fail.
    pub(crate) fn const_expr(&mut self, ty: &Ty, max: u64) -> Folded {
        let names: Vec<usize> = (0..self.scope.consts.len())
            .filter(|&c| self.scope.consts[c].ty == *ty && self.scope.consts[c].size <= max)
            .collect();
        let compound = self.nesting < MAX_NESTING && self.spend(1);
        let choices = 1 + usize::from(!names.is_empty()) + if compound { 3 } else { 0 };
        let choice = self.pick(choices);
        self.nesting += 1;
        let folded = match choice {
            0 => self.const_literal(ty, max),
            1 if !names.is_empty() => {
                let pick = self.pick(names.len());
                let c = &self.scope.consts[names[pick]];
                let folded = Folded {
                    value: c.value,
                    size: c.size,
                };
                let name = c.name.clone();
                self.w.text(&name);
                folded
            }
            n => self.const_compound(ty, max, n % 3),
        };
        self.nesting -= 1;
        folded
    }

    fn const_literal(&mut self, ty: &Ty, max: u64) -> Folded {
        let (text, value, size) = match ty {
            Ty::I64 => {
                let (text, value) = self.int_literal();
                (text, value, 0)
            }
            Ty::U8 => {
                let (text, value) = self.u8_literal();
                (text, value, 0)
            }
            Ty::Char => {
                let (text, c) = CHARS[self.pick(CHARS.len())];
                (text.to_string(), i64::from(u32::from(c)), 0)
            }
            Ty::String => {
                let len = self.upto(max.min(8));
                (format!("\"{}\"", self.string_text(len, 26)), 0, len)
            }
            Ty::F64
            | Ty::Bool
            | Ty::Unit
            | Ty::Array(_)
            | Ty::Optional(_)
            | Ty::Class { .. }
            | Ty::Enum { .. } => {
                self.literal(ty, max);
                return Folded { value: 0, size: 0 };
            }
        };
        self.w.text(&text);
        Folded { value, size }
    }

    /// One of three compound forms of a constant of type `ty`, chosen by `form`.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per type and form, so the constant forms read in one place"
    )]
    fn const_compound(&mut self, ty: &Ty, max: u64, form: usize) -> Folded {
        match (ty, form) {
            (Ty::I64 | Ty::U8, 0) => self.const_arithmetic(ty),
            (Ty::I64, 1) => {
                // `-(e)`, which overflows only on the smallest `i64`.
                let (piece, operand) = self.capture(|g| g.const_expr(ty, 0));
                if let Some(value) = operand.value.checked_neg() {
                    self.w.text("(-(");
                    self.w.paste(&piece);
                    self.w.text("))");
                    Folded { value, size: 0 }
                } else {
                    self.w.paste(&piece);
                    operand
                }
            }
            (Ty::I64, _) => {
                let u8_names = self.scope.consts.iter().any(|c| c.ty == Ty::U8);
                self.w.text("i64(");
                let operand = if u8_names && self.pick(2) == 1 {
                    let names: Vec<usize> = (0..self.scope.consts.len())
                        .filter(|&c| self.scope.consts[c].ty == Ty::U8)
                        .collect();
                    let pick = self.pick(names.len());
                    let c = &self.scope.consts[names[pick]];
                    let folded = Folded {
                        value: c.value,
                        size: 0,
                    };
                    let name = c.name.clone();
                    self.w.text(&name);
                    folded
                } else {
                    self.const_expr(&Ty::Char, 0)
                };
                self.w.text(")");
                operand
            }
            (Ty::F64, 0) => {
                const OPS: &[&str] = &[" + ", " - ", " * ", " / ", " % "];
                self.w.text("(");
                self.const_expr(ty, 0);
                self.one_of(OPS);
                self.const_expr(ty, 0);
                self.w.text(")");
                Folded { value: 0, size: 0 }
            }
            (Ty::F64, 1) => {
                self.w.text("(-(");
                self.const_expr(ty, 0);
                self.w.text("))");
                Folded { value: 0, size: 0 }
            }
            (Ty::F64, _) => {
                self.w.text("f64(");
                self.const_expr(&Ty::I64, 0);
                self.w.text(")");
                Folded { value: 0, size: 0 }
            }
            (Ty::Bool, 0) if self.pick(2) == 0 => {
                self.w.text("(!(");
                self.const_expr(ty, 0);
                self.w.text("))");
                Folded { value: 0, size: 0 }
            }
            (Ty::Bool, 0) => {
                self.w.text("(");
                self.const_expr(ty, 0);
                self.one_of(&[" && ", " || "]);
                self.const_expr(ty, 0);
                self.w.text(")");
                Folded { value: 0, size: 0 }
            }
            (Ty::Bool, 1) => {
                let operand = self.value_type();
                self.w.text("(");
                self.const_expr(&operand, MAX_SIZE);
                self.one_of(&[" == ", " != "]);
                self.const_expr(&operand, MAX_SIZE);
                self.w.text(")");
                Folded { value: 0, size: 0 }
            }
            (Ty::Bool, _) => {
                const ORDERED: &[Ty] = &[Ty::I64, Ty::U8, Ty::F64, Ty::Char, Ty::String];
                let operand = ORDERED[self.pick(ORDERED.len())].clone();
                let ops = if self.pick(2) == 0 {
                    [" < ", " <= "]
                } else {
                    [" > ", " >= "]
                };
                self.w.text("(");
                self.const_expr(&operand, MAX_SIZE);
                for _ in 0..=self.pick(2) {
                    self.one_of(&ops);
                    self.const_expr(&operand, MAX_SIZE);
                }
                self.w.text(")");
                Folded { value: 0, size: 0 }
            }
            (Ty::String, _) => {
                self.w.text("(");
                let lhs = self.const_expr(ty, max);
                self.w.text(" + ");
                let rhs = self.const_expr(ty, max - lhs.size);
                self.w.text(")");
                Folded {
                    value: 0,
                    size: lhs.size + rhs.size,
                }
            }
            // A `char` constant has no compound form.
            (
                Ty::U8
                | Ty::Char
                | Ty::Unit
                | Ty::Array(_)
                | Ty::Optional(_)
                | Ty::Class { .. }
                | Ty::Enum { .. },
                _,
            ) => self.const_literal(ty, max),
        }
    }

    /// `(a op b)` of integers, with an operator under which it neither overflows its type nor
    /// divides by zero; `+` or `/` always holds one.
    fn const_arithmetic(&mut self, ty: &Ty) -> Folded {
        let (lhs_piece, lhs) = self.capture(|g| g.const_expr(ty, 0));
        let (rhs_piece, rhs) = self.capture(|g| g.const_expr(ty, 0));
        let (l, r) = (lhs.value, rhs.value);
        let results = [
            (" + ", l.checked_add(r)),
            (" - ", l.checked_sub(r)),
            (" * ", l.checked_mul(r)),
            (" / ", l.checked_div(r)),
            (" % ", l.checked_rem(r)),
        ];
        let fits = |n: &i64| *ty == Ty::I64 || (0..=255).contains(n);
        let valid: Vec<(&str, i64)> = results
            .iter()
            .filter_map(|(op, n)| n.filter(fits).map(|n| (*op, n)))
            .collect();
        let (op, value) = valid[self.pick(valid.len())];
        self.w.text("(");
        self.w.paste(&lhs_piece);
        self.w.text(op);
        self.w.paste(&rhs_piece);
        self.w.text(")");
        Folded { value, size: 0 }
    }

    /// Runs `write`, and returns what it wrote instead of writing it.
    fn capture<T>(&mut self, write: impl FnOnce(&mut Self) -> T) -> (Piece, T) {
        let outer = self.w.swap(Piece::default());
        let result = write(self);
        (self.w.swap(outer), result)
    }
}

/// A payload as written, `(a, b)` or `(l: a, m: b)`, of `values` and their `labels`, if any;
/// nothing for none.
pub(crate) fn payload(labels: &[String], values: &[String]) -> String {
    if values.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = if labels.is_empty() {
        values.to_vec()
    } else {
        labels
            .iter()
            .zip(values)
            .map(|(label, value)| format!("{label}: {value}"))
            .collect()
    };
    format!("({})", parts.join(", "))
}

/// The size of the smallest value of `ty` that a plain write may store (see `size_for`): the floor
/// of the type, or of the payload of an optional, which holds `None` at no size.
fn least_size(ty: &Ty) -> u64 {
    match ty {
        Ty::Optional(payload) => payload.floor(),
        _ => ty.floor(),
    }
}

/// A parameter: in scope, never assigned.
fn param(name: String, ty: Ty, size: u64) -> Var {
    Var::new(name, ty, false, size, 0, 0)
}

/// A `mut` parameter: a variable that the body may assign. It holds at most `size`, the most that
/// a caller's place holds, and what the body adds (`adds`). It stores back a plain write of at
/// most the smallest value of its type, which fits every place of that type. An array has the
/// length and the growth, and a `string` the room, that `adds` gives.
fn mut_param(name: String, ty: Ty, size: u64, adds: Adds) -> Var {
    let base = match &ty {
        Ty::Array(element) => element.floor(),
        _ => least_size(&ty),
    };
    let mut var = Var::new(name, ty, true, base, adds.room, 0);
    // An element reaches that size too, as in a plain parameter.
    var.size = size + adds.room + adds.grow;
    var.elem = var.size;
    var.grow = adds.grow;
    var.len = adds.len;
    var
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate::coverage::{SEEDS, search_seeds, seed};

    /// GIVEN a `mut` parameter of `[i64]` that a caller's place holds at most 100 of
    /// WHEN the parameter is a variable of the body
    /// THEN an element of it can reach that size, as an element of a plain parameter can
    #[test]
    fn a_mut_array_parameter_should_hold_elements_of_the_size_it_takes() {
        let ty = Ty::Array(Box::new(Ty::I64));
        let var = mut_param("v1".to_string(), ty.clone(), 100, Adds::default());
        let plain = param("v1".to_string(), ty, 100);
        assert_eq!((var.size, var.elem), (100, plain.elem));
    }

    /// GIVEN a `mut` parameter of `string` that takes 100, adds 16 of room and 64 of growth
    /// WHEN the parameter is a variable of the body
    /// THEN it holds the sum of the three, and so does an element of it
    #[test]
    fn a_mut_parameter_should_hold_what_it_takes_and_what_it_adds() {
        let adds = Adds {
            room: 16,
            grow: 64,
            ..Adds::default()
        };
        let var = mut_param("v1".to_string(), Ty::String, 100, adds);
        assert_eq!((var.size, var.elem), (180, 180));
    }

    /// GIVEN a class, an enum, an optional of each, and types with no size
    /// WHEN a plain write stores the smallest value of the type
    /// THEN its size is that of the class object or of the first variant, and 0 for the rest
    #[test]
    fn least_size_should_give_the_size_of_the_smallest_value() {
        let class = Ty::Class {
            id: 0,
            size: 9,
            applied: None,
        };
        let enumeration = Ty::Enum {
            id: 0,
            least: 5,
            applied: None,
        };
        let optional = |ty: &Ty| Ty::Optional(Box::new(ty.clone()));
        let rows = [
            (class.clone(), 9),
            (optional(&class), 9),
            (enumeration.clone(), 5),
            (optional(&enumeration), 5),
            (Ty::String, 0),
            (Ty::Array(Box::new(class.clone())), 0),
            (optional(&Ty::String), 0),
            (optional(&optional(&class)), 0),
            (Ty::I64, 0),
            (optional(&Ty::I64), 0),
        ];
        for (ty, size) in rows {
            assert_eq!(least_size(&ty), size, "{ty}");
        }
    }

    #[test]
    fn a_draw_should_give_an_interface_the_associated_type_of_its_part() {
        for draw in 0..18 {
            let expected = match draw {
                0..=8 => None,
                9..=11 => Some((false, "i64")),
                12..=14 => Some((false, "string")),
                _ => Some((true, "string")),
            };
            let drawn = Assoc::drawn(7, draw).map(|a| {
                assert_eq!(a.name, "A7");
                (a.bounded, a.plain)
            });
            assert_eq!(drawn, expected, "draw {draw}");
        }
    }

    #[test]
    fn a_pick_of_zero_should_leave_a_requirement_without_a_result() {
        assert_eq!([0, 1, 2].map(has_result), [false, true, true],);
    }

    #[test]
    fn a_draw_of_zero_should_come_up_heads() {
        assert_eq!([0, 1].map(heads), [true, false]);
    }

    /// An empty input draws zero everywhere, so `main` calls the function of the conformances,
    /// and the program writes that function.
    #[test]
    fn an_empty_input_should_call_the_function_of_the_conformances() {
        let source = crate::generate::generate(&[]).plain;
        assert!(source.contains("    shapes()\n"), "{source}");
        assert!(source.contains("fn shapes() {"), "{source}");
    }

    /// `later<k>` takes the bounded parameter bare, then in a form for each level of the fix for
    /// #283. Each parameter has its own argument, in the same place, and the call has the site
    /// first. The helpers it calls are declared once, and the draws put `T` and `U` in `Two` in
    /// both orders.
    #[test]
    fn a_program_with_interfaces_should_call_later_with_the_forms_that_name_t() {
        let mut found = false;
        let mut pairs = std::collections::BTreeSet::new();
        // What no seed has yet: the search stops when it is empty, and the assertion prints it.
        let absent = |found: bool, pairs: &std::collections::BTreeSet<&str>| -> Vec<String> {
            let mut absent = Vec::new();
            if !found {
                absent.push("no seed declares `later0`".to_string());
            }
            if pairs.len() != 2 {
                absent.push(format!(
                    "{} of the 2 pairs of `Two`: {pairs:?}",
                    pairs.len()
                ));
            }
            absent
        };
        search_seeds(SEEDS, |n| {
            let g = crate::generate::generate(&seed(n));
            for pair in ["Two<[T], [U]>", "Two<[U], [T]>"] {
                if g.program.contains(&format!("_h: {pair}")) {
                    pairs.insert(pair);
                }
            }
            if !g.program.contains("fn later0<") {
                return absent(found, &pairs).is_empty();
            }
            found = true;
            for part in [
                "fn same<X>(x: X) -> X { x }",
                "fn none_of<X>() -> X? { None }",
                "class Two<A, B> {",
                "fn pair<X>() -> Two<X, X> { Two(a: None, b: None) }",
            ] {
                assert_eq!(
                    g.program.matches(part).count(),
                    1,
                    "{part} in {}",
                    g.program
                );
            }
            // The site is the first argument of the call.
            let call = "    let _ = later0(";
            let site = g
                .bound_sites
                .iter()
                .find(|site| g.program[..site.at.start].ends_with(call))
                .unwrap_or_else(|| panic!("a bound site in the call of `later0` in {}", g.program));
            let value = &g.program[site.at.clone()];
            let args: Vec<&str> = g.program[site.at.end..]
                .lines()
                .next()
                .and_then(|line| line.strip_prefix(", "))
                .and_then(|line| line.strip_suffix(')'))
                .unwrap_or_else(|| panic!("the arguments of `later0` in {}", g.program))
                .split(", ")
                .collect();
            let params: Vec<&str> = g
                .program
                .lines()
                .find_map(|line| line.strip_prefix("fn later0<"))
                .and_then(|line| line.split_once("(_a: T, _"))
                .and_then(|(_, line)| line.split_once(") -> i64"))
                .unwrap_or_else(|| panic!("the parameters of `later0` in {}", g.program))
                .0
                .split(", _")
                .collect();
            assert_eq!(args.len(), 7, "{args:?}");
            assert_eq!(params.len(), 7, "{params:?}");
            for (param, arg) in params.iter().zip(&args) {
                let want = match param.as_bytes()[0] {
                    b'b' => format!("[same({value})]"),
                    b'c' => "none_of()".to_string(),
                    b'd' => format!("H0(v: {value})"),
                    b'e' => "[none_of()]".to_string(),
                    b'f' => "same(none_of())".to_string(),
                    b'g' => "same([none_of()])".to_string(),
                    b'h' => "pair()".to_string(),
                    _ => panic!("the parameter {param} in {}", g.program),
                };
                assert_eq!(*arg, want, "the argument of {param} in {}", g.program);
            }
            absent(found, &pairs).is_empty()
        });
        let absent = absent(found, &pairs);
        assert!(absent.is_empty(), "{absent:#?}");
    }

    /// The draws that order the seven parameters after `_a` of `later<k>` reach many orders (the
    /// seeds hold 64 of the 5040), so a draw that reaches fewer than 16 fails.
    #[test]
    fn later_should_take_its_parameters_in_many_orders() {
        let mut orders = std::collections::BTreeSet::new();
        for data in crate::generate::coverage::seeds() {
            for line in crate::generate::generate(&data).program.lines() {
                if let Some((_, params)) = line
                    .strip_prefix("fn later")
                    .and_then(|rest| rest.split_once("(_a: T, _"))
                    && let Some((params, _)) = params.split_once(") -> i64")
                {
                    let names: Vec<&str> = params.split(", _").map(|p| &p[..1]).collect();
                    assert_eq!(names.len(), 7, "{line}");
                    orders.insert(names.join(" "));
                }
            }
        }
        assert!(
            orders.len() >= 16,
            "only {} orders: {orders:?}",
            orders.len()
        );
    }

    /// Every row of the table of replacements has a site in the programs of the seeds, and a
    /// site is the argument alone: what comes before it opens the argument list or names the
    /// field, and what comes after it ends the argument.
    #[test]
    fn the_seeds_should_hold_a_bound_site_for_each_replacement() {
        let mut rows = std::collections::BTreeSet::new();
        let expected = [
            ("AnyObject", "true"),
            ("Display & Eq", "NoBound()"),
            ("Eq", "NoBound()"),
            ("Ord", "NoBound()"),
            ("Pinned", "true"),
            ("generic interface", "NoBound()"),
            ("interface", "NoBound()"),
            ("two interfaces", "NoBound()"),
        ]
        .map(|(row, broken)| (row.to_string(), broken));
        search_seeds(SEEDS, |n| {
            let g = crate::generate::generate(&seed(n));
            for site in &g.bound_sites {
                let (before, after) = (&g.program[..site.at.start], &g.program[site.at.end..]);
                assert!(before.ends_with('(') || before.ends_with("v: "), "{before}");
                assert!(after.starts_with(')') || after.starts_with(','), "{after}");
                let row = match site.bound.as_str() {
                    "Display" => panic!("every type conforms to `Display`: it has no site"),
                    bound @ ("Eq" | "Ord" | "AnyObject" | "Pinned" | "Display & Eq") => bound,
                    bound if bound.contains(" & ") => "two interfaces",
                    bound if bound.contains('<') => "generic interface",
                    _ => "interface",
                };
                rows.insert((row.to_string(), site.broken));
            }
            expected.iter().all(|row| rows.contains(row))
        });
        let unexpected: Vec<_> = rows.iter().filter(|row| !expected.contains(row)).collect();
        assert!(
            unexpected.is_empty(),
            "rows not in the table: {unexpected:?}"
        );
        let missing: Vec<_> = expected.iter().filter(|row| !rows.contains(row)).collect();
        assert!(missing.is_empty(), "rows with no site: {missing:?}");
    }
}
