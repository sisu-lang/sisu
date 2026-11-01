//! The expression rules: each writes one construct of the wanted type. Every expression but a
//! literal, a name and a call is written in parentheses, so it is an operand wherever it goes.

use std::ops::Range;

use super::scope::{Nodes, RESULT, Scope, Ty, Var};
use super::stmt::MAX_BLOCKS;
use super::writer::{Mark, Rewrite, Site, allowed};
use super::{Gen, LINE, MAX_BOUND, MAX_LOOPS, MAX_NESTING, MAX_SIZE, PART_SIZE};

/// What a rule wrote.
#[derive(Clone, Copy)]
pub(crate) struct Info {
    /// The most size the value reaches (decision 5): for a `string`, its chars.
    pub(crate) size: u64,
    /// Whether it has no effect and cannot panic, and reads no field, element or mutable
    /// local (decision 2).
    pub(crate) pure: bool,
    /// For an array: how many elements it holds at least.
    pub(crate) len: u64,
    /// Whether `check/constant.rs` folds it, with no step that overflows or divides by zero.
    pub(crate) folds: bool,
    /// The value of an `i64` or a `u8` the generator knows.
    pub(crate) value: Option<i64>,
}

impl Info {
    fn leaf(size: u64) -> Info {
        Info {
            size,
            pure: true,
            len: 0,
            folds: false,
            value: None,
        }
    }

    /// A literal, or a constant's name, of the value `value` if an integer.
    fn literal(size: u64, value: Option<i64>) -> Info {
        Info {
            folds: true,
            value,
            ..Info::leaf(size)
        }
    }

    /// The value of a rule built from `parts`, which has an effect or may panic unless `pure`.
    pub(crate) fn of(parts: &[Info], size: u64, pure: bool) -> Info {
        Info {
            pure: pure && parts.iter().all(|part| part.pure),
            ..Info::leaf(size)
        }
    }

    /// The value of an operator or a conversion over `parts`, which folds when they all do and
    /// it is pure.
    fn folding(parts: &[Info], size: u64, pure: bool) -> Info {
        let info = Info::of(parts, size, pure);
        Info {
            folds: info.pure && parts.iter().all(|part| part.folds),
            ..info
        }
    }
}

/// The arithmetic operators, as written.
pub(crate) const OPS: &[&str] = &[" + ", " - ", " * ", " / ", " % "];

/// `l OPS[op] r` on a `ty`, an `i64` or a `u8`, as the run time has it: the value, or the
/// message it panics with.
pub(crate) fn arithmetic_value(ty: &Ty, op: usize, l: i64, r: i64) -> Result<i64, &'static str> {
    if op >= 3 && r == 0 {
        return Err("division by zero");
    }
    let value = match op {
        0 => l.checked_add(r),
        1 => l.checked_sub(r),
        2 => l.checked_mul(r),
        3 => l.checked_div(r),
        _ => l.checked_rem(r),
    };
    value
        .filter(|n| *ty != Ty::U8 || (0..=255).contains(n))
        .ok_or("integer overflow")
}

/// Every this many expressions, the value of one goes through `dbg`.
const DBG_EVERY: u64 = 37;

/// The types a `const` can have.
const CONSTANT: &[Ty] = Ty::VALUES;

/// The size of an array of `len` elements of at most `element` each (decision 5).
pub(crate) fn array_size(len: u64, element: u64) -> u64 {
    len + len * element
}

/// The rules an expression can be written by.
#[derive(Clone, Copy, Debug)]
enum Rule {
    Literal,
    Name,
    /// `(v[i])` of an array local, in bounds.
    Element,
    Negate,
    Not,
    Arithmetic,
    Logic,
    Equal,
    Order,
    /// `i64(e)` of a `u8` or a `char`, or `f64(e)` of an `i64`.
    Convert,
    /// `(i64(e) ?? d)` of an `f64`, `(u8(e) ?? d)` or `(char(e) ?? d)` of an `i64`.
    Fallible,
    /// A method of `check/number.rs` with an exact result.
    Math,
    /// `is_nan`, `is_finite` or `is_infinite` of an exact `f64`.
    Classify,
    Concat,
    Interpolate,
    ReadLine,
    Call,
    /// `h(a)` or `h<A>(a)`, a call of a generic function.
    Generic,
    If,
    Block,
    /// `({ let mut c = n loop { ... } })`, a `loop` value.
    Loop,
    /// A `T` where a `T?` is wanted.
    Wrap,
    /// `read_line()`, or `u8(e)`, `char(e)` or `i64(e)`, as the optional they give.
    Raw,
    /// `(a ?? b)`.
    Coalesce,
    /// `[v; n]`.
    Fill,
    /// `chars`, `bytes` or `words` of a `string`.
    Split,
    Len,
    Sum,
    Contains,
    /// `string(e)` of a `[char]`.
    FromChars,
    Index,
    /// `((e).g)` of a class object.
    Field,
    /// `((o)?.g)` of an optional class object.
    SafeField,
    /// `((o)?.m(...))` of an optional class object.
    SafeMethod,
    /// `(a is b)` of two class objects.
    Is,
    Match,
}

/// The `f64` methods with an exact result and no argument.
const ROUNDING: &[&str] = &["abs", "sqrt", "floor", "ceil", "trunc", "round"];
/// The methods of `i64` and `f64` that take arguments of the receiver's type.
const BOUNDS: &[(&str, usize)] = &[("min", 1), ("max", 1), ("clamp", 2)];
/// The libm methods, whose results are inexact (decision 11), and their argument counts.
const LIBM: &[(&str, usize)] = &[
    ("sin", 0),
    ("cos", 0),
    ("tan", 0),
    ("asin", 0),
    ("acos", 0),
    ("atan", 0),
    ("atan2", 1),
    ("exp", 0),
    ("ln", 0),
    ("log10", 0),
    ("log2", 0),
    ("pow", 1),
    ("hypot", 1),
];
const CLASSIFY: &[&str] = &["is_nan", "is_finite", "is_infinite"];

/// Chars a string literal may hold, as written: no `@` (decision 3), and no raw `{`, `}`, `"` or
/// `\`. The escapes from `\r` on hold a line break, which a panic message may not.
const STRING_CHARS: &[&str] = &[
    "a",
    "b",
    "z",
    "Q",
    "0",
    "7",
    " ",
    ".",
    ",",
    "!",
    "-",
    "+",
    "<",
    ">",
    "#",
    "é",
    "\\t",
    "\\\\",
    "\\\"",
    "\\'",
    "\\{",
    "\\}",
    "\\0",
    "\\u{1F600}",
    "\\r",
    "\\n",
];
/// The chars of `STRING_CHARS` a panic message may hold.
const MESSAGE_CHARS: usize = 24;
/// Char literals and their values, smallest first.
pub(crate) const CHARS: &[(&str, char)] = &[
    ("'a'", 'a'),
    ("'Z'", 'Z'),
    ("'0'", '0'),
    ("' '", ' '),
    ("'é'", 'é'),
    ("'\\n'", '\n'),
    ("'\\t'", '\t'),
    ("'\\\\'", '\\'),
    ("'\\''", '\''),
    ("'\"'", '"'),
    ("'\\0'", '\0'),
    ("'\\u{1F600}'", '\u{1F600}'),
    ("'\\{'", '{'),
    ("'}'", '}'),
];
/// The types a `match` tests by value, besides enums.
const MATCHED: &[Ty] = &[Ty::Bool, Ty::I64, Ty::U8, Ty::Char, Ty::String];
/// The strings a `match` on a `string` tests.
const WORDS: [&str; 5] = ["\"\"", "\"a\"", "\"b\"", "\"ab\"", "\"é\""];
/// How deep a variant pattern nests in another.
const MAX_PATTERN_DEPTH: u32 = 2;

/// An arm of a `match`: its pattern, what the pattern binds, and whether a guard follows.
struct Arm {
    pattern: String,
    binds: Vec<(String, Ty)>,
    guard: bool,
}

/// The chars a line of stdin holds: no `@` (decision 3).
const LINE_CHARS: &[u8] = b"abcxyzABC0189 .,-+";

