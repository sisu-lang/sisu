//! The program and its items: `const`s, functions, and `main`.

use std::fmt::Write as _;

use super::expr::{CHARS, OPS, arithmetic_value};
use super::scope::{Class, Const, Enum, Field, Func, Ty, Var, Variant};
use super::stmt::Jumps;
use super::writer::Piece;
use super::{Answer, Cost, DEPTHS, Gen, Known, MAX_NESTING, MAX_SIZE};

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

/// A known site (decision 3): a function `k<n>` of straight-line `i64` or `u8` arithmetic
/// over its two parameters, a call of it on literals, and what the call gives, worked out
/// here.
struct KnownSite {
    marker: String,
    function: String,
    call: String,
    answer: Result<i64, &'static str>,
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
            self.w.text(&format!(", {param}: {ty}"));
        }
        self.w.text(")");
        if ret != Ty::Unit {
            self.w.text(&format!(" -> {ret}"));
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
        self.w.text(&format!("class K{id} "));
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
            self.w.text(&format!("{keyword}{name}: {ty}"));
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
        self.w.close();
        self.w.line();
    }

    /// `enum E { V ... }`: variants with no payload, with fields, or with labeled fields. The
    /// first holds only types declared before; a later one may hold the enum itself, which a
    /// value built finite ends in the first.
    pub(crate) fn enumeration(&mut self) {
        let id = self.scope.enums.len();
        self.w.text(&format!("enum E{id} "));
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
        self.scope.enums.push(Enum {
            variants,
            least,
            has_class,
            applied: None,
        });
        self.scope.types.push(Ty::enumeration(id, least));
        self.w.close();
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
