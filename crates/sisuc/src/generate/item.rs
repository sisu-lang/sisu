//! The program and its items: `const`s, functions, and `main`.

use std::fmt::Write as _;

use super::expr::{CHARS, OPS, arithmetic_value};
use super::scope::{Class, Const, Enum, Field, Func, Ty, Var, Variant};
use super::stmt::Jumps;
use super::writer::Piece;
use super::{
    Answer, BoundSite, Cost, DEPTHS, Gen, Known, MAX_NESTING, MAX_SIZE, NO_BOUND, NOT_A_CLASS,
};

/// The most items before `main`.
const MAX_ITEMS: usize = 8;
/// The most parameters a function takes after `depth`.
const MAX_PARAMS: usize = 3;
/// The most lines of stdin.
const MAX_LINES: usize = 4;
/// The sizes a function's `string` parameter or result may have.
const SIZES: [u64; 3] = [0, 16, 100];
/// The most fields of a class, and of a variant.
pub(crate) const MAX_FIELDS: usize = 3;
/// The most classes and enums a program declares.
const MAX_TYPES: usize = 3;
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
}

impl Calls {
    fn push(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }

    fn extend(&mut self, lines: impl IntoIterator<Item = String>) {
        self.lines.extend(lines);
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

/// A requirement `fn name(self, p: T, ...) -> R`.
struct Requirement {
    name: String,
    params: Vec<(String, RequirementType)>,
    ret: Option<RequirementType>,
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
    /// A method that meets the requirement of the interface with `arg` for its parameter:
    /// `fn r(self, _p: T) -> R { value }`.
    fn method(&self, arg: Option<&str>) -> String {
        let mut method = format!("fn {}(self", self.name);
        for (name, ty) in &self.params {
            write!(method, ", _{name}: {}", ty.written(arg))
                .expect("writing to a `String` should succeed");
        }
        method += ")";
        match &self.ret {
            Some(ty) => {
                write!(
                    method,
                    " -> {} {{ {} }}",
                    ty.written(arg),
                    ty.value(arg, "self")
                )
                .expect("writing to a `String` should succeed");
            }
            None => method += " {}",
        }
        method
    }

    /// The call of the requirement on the value `var`.
    fn invocation(&self, var: &str, arg: Option<&str>) -> String {
        let args: Vec<String> = (self.params.iter())
            .map(|(_, ty)| ty.value(arg, var))
            .collect();
        format!("{var}.{}({})", self.name, args.join(", "))
    }

    /// A statement that calls the requirement on the value `var`, and prints an `i64` or a
    /// `string` result.
    fn call(&self, var: &str, arg: Option<&str>) -> String {
        let call = self.invocation(var, arg);
        match self.ret.as_ref().map(|ty| ty.written.as_str()) {
            None => call,
            Some("i64" | "string") => format!("print({call})"),
            Some(_) => format!("let _ = {call}"),
        }
    }

    /// A statement that calls the requirement on the value `var` of a type that conforms to its
    /// interface with other arguments too. Only the expected type of a result `T` picks the
    /// conformance, so the result is bound to a `let` of its type; a requirement with another
    /// result gives none.
    fn picked_call(&self, var: &str, arg: Option<&str>) -> Option<String> {
        let ret = self.ret.as_ref().filter(|ty| ty.written == "T")?;
        Some(format!(
            "let _: {} = {}",
            ret.written(arg),
            self.invocation(var, arg)
        ))
    }
}

/// What a constant expression is known to be: an integer's or a `char`'s value, and a
/// `string`'s length.
#[derive(Clone, Copy)]
pub(crate) struct Folded {
    value: i64,
    size: u64,
}

impl Gen<'_> {
    /// Writes the program, and returns its cost (decision 5) and its known sites (decision 3).
    pub(crate) fn program(&mut self) -> (u64, Vec<Known>) {
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
        self.fuel += functions;
        let mut items = 0;
        while items < MAX_ITEMS && self.pick(4) != 0 && self.spend(1) {
            if self.pick(2) == 0 {
                self.constant();
            } else {
                self.function(None);
                self.w.line();
            }
            items += 1;
        }
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
        if called {
            let declared = self.interfaces();
            self.shapes(&declared);
        }
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
        (self.cost.of_main(&costs), known)
    }

    /// Writes a known site's text by `write`, which the coverage test leaves out.
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
    /// method `fn m(self, depth: i64, ...)` of the class `owner`.
    fn function(&mut self, owner: Option<Ty>) {
        let name = self.scope.fresh(if owner.is_some() { "m" } else { "f" });
        let mut params = Vec::new();
        for _ in 0..self.pick(MAX_PARAMS + 1) {
            let ty = self.any_type();
            let size = self.size_for(&ty, &SIZES);
            params.push((self.scope.fresh("v"), ty, size));
        }
        let ret = if self.pick(Ty::VALUES.len() + 1) == 0 {
            Ty::Unit
        } else {
            self.any_type()
        };
        let ret_size = self.size_for(&ret, &SIZES);
        let receiver = if owner.is_some() { "self, " } else { "" };
        self.w.text(&format!("fn {name}({receiver}depth: i64"));
        for (param, ty, _) in &params {
            let written = self.spelled_in(ty, owner.as_ref());
            self.w.text(&format!(", {param}: {written}"));
        }
        self.w.text(")");
        if ret != Ty::Unit {
            let written = self.spelled_in(&ret, owner.as_ref());
            self.w.text(&format!(" -> {written}"));
        }
        self.w.text(" ");
        self.scope.funcs.push(Func {
            name,
            owner: owner.clone(),
            params: params
                .iter()
                .map(|(_, ty, size)| (ty.clone(), *size))
                .collect(),
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
        for (name, ty, size) in params {
            self.scope.vars.push(param(name, ty, size));
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
                    let param = self.scope.fresh("p");
                    let ty = self.requirement_type(takes, None);
                    write!(signature, ", {param}: {}", ty.written)
                        .expect("writing to a `String` should succeed");
                    params.push((param, ty));
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
    /// bare and inside other types (`Gen::later`), and the helpers `same` and `none_of`, which
    /// its call nests; and, when two interfaces are declared, a class that conforms to both and a
    /// function with both as bounds. A generic interface is conformed to with `i64`, and the class
    /// with `string` as well. And `fn shapes()`, which calls each method on a value of each type
    /// and the generic functions with both types, and builds the class. A call on a type with two
    /// conformances to one interface is picked by the type of its result. The methods hold the
    /// smallest value of their result. Each interface is used, so none is reported. Records the
    /// bound sites of its calls (property 6).
    fn shapes(&mut self, declared: &[Shape]) {
        let mut calls = Calls::default();
        let once = |k: usize| Conformed {
            interface: k,
            arg: declared[k].generic.then_some("i64"),
        };
        if !declared.is_empty() {
            // The helpers whose calls `later` nests in its arguments.
            self.w.text("fn same<X>(x: X) -> X { x }");
            self.w.line();
            self.w.text("fn none_of<X>() -> X? { None }");
            self.w.line();
        }
        for (k, shape) in declared.iter().enumerate() {
            let single = [once(k)];
            let all = requirements_of(declared, &conformed_closure(declared, &single));
            let mut values = Vec::new();
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
                let entries = self.conforming_type(class, &ty, &roots, declared, listed);
                calls.push(format!("let {var} = {value}"));
                // The requirements of the interface, and of those it inherits.
                for entry in &entries {
                    let again = entries
                        .iter()
                        .filter(|e| e.interface == entry.interface)
                        .count()
                        > 1;
                    for requirement in &declared[entry.interface].requirements {
                        calls.extend(if again {
                            requirement.picked_call(&var, entry.arg)
                        } else {
                            Some(requirement.call(&var, entry.arg))
                        });
                    }
                }
                calls.push(format!("print({var}.plain())"));
                values.push(var);
            }
            let bound = once(k).written(declared);
            self.via(&format!("via{k}"), &bound, &all);
            for var in &values {
                calls.site(&format!("via{k}("), var, ")", &bound, NO_BOUND);
            }
            if let Some(assoc) = &shape.assoc {
                self.via_assoc(k, declared);
                for var in &values {
                    let (before, after) = (
                        format!("let _ = pick{k}("),
                        format!(", {})", assoc.value(var)),
                    );
                    calls.site(&before, var, &after, &bound, NO_BOUND);
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
            self.generic_extension(k, declared, &values[0], &mut calls);
            self.later(k, &bound, &values, &mut calls);
        }
        // Two interfaces in one bound: a class that conforms to both.
        if declared.len() >= 2 {
            let roots = [once(0), once(1)];
            let (a, b) = (roots[0].written(declared), roots[1].written(declared));
            let all = requirements_of(declared, &conformed_closure(declared, &roots));
            self.conforming_type(true, "QBoth", &roots, declared, true);
            self.via("both", &format!("{a} & {b}"), &all);
            calls.push("let qboth = QBoth(v: 1)");
            calls.site("both(", "qboth", ")", &format!("{a} & {b}"), NO_BOUND);
        }
        self.language_bounds(!declared.is_empty(), &mut calls);
        calls.extend(self.associated_cycle());
        self.write_shapes(&calls);
    }

    /// `fn shapes()`, which runs `calls`. Records where `program` holds each bound site.
    fn write_shapes(&mut self, calls: &Calls) {
        self.w.text("fn shapes() ");
        self.w.open();
        for (line, call) in calls.lines.iter().enumerate() {
            self.w.line();
            let start = self.w.out.program.len();
            self.w.text(call);
            for (_, site) in calls.sites.iter().filter(|(at, _)| *at == line) {
                self.bound_sites.push(BoundSite {
                    at: start + site.at.start..start + site.at.end,
                    bound: site.bound.clone(),
                    broken: site.broken,
                });
            }
        }
        self.w.close();
        self.w.line();
    }

    /// `fn later<k><T: bound>(_a: T, ...)`, whose other four parameters are `[T]`, `T?`, `H<k><T>`
    /// and `[T?]`, in an order that the draws choose. Gives the statement of `shapes` that calls
    /// it with `value`, one of `values`, for `_a`, which is the bound site. The other arguments
    /// name `T` in each way that #283 broke: a call nested in an argument (`same`), a call whose
    /// type comes only from its context (`none_of`) and a type that holds `T` (`H<k>(v: value)`,
    /// `[none_of()]`). When `value` breaks the bound, they must report nothing more.
    fn later(&mut self, k: usize, bound: &str, values: &[String], calls: &mut Calls) {
        let value = &values[self.pick(values.len())];
        let mut rest = vec![
            ("_b: [T]".to_string(), format!("[same({value})]")),
            ("_c: T?".to_string(), "none_of()".to_string()),
            (format!("_d: H{k}<T>"), format!("H{k}(v: {value})")),
            ("_e: [T?]".to_string(), "[none_of()]".to_string()),
        ];
        for at in 0..rest.len() {
            let swap = at + self.pick(rest.len() - at);
            rest.swap(at, swap);
        }
        let (params, args): (Vec<String>, Vec<String>) = rest.into_iter().unzip();
        self.w.text(&format!(
            "fn later{k}<T: {bound}>(_a: T, {}) -> i64 {{ 1 }}",
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
    /// Writes the statements into `calls`, one for each call, so that each holds at most one bound
    /// site.
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
        calls.site("let _ = lang_eq(", "rank_a", ")", "Eq", NO_BOUND);
        calls.site("let _ = lang_ord(", "rank_b", ")", "Ord", NO_BOUND);
        calls.site(
            "let _ = lang_both(",
            "rank_a",
            ")",
            "Display & Eq",
            NO_BOUND,
        );
        calls.site(
            "let _ = lang_both(",
            "[1, 2]",
            ")",
            "Display & Eq",
            NO_BOUND,
        );
        calls.push("let anchor = Anchor()");
        // `lang_held` has no site: its first argument, a `T?`, names `T` before its second.
        calls.site(
            "let _ = lang_same(",
            "rank_a",
            ", rank_b)",
            "AnyObject",
            NOT_A_CLASS,
        );
        calls.push("let _ = lang_held(rank_m, rank_a)");
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
    /// conformance holds through the bound of `via<k>`. Its methods call nothing, so no cycle
    /// goes through them, and each interface has one block, so no two conformances overlap.
    fn generic_extension(&mut self, k: usize, declared: &[Shape], value: &str, calls: &mut Calls) {
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
                self.w.text(&requirement.method(entry.arg));
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
                self.w.text(&requirement.method(arg));
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
                    self.w.text(&requirement.method(entry.arg));
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
    /// `y` when the interface bounds the type by itself.
    fn via_assoc(&mut self, k: usize, declared: &[Shape]) {
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
        if assoc.bounded {
            for requirement in &declared[k].requirements {
                self.w.line();
                self.w.text(&requirement.call("y", bound.arg));
            }
        }
        self.w.line();
        self.w.text("y");
        self.w.close();
        self.w.line();
    }

    /// `fn name<T: bounds>(x: T)`, which calls each of `requirements` on `x`.
    fn via(
        &mut self,
        name: &str,
        bounds: &str,
        requirements: &[(&Requirement, Option<&'static str>)],
    ) {
        let param = if requirements.is_empty() { "_x" } else { "x" };
        self.w.text(&format!("fn {name}<T: {bounds}>({param}: T) "));
        self.w.open();
        for (requirement, arg) in requirements {
            self.w.line();
            self.w.text(&requirement.call("x", *arg));
        }
        self.w.close();
        self.w.line();
    }

    /// The most size for a value of `ty` (decision 5): a class object's own, else its floor
    /// and one of `sizes`, an optional's floor that of its payload. 0 for a type without one.
    pub(crate) fn size_for(&mut self, ty: &Ty, sizes: &[u64]) -> u64 {
        let floor = match ty {
            Ty::Class { size, .. } => return *size,
            _ if !ty.sized() => return 0,
            Ty::Optional(payload) => payload.floor(),
            _ => ty.floor(),
        };
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
            self.function(Some(ty.clone()));
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
            variants.push(Variant {
                name,
                recursive: fields
                    .iter()
                    .any(|f| matches!(f, Ty::Enum { id: own, .. } if *own == id)),
                fields,
                labels,
                inferable: false,
            });
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
                const OPS: [&str; 5] = [" + ", " - ", " * ", " / ", " % "];
                self.w.text("(");
                self.const_expr(ty, 0);
                self.one_of(&OPS);
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
                const ORDERED: [Ty; 5] = [Ty::I64, Ty::U8, Ty::F64, Ty::Char, Ty::String];
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

/// A parameter: in scope, never assigned.
fn param(name: String, ty: Ty, size: u64) -> Var {
    Var::new(name, ty, false, size, 0, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// `later<k>` takes the bounded parameter bare, then in each form that #283 broke, and the
    /// helpers it calls are declared once.
    #[test]
    fn a_program_with_interfaces_should_call_later_with_the_forms_that_name_t() {
        let mut found = false;
        for data in crate::generate::coverage::seeds() {
            let source = crate::generate::generate(&data).program;
            if !source.contains("fn later0<") {
                continue;
            }
            found = true;
            for part in [
                "fn same<X>(x: X) -> X { x }",
                "fn none_of<X>() -> X? { None }",
            ] {
                assert_eq!(source.matches(part).count(), 1, "{part} in {source}");
            }
            for part in ["_b: [T]", "_c: T?", "_d: H0<T>", "_e: [T?]"] {
                assert!(source.contains(part), "{part} in {source}");
            }
            let call = source
                .lines()
                .find(|line| line.starts_with("    let _ = later0("))
                .unwrap_or_else(|| panic!("a call of `later0` in {source}"));
            for part in ["same(", "none_of()", "[none_of()]", "H0(v: "] {
                assert!(call.contains(part), "{part} in {call}");
            }
        }
        assert!(found, "no seed declares `later0`");
    }

    /// Every row of the table of replacements has a site in the programs of the seeds, and a
    /// site is the argument alone: what comes before it opens the argument list or names the
    /// field, and what comes after it ends the argument.
    #[test]
    fn the_seeds_should_hold_a_bound_site_for_each_replacement() {
        let mut rows = std::collections::BTreeSet::new();
        for data in crate::generate::coverage::seeds() {
            let g = crate::generate::generate(&data);
            for site in &g.bound_sites {
                let (before, after) = (&g.program[..site.at.start], &g.program[site.at.end..]);
                assert!(
                    before.ends_with('(') || before.ends_with("v: "),
                    "{before}`{}`",
                    &g.program[site.at.clone()]
                );
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
        }
        let rows: Vec<(String, &str)> = rows.into_iter().collect();
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
        assert_eq!(rows, expected);
    }

    /// `later<k>` takes the bounded parameter bare, then in each form that #283 broke, and its call
    /// has the site as its first argument.
    #[test]
    fn a_later_call_should_have_its_bound_site_first() {
        let mut found = false;
        for data in crate::generate::coverage::seeds() {
            let g = crate::generate::generate(&data);
            for site in &g.bound_sites {
                let line = g.program[..site.at.start]
                    .lines()
                    .last()
                    .unwrap_or_default();
                if line.starts_with("    let _ = later") {
                    found = true;
                    let call = g.program[site.at.start..]
                        .lines()
                        .next()
                        .unwrap_or_default();
                    for part in ["same(", "none_of()", "[none_of()]", "H"] {
                        assert!(call.contains(part), "{part} in {call}");
                    }
                }
            }
        }
        assert!(found, "no seed calls `later`");
    }
}