impl Gen<'_> {
    /// Writes an expression of type `ty`, a value type, holding at most `max` chars if a
    /// `string`.
    ///
    /// After `self.synth` is set, a `u8`, an array or an optional is one of its own type, for a
    /// position with no expected type: the checker types an integer literal there by the other
    /// operands, and an operand made only of literals, as `({ 5 })` is, as an `i64`; it cannot
    /// type `None` or `[]` at all. Such a value is at least `ty.least()`, which `max` holds.
    pub(crate) fn expr(&mut self, ty: &Ty, max: u64) -> Info {
        let synth = std::mem::take(&mut self.synth) && ty.inferred();
        let typed = std::mem::take(&mut self.typed);
        // Now and then the value goes through `dbg`. The count reads no byte of the data, so no
        // other choice of any seed moves. `dbg` has the type of its argument, which is on one
        // line and which no rewrite changes. So the line `dbg` writes is the same in the program
        // and in its rewritten copy.
        self.expressions += 1;
        let debugged = self.expressions.is_multiple_of(DBG_EVERY) && *ty != Ty::Unit;
        let (one_line, frozen) = (self.one_line, self.frozen);
        if debugged {
            self.one_line = true;
            self.frozen = true;
        }
        debug_assert!(!synth || ty.least() <= max, "no {ty} fits {max}");
        debug_assert!(ty.floor() <= max, "no {ty} fits {max}");
        let u8 = synth && *ty == Ty::U8;
        // `(u8(e) ?? d)` is the smallest `u8` of its own type.
        let mut rules = vec![if u8 { Rule::Fallible } else { Rule::Literal }];
        if self.has_name(ty, max) {
            rules.push(Rule::Name);
        }
        if self.scope.vars.iter().any(|var| indexable(var, ty, max)) {
            rules.push(Rule::Element);
        }
        if self.nesting < MAX_NESTING && self.spend(1) {
            self.compound_rules(ty, max, synth, &mut rules);
        }
        if u8 {
            rules.retain(|rule| !matches!(rule, Rule::Fallible));
            rules.insert(0, Rule::Fallible);
        }
        let rule = rules[self.pick(rules.len())];
        let start = self.w.mark();
        if debugged {
            self.w.text("dbg(");
        }
        self.nesting += 1;
        self.bare = typed && matches!(rule, Rule::Literal);
        let mut info = self.rule(rule, ty, max, synth);
        self.bare = false;
        self.nesting -= 1;
        if debugged {
            self.w.text(")");
            (self.one_line, self.frozen) = (one_line, frozen);
            // It writes to standard error.
            info = Info::of(&[info], info.size, false);
        }
        // A value of a type without a size counts none, whatever its parts held.
        if !ty.sized() {
            info.size = 0;
        }
        debug_assert!(
            info.size <= max,
            "{rule:?} wrote {} chars over {max}",
            info.size
        );
        self.rewrite(start, ty, &info);
        // A later operand runs after this one's effect.
        self.effect |= !info.pure;
        info
    }

    /// Now and then rewrites the expression of type `ty` from `start` on in the rewritten
    /// copy, by a rewrite whose condition its site meets (decision 2).
    fn rewrite(&mut self, start: Mark, ty: &Ty, info: &Info) {
        let site = Site {
            pure: info.pure,
            unconditional: self.scope.conditional == 0 && !self.effect,
            folds: info.folds && CONSTANT.contains(ty),
            ..Site::default()
        };
        // A `let` needs a statement to go before, and a value to hold.
        let can_bind = self.w.can_bind() && *ty != Ty::Unit;
        let mut rewrites = [Rewrite::Block, Rewrite::Let, Rewrite::Const]
            .into_iter()
            .filter(|&rewrite| (rewrite != Rewrite::Let || can_bind) && allowed(rewrite, &site));
        let count = rewrites.clone().count();
        if count == 0 || self.rewrite_pick(8) != 7 {
            return;
        }
        let pick = self.rewrite_pick(count);
        // `dbg` writes its argument's text, which no rewrite may change. The picks above are
        // made all the same, so the rewrites after it stay where they were.
        if self.frozen {
            return;
        }
        match rewrites
            .nth(pick)
            .expect("the pick should be below the count")
        {
            Rewrite::Block => self.w.block(start),
            Rewrite::Let => self.w.bind(start, ty),
            Rewrite::Const => self.w.constant(start, ty),
            Rewrite::DeadIf | Rewrite::Swap => unreachable!("only a statement or an `if` takes it"),
        }
    }

    fn compound_rules(&self, ty: &Ty, max: u64, synth: bool, rules: &mut Vec<Rule>) {
        let own: &[Rule] = match ty {
            Ty::I64 => &[
                Rule::Negate,
                Rule::Arithmetic,
                Rule::Convert,
                Rule::Fallible,
                Rule::Math,
                Rule::Len,
                Rule::Sum,
            ],
            Ty::U8 => &[Rule::Arithmetic, Rule::Fallible],
            Ty::F64 => &[
                Rule::Negate,
                Rule::Arithmetic,
                Rule::Convert,
                Rule::Math,
                Rule::Sum,
            ],
            Ty::Bool => &[
                Rule::Not,
                Rule::Logic,
                Rule::Equal,
                Rule::Order,
                Rule::Classify,
                Rule::Contains,
            ],
            Ty::Char => &[Rule::Fallible],
            Ty::String => &[Rule::Concat, Rule::Interpolate, Rule::FromChars],
            Ty::Array(element) => {
                if element.floor() <= max && !element.shared() {
                    rules.push(Rule::Fill);
                }
                // `words` gives at most one more than its chars.
                if matches!(**element, Ty::Char | Ty::U8) || **element == Ty::String && max > 0 {
                    rules.push(Rule::Split);
                }
                // `bytes` is the one way to a `[u8]` but literals and fills, so it comes more.
                if **element == Ty::U8 {
                    rules.extend([Rule::Split, Rule::Split]);
                }
                &[]
            }
            Ty::Optional(payload) => {
                let nested = matches!(**payload, Ty::Optional(_));
                if !synth && payload.floor() <= max && (!nested || self.has_name(payload, max)) {
                    rules.push(Rule::Wrap);
                }
                if !self.fields(safe_member(ty), max).is_empty() {
                    rules.push(Rule::SafeField);
                }
                // Twice: a method of the payload's type is rare.
                if self
                    .callees(safe_member(ty), max)
                    .iter()
                    .any(|&f| self.method(f))
                {
                    rules.extend([Rule::SafeMethod, Rule::SafeMethod]);
                }
                // A name of an optional of `ty` is rare, so a `??` on it comes often.
                if self.has_name(&Ty::Optional(Box::new(ty.clone())), max) {
                    rules.extend([Rule::Coalesce; 4]);
                }
                if matches!(**payload, Ty::U8 | Ty::Char | Ty::I64)
                    || **payload == Ty::String && max >= LINE
                {
                    rules.push(Rule::Raw);
                }
                &[]
            }
            Ty::Unit | Ty::Class { .. } | Ty::Enum { .. } => &[],
        };
        rules.extend_from_slice(own);
        if *ty == Ty::Bool && !self.scope.classes.is_empty() {
            rules.push(Rule::Is);
        }
        if !self.fields(ty, max).is_empty() {
            rules.push(Rule::Field);
        }
        if *ty == Ty::String && max >= LINE {
            rules.push(Rule::ReadLine);
        }
        if *ty != Ty::Unit && ty.least() <= max {
            rules.push(Rule::Coalesce);
            rules.push(Rule::Index);
        }
        if !self.callees(ty, max).is_empty() {
            rules.push(Rule::Call);
        }
        if !self.gcallees(ty, max).is_empty() {
            rules.push(Rule::Generic);
        }
        rules.push(Rule::If);
        if !self.one_line && self.blocks < MAX_BLOCKS {
            rules.push(Rule::Block);
            // A `break` value must have one (E0043), and the `loop` counts toward `MAX_LOOPS`.
            if *ty != Ty::Unit && self.cost.depth() < MAX_LOOPS && self.fuel >= 2 {
                rules.push(Rule::Loop);
            }
            // A `match` near the top only: its arms would otherwise take most of the fuel.
            if self.nesting < 3 {
                rules.push(Rule::Match);
            }
        }
    }

    /// `synth` passes on to the operand that gives the rule's type.
    fn rule(&mut self, rule: Rule, ty: &Ty, max: u64, synth: bool) -> Info {
        match rule {
            Rule::Literal => match ty {
                Ty::Array(element) => self.array_literal(element, max, synth),
                Ty::Optional(payload) if synth => self.maybe(payload, max),
                Ty::Optional(payload) => self.none(payload),
                _ => self.literal(ty, max),
            },
            Rule::Name => self.name(ty, max),
            Rule::Negate | Rule::Not => {
                self.w.text(if *ty == Ty::Bool { "(!(" } else { "(-(" });
                let operand = self.expr(ty, max);
                self.w.text("))");
                // Only the negation of the smallest `i64` overflows.
                let value = operand.value.and_then(i64::checked_neg);
                Info {
                    value,
                    ..Info::folding(&[operand], 0, *ty != Ty::I64 || value.is_some())
                }
            }
            Rule::Arithmetic => self.arithmetic(ty, synth),
            Rule::Logic => {
                self.w.text("(");
                let lhs = self.expr(&Ty::Bool, 0);
                self.one_of(&[" && ", " || "]);
                let rhs = self.conditionally(|g| g.expr(&Ty::Bool, 0));
                self.w.text(")");
                Info::folding(&[lhs, rhs], 0, true)
            }
            Rule::Equal => {
                let operand = self.eq_type();
                self.w.text("(");
                self.synth = true;
                let lhs = self.expr(&operand, MAX_SIZE);
                self.one_of(&[" == ", " != "]);
                let rhs = self.expr(&operand, MAX_SIZE);
                self.w.text(")");
                Info::folding(&[lhs, rhs], 0, true)
            }
            Rule::Order => self.order(),
            Rule::Convert => self.convert(ty),
            Rule::Fallible => self.fallible(ty),
            Rule::Math => self.math(ty),
            Rule::Classify => {
                self.w.text("(");
                let operand = self.expr(&Ty::F64, 0);
                let method = CLASSIFY[self.pick(CLASSIFY.len())];
                self.w.text(&format!(").{method}()"));
                Info::of(&[operand], 0, true)
            }
            Rule::Concat => {
                self.w.text("(");
                let lhs = self.expr(&Ty::String, max);
                self.w.text(" + ");
                let rhs = self.expr(&Ty::String, max - lhs.size);
                self.w.text(")");
                Info::folding(&[lhs, rhs], lhs.size + rhs.size, true)
            }
            Rule::Interpolate => self.interpolate(max),
            Rule::ReadLine => {
                self.w.text("(read_line() ?? ");
                let default = self.conditionally(|g| g.expr(&Ty::String, max));
                self.w.text(")");
                Info::of(&[default], default.size.max(LINE), false)
            }
            Rule::Call => self.call(ty, max),
            Rule::Generic => self.gcall(ty, max),
            Rule::If => self.if_expr(ty, max, synth),
            Rule::Block => {
                self.w.text("(");
                let info = self.value_block(ty, max, synth);
                self.w.text(")");
                info
            }
            Rule::Loop => self.loop_value(ty, max, synth),
            Rule::Wrap => self.wrap(ty, max),
            Rule::Raw => self.raw(ty),
            Rule::Coalesce => self.coalesce(ty, max),
            Rule::Fill => self.fill(ty, max, synth),
            Rule::Split => self.split(ty, max),
            Rule::Len => self.len(),
            Rule::Sum => self.sum(ty),
            Rule::Contains => self.contains(),
            Rule::FromChars => {
                self.w.text("string(");
                let chars = self.expr(&Ty::Array(Box::new(Ty::Char)), max);
                self.w.text(")");
                Info::of(&[chars], chars.size, true)
            }
            Rule::Index => self.index(ty, max),
            Rule::Element => self.element(ty, max),
            Rule::Field => self.field(ty, max, false),
            Rule::SafeField => self.field(ty, max, true),
            Rule::SafeMethod => self.safe_method(ty, max),
            Rule::Is => self.identity(),
            Rule::Match => self.match_expr(ty, max, synth),
        }
    }

    /// `((o).g)` of a field of type `ty`, or if `safe`, `((o)?.g)` of one of the type
    /// `safe_member` says.
    fn field(&mut self, ty: &Ty, max: u64, safe: bool) -> Info {
        let wanted = if safe { safe_member(ty) } else { ty };
        let fields = self.fields(wanted, max);
        let (class, field) = fields[self.pick(fields.len())];
        let (name, cap) = {
            let field = &self.scope.classes[class].fields[field];
            (field.name.clone(), field.cap)
        };
        self.w.text("((");
        let receiver = self.receiver(class, safe);
        self.w
            .text(&format!("){}{name})", if safe { "?." } else { "." }));
        // A field may be `mut`, and an assignment between here and a `let` moved before this
        // statement would change it (decision 2).
        Info::of(&[receiver], cap, false)
    }

    /// `((o)?.m(depth, args))` of a method that returns the type `safe_member` says of `ty`.
    fn safe_method(&mut self, ty: &Ty, max: u64) -> Info {
        let methods: Vec<usize> = self
            .callees(safe_member(ty), max)
            .into_iter()
            .filter(|&f| self.method(f))
            .collect();
        let method = methods[self.pick(methods.len())];
        self.invoke(method, true)
    }

    /// `(a is b)` of two objects of one class.
    fn identity(&mut self) -> Info {
        let class = self.pick(self.scope.classes.len());
        self.w.text("(");
        let lhs = self.receiver(class, false);
        self.w.text(" is ");
        let rhs = self.receiver(class, false);
        self.w.text(")");
        Info::of(&[lhs, rhs], 0, true)
    }

    /// The fields of a type `ty` within `max`: each one's class and its index there.
    fn fields(&self, ty: &Ty, max: u64) -> Vec<(usize, usize)> {
        let classes = self.scope.classes.iter().enumerate();
        classes
            .flat_map(|(c, class)| {
                class
                    .fields
                    .iter()
                    .enumerate()
                    .filter_map(move |(f, field)| {
                        (field.ty == *ty && field.cap <= max).then_some((c, f))
                    })
            })
            .collect()
    }

    /// An object of the class `class`, or an optional one if `safe`: what a `.` or a `?.`
    /// reads.
    fn receiver(&mut self, class: usize, safe: bool) -> Info {
        let ty = self.scope.class_ty(class);
        let size = self.scope.classes[class].size;
        if safe {
            self.synth = true;
            self.expr(&Ty::Optional(Box::new(ty)), size)
        } else {
            self.expr(&ty, size)
        }
    }

    /// Whether the function `f` is a method.
    fn method(&self, f: usize) -> bool {
        self.scope.funcs[f].owner.is_some()
    }

    /// `K(g: a, h: b)`: every field, each within its cap. An instance of a generic class is
    /// `G<n><A>(g: a)`, or `G<n>(g: a)` as often as its fields name every type parameter, where
    /// each field is of its own type (`Gen::expr`).
    pub(crate) fn object(&mut self, ty: &Ty) -> Info {
        let Ty::Class { id, size, .. } = *ty else {
            unreachable!("only a class has objects")
        };
        // A field of its own type holds at least what `Ty::least` says.
        let fits = self.scope.classes[id]
            .fields
            .iter()
            .all(|field| field.ty.least() <= field.cap);
        let infer = fits && self.scope.classes[id].inferable && self.pick(2) == 0;
        let name = if infer { ty.bare() } else { ty.to_string() };
        // An alias of the class builds it as the class does, with the alias's fixed arguments.
        let name = if infer {
            name
        } else {
            self.constructor(ty, name)
        };
        self.w.text(&format!("{name}("));
        let mut parts = Vec::new();
        for f in 0..self.scope.classes[id].fields.len() {
            let (name, field, cap) = {
                let field = &self.scope.classes[id].fields[f];
                (field.name.clone(), field.ty.clone(), field.cap)
            };
            if f > 0 {
                self.w.text(", ");
            }
            self.w.text(&format!("{name}: "));
            self.synth = infer;
            parts.push(self.expr(&field, cap));
        }
        self.w.text(")");
        Info::of(&parts, size, true)
    }

    /// `E.V`, `E.V(a, b)` or `E.V(l: a, m: b)` within `max`, of any variant but `except`. A
    /// variant that holds the enum itself comes only while fuel lasts, so a value ends.
    pub(crate) fn variant(&mut self, ty: &Ty, max: u64, except: Option<usize>) -> Info {
        let Ty::Enum { id, .. } = *ty else {
            unreachable!("only an enum has variants")
        };
        let fitting = |g: &Self, recursive: bool| -> Vec<usize> {
            let variants = &g.scope.enums[id].variants;
            (0..variants.len())
                .filter(|&v| {
                    let floor: u64 = variants[v].fields.iter().map(Ty::floor).sum();
                    Some(v) != except && floor < max && variants[v].recursive == recursive
                })
                .collect()
        };
        let mut variants = fitting(self, false);
        let recursive = fitting(self, true);
        if !recursive.is_empty() && self.nesting < MAX_NESTING && self.spend(1) {
            variants.extend(recursive);
        }
        let v = variants[self.pick(variants.len())];
        let (name, fields, labels, inferable) = {
            let variant = &self.scope.enums[id].variants[v];
            (
                variant.name.clone(),
                variant.fields.clone(),
                variant.labels.clone(),
                variant.inferable,
            )
        };
        // Where the fields name every type parameter of a generic enum, the type arguments may
        // be left out; each field is then of its own type (`Gen::expr`).
        let fits = fields.iter().map(Ty::least).fold(0, u64::saturating_add) < max;
        let infer = inferable && fits && self.pick(2) == 0;
        let enum_name = if infer { ty.bare() } else { ty.to_string() };
        let enum_name = if infer {
            enum_name
        } else {
            self.constructor(ty, enum_name)
        };
        // `Ok(..)` and `Err(..)` take their `Result` from the type that the `let` writes.
        let is_result = matches!(ty, Ty::Enum { applied: Some(a), .. } if a.generic == RESULT);
        if is_result && std::mem::take(&mut self.bare) && self.pick(2) == 0 {
            self.w.text(&name);
        } else {
            self.w.text(&format!("{enum_name}.{name}"));
        }
        if fields.is_empty() {
            return Info::leaf(1);
        }
        self.w.text("(");
        let mut left = max - 1;
        let mut parts = Vec::new();
        for (f, field) in fields.iter().enumerate() {
            if f > 0 {
                self.w.text(", ");
            }
            if let Some(label) = labels.get(f) {
                self.w.text(&format!("{label}: "));
            }
            let later: u64 = fields[f + 1..]
                .iter()
                .map(|ty| if infer { ty.least() } else { ty.floor() })
                .sum();
            self.synth = infer;
            let part = self.expr(field, left - later);
            left -= part.size;
            parts.push(part);
        }
        self.w.text(")");
        Info::of(&parts, max - left, true)
    }

    /// `((s).len())` of a `string` or `((a).len())` of an array.
    fn len(&mut self) -> Info {
        self.w.text("((");
        self.synth = true;
        let receiver = if self.pick(2) == 0 {
            self.expr(&Ty::String, MAX_SIZE)
        } else {
            let element = self.bare_type();
            self.expr(&Ty::Array(Box::new(element)), MAX_SIZE)
        };
        self.w.text(").len())");
        Info::of(&[receiver], 0, true)
    }

    /// `((a).sum())` of an array of `ty`.
    fn sum(&mut self, ty: &Ty) -> Info {
        self.w.text("((");
        self.synth = true;
        let receiver = self.expr(&Ty::Array(Box::new(ty.clone())), MAX_SIZE);
        self.w.text(").sum())");
        // An `i64` sum may overflow.
        Info::of(&[receiver], 0, *ty == Ty::F64)
    }

    /// `((a).contains(v))`.
    fn contains(&mut self) -> Info {
        let element = self.eq_type();
        self.w.text("((");
        self.synth = true;
        let receiver = self.expr(&Ty::Array(Box::new(element.clone())), MAX_SIZE);
        self.w.text(").contains(");
        let value = self.expr(&element, MAX_SIZE);
        self.w.text("))");
        // A call of the prelude's `contains`, which runs the element's `eq`: not pure for every
        // type (see `PURE_PRELUDE`).
        Info::of(&[receiver, value], 0, false)
    }

    /// `[a, b, ...]`, of one element at least if `synth`, each one of its own type then.
    pub(crate) fn array_literal(&mut self, element: &Ty, max: u64, synth: bool) -> Info {
        let least = if synth {
            element.least()
        } else {
            element.floor()
        };
        let most = (max / (1 + least)).min(4);
        let len = u64::from(synth) + self.upto(most - u64::from(synth));
        self.w.text("[");
        let each = (max - len).checked_div(len).unwrap_or(0);
        let mut parts = Vec::new();
        for i in 0..len {
            if i > 0 {
                self.w.text(", ");
            }
            self.synth = synth;
            parts.push(self.expr(element, each));
        }
        self.w.text("]");
        let elements: u64 = parts.iter().map(|part| part.size).sum();
        Info {
            len,
            ..Info::of(&parts, len + elements, true)
        }
    }

    /// `[v; n]`, with a literal `n` of at most `MAX_BOUND`, so a `for` may run over it.
    pub(crate) fn fill(&mut self, ty: &Ty, max: u64, synth: bool) -> Info {
        let Ty::Array(element) = ty else {
            unreachable!("only an array is filled")
        };
        let least = if synth {
            element.least()
        } else {
            element.floor()
        };
        let len = self.upto((max / (1 + least)).min(MAX_BOUND));
        self.w.text("[");
        self.synth = synth;
        let value = self.expr(element, (max - len).checked_div(len).unwrap_or(max));
        self.w.text(&format!("; {len}]"));
        Info {
            len,
            ..Info::of(&[value], array_size(len, value.size), true)
        }
    }

    /// A `None` of an optional of `payload`. `Option<T>.None` writes its type, and the others take
    /// it from their context.
    fn none(&mut self, payload: &Ty) -> Info {
        match self.pick(3) {
            0 => self.w.text("None"),
            1 => self.w.text("Option.None"),
            _ => self.w.text(&format!("Option<{payload}>.None")),
        }
        Info::leaf(0)
    }

    /// `(if c { v } else { None })`, an optional of its own type.
    fn maybe(&mut self, payload: &Ty, max: u64) -> Info {
        self.w.text("(if ");
        let cond = self.expr(&Ty::Bool, 0);
        self.w.text(" { ");
        self.synth = true;
        let value = self.conditionally(|g| g.expr(payload, max));
        self.w.text(" } else { None })");
        Info::of(&[cond, value], value.size, true)
    }

    /// `read_line()`, `u8(e)` or `char(e)` of an `i64`, or `i64(e)` of an `f64`: the optional
    /// they give.
    fn raw(&mut self, ty: &Ty) -> Info {
        let Ty::Optional(payload) = ty else {
            unreachable!("only an optional is raw")
        };
        if **payload == Ty::String {
            self.w.text("read_line()");
            return Info::of(&[], LINE, false);
        }
        let source = if **payload == Ty::I64 {
            Ty::F64
        } else {
            Ty::I64
        };
        self.w.text(&format!("{payload}("));
        let operand = self.expr(&source, 0);
        self.w.text(")");
        Info::of(&[operand], 0, true)
    }

    /// `(a ?? b)`: of a `T?` `a` and a `T` `b` for a `T`, of two `T?` for a `T?`, or of a
    /// named `T??` `a` and a `T?` `b` for a `T?`: `??` takes one layer off.
    fn coalesce(&mut self, ty: &Ty, max: u64) -> Info {
        let optional = match ty {
            Ty::Optional(_) => ty.clone(),
            _ => Ty::Optional(Box::new(ty.clone())),
        };
        let nested = Ty::Optional(Box::new(ty.clone()));
        let named = optional == *ty && self.has_name(&nested, max);
        self.w.text("(");
        let lhs = if named {
            self.name(&nested, max)
        } else {
            self.synth = true;
            self.expr(&optional, max)
        };
        self.w.text(" ?? ");
        // The `T?` the right side gives must be one: a `T` there would make the result a `T`.
        // After a `T??`, the right side has the `T?` it gives as its expected type.
        self.synth = optional == *ty && !named;
        let rhs = self.conditionally(|g| g.expr(ty, max));
        self.w.text(")");
        Info::of(&[lhs, rhs], lhs.size.max(rhs.size), true)
    }

    /// `chars`, `bytes` or `words` of a `string`, as the array's element wants.
    fn split(&mut self, ty: &Ty, max: u64) -> Info {
        let Ty::Array(element) = ty else {
            unreachable!("only an array is split")
        };
        // A char is at most 4 bytes, and the words of `n` chars at most `n + 1` in size.
        let (method, chars) = match **element {
            Ty::Char => ("chars", max),
            Ty::U8 => ("bytes", max / 4),
            _ => ("words", max - 1),
        };
        self.w.text("((");
        let receiver = self.expr(&Ty::String, chars);
        self.w.text(&format!(").{method}())"));
        let size = match **element {
            Ty::Char => receiver.size,
            Ty::U8 => 4 * receiver.size,
            _ => receiver.size + 1,
        };
        Info::of(&[receiver], size, true)
    }

    /// `(v[i])` of an array local whose elements are `ty`s of at most `max`, with `i` below
    /// its length.
    fn element(&mut self, ty: &Ty, max: u64) -> Info {
        let vars: Vec<usize> = (0..self.scope.vars.len())
            .filter(|&v| indexable(&self.scope.vars[v], ty, max))
            .collect();
        let pick = self.pick(vars.len());
        let var = &self.scope.vars[vars[pick]];
        let (name, len, elem) = (var.name.clone(), var.len, var.elem);
        let at = self.upto(len - 1);
        self.w.text(&format!("({name}[{at}])"));
        Info::of(&[], elem, false)
    }

    /// `([a, b, ...][i])` of a literal, with `i` in bounds.
    fn index(&mut self, ty: &Ty, max: u64) -> Info {
        // Its elements, of one at least, hold `max` between them at most if they have a size.
        let most = if ty.sized() {
            MAX_SIZE.min(max + 1)
        } else {
            MAX_SIZE
        };
        self.w.text("(");
        let array = self.array_literal(ty, most, true);
        let at = self.upto(array.len - 1);
        self.w.text(&format!("[{at}])"));
        Info::of(&[array], array.size - array.len, false)
    }

    /// Runs `write` where the code may not run (decision 2).
    pub(crate) fn conditionally<T>(&mut self, write: impl FnOnce(&mut Self) -> T) -> T {
        self.scope.conditional += 1;
        let result = write(self);
        self.scope.conditional -= 1;
        result
    }

    /// A value type, the smallest first.
    pub(crate) fn value_type(&mut self) -> Ty {
        Ty::VALUES[self.pick(Ty::VALUES.len())].clone()
    }

    /// A value type, an array or an optional, nested at most twice.
    pub(crate) fn any_type(&mut self) -> Ty {
        self.type_within(2, true)
    }

    /// A type of `any_type` with no optional of an optional in it: one that a value written
    /// with no expected type can have (`Ty::least`).
    pub(crate) fn bare_type(&mut self) -> Ty {
        self.type_within(2, false)
    }

    /// A `bare_type` whose values `==` compares: one with no class in it. A class compares with
    /// `is`, or against `None`, and has no `Eq`.
    pub(crate) fn eq_type(&mut self) -> Ty {
        loop {
            let ty = self.bare_type();
            if !self.scope.contains_class(&ty) {
                return ty;
            }
        }
    }

    /// With `nest`, an optional may be the payload of an optional.
    fn type_within(&mut self, nesting: u32, nest: bool) -> Ty {
        let values = Ty::VALUES.len();
        // The declared types share one choice, so they take no more of the values' as they grow;
        // so do the instances of generic types, whose number does not crowd out the declared.
        let declared = usize::from(!self.scope.types.is_empty());
        let generic = usize::from(!self.scope.generic_types.is_empty());
        let choice = self.pick(values + declared + generic + if nesting > 0 { 2 } else { 0 });
        if choice < values {
            return Ty::VALUES[choice].clone();
        }
        if choice < values + declared {
            let pick = self.pick(self.scope.types.len());
            return self.scope.types[pick].clone();
        }
        if choice < values + declared + generic {
            let pick = self.pick(self.scope.generic_types.len());
            return self.scope.generic_types[pick].clone();
        }
        let choice = choice - declared - generic;
        // Arrays of strings, whose elements `+=` may grow, come more often.
        let inner = if choice == values && self.pick(2) == 0 {
            Ty::String
        } else {
            self.type_within(nesting - 1, nest)
        };
        match (choice - values, inner) {
            (0, inner) => Ty::Array(Box::new(inner)),
            (_, Ty::Optional(payload)) if !nest => Ty::Optional(payload),
            (_, inner) => Ty::Optional(Box::new(inner)),
        }
    }

    pub(crate) fn has_name(&self, ty: &Ty, max: u64) -> bool {
        names(&self.scope, ty, max).next().is_some()
    }

    pub(crate) fn name(&mut self, ty: &Ty, max: u64) -> Info {
        let pick = self.pick(names(&self.scope, ty, max).count());
        let (name, info) = names(&self.scope, ty, max)
            .nth(pick)
            .expect("the rule should be chosen only when a name fits");
        self.w.text(name);
        info
    }

    pub(crate) fn literal(&mut self, ty: &Ty, max: u64) -> Info {
        match ty {
            Ty::I64 => {
                let (text, value) = self.int_literal();
                self.w.text(&text);
                Info::literal(0, Some(value))
            }
            Ty::U8 => {
                let (text, value) = self.u8_literal();
                self.w.text(&text);
                Info::literal(0, Some(value))
            }
            Ty::F64 => {
                let text = self.float_literal();
                self.w.text(&text);
                Info::literal(0, None)
            }
            Ty::Bool => {
                let text = if self.pick(2) == 0 { "false" } else { "true" };
                self.w.text(text);
                Info::literal(0, None)
            }
            Ty::Char => {
                let (text, _) = CHARS[self.pick(CHARS.len())];
                self.w.text(text);
                Info::literal(0, None)
            }
            Ty::String => {
                let len = self.upto(max.min(8));
                let text = self.string_text(len, STRING_CHARS.len());
                self.w.text(&format!("\"{text}\""));
                Info::literal(len, None)
            }
            Ty::Class { .. } => self.object(ty),
            Ty::Enum { .. } => self.variant(ty, max, None),
            Ty::Unit | Ty::Array(_) | Ty::Optional(_) => {
                unreachable!("`Rule::Literal` writes a literal of a scalar only")
            }
        }
    }

    /// An `i64` literal in one of its written forms, a negative one in parentheses, and its
    /// value.
    pub(crate) fn int_literal(&mut self) -> (String, i64) {
        // Arithmetic over a value near either end of `i64` overflows, so those come 4 in 64.
        let form = self.pick(64);
        match form {
            60 => {
                let n = self.int(100_000, i64::MAX);
                (separated(n), n)
            }
            61 => {
                let n = self.int(0, i64::MAX);
                (format!("{n:#x}"), n)
            }
            62 => ("9223372036854775807".to_string(), i64::MAX),
            63 => ("(-9223372036854775808)".to_string(), i64::MIN),
            _ => match form % 7 {
                0 => {
                    let n = self.int(0, 9);
                    (n.to_string(), n)
                }
                1 => {
                    let n = self.int(0, 1_000);
                    (n.to_string(), n)
                }
                2 => {
                    let n = self.int(1_000, 99_999);
                    (separated(n), n)
                }
                3 => {
                    let n = self.int(0, 4_095);
                    (format!("{n:#x}"), n)
                }
                4 => {
                    let n = self.int(0, 255);
                    (format!("{n:#b}"), n)
                }
                5 => {
                    let n = self.int(0, 4_095);
                    (format!("{n:#o}"), n)
                }
                _ => {
                    let n = self.int(1, 1_000);
                    (format!("(-{n})"), -n)
                }
            },
        }
    }

    /// A `u8` literal in one of its written forms, and its value.
    pub(crate) fn u8_literal(&mut self) -> (String, i64) {
        let n = self.int(0, 255);
        let text = match self.pick(3) {
            0 => n.to_string(),
            1 => format!("{n:#x}"),
            _ => format!("{n:#b}"),
        };
        (text, n)
    }

    /// An `f64` literal in one of its written forms, a negative one in parentheses, or one of
    /// `f64`'s constants.
    pub(crate) fn float_literal(&mut self) -> String {
        const CONSTANTS: &[&str] = &[
            "pi",
            "tau",
            "e",
            "infinity",
            "nan",
            "max",
            "min_positive",
            "epsilon",
        ];
        match self.pick(8) {
            0 => "0.0".to_string(),
            1 => format!("{}.{}", self.int(0, 100), self.int(0, 99)),
            2 => format!("{}e{}", self.int(1, 9), self.int(-300, 300)),
            3 => format!("{}.{}e+{}", self.int(0, 9), self.int(0, 9), self.int(0, 20)),
            4 => format!(
                "{}.{}",
                separated(self.int(1_000, 1_000_000)),
                self.int(0, 9)
            ),
            5 => [
                "1.7976931348623157e308",
                "5e-324",
                "2.2250738585072014e-308",
            ][self.pick(3)]
            .to_string(),
            6 => format!("(-{}.{})", self.int(0, 100), self.int(0, 9)),
            _ => format!("f64.{}", CONSTANTS[self.pick(CONSTANTS.len())]),
        }
    }

    /// The text of a string literal of `len` chars, each one of the first `chars` of
    /// `STRING_CHARS`.
    pub(crate) fn string_text(&mut self, len: u64, chars: usize) -> String {
        (0..len).map(|_| STRING_CHARS[self.pick(chars)]).collect()
    }

    /// A panic message: a literal without a line break.
    pub(crate) fn message(&mut self) -> String {
        let len = self.upto(8);
        let text = self.string_text(len, MESSAGE_CHARS);
        format!("\"{text}\"")
    }

    /// A line of stdin, without its line break.
    pub(crate) fn stdin_line(&mut self) -> String {
        let len = self.upto(LINE);
        (0..len)
            .map(|_| char::from(LINE_CHARS[self.pick(LINE_CHARS.len())]))
            .collect()
    }

    fn arithmetic(&mut self, ty: &Ty, synth: bool) -> Info {
        self.w.text("(");
        // `a op b` of `a` in `10..=28` and `b` in `1..=9` fits a `u8` and divides by no zero.
        let lhs = if matches!(ty, Ty::I64 | Ty::U8) && self.pick(8) != 0 {
            let n = self.int(10, 28);
            // With no expected type, a `u8` must be one by its own type (`Gen::expr`).
            if synth && *ty == Ty::U8 {
                self.w.text(&format!("(u8({n}) ?? {n})"));
                Info::leaf(0)
            } else {
                self.w.text(&n.to_string());
                Info::literal(0, Some(n))
            }
        } else {
            self.synth = synth;
            self.expr(ty, 0)
        };
        let op = self.pick(OPS.len());
        self.w.text(OPS[op]);
        let rhs = self.right_operand(ty);
        self.w.text(")");
        // Integer arithmetic is pure where the generator knows it neither overflows nor divides
        // by zero (decision 2).
        let value = lhs
            .value
            .zip(rhs.value)
            .and_then(|(l, r)| arithmetic_value(ty, op, l, r).ok());
        Info {
            value,
            ..Info::folding(&[lhs, rhs], 0, *ty == Ty::F64 || value.is_some())
        }
    }

    /// The right operand of arithmetic of `ty`: for an `i64`, mostly, and for a `u8`, always,
    /// a literal in `1..=9`, so the arithmetic rarely overflows or divides by zero; else any
    /// expression. Any other `u8` is as likely as not to overflow.
    pub(crate) fn right_operand(&mut self, ty: &Ty) -> Info {
        if *ty == Ty::U8 || *ty == Ty::I64 && self.pick(8) != 0 {
            let n = self.int(1, 9);
            self.w.text(&n.to_string());
            return Info::literal(0, Some(n));
        }
        self.expr(ty, 0)
    }

    /// A chain `a < b <= c` or `a > b >= c` of two or three operands of one ordered type.
    fn order(&mut self) -> Info {
        const ORDERED: &[Ty] = &[Ty::I64, Ty::U8, Ty::F64, Ty::Char, Ty::String];
        let operand = ORDERED[self.pick(ORDERED.len())].clone();
        let ops = if self.pick(2) == 0 {
            [" < ", " <= "]
        } else {
            [" > ", " >= "]
        };
        self.w.text("(");
        self.synth = true;
        let mut parts = vec![self.expr(&operand, MAX_SIZE)];
        for i in 0..=self.pick(2) {
            self.one_of(&ops);
            // A chain stops at its first false comparison, so an operand after the second may
            // not run.
            let part = if i == 0 {
                self.expr(&operand, MAX_SIZE)
            } else {
                self.conditionally(|g| g.expr(&operand, MAX_SIZE))
            };
            parts.push(part);
        }
        self.w.text(")");
        Info::folding(&parts, 0, true)
    }

    fn convert(&mut self, ty: &Ty) -> Info {
        let operand = match ty {
            Ty::F64 => {
                self.w.text("f64(");
                self.expr(&Ty::I64, 0)
            }
            _ if self.pick(2) == 0 => {
                self.w.text("i64(");
                self.expr(&Ty::Char, 0)
            }
            _ => {
                self.w.text("i64(");
                self.synth = true;
                self.expr(&Ty::U8, 0)
            }
        };
        self.w.text(")");
        Info::folding(&[operand], 0, true)
    }

    /// `(u8(e) ?? d)`, `(char(e) ?? d)` or `(i64(e) ?? d)`.
    fn fallible(&mut self, ty: &Ty) -> Info {
        let source = if *ty == Ty::I64 { Ty::F64 } else { Ty::I64 };
        self.w.text(&format!("({ty}("));
        let operand = self.expr(&source, 0);
        self.w.text(") ?? ");
        let default = self.conditionally(|g| g.expr(ty, 0));
        self.w.text(")");
        Info::of(&[operand, default], 0, true)
    }

    /// An exact method of an `i64` or an `f64`.
    fn math(&mut self, ty: &Ty) -> Info {
        let rounding = if *ty == Ty::F64 { ROUNDING.len() } else { 0 };
        let choice = self.pick(rounding + BOUNDS.len());
        self.w.text("(");
        let mut parts = vec![self.expr(ty, 0)];
        if choice < rounding {
            self.w.text(&format!(").{}()", ROUNDING[choice]));
        } else {
            let (method, args) = BOUNDS[choice - rounding];
            self.w.text(&format!(").{method}("));
            for arg in 0..args {
                if arg > 0 {
                    self.w.text(", ");
                }
                parts.push(self.expr(ty, 0));
            }
            self.w.text(")");
        }
        Info::of(&parts, 0, true)
    }

    /// An inexact `f64` (decision 11): a libm method of an exact `f64` or of another inexact
    /// one. The caller discards it.
    pub(crate) fn inexact(&mut self) {
        let (method, args) = LIBM[self.pick(LIBM.len())];
        self.w.text("(");
        self.inexact_operand();
        self.w.text(&format!(").{method}("));
        if args == 1 {
            self.inexact_operand();
        }
        self.w.text(")");
    }

    fn inexact_operand(&mut self) {
        if self.nesting < MAX_NESTING && self.spend(1) && self.pick(2) == 1 {
            self.nesting += 1;
            self.inexact();
            self.nesting -= 1;
        } else {
            self.expr(&Ty::F64, 0);
        }
    }

    /// `"text{part:spec}..."`, the parts' printed lengths summed within `max`.
    pub(crate) fn interpolate(&mut self, max: u64) -> Info {
        let one_line = std::mem::replace(&mut self.one_line, true);
        self.w.text("\"");
        let mut size = 0;
        let mut parts = Vec::new();
        for _ in 0..=self.pick(3) {
            let text_len = self.upto(3.min(max - size));
            let text = self.string_text(text_len, MESSAGE_CHARS);
            self.w.text(&text);
            size += text_len;
            if let Some((ty, most, nodes)) = self.compound_part(max - size) {
                self.w.text("{");
                self.synth = true;
                let info = self.expr(&ty, most);
                // No width, no alignment, no precision: no spec, or `#`, one time in two. `nodes`
                // is from before `expr`: the part's type and every type in it exist before `expr`
                // runs, and a type that `expr` makes holds no part of this value.
                if self.pick(2) == 0 {
                    self.w.text("}");
                    size += ty.printed(info.size, None, nodes);
                } else {
                    self.w.text(":#}");
                    size += ty.printed_pretty(info.size, nodes);
                }
                parts.push(info);
                continue;
            }
            // A `string` always fits.
            let mut fitting: Vec<Ty> = Ty::VALUES
                .iter()
                .filter(|ty| ty.printed(0, None, Nodes::default()) <= max - size)
                .cloned()
                .collect();
            // An `f64` comes twice: only it takes every part of a spec.
            if fitting.contains(&Ty::F64) {
                fitting.push(Ty::F64);
            }
            let ty = fitting[self.pick(fitting.len())].clone();
            let room = max - size;
            self.w.text("{");
            self.synth = true;
            let info = self.expr(&ty, room);
            let (spec, width, precision) = self.format_spec(&ty);
            let printed = ty
                .printed(info.size, precision, Nodes::default())
                .max(width);
            if printed > room {
                // The spec's width or precision does not fit: write the part without it.
                self.w.text("}");
                size += ty.printed(info.size, None, Nodes::default());
            } else {
                self.w.text(&format!("{spec}}}"));
                size += printed;
            }
            parts.push(info);
        }
        self.w.text("\"");
        self.one_line = one_line;
        Info::of(&parts, size, true)
    }

    /// The type of a compound part, now and then, the most size its value may hold, and the
    /// `Nodes` that bound counts. The choice reads `cu` and the type reads `u`, so that a part for
    /// which `cu` chooses no compound value draws what it drew before. The size is the greatest
    /// of at most `PART_SIZE` for which `{x:#}` writes within `room` chars, and so does a plain
    /// part.
    fn compound_part(&mut self, room: u64) -> Option<(Ty, u64, Nodes)> {
        if self.cu.choose_index(2).unwrap_or(0) == 0 {
            return None;
        }
        let ty = self.part_type();
        let nodes = self.scope.nodes;
        let most = (ty.least()..=PART_SIZE)
            .rev()
            .find(|&size| ty.printed_pretty(size, nodes) <= room)?;
        Some((ty, most, nodes))
    }

    /// The type of a compound part: an array of a `bare_type` or of an optional, or a declared or
    /// generic class or enum, as the type of a `let` can be. Never an optional, which W0015 warns
    /// of, and never `unit`.
    fn part_type(&mut self) -> Ty {
        let choice = self.pick(4);
        let declared = choice == 2;
        let len = match choice {
            2 => self.scope.types.len(),
            3 => self.scope.generic_types.len(),
            _ => 0,
        };
        if len > 0 {
            let at = self.pick(len);
            let pool = if declared {
                &self.scope.types
            } else {
                &self.scope.generic_types
            };
            return pool[at].clone();
        }
        let element = if choice == 1 {
            Ty::Optional(Box::new(self.type_within(0, false)))
        } else {
            self.bare_type()
        };
        Ty::Array(Box::new(element))
    }

    /// A format spec `[:#]` or `[:[align][width][.precision]]`, its width, and its precision;
    /// only an `f64` takes a precision (E0111). A part with no width and no precision gets `#`
    /// one time in four; every part here is a scalar or a `string`, so `#` changes nothing. A
    /// compound part has its own rule (`interpolate`).
    fn format_spec(&mut self, ty: &Ty) -> (String, u64, Option<u64>) {
        let width = if self.pick(3) == 0 {
            0
        } else {
            self.upto(11) + 1
        };
        let precision = (*ty == Ty::F64 && self.pick(3) != 0).then(|| self.upto(6));
        if width == 0 && precision.is_none() {
            return (["", "", "", ":#"][self.pick(4)].to_string(), 0, None);
        }
        let mut spec = ":".to_string();
        if width > 0 {
            spec += ["", "<", ">", "^"][self.pick(4)];
            spec += &width.to_string();
        }
        if let Some(precision) = precision {
            spec.push('.');
            spec += &precision.to_string();
        }
        (spec, width, precision)
    }

    /// The functions and methods a call here may name that return `ty` within `max` chars: in
    /// `main`, every one a call at depth 0 fits the budget for; in a function, itself and the
    /// earlier ones.
    pub(crate) fn callees(&self, ty: &Ty, max: u64) -> Vec<usize> {
        let last = self.scope.func.map_or(self.scope.funcs.len(), |f| f + 1);
        (0..last)
            .filter(|&f| {
                let func = &self.scope.funcs[f];
                func.ret == *ty
                    && func.ret_size <= max
                    && (self.scope.func.is_some() || self.cost.times() <= self.budget)
            })
            .collect()
    }

    /// How many functions and methods, the first ones, a call statement may name, of any type.
    pub(crate) fn callees_any(&self) -> usize {
        if self.scope.func.is_none() && self.cost.times() > self.budget {
            return 0;
        }
        self.scope.func.map_or(self.scope.funcs.len(), |f| f + 1)
    }

    /// A call of a function or a method that returns `ty`, `unit` for a statement.
    pub(crate) fn call(&mut self, ty: &Ty, max: u64) -> Info {
        let callees = self.callees(ty, max);
        let callee = callees[self.pick(callees.len())];
        self.invoke(callee, false)
    }

    /// `f(depth, args)`, or of a method `((e).m(depth, args))`, or `((o)?.m(depth, args))` if
    /// `safe`, whose arguments may not run.
    pub(crate) fn invoke(&mut self, callee: usize, safe: bool) -> Info {
        // The depth first: the budget it takes is gone before the receiver's calls take theirs.
        let depth = if self.scope.func.is_some() {
            self.cost.call(callee, None);
            "depth - 1".to_string()
        } else {
            let times = self.cost.times();
            let costs = self.scope.funcs[callee].cost;
            let fitting = (0..costs.len())
                .take_while(|&d| times.saturating_mul(costs[d]) <= self.budget)
                .count();
            let depth = self.pick(fitting);
            self.budget -= times * costs[depth];
            self.cost.call(callee, Some(depth));
            depth.to_string()
        };
        let mut parts = Vec::new();
        let owner = self.scope.funcs[callee].owner.clone();
        if let Some(Ty::Class { id, .. }) = owner {
            self.w.text("((");
            parts.push(self.receiver(id, safe));
            self.w.text(if safe { ")?." } else { ")." });
        }
        let name = self.scope.funcs[callee].name.clone();
        self.w.text(&format!("{name}({depth}"));
        // `depth - 1` is arithmetic the generator does not know the value of (decision 2).
        self.effect |= self.scope.func.is_some();
        let params = self.scope.funcs[callee].params.clone();
        let args = |g: &mut Self| -> Vec<Info> {
            params
                .iter()
                .map(|(ty, size)| {
                    g.w.text(", ");
                    g.expr(ty, *size)
                })
                .collect()
        };
        parts.extend(if safe {
            self.conditionally(args)
        } else {
            args(self)
        });
        self.w.text(")");
        if owner.is_some() {
            self.w.text(")");
        }
        Info::of(&parts, self.scope.funcs[callee].ret_size, false)
    }

    /// `(if c { a } else if d { b } else { e })` on one line; a branch after the first may
    /// panic instead, while the first keeps the `if` a value (E0043).
    fn if_expr(&mut self, ty: &Ty, max: u64, synth: bool) -> Info {
        self.w.text("(if ");
        let cond_start = self.w.at();
        let (cond, mut binding) = self.condition();
        let cond_text = cond_start..self.w.at();
        let mut parts = vec![cond];
        let mut arms = Vec::new();
        let mut size = 0;
        self.conditionally(|g| {
            loop {
                g.w.text(" { ");
                let arm = g.w.at();
                let branch = if parts.len() == 1 {
                    let vars = g.scope.vars.len();
                    g.scope.vars.extend(binding.take());
                    g.synth = synth;
                    let branch = g.expr(ty, max);
                    g.scope.vars.truncate(vars);
                    branch
                } else {
                    g.branch(ty, max)
                };
                size = size.max(branch.size);
                parts.push(branch);
                arms.push(arm..g.w.at());
                g.w.text(" } else");
                if g.pick(3) < 2 || !g.spend(1) {
                    break;
                }
                g.w.text(" if ");
                parts.push(g.expr(&Ty::Bool, 0));
            }
            g.w.text(" { ");
            let arm = g.w.at();
            let branch = g.branch(ty, max);
            size = size.max(branch.size);
            parts.push(branch);
            arms.push(arm..g.w.at());
            g.w.text(" })");
        });
        // With no expected type, the first branch types the `if`, so a `[]`, `None` or `7`
        // there would not be the `ty` it should.
        if !(synth && ty.inferred()) {
            self.swap(cond_text, &arms);
        }
        Info::of(&parts, size, true)
    }

    /// A `T` where a `T?` is expected. A value converts through one `?` only, so a `T` that is
    /// an optional is a name: the expected `T?` would reach into any other form of it.
    fn wrap(&mut self, ty: &Ty, max: u64) -> Info {
        let Ty::Optional(payload) = ty else {
            unreachable!("only an optional wraps")
        };
        // `Some(x)` and `Option.Some(x)` convert as `x` does.
        let open = ["", "Some(", "Option.Some("][self.pick(3)];
        self.w.text(open);
        let value = if matches!(**payload, Ty::Optional(_)) {
            self.name(payload, max)
        } else {
            self.expr(payload, max)
        };
        if !open.is_empty() {
            self.w.text(")");
        }
        // A constant of an optional type does not fold.
        Info {
            folds: false,
            ..value
        }
    }

    /// Now and then swaps the arms of an `if` with the condition `cond` and the arms `arms`, if
    /// it has two, and negates the condition, in the rewritten copy (decision 2). A `let`
    /// condition has no negation.
    pub(crate) fn swap(&mut self, cond: Range<usize>, arms: &[Range<usize>]) {
        let [a, b] = arms else { return };
        let plain = !self.w.out.rewritten[cond.clone()].starts_with("let ");
        if plain
            && allowed(Rewrite::Swap, &Site::default())
            && self.rewrite_pick(8) == 7
            && !self.frozen
        {
            self.w.swap_arms(cond, a.clone(), b.clone());
        }
    }

    /// The head of an `if`: a condition, or `let p = e` of an optional `e` (`let_pattern`), and
    /// what the first branch binds.
    pub(crate) fn condition(&mut self) -> (Info, Option<Var>) {
        if self.pick(4) != 3 || !self.spend(1) {
            return (self.expr(&Ty::Bool, 0), None);
        }
        let payload = match self.bare_type() {
            Ty::Optional(payload) => *payload,
            ty => ty,
        };
        let (pattern, name) = self.let_pattern(&payload, true);
        self.w.text(&format!("let {pattern} = "));
        self.synth = true;
        let value = self.expr(&Ty::Optional(Box::new(payload.clone())), MAX_SIZE);
        let loops = self.cost.depth();
        let binding = name.map(|name| Var::new(name, payload, false, value.size, 0, loops));
        (value, binding)
    }

    /// A branch of an `if` after the first: a value, or now and then a panic: a call of
    /// `panic` or `todo`, or an index past the end of an array.
    fn branch(&mut self, ty: &Ty, max: u64) -> Info {
        // A panic ends the run, and a run takes hundreds of these branches, so it comes once
        // in 64.
        if self.pick(64) != 63 {
            return self.expr(ty, max);
        }
        // The index takes half: it is the one panic that reads a value first. An optional of an
        // optional has no value of its own type to index, so `panic` takes its place.
        match self.pick(4) {
            1 => {
                let todo = if self.pick(2) == 0 {
                    "todo".to_string()
                } else {
                    format!("todo({})", self.message())
                };
                self.w.text(&todo);
            }
            pick if pick == 0 || ty.least() >= MAX_SIZE => {
                let message = self.message();
                self.w.text(&format!("panic({message})"));
            }
            _ => {
                // `[e][1]`: index 1 out of bounds for length 1.
                self.w.text("([");
                self.synth = true;
                self.expr(ty, MAX_SIZE - 1);
                self.w.text("][1])");
            }
        }
        Info::of(&[], 0, false)
    }
}

