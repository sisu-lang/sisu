//! The expression rules: each writes one construct of the wanted type. Every expression but a
//! literal, a name and a call is written in parentheses, so it is an operand wherever it goes.

use super::scope::{Scope, Ty, Var};
use super::stmt::MAX_BLOCKS;
use super::{Gen, LINE, MAX_BOUND, MAX_NESTING, MAX_SIZE};

/// What a rule wrote.
#[derive(Clone, Copy)]
pub(crate) struct Info {
    /// The most size the value reaches (decision 5): for a `string`, its chars.
    pub(crate) size: u64,
    /// Whether it has no effect and cannot panic (decision 2).
    pub(crate) pure: bool,
    /// For an array: how many elements it holds at least.
    pub(crate) len: u64,
}

impl Info {
    fn leaf(size: u64) -> Info {
        Info {
            size,
            pure: true,
            len: 0,
        }
    }

    /// The value of a rule built from `parts`, which has an effect or may panic unless `pure`.
    fn of(parts: &[Info], size: u64, pure: bool) -> Info {
        Info {
            size,
            pure: pure && parts.iter().all(|part| part.pure),
            len: 0,
        }
    }
}

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
    If,
    Block,
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
const ROUNDING: [&str; 6] = ["abs", "sqrt", "floor", "ceil", "trunc", "round"];
/// The methods of `i64` and `f64` that take arguments of the receiver's type.
const BOUNDS: [(&str, usize); 3] = [("min", 1), ("max", 1), ("clamp", 2)];
/// The libm methods, whose results are inexact (decision 11), and their argument counts.
const LIBM: [(&str, usize); 13] = [
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
const CLASSIFY: [&str; 3] = ["is_nan", "is_finite", "is_infinite"];

/// Chars a string literal may hold, as written: no `@` (decision 3), and no raw `{`, `}`, `"` or
/// `\`. The escapes from `\r` on hold a line break, which a panic message may not.
const STRING_CHARS: [&str; 26] = [
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
pub(crate) const CHARS: [(&str, char); 14] = [
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
const MATCHED: [Ty; 5] = [Ty::Bool, Ty::I64, Ty::U8, Ty::Char, Ty::String];
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
        self.nesting += 1;
        let mut info = self.rule(rule, ty, max, synth);
        self.nesting -= 1;
        // A value of a type without a size counts none, whatever its parts held.
        if !ty.sized() {
            info.size = 0;
        }
        debug_assert!(
            info.size <= max,
            "{rule:?} wrote {} chars over {max}",
            info.size
        );
        info
    }

    fn compound_rules(&mut self, ty: &Ty, max: u64, synth: bool, rules: &mut Vec<Rule>) {
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
                if !synth && payload.floor() <= max {
                    rules.push(Rule::Wrap);
                }
                if !self.fields(payload, max).is_empty() {
                    rules.push(Rule::SafeField);
                }
                // Twice: a method of the payload's type is rare.
                if self.callees(payload, max).iter().any(|&f| self.method(f)) {
                    rules.extend([Rule::SafeMethod, Rule::SafeMethod]);
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
        rules.push(Rule::If);
        if !self.one_line && self.blocks < MAX_BLOCKS {
            rules.push(Rule::Block);
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
                Ty::Optional(_) => {
                    self.w.text("None");
                    Info::leaf(0)
                }
                _ => self.literal(ty, max),
            },
            Rule::Name => self.name(ty, max),
            Rule::Negate | Rule::Not => {
                self.w.text(if *ty == Ty::Bool { "(!(" } else { "(-(" });
                let operand = self.expr(ty, max);
                self.w.text("))");
                Info::of(&[operand], 0, *ty != Ty::I64)
            }
            Rule::Arithmetic => self.arithmetic(ty, synth),
            Rule::Logic => {
                self.w.text("(");
                let lhs = self.expr(&Ty::Bool, 0);
                self.one_of(&[" && ", " || "]);
                let rhs = self.conditionally(|g| g.expr(&Ty::Bool, 0));
                self.w.text(")");
                Info::of(&[lhs, rhs], 0, true)
            }
            Rule::Equal => {
                let operand = self.any_type();
                self.w.text("(");
                self.synth = true;
                let lhs = self.expr(&operand, MAX_SIZE);
                self.one_of(&[" == ", " != "]);
                let rhs = self.expr(&operand, MAX_SIZE);
                self.w.text(")");
                Info::of(&[lhs, rhs], 0, true)
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
                Info::of(&[lhs, rhs], lhs.size + rhs.size, true)
            }
            Rule::Interpolate => self.interpolate(max),
            Rule::ReadLine => {
                self.w.text("(read_line() ?? ");
                let default = self.conditionally(|g| g.expr(&Ty::String, max));
                self.w.text(")");
                Info::of(&[default], default.size.max(LINE), false)
            }
            Rule::Call => self.call(ty, max),
            Rule::If => self.if_expr(ty, max, synth),
            Rule::Block => {
                self.w.text("(");
                let info = self.value_block(ty, max, synth);
                self.w.text(")");
                info
            }
            Rule::Wrap => {
                let Ty::Optional(payload) = ty else {
                    unreachable!("only an optional wraps")
                };
                self.expr(payload, max)
            }
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

    /// `((o).g)` of a field of type `ty`, or if `safe`, `((o)?.g)` of one of its payload's.
    fn field(&mut self, ty: &Ty, max: u64, safe: bool) -> Info {
        let wanted = match ty {
            Ty::Optional(payload) if safe => payload,
            _ => ty,
        };
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
        Info::of(&[receiver], cap, true)
    }

    /// `((o)?.m(depth, args))` of a method that returns the payload of `ty`, an optional.
    fn safe_method(&mut self, ty: &Ty, max: u64) -> Info {
        let Ty::Optional(payload) = ty else {
            unreachable!("only an optional is a safe call's")
        };
        let methods: Vec<usize> = self
            .callees(payload, max)
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
        let size = self.scope.classes[class].size;
        let ty = Ty::Class { id: class, size };
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

    /// `K(g: a, h: b)`: every field, each within its cap.
    fn object(&mut self, ty: &Ty) -> Info {
        let Ty::Class { id, size } = *ty else {
            unreachable!("only a class has objects")
        };
        self.w.text(&format!("{ty}("));
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
                    let holds = variants[v].fields.iter().any(|field| field == ty);
                    Some(v) != except && floor < max && holds == recursive
                })
                .collect()
        };
        let mut variants = fitting(self, false);
        let recursive = fitting(self, true);
        if !recursive.is_empty() && self.nesting < MAX_NESTING && self.spend(1) {
            variants.extend(recursive);
        }
        let v = variants[self.pick(variants.len())];
        let (name, fields, labels) = {
            let variant = &self.scope.enums[id].variants[v];
            (
                variant.name.clone(),
                variant.fields.clone(),
                variant.labels.clone(),
            )
        };
        self.w.text(&format!("{ty}.{name}"));
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
            let later: u64 = fields[f + 1..].iter().map(Ty::floor).sum();
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
            let element = self.any_type();
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
        let element = self.any_type();
        self.w.text("((");
        self.synth = true;
        let receiver = self.expr(&Ty::Array(Box::new(element.clone())), MAX_SIZE);
        self.w.text(").contains(");
        let value = self.expr(&element, MAX_SIZE);
        self.w.text("))");
        Info::of(&[receiver, value], 0, true)
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

    /// `(a ?? b)`: of a `T?` `a` and a `T` `b` for a `T`, of two `T?` for a `T?`.
    fn coalesce(&mut self, ty: &Ty, max: u64) -> Info {
        let optional = match ty {
            Ty::Optional(_) => ty.clone(),
            _ => Ty::Optional(Box::new(ty.clone())),
        };
        self.w.text("(");
        self.synth = true;
        let lhs = self.expr(&optional, max);
        self.w.text(" ?? ");
        // The `T?` the right side gives must be one: a `T` there would make the result a `T`.
        self.synth = optional == *ty;
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
        self.type_within(2)
    }

    fn type_within(&mut self, nesting: u32) -> Ty {
        let values = Ty::VALUES.len();
        // The declared types share one choice, so they take no more of the values' as they grow.
        let declared = usize::from(!self.scope.types.is_empty());
        let choice = self.pick(values + declared + if nesting > 0 { 2 } else { 0 });
        if choice < values {
            return Ty::VALUES[choice].clone();
        }
        if choice < values + declared {
            let pick = self.pick(self.scope.types.len());
            return self.scope.types[pick].clone();
        }
        let choice = choice - declared;
        // Arrays of strings, whose elements `+=` may grow, come more often.
        let inner = if choice == values && self.pick(2) == 0 {
            Ty::String
        } else {
            self.type_within(nesting - 1)
        };
        match (choice - values, inner) {
            (0, inner) => Ty::Array(Box::new(inner)),
            // `optional_of` does not nest optionals.
            (_, Ty::Optional(payload)) => Ty::Optional(payload),
            (_, inner) => Ty::Optional(Box::new(inner)),
        }
    }

    fn has_name(&self, ty: &Ty, max: u64) -> bool {
        names(&self.scope, ty, max).next().is_some()
    }

    fn name(&mut self, ty: &Ty, max: u64) -> Info {
        let pick = self.pick(names(&self.scope, ty, max).count());
        let (name, size) = names(&self.scope, ty, max)
            .nth(pick)
            .expect("the rule should be chosen only when a name fits");
        self.w.text(name);
        Info::leaf(size)
    }

    pub(crate) fn literal(&mut self, ty: &Ty, max: u64) -> Info {
        match ty {
            Ty::I64 => {
                let (text, _) = self.int_literal();
                self.w.text(&text);
                Info::leaf(0)
            }
            Ty::U8 => {
                let (text, _) = self.u8_literal();
                self.w.text(&text);
                Info::leaf(0)
            }
            Ty::F64 => {
                let text = self.float_literal();
                self.w.text(&text);
                Info::leaf(0)
            }
            Ty::Bool => {
                let text = if self.pick(2) == 0 { "false" } else { "true" };
                self.w.text(text);
                Info::leaf(0)
            }
            Ty::Char => {
                let (text, _) = CHARS[self.pick(CHARS.len())];
                self.w.text(text);
                Info::leaf(0)
            }
            Ty::String => {
                let len = self.upto(max.min(8));
                let text = self.string_text(len, STRING_CHARS.len());
                self.w.text(&format!("\"{text}\""));
                Info::leaf(len)
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
        const CONSTANTS: [&str; 8] = [
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
        const OPS: [&str; 5] = [" + ", " - ", " * ", " / ", " % "];
        self.w.text("(");
        // `a op b` of `a` in `10..=28` and `b` in `1..=9` fits a `u8` and divides by no zero.
        let lhs = if matches!(ty, Ty::I64 | Ty::U8) && self.pick(8) != 0 {
            let n = self.int(10, 28);
            // With no expected type, a `u8` must be one by its own type (`Gen::expr`).
            if synth && *ty == Ty::U8 {
                self.w.text(&format!("(u8({n}) ?? {n})"));
            } else {
                self.w.text(&n.to_string());
            }
            Info::leaf(0)
        } else {
            self.synth = synth;
            self.expr(ty, 0)
        };
        self.one_of(&OPS);
        let rhs = self.right_operand(ty);
        self.w.text(")");
        Info::of(&[lhs, rhs], 0, *ty == Ty::F64)
    }

    /// The right operand of arithmetic of `ty`: for an `i64`, mostly, and for a `u8`, always,
    /// a literal in `1..=9`, so the arithmetic rarely overflows or divides by zero; else any
    /// expression. Any other `u8` is as likely as not to overflow.
    pub(crate) fn right_operand(&mut self, ty: &Ty) -> Info {
        if *ty == Ty::U8 || *ty == Ty::I64 && self.pick(8) != 0 {
            let n = self.int(1, 9);
            self.w.text(&n.to_string());
            return Info::leaf(0);
        }
        self.expr(ty, 0)
    }

    /// A chain `a < b <= c` or `a > b >= c` of two or three operands of one ordered type.
    fn order(&mut self) -> Info {
        const ORDERED: [Ty; 5] = [Ty::I64, Ty::U8, Ty::F64, Ty::Char, Ty::String];
        let operand = ORDERED[self.pick(ORDERED.len())].clone();
        let ops = if self.pick(2) == 0 {
            [" < ", " <= "]
        } else {
            [" > ", " >= "]
        };
        self.w.text("(");
        self.synth = true;
        let mut parts = vec![self.expr(&operand, MAX_SIZE)];
        for _ in 0..=self.pick(2) {
            self.one_of(&ops);
            parts.push(self.expr(&operand, MAX_SIZE));
        }
        self.w.text(")");
        Info::of(&parts, 0, true)
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
        Info::of(&[operand], 0, true)
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
            // A `string` always fits.
            let mut fitting: Vec<Ty> = Ty::VALUES
                .iter()
                .filter(|ty| ty.printed(0, None) <= max - size)
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
            let printed = ty.printed(info.size, precision).max(width);
            if printed > room {
                // The spec's width or precision does not fit: write the part without it.
                self.w.text("}");
                size += ty.printed(info.size, None);
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

    /// A format spec `[:[align][width][.precision]]`, its width, and its precision; only an
    /// `f64` takes a precision (E0111).
    fn format_spec(&mut self, ty: &Ty) -> (String, u64, Option<u64>) {
        let width = if self.pick(3) == 0 {
            0
        } else {
            self.upto(11) + 1
        };
        let precision = (*ty == Ty::F64 && self.pick(3) != 0).then(|| self.upto(6));
        if width == 0 && precision.is_none() {
            return (String::new(), 0, None);
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
        let (cond, mut binding) = self.condition();
        let mut parts = vec![cond];
        let mut size = 0;
        self.conditionally(|g| {
            loop {
                g.w.text(" { ");
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
                g.w.text(" } else");
                if g.pick(3) < 2 || !g.spend(1) {
                    break;
                }
                g.w.text(" if ");
                parts.push(g.expr(&Ty::Bool, 0));
            }
            g.w.text(" { ");
            let branch = g.branch(ty, max);
            size = size.max(branch.size);
            parts.push(branch);
            g.w.text(" })");
        });
        Info::of(&parts, size, true)
    }

    /// The head of an `if`: a condition, or `let p = e` of an optional `e`, and what the first
    /// branch binds.
    pub(crate) fn condition(&mut self) -> (Info, Option<Var>) {
        if self.pick(4) != 3 || !self.spend(1) {
            return (self.expr(&Ty::Bool, 0), None);
        }
        let payload = match self.any_type() {
            Ty::Optional(payload) => *payload,
            ty => ty,
        };
        let name = if self.pick(4) == 0 {
            None
        } else {
            Some(self.scope.fresh("v"))
        };
        self.w
            .text(&format!("let {} = ", name.as_deref().unwrap_or("_")));
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
        // The index takes half: it is the one panic that reads a value first.
        match self.pick(4) {
            0 => {
                let message = self.message();
                self.w.text(&format!("panic({message})"));
            }
            1 => {
                let todo = if self.pick(2) == 0 {
                    "todo".to_string()
                } else {
                    format!("todo({})", self.message())
                };
                self.w.text(&todo);
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
    /// `string` or an enum value: arms whose patterns do not overlap, so none is unreachable,
    /// then `_` or a binding where they leave a value unmatched.
    pub(crate) fn match_expr(&mut self, ty: &Ty, max: u64, synth: bool) -> Info {
        let enums = self.scope.enums.len();
        // The enums share one choice with the other types.
        let choice = self.pick(MATCHED.len() + usize::from(enums > 0));
        let tested = if choice < MATCHED.len() {
            MATCHED[choice].clone()
        } else {
            let id = self.pick(enums);
            let least = self.scope.enums[id].least;
            Ty::Enum { id, least }
        };
        self.w.text("(match ");
        self.synth = true;
        let scrutinee = self.expr(&tested, MAX_SIZE);
        self.w.text(" ");
        self.blocks += 1;
        self.w.open();
        let (mut arms, exhaustive) = match tested {
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
    fn variant_pattern(
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
            format!("{ty}.{name}")
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

    /// A pattern that matches some values of `ty` but not all: a literal, a range or a
    /// variant; none for a type only names and `_` match.
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
            Ty::Enum { .. }
            | Ty::F64
            | Ty::Unit
            | Ty::Array(_)
            | Ty::Optional(_)
            | Ty::Class { .. } => None,
        }
    }
}

/// The locals, then the constants, of type `ty` that hold at most `max` chars, and their sizes.
fn names<'s>(scope: &'s Scope, ty: &'s Ty, max: u64) -> impl Iterator<Item = (&'s str, u64)> {
    let vars = scope
        .vars
        .iter()
        .filter(move |var| var.ty == *ty && var.size <= max)
        .map(|var| (var.name.as_str(), var.size));
    let consts = scope
        .consts
        .iter()
        .filter(move |c| c.ty == *ty && c.size <= max)
        .map(|c| (c.name.as_str(), c.size));
    vars.chain(consts)
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