impl Gen<'_> {
    /// `(match s { p [if g] => e ... })`, an arm a line, of a `bool`, a number, a `char`, a
    /// `string`, an enum value, or an optional of one of these, or of an optional of one: arms
    /// whose patterns do not overlap, so none is unreachable, then `_` or a binding where they
    /// leave a value unmatched.
    pub(crate) fn match_expr(&mut self, ty: &Ty, max: u64, synth: bool) -> Info {
        let tested = if self.pick(4) == 0 {
            let optional = Ty::Optional(Box::new(self.matched()));
            if self.pick(3) == 0 {
                Ty::Optional(Box::new(optional))
            } else {
                optional
            }
        } else {
            self.matched()
        };
        self.w.text("(match ");
        let scrutinee = match &tested {
            // An optional of an optional has no value of its own type: a name of it, or
            // `Some` of an optional, is one.
            Ty::Optional(optional) if tested.least() == u64::MAX => {
                if self.has_name(&tested, MAX_SIZE) && self.pick(2) == 0 {
                    self.name(&tested, MAX_SIZE)
                } else {
                    self.w.text("Some(");
                    self.synth = true;
                    let value = self.expr(optional, MAX_SIZE);
                    self.w.text(")");
                    value
                }
            }
            _ => {
                self.synth = true;
                self.expr(&tested, MAX_SIZE)
            }
        };
        self.w.text(" ");
        self.blocks += 1;
        self.w.open();
        let (mut arms, exhaustive) = match tested {
            Ty::Optional(_) => self.optional_arms(&tested),
            Ty::Bool => self.bool_arms(),
            Ty::String => (self.word_arms(), false),
            Ty::Enum { .. } => self.variant_arms(&tested),
            _ => (self.range_arms(&tested), false),
        };
        if !exhaustive {
            let mut binds = Vec::new();
            let pattern = if self.pick(2) == 0 {
                "_".to_string()
            } else {
                let name = self.scope.fresh("v");
                binds.push((name.clone(), tested.clone()));
                name
            };
            arms.push(Arm {
                pattern,
                binds,
                guard: false,
            });
        }
        let mut parts = vec![scrutinee];
        let mut size = 0;
        for arm in arms {
            self.w.line();
            self.w.text(&arm.pattern);
            self.conditionally(|g| {
                let vars = g.scope.vars.len();
                let loops = g.cost.depth();
                for (name, ty) in arm.binds {
                    let size = ty.within(scrutinee.size);
                    g.scope.vars.push(Var::new(name, ty, false, size, 0, loops));
                }
                if arm.guard {
                    g.w.text(" if ");
                    parts.push(g.expr(&Ty::Bool, 0));
                }
                g.w.text(" => ");
                g.synth = synth;
                let body = g.expr(ty, max);
                size = size.max(body.size);
                parts.push(body);
                g.scope.vars.truncate(vars);
            });
        }
        self.w.close();
        self.blocks -= 1;
        self.w.text(")");
        Info::of(&parts, size, true)
    }

    /// A type that a `match` tests by value: a `bool`, a number, a `char`, a `string` or an
    /// enum.
    fn matched(&mut self) -> Ty {
        let enums = self.scope.enums.len();
        // The enums share one choice with the other types.
        let choice = self.pick(MATCHED.len() + usize::from(enums > 0));
        if choice < MATCHED.len() {
            MATCHED[choice].clone()
        } else {
            let id = self.pick(enums);
            self.scope.enum_ty(id)
        }
    }

    /// Arms of the optional `ty`: the arms of its payload, each as written, which matches the
    /// value inside, or in `Some(..)`, which an optional payload needs; then, where they leave a
    /// value, now and then `Some(v)` or `Some(_)`; and now and then `None` among them. Exhaustive
    /// when `None` and each value are in arms without a guard.
    fn optional_arms(&mut self, ty: &Ty) -> (Vec<Arm>, bool) {
        let Ty::Optional(payload) = ty else {
            unreachable!("only an optional has a payload")
        };
        let (inner, mut covered) = match &**payload {
            Ty::Optional(_) => self.optional_arms(payload),
            Ty::Bool => self.bool_arms(),
            Ty::String => (self.word_arms(), false),
            Ty::Enum { .. } => self.variant_arms(payload),
            _ => (self.range_arms(payload), false),
        };
        let nested = matches!(**payload, Ty::Optional(_));
        let mut arms = Vec::new();
        for arm in inner {
            let pattern = if nested || self.pick(2) == 0 {
                format!("Some({})", arm.pattern)
            } else {
                arm.pattern
            };
            arms.push(Arm { pattern, ..arm });
        }
        if !covered && self.pick(3) == 0 {
            let guard = self.pick(4) == 0;
            let mut binds = Vec::new();
            let inside = if self.pick(2) == 0 {
                "_".to_string()
            } else {
                let name = self.scope.fresh("v");
                binds.push((name.clone(), (**payload).clone()));
                name
            };
            arms.push(Arm {
                pattern: format!("Some({inside})"),
                binds,
                guard,
            });
            covered = !guard;
        }
        let none = self.pick(4) != 0;
        let guard = self.pick(4) == 0;
        if none {
            let at = self.pick(arms.len() + 1);
            let pattern = "None".to_string();
            let binds = Vec::new();
            arms.insert(
                at,
                Arm {
                    pattern,
                    binds,
                    guard,
                },
            );
        }
        (arms, covered && none && !guard)
    }

    /// The pattern of an `if let` or a `while let` on an optional of `payload`, and the name it
    /// binds to the payload, if any: a name or `_`, which unwraps; `Some(..)`; `None` when
    /// `none`; or a pattern of the value inside, as written or in `Some(..)`.
    pub(crate) fn let_pattern(&mut self, payload: &Ty, none: bool) -> (String, Option<String>) {
        match self.pick(8) {
            0 => ("_".to_string(), None),
            5 => {
                let name = self.scope.fresh("v");
                (format!("Some({name})"), Some(name))
            }
            6 if none && self.pick(2) == 0 => ("None".to_string(), None),
            6 => ("Some(_)".to_string(), None),
            7 => match self.refutable(payload, 0) {
                Some(inside) if self.pick(2) == 0 => (inside, None),
                inside => {
                    let inside = inside.unwrap_or_else(|| "_".to_string());
                    (format!("Some({inside})"), None)
                }
            },
            _ => {
                let name = self.scope.fresh("v");
                (name.clone(), Some(name))
            }
        }
    }

    /// Arms of `true`, `false` or both, each at most once; exhaustive when both come without
    /// a guard.
    fn bool_arms(&mut self) -> (Vec<Arm>, bool) {
        let mut left = vec!["false", "true"];
        let mut covered = 0;
        let mut arms = Vec::new();
        for _ in 0..=self.pick(2) {
            if left.is_empty() {
                break;
            }
            let guard = self.pick(4) == 0;
            let pattern = if left.len() == 2 && self.pick(3) == 0 {
                let both = left.join(" | ");
                left.clear();
                covered += if guard { 0 } else { 2 };
                both
            } else {
                covered += usize::from(!guard);
                left.remove(self.pick(left.len())).to_string()
            };
            arms.push(Arm {
                pattern,
                binds: Vec::new(),
                guard,
            });
        }
        (arms, covered == 2)
    }

    /// Arms of string literals, each at most once, alone or two together.
    fn word_arms(&mut self) -> Vec<Arm> {
        let mut left: Vec<&str> = WORDS.to_vec();
        let mut arms = Vec::new();
        // Two arms take four words at most, which leaves one for a third.
        for _ in 0..=self.pick(2) {
            let mut pattern = left.remove(self.pick(left.len())).to_string();
            if !left.is_empty() && self.pick(3) == 0 {
                pattern += " | ";
                pattern += left.remove(self.pick(left.len()));
            }
            let guard = self.pick(4) == 0;
            arms.push(Arm {
                pattern,
                binds: Vec::new(),
                guard,
            });
        }
        arms
    }

    /// Arms of an `i64`, a `u8` or a `char`: literals, ranges and alternatives of literals, each
    /// past the one before, so no two overlap.
    fn range_arms(&mut self, ty: &Ty) -> Vec<Arm> {
        let mut next = match ty {
            Ty::I64 => self.int(-3, 3),
            Ty::U8 => self.int(0, 3),
            _ => i64::from(b'a') + self.int(0, 3),
        };
        let mut arms = Vec::new();
        // Three arms of 6 values at most keep a `char` within the letters.
        for _ in 0..=self.pick(2) {
            let lo = next + self.int(0, 2);
            let (pattern, after) = self.range_pattern(ty, lo, true);
            next = after;
            let guard = self.pick(4) == 0;
            arms.push(Arm {
                pattern,
                binds: Vec::new(),
                guard,
            });
        }
        arms
    }

    /// A pattern of an `i64`, a `u8` or a `char` that matches `lo` and values above it only:
    /// a literal, `lo..hi`, `lo..=hi`, or, if `alternatives`, `lo | n`; and the least value
    /// above every value it matches.
    fn range_pattern(&mut self, ty: &Ty, lo: i64, alternatives: bool) -> (String, i64) {
        let text = |n: i64| match ty {
            Ty::Char => format!(
                "'{}'",
                char::from(u8::try_from(n).expect("a tested char should be a letter"))
            ),
            _ => n.to_string(),
        };
        match self.pick(if alternatives { 4 } else { 3 }) {
            0 => (text(lo), lo + 1),
            1 => {
                let hi = lo + 1 + self.int(0, 3);
                (format!("{}..{}", text(lo), text(hi)), hi)
            }
            2 => {
                let hi = lo + self.int(0, 3);
                (format!("{}..={}", text(lo), text(hi)), hi + 1)
            }
            _ => {
                let other = lo + 1 + self.int(0, 2);
                (format!("{} | {}", text(lo), text(other)), other + 1)
            }
        }
    }

    /// Arms of the variants of the enum `ty`, each in one arm at most, alone or two together,
    /// which then bind nothing; exhaustive when each variant is in an arm without a guard whose
    /// pattern matches each of its values.
    fn variant_arms(&mut self, ty: &Ty) -> (Vec<Arm>, bool) {
        let Ty::Enum { id, .. } = *ty else {
            unreachable!("only an enum has variants")
        };
        let count = self.scope.enums[id].variants.len();
        let mut left: Vec<usize> = (0..count).collect();
        let mut covered = 0;
        let mut arms = Vec::new();
        for _ in 0..=self.pick(count) {
            if left.is_empty() {
                break;
            }
            let guard = self.pick(4) == 0;
            let first = left.remove(self.pick(left.len()));
            let mut binds = Vec::new();
            let pattern = if !left.is_empty() && self.pick(4) == 0 {
                let second = left.remove(self.pick(left.len()));
                let (a, a_total) = self.variant_pattern(ty, first, None, 0);
                let (b, b_total) = self.variant_pattern(ty, second, None, 0);
                covered += usize::from(a_total && !guard) + usize::from(b_total && !guard);
                format!("{a} | {b}")
            } else {
                let (pattern, total) = self.variant_pattern(ty, first, Some(&mut binds), 0);
                covered += usize::from(total && !guard);
                pattern
            };
            arms.push(Arm {
                pattern,
                binds,
                guard,
            });
        }
        (arms, covered == count)
    }

    /// `V`, `E.V`, `V(p, ...)` or `E.V(p, ...)` of the variant `v` of the enum `ty`, binding
    /// names into `binds` if any; and whether it matches every value of the variant.
    pub(crate) fn variant_pattern(
        &mut self,
        ty: &Ty,
        v: usize,
        mut binds: Option<&mut Vec<(String, Ty)>>,
        depth: u32,
    ) -> (String, bool) {
        let Ty::Enum { id, .. } = *ty else {
            unreachable!("only an enum has variants")
        };
        let (name, fields) = {
            let variant = &self.scope.enums[id].variants[v];
            (variant.name.clone(), variant.fields.clone())
        };
        let mut text = if self.pick(2) == 0 {
            name
        } else {
            // An alias of the enum is the enum, as a qualifier says.
            let enum_name = self.constructor(ty, ty.bare());
            format!("{enum_name}.{name}")
        };
        if fields.is_empty() {
            return (text, true);
        }
        let mut total = true;
        let mut parts = Vec::new();
        for field in &fields {
            let choice = self.pick(3);
            if choice == 1
                && let Some(binds) = binds.as_deref_mut()
            {
                let name = self.scope.fresh("v");
                binds.push((name.clone(), field.clone()));
                parts.push(name);
                continue;
            }
            let refutable = if choice == 0 {
                None
            } else {
                self.refutable(field, depth)
            };
            total &= refutable.is_none();
            parts.push(refutable.unwrap_or_else(|| "_".to_string()));
        }
        text = format!("{text}({})", parts.join(", "));
        (text, total)
    }

    /// A pattern that matches some values of `ty` but not all: a literal, a range, a variant,
    /// or for an optional `None`, `Some(..)` or a pattern of the value inside; none for a type
    /// only names and `_` match.
    fn refutable(&mut self, ty: &Ty, depth: u32) -> Option<String> {
        match ty {
            Ty::I64 | Ty::U8 | Ty::Char => {
                let lo = match ty {
                    Ty::I64 => self.int(-3, 9),
                    Ty::U8 => self.int(0, 9),
                    _ => i64::from(b'a') + self.int(0, 9),
                };
                Some(self.range_pattern(ty, lo, false).0)
            }
            Ty::Bool => Some(if self.pick(2) == 0 { "false" } else { "true" }.to_string()),
            Ty::String => Some(WORDS[self.pick(WORDS.len())].to_string()),
            // An enum has two variants at least, so one variant's pattern leaves the other.
            Ty::Enum { id, .. } if depth < MAX_PATTERN_DEPTH => {
                let v = self.pick(self.scope.enums[*id].variants.len());
                Some(self.variant_pattern(ty, v, None, depth + 1).0)
            }
            Ty::Optional(payload) if depth < MAX_PATTERN_DEPTH => Some(match self.pick(3) {
                0 => "None".to_string(),
                pick => match self.refutable(payload, depth + 1) {
                    // A `None` inside would match the outer `None`, so only `Some` writes it.
                    Some(inside) if pick == 2 && !matches!(**payload, Ty::Optional(_)) => inside,
                    inside => format!("Some({})", inside.unwrap_or_else(|| "_".to_string())),
                },
            }),
            Ty::Enum { .. }
            | Ty::F64
            | Ty::Unit
            | Ty::Array(_)
            | Ty::Optional(_)
            | Ty::Class { .. } => None,
        }
    }
}

/// The locals, then the constants, of type `ty` that hold at most `max` chars, and what a read
/// of each is.
fn names<'s>(scope: &'s Scope, ty: &'s Ty, max: u64) -> impl Iterator<Item = (&'s str, Info)> {
    let vars = scope
        .vars
        .iter()
        .filter(move |var| var.ty == *ty && var.size <= max)
        .map(|var| {
            let info = Info {
                pure: !var.mutable,
                ..Info::leaf(var.size)
            };
            (var.name.as_str(), info)
        });
    let consts = scope
        .consts
        .iter()
        .filter(move |c| c.ty == *ty && c.size <= max)
        .map(|c| {
            let value = matches!(c.ty, Ty::I64 | Ty::U8).then_some(c.value);
            (c.name.as_str(), Info::literal(c.size, value))
        });
    vars.chain(consts)
}

/// The type of the member that `o?.m` reads to give `ty`, an optional: its payload, or `ty`
/// itself for an optional of an optional, since `?.` adds no layer to an optional member.
fn safe_member(ty: &Ty) -> &Ty {
    match ty {
        Ty::Optional(payload) if !matches!(**payload, Ty::Optional(_)) => payload,
        _ => ty,
    }
}

/// Whether an index into `var` reads a `ty` of at most `max` in bounds.
fn indexable(var: &Var, ty: &Ty, max: u64) -> bool {
    matches!(&var.ty, Ty::Array(element) if **element == *ty) && var.len > 0 && var.elem <= max
}

/// `n` with a `_` between each group of three digits.
fn separated(n: i64) -> String {
    let digits = n.to_string();
    let mut text = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            text.push('_');
        }
        text.push(digit);
    }
    text
}

#[cfg(test)]
mod tests {
    use arbitrary::Unstructured;

    use super::{Gen, MAX_SIZE, Rule, Ty, Var, arithmetic_value};
    use crate::generate::writer::Rewrite;

    #[test]
    fn arithmetic_values_are_the_run_time_s() {
        // Each as `sisu build` runs it, checked 2026-10-22: division truncates, and a `u8`
        // that leaves `0..=255` overflows.
        let (i64, u8) = (&Ty::I64, &Ty::U8);
        let overflow = Err("integer overflow");
        let cases = [
            (i64, " + ", 2, 3, Ok(5)),
            (i64, " - ", 2, 3, Ok(-1)),
            (i64, " * ", 2, 3, Ok(6)),
            (i64, " / ", -7, 2, Ok(-3)),
            (i64, " % ", -7, 2, Ok(-1)),
            (i64, " % ", 7, -2, Ok(1)),
            (i64, " + ", i64::MAX, 1, overflow),
            (i64, " / ", i64::MIN, -1, overflow),
            (i64, " % ", i64::MIN, -1, overflow),
            (i64, " / ", 0, 0, Err("division by zero")),
            (i64, " % ", 5, 0, Err("division by zero")),
            (u8, " - ", 0, 1, overflow),
            (u8, " + ", 255, 1, overflow),
            (u8, " * ", 16, 16, overflow),
            (u8, " / ", 7, 2, Ok(3)),
            (u8, " + ", 254, 1, Ok(255)),
        ];
        for (ty, op, l, r, value) in cases {
            let at = super::OPS.iter().position(|o| *o == op);
            let at = at.expect("each case should name an operator");
            assert_eq!(
                arithmetic_value(ty, at, l, r),
                value,
                "{l}{op}{r} on a {ty}"
            );
        }
    }

    /// The text between the parentheses of each `dbg(` call in `text`, the calls inside a
    /// call's argument included. A generated string holds no parenthesis, and no name ends in
    /// `dbg`.
    fn dbg_arguments(text: &str) -> Vec<&str> {
        let mut found = Vec::new();
        for (at, _) in text.match_indices("dbg(") {
            let start = at + "dbg(".len();
            let mut depth = 1;
            for (i, c) in text[start..].char_indices() {
                depth += i32::from(c == '(') - i32::from(c == ')');
                if depth == 0 {
                    found.push(&text[start..start + i]);
                    break;
                }
            }
        }
        found
    }

    /// No rewrite changes the argument of a `dbg`, since the line it writes holds that text. Each
    /// call in a program is in its rewritten copy as written. 300 seeds hold about 100 calls.
    /// Their arguments hold the sites that a rewrite would take.
    #[test]
    fn a_dbg_argument_should_stay_as_written_in_the_rewritten_copy() {
        let mut calls = 0;
        let mut rewritten_programs = 0;
        for n in 0..300 {
            let g = crate::generate::generate(&crate::generate::coverage::seed(n));
            let arguments = dbg_arguments(&g.program);
            calls += arguments.len();
            rewritten_programs += usize::from(!arguments.is_empty() && g.rewritten != g.program);
            for argument in arguments {
                let call = format!("dbg({argument})");
                assert!(
                    g.rewritten.contains(&call),
                    "seed {n}: {call}\n{}",
                    g.rewritten
                );
            }
        }
        assert!(calls >= 100, "{calls} calls");
        assert!(rewritten_programs >= 50, "{rewritten_programs} programs");
    }

    /// `echo` was reserved before `dbg`, and now is an ordinary name. Some locals take it, and the
    /// programs that name one still check, as a local never takes the name of one in scope.
    #[test]
    fn a_generated_local_should_sometimes_be_named_echo() {
        let named = (0..100)
            .map(|n| crate::generate::generate(&crate::generate::coverage::seed(n)).program)
            .filter(|program| program.contains("let echo") || program.contains("let mut echo"))
            .count();
        assert!(named >= 20, "{named} programs of 100 name a local echo");
    }

    /// A compound part may make a new enum instance, such as the `G<T>` that a generic call
    /// lowers. Seeds 29794 and 96755 once counted the nodes of that instance in the size of the
    /// part, and the size passed `max`.
    #[test]
    fn interpolate_should_not_count_the_enum_instances_that_its_part_makes() {
        for n in [29794, 96755] {
            crate::generate::generate(&crate::generate::coverage::seed(n));
        }
    }

    #[test]
    fn arithmetic_of_literals_is_known_and_folds() {
        let mut g = Gen::new(&[1, 5, 3, 2, 9, 4]);
        let info = g.rule(Rule::Arithmetic, &Ty::I64, MAX_SIZE, false);
        assert_eq!(g.w.out.program, "(15 / 1)");
        assert_eq!((info.value, info.pure, info.folds), (Some(15), true, true));
    }

    #[test]
    fn a_u8_left_literal_is_bare_where_its_type_is_expected() {
        // Byte by byte: a literal on the left, 15, `/`, and on the right 1. With no expected
        // type, `(u8(n) ?? n)` is a `u8` by its own type; with one, a bare literal is, and its
        // value is known.
        for (synth, text) in [(false, "(15 / 1)"), (true, "((u8(15) ?? 15) / 1)")] {
            let mut g = Gen::new(&[1, 5, 3, 0]);
            let info = g.rule(Rule::Arithmetic, &Ty::U8, MAX_SIZE, synth);
            assert_eq!(g.w.out.program, text);
            if !synth {
                assert_eq!(info.value, Some(15));
            }
        }
    }

    #[test]
    fn a_negated_literal_is_known_and_folds() {
        let mut g = Gen::new(&[]);
        let info = g.rule(Rule::Negate, &Ty::I64, MAX_SIZE, false);
        assert_eq!(g.w.out.program, "(-(0))");
        assert_eq!((info.value, info.pure, info.folds), (Some(0), true, true));
    }

    #[test]
    fn float_negation_and_arithmetic_are_pure() {
        // Neither panics on an `f64`, whose value the generator does not track.
        for rule in [Rule::Negate, Rule::Arithmetic] {
            let mut g = Gen::new(&[]);
            let info = g.rule(rule, &Ty::F64, MAX_SIZE, false);
            assert!(info.pure && info.folds, "{rule:?}: {}", g.w.out.program);
        }
    }

    #[test]
    fn a_wrapped_literal_does_not_fold() {
        // `0` where an `i64?` is expected: a constant of an optional type does not fold.
        let mut g = Gen::new(&[]);
        let info = g.rule(
            Rule::Wrap,
            &Ty::Optional(Box::new(Ty::I64)),
            MAX_SIZE,
            false,
        );
        assert_eq!(g.w.out.program, "0");
        assert!(!info.folds);
    }

    #[test]
    fn a_wrap_should_be_written_with_or_without_some() {
        // The first byte picks the spelling.
        let ty = Ty::Optional(Box::new(Ty::I64));
        for (byte, text) in [(0, "0"), (1, "Some(0)"), (2, "Option.Some(0)")] {
            let bytes = [byte];
            let mut g = Gen::new(&bytes);
            g.rule(Rule::Wrap, &ty, MAX_SIZE, false);
            assert_eq!(g.w.out.program, text);
        }
    }

    #[test]
    fn a_none_should_be_written_with_or_without_its_enum() {
        for (byte, text) in [(0, "None"), (1, "Option.None"), (2, "Option<i64>.None")] {
            let bytes = [byte];
            let mut g = Gen::new(&bytes);
            g.none(&Ty::I64);
            assert_eq!(g.w.out.program, text);
        }
    }

    #[test]
    fn a_type_should_be_spelled_with_a_question_mark_or_with_option() {
        // The first byte picks the spelling of the inner optional, the second the outer's.
        let ty = Ty::Array(Box::new(Ty::Optional(Box::new(Ty::Optional(Box::new(
            Ty::I64,
        ))))));
        for (bytes, text) in [
            ([0, 0], "[i64??]"),
            ([1, 0], "[Option<i64>?]"),
            ([0, 1], "[Option<i64?>]"),
            ([1, 1], "[Option<Option<i64>>]"),
        ] {
            let mut g = Gen::new(&bytes);
            assert_eq!(g.spell(&ty), text, "{bytes:?}");
        }
    }

    #[test]
    fn a_wrapped_optional_of_an_optional_should_be_a_name_of_its_payload() {
        // The generator builds no `i64?` from nothing, so an `i64??` wraps only a name.
        let (payload, nested) = (
            Ty::Optional(Box::new(Ty::I64)),
            Ty::Optional(Box::new(Ty::Optional(Box::new(Ty::I64)))),
        );
        let mut g = Gen::new(&[]);
        let wraps = |g: &mut Gen<'_>| {
            let mut rules = Vec::new();
            g.compound_rules(&nested, MAX_SIZE, false, &mut rules);
            rules.iter().any(|r| matches!(r, Rule::Wrap))
        };
        assert!(!wraps(&mut g));
        g.scope
            .vars
            .push(Var::new("w".to_string(), payload, false, 0, 0, 0));
        assert!(wraps(&mut g));
        g.rule(Rule::Wrap, &nested, MAX_SIZE, false);
        assert_eq!(g.w.out.program, "w");
    }

    #[test]
    fn a_panicking_branch_should_take_each_form_but_index_no_optional_of_an_optional() {
        // A first byte of 63 makes the branch panic; the second picks the form.
        let nested = Ty::Optional(Box::new(Ty::Optional(Box::new(Ty::I64))));
        for (ty, form, text) in [
            (&Ty::I64, 0, "panic(\"\")"),
            (&Ty::I64, 1, "todo"),
            (&Ty::I64, 2, "([0][1])"),
            (&nested, 2, "panic(\"\")"),
        ] {
            let bytes = [63, form];
            let mut g = Gen::new(&bytes);
            g.branch(ty, MAX_SIZE);
            assert_eq!(g.w.out.program, text, "{ty}, form {form}");
        }
    }

    #[test]
    fn a_safe_member_should_take_a_layer_off_one_layer_only() {
        let payload = Ty::Optional(Box::new(Ty::I64));
        let nested = Ty::Optional(Box::new(payload.clone()));
        assert_eq!(super::safe_member(&payload), &Ty::I64);
        assert_eq!(super::safe_member(&nested), &nested);
    }

    #[test]
    fn an_if_typed_by_its_first_arm_is_not_swapped() {
        // With no expected type, `(if false { (u8(0) ?? 0) } else { 0 })` is a `u8` by its first
        // arm; swapped, the bare `0` would make it an `i64`. Where the arms' type is not
        // inferred, or an expected type gives it, the arms swap.
        for (ty, synth, swaps) in [
            (Ty::U8, true, false),
            (Ty::I64, true, true),
            (Ty::U8, false, true),
        ] {
            let mut g = Gen::new(&[]);
            g.ru = Unstructured::new(&[7; 64]);
            g.rule(Rule::If, &ty, MAX_SIZE, synth);
            let swapped = g.w.made.contains(&Rewrite::Swap);
            assert_eq!(
                swapped, swaps,
                "{ty} with synth {synth}: {}",
                g.w.out.rewritten
            );
        }
    }

    #[test]
    fn a_panicking_step_of_literals_does_not_fold() {
        // Its parts fold, but a `const` lifted over it would not check (E0088), so the Const
        // rewrite must not take it.
        let mut g = Gen::new(&[1, 5, 3, 0, 0, 0]);
        let info = g.rule(Rule::Arithmetic, &Ty::I64, MAX_SIZE, false);
        assert_eq!(g.w.out.program, "(15 / 0)");
        assert_eq!((info.value, info.pure, info.folds), (None, false, false));
    }

    /// Arms as (pattern, guarded).
    type Arms<'a> = &'a [(&'a str, bool)];

    #[test]
    fn optional_arms_should_cover_the_payload_as_written_or_in_some_and_none() {
        // Byte by byte: one `bool` arm, its guard, `false | true`, then whether it is written in
        // `Some(..)`; where the arms leave a value, a `Some(..)` arm, its guard and what it
        // binds; then `None`, its guard and its place among the arms.
        let ty = Ty::Optional(Box::new(Ty::Bool));
        // Each row: the bytes, the arms as (pattern, guarded), and whether they are exhaustive.
        let rows: [(&[u8], Arms<'_>, bool); 3] = [
            (
                &[0, 1, 0, 1, 1, 1, 0],
                &[("None", false), ("false | true", false)],
                true,
            ),
            (&[0, 1, 0, 0, 0, 0], &[("Some(false | true)", false)], false),
            (
                &[0, 0, 0, 1, 0, 1, 1, 1, 1, 2],
                &[("false | true", true), ("Some(v1)", false), ("None", false)],
                true,
            ),
        ];
        for (bytes, expected, exhaustive) in rows {
            let mut g = Gen::new(bytes);
            let (arms, found) = g.optional_arms(&ty);
            let arms: Vec<_> = arms.iter().map(|a| (a.pattern.as_str(), a.guard)).collect();
            assert_eq!((&arms[..], found), (expected, exhaustive), "{bytes:?}");
        }
    }

    #[test]
    fn a_let_pattern_on_an_optional_should_take_each_form() {
        // The first byte picks the form; with `6`, the second picks `None` where it may be
        // written; with `7`, the bytes after the value's pattern pick `Some(..)` around it.
        let rows: [(&[u8], bool, &str, Option<&str>); 9] = [
            (&[0], true, "_", None),
            (&[1], true, "v1", Some("v1")),
            (&[5], true, "Some(v1)", Some("v1")),
            (&[6, 0], true, "None", None),
            (&[6, 1], true, "Some(_)", None),
            (&[6, 0], false, "Some(_)", None),
            (&[7, 0, 0], true, "-3", None),
            (&[7, 0, 0, 1], true, "Some(-3)", None),
            (&[7, 0, 1], false, "-3..-2", None),
        ];
        for (bytes, none, pattern, bound) in rows {
            let mut g = Gen::new(bytes);
            let (found, name) = g.let_pattern(&Ty::I64, none);
            assert_eq!(
                (found.as_str(), name.as_deref()),
                (pattern, bound),
                "{bytes:?}"
            );
        }
    }

    #[test]
    fn a_refutable_optional_pattern_should_write_none_some_or_the_value_inside() {
        // The first byte picks `None`, `Some(..)` or the value's own pattern, which an optional
        // payload writes in `Some(..)`, as its `None` would match the outer one. Below
        // `MAX_PATTERN_DEPTH` layers, `_` stands for the rest.
        let opt = |ty: Ty| Ty::Optional(Box::new(ty));
        let rows = [
            (opt(Ty::I64), 0, &[0][..], Some("None")),
            (opt(Ty::I64), 0, &[1], Some("Some(-3)")),
            (opt(Ty::I64), 0, &[2], Some("-3")),
            (opt(opt(Ty::I64)), 0, &[2], Some("Some(None)")),
            (opt(opt(opt(Ty::I64))), 0, &[1, 1], Some("Some(Some(_))")),
            (opt(Ty::I64), super::MAX_PATTERN_DEPTH, &[1], None),
        ];
        for (ty, depth, bytes, expected) in rows {
            let mut g = Gen::new(bytes);
            assert_eq!(
                g.refutable(&ty, depth).as_deref(),
                expected,
                "{ty}, {bytes:?}"
            );
        }
    }

    #[test]
    fn a_match_on_an_optional_of_an_optional_should_test_a_name_or_a_some() {
        // Byte by byte: an optional scrutinee, of a `bool`, of two layers, then a name of one
        // where a name fits.
        let nested = Ty::Optional(Box::new(Ty::Optional(Box::new(Ty::Bool))));
        for (named, bytes, start) in [
            (true, [0, 0, 0, 0], "(match w "),
            (true, [0, 0, 0, 1], "(match Some("),
            (false, [0, 0, 0, 0], "(match Some("),
        ] {
            let mut g = Gen::new(&bytes);
            if named {
                g.scope
                    .vars
                    .push(Var::new("w".to_string(), nested.clone(), false, 0, 0, 0));
            }
            g.match_expr(&Ty::I64, MAX_SIZE, false);
            let program = &g.w.out.program;
            assert!(program.starts_with(start), "{bytes:?}: {program}");
        }
    }
}
