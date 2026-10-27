//! The coverage test of ADR 0033: programs generated from fixed seeds check clean, and between
//! them hold every variant of the syntax tree's, the typed tree's and the types' enums, every
//! built-in method on each receiver, every prelude function and `f64` constant, each value of
//! the syntax tree's `bool` fields, and the literal forms only the text shows. Each enum is read
//! with `syn`, so a new variant fails the test until a rule writes it. The walkers match without
//! a `_` arm, so a new variant does not compile until they record it.

use std::fmt::Write as _;

use rustc_hash::{FxHashMap, FxHashSet};

use super::item::{MAX_KNOWN, MOST};
use super::scope::{Class, Const, Enum, Field, Func, Ty, Var, Variant};
use super::writer::{Rewrite, Site, Writer, allowed};
use super::{Answer, BUDGET, Cost, DEPTHS, FUEL, Gen, MAX_BOUND, MAX_NESTING, MAX_SIZE, generate};
use crate::diagnostic::Severity;
use crate::front_end::front_end;
use crate::symbol::Symbol;
use crate::types::{Type, TypeKind};
use crate::{ast, lexer, parser, tir};

/// How many seeds, and how many bytes each holds.
const SEEDS: u64 = 64;
const SEED_LEN: usize = 4_096;

/// What no generated program holds, and why.
const EXCLUDED: [(&str, &str); 10] = [
    (
        "ast::BrokenKind::Function",
        "only an item with a syntax error is broken",
    ),
    (
        "ast::BrokenKind::Class",
        "only an item with a syntax error is broken",
    ),
    (
        "ast::BrokenKind::Enum",
        "only an item with a syntax error is broken",
    ),
    (
        "ast::BrokenKind::Constant",
        "only an item with a syntax error is broken",
    ),
    (
        "ast::Bound::Name",
        "a named range bound is an error (E0126)",
    ),
    (
        "types::Step::Type",
        "a private worklist item of `types::write_steps`, which is no type",
    ),
    (
        "types::Step::Close",
        "a private worklist item of `types::write_steps`, which is no type",
    ),
    (
        "types::Step::Rest",
        "a private worklist item of `types::write_steps`, which is no type",
    ),
    (
        "types::Head::Class",
        "the key by which `types::write_type` asks for a name, which is no type",
    ),
    (
        "types::Head::Enum",
        "the key by which `types::write_type` asks for a name, which is no type",
    ),
];

/// The receivers of the built-in methods `check/number.rs` does not list, by their typed
/// tree's variant.
const RECEIVERS: [(&str, &[&str]); 7] = [
    ("tir::ExprKind::Len", &["String", "Array"]),
    ("tir::ExprKind::Chars", &["String"]),
    ("tir::ExprKind::Bytes", &["String"]),
    ("tir::ExprKind::Words", &["String"]),
    ("tir::ExprKind::FromChars", &["Array"]),
    ("tir::ExprKind::Sum", &["Array"]),
    ("tir::ExprKind::Contains", &["Array"]),
];

/// The scalar conversions, from and to, as `convert` in `check/string.rs` takes them: `i64(e)`
/// of a `u8`, a `char` or an `f64`, and `u8(e)`, `char(e)` and `f64(e)` of an `i64`.
const CONVERSIONS: [(&str, &str); 6] = [
    ("U8", "I64"),
    ("Char", "I64"),
    ("F64", "I64"),
    ("I64", "U8"),
    ("I64", "Char"),
    ("I64", "F64"),
];

/// What the code of the generated programs reaches between them, which no variant shows: without
/// one, a capability of the generator is off.
const REACH: [&str; 62] = [
    "reach: a parameter other than `depth` is read",
    "reach: `main` calls a function at a depth of 1 or more",
    "reach: a function calls itself",
    "reach: a loop runs its body",
    "reach: a string literal holds an escape",
    "reach: an `if` statement jumps in every branch",
    "reach: a `for` over an array runs its body",
    "reach: a `while let` takes its `Some` branch",
    "reach: a `while let` walks an enum chain",
    "reach: an index is in bounds",
    "reach: an index is meant to panic",
    "reach: an index reads a local",
    "reach: an index reads an immutable local",
    "reach: an index reads an array literal in bounds",
    "reach: a plain `=` to a local that is not an array",
    "reach: `==` or `!=` compares with `None`",
    "reach: a `+=` adds chars to a string local",
    "reach: a compound assignment targets a field",
    "reach: a variant is built with labeled fields",
    "reach: a variant is built with positional fields",
    "reach: `f64` arithmetic or `op=` takes a right operand that is not a literal",
    "reach: `i64` arithmetic or `op=` takes a right operand that is not a literal",
    "reach: `i64` or `u8` arithmetic takes a left operand that is not a literal",
    "reach: a `loop` is a statement",
    "reach: a `loop` gives a value",
    "reach: a `while` has a loop label",
    "reach: a `while let` has a loop label",
    "reach: a `for` has a loop label",
    "reach: a `loop` has a loop label",
    "reach: a `break` names a loop label",
    "reach: a `continue` names a loop label",
    "reach: a labeled `break` gives a value",
    "reach: a labeled jump leaves an inner loop",
    "reach: `break l -1` is a label and `-1`",
    "reach: `break x -1` is `x - 1`",
    "reach: a generic function is called with written type arguments",
    "reach: a generic function is called with inferred type arguments",
    "reach: a generic function calls a generic function",
    "reach: a generic function matches a generic enum",
    "reach: a generic class is built with written type arguments",
    "reach: a generic class is built with inferred type arguments",
    "reach: a generic variant is built with inferred type arguments",
    "reach: a generic type is a type argument",
    "reach: an optional is a type argument",
    "reach: a type has one `?` layer",
    "reach: a type has two or more `?` layers",
    "reach: `??` on an optional of an optional",
    "reach: `Some` builds an optional",
    "reach: a type is spelled `Option<..>`",
    "reach: `Option.None` or `Option<..>.None`",
    "reach: a variant, literal or range pattern stands at an optional position",
    "reach: a binding or `_` covers a whole optional in a `match` arm",
    "reach: a `match` on an optional of an optional",
    "reach: `Ok` builds a `Result`",
    "reach: `Err` builds a `Result`",
    "reach: a `match` on a `Result`",
    "reach: a `char?` is stored",
    "reach: a `bool?` is stored",
    "reach: an optional of an enum is stored",
    "reach: an optional of an optional of `char` or `bool` is stored",
    "reach: an `Option`-shaped enum has a niche",
    "reach: an `Option`-shaped enum holds itself",
];

/// Whether a program's text holds a form.
type Holds = fn(&str) -> bool;

/// The forms only a program's text shows, each with how to find it.
const TEXT: [(&str, Holds); 8] = [
    ("text: `_` between digits", |text| {
        text.as_bytes()
            .windows(3)
            .any(|w| w[0].is_ascii_digit() && w[1] == b'_' && w[2].is_ascii_digit())
    }),
    ("text: the smallest `i64`", |text| {
        text.contains("-9223372036854775808")
    }),
    ("text: `0x`", |text| text.contains("0x")),
    ("text: a `0x` of more than three digits", |text| {
        text.split("0x")
            .skip(1)
            .any(|rest| rest.bytes().take_while(u8::is_ascii_hexdigit).count() > 3)
    }),
    ("text: `0b`", |text| text.contains("0b")),
    ("text: `0o`", |text| text.contains("0o")),
    ("text: `\\u{...}`", |text| text.contains("\\u{")),
    (
        "text: a format spec with align, width and precision",
        |text| {
            text.split(':').skip(1).any(|spec| {
                let mut chars = spec.chars();
                matches!(chars.next(), Some('<' | '>' | '^')) && {
                    let rest: String = chars.collect();
                    let (width, precision) = rest.split_once('.').unwrap_or(("", ""));
                    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
                    digits(width) && precision.split_once('}').is_some_and(|(p, _)| digits(p))
                }
            })
        },
    ),
];

/// Fixed byte strings from a seeded splitmix64.
pub(crate) fn seeds() -> impl Iterator<Item = Vec<u8>> {
    (0..SEEDS).map(seed)
}

/// The data of seed `n`.
pub(crate) fn seed(n: u64) -> Vec<u8> {
    let mut state = n;
    (0..SEED_LEN / 8)
        .flat_map(|_| splitmix64(&mut state).to_le_bytes())
        .collect()
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The program's syntax tree and typed tree, after asserting it checks without an error. The
/// typed tree is the checker's, before `mono`, so it holds the generic declarations with their
/// type parameters (`types::TypeKind::Param`).
fn checked(source: &str) -> (ast::Program, tir::Program) {
    let (program, diagnostics) = crate::front_end::checked(source);
    // An unused loop label would be a label no rule meant to write. Every generated function
    // returns and every generated loop ends, so W0010 and W0011 may not appear.
    let errors: Vec<String> = diagnostics
        .iter()
        .filter(|d| {
            d.severity() == Severity::Error
                || matches!(d.code().as_str(), "W0008" | "W0010" | "W0011")
        })
        .map(|d| {
            let before = &source[..d.span().start()];
            let line = 1 + before.matches('\n').count();
            let column = 1 + before.len() - before.rfind('\n').map_or(0, |at| at + 1);
            format!("{} at {line}:{column}", d.code().as_str())
        })
        .collect();
    let program = program.filter(|_| errors.is_empty()).unwrap_or_else(|| {
        let mut numbered = String::new();
        for (i, line) in source.lines().enumerate() {
            writeln!(numbered, "{:>4} {line}", i + 1)
                .expect("writing to a `String` should succeed");
        }
        panic!("a generated program fails to check: {errors:?}\n{numbered}")
    });
    let lexed = lexer::lex(source).expect("a program that checks should lex");
    (parser::parse(lexed, false).program, program)
}

/// Everything the generated programs should hold between them.
fn universe() -> FxHashSet<String> {
    let mut universe = FxHashSet::default();
    for (module, source) in [
        ("ast", include_str!("../ast.rs")),
        ("tir", include_str!("../tir.rs")),
        ("types", include_str!("../types.rs")),
    ] {
        let file = syn::parse_file(source).expect("the module should parse");
        for item in &file.items {
            if let syn::Item::Enum(e) = item {
                for variant in &e.variants {
                    universe.insert(format!("{module}::{}::{}", e.ident, variant.ident));
                }
            }
        }
    }
    let ast = syn::parse_file(include_str!("../ast.rs")).expect("ast.rs should parse");
    for item in &ast.items {
        match item {
            syn::Item::Struct(s) => bool_fields(&s.ident.to_string(), &s.fields, &mut universe),
            syn::Item::Enum(e) => {
                for variant in &e.variants {
                    let owner = format!("{}::{}", e.ident, variant.ident);
                    bool_fields(&owner, &variant.fields, &mut universe);
                }
            }
            _ => {}
        }
    }
    let number =
        syn::parse_file(include_str!("../check/number.rs")).expect("number.rs should parse");
    for row in const_rows(&number, "METHODS") {
        let [op, receivers, ..] = &row[..] else {
            panic!("a METHODS row should hold an operation and its receivers");
        };
        let receivers: &[&str] = match receivers.as_str() {
            "F64" => &["F64"],
            "Number" => &["I64", "F64"],
            other => panic!("an unknown receiver set `{other}`"),
        };
        for receiver in receivers {
            universe.insert(format!("tir::MathOp::{op} on {receiver}"));
        }
    }
    for row in const_rows(&number, "CONSTANTS") {
        universe.insert(format!("f64.{}", row[0]));
    }
    let prelude =
        syn::parse_file(include_str!("../check/prelude.rs")).expect("prelude.rs should parse");
    for row in const_rows(&prelude, "PRELUDE") {
        universe.insert(format!("prelude {}", row[0]));
    }
    for (variant, receivers) in RECEIVERS {
        for receiver in receivers {
            universe.insert(format!("{variant} on {receiver}"));
        }
    }
    for (from, to) in CONVERSIONS {
        universe.insert(format!("tir::ExprKind::Convert from {from} to {to}"));
    }
    for key in REACH {
        universe.insert(key.to_string());
    }
    for (feature, _) in TEXT {
        universe.insert(feature.to_string());
    }
    universe
}

/// `owner.field = true` and `= false` for each `bool` field of `fields`.
fn bool_fields(owner: &str, fields: &syn::Fields, universe: &mut FxHashSet<String>) {
    for field in fields {
        let is_bool = matches!(&field.ty, syn::Type::Path(p) if p.path.is_ident("bool"));
        if let (true, Some(name)) = (is_bool, &field.ident) {
            for value in [true, false] {
                universe.insert(format!("ast::{owner}.{name} = {value}"));
            }
        }
    }
}

/// The rows of the array `const name`, a tuple row as its elements and any other row as one:
/// each element is the last segment of its path, the value of its string literal, or empty
/// otherwise. `(MathOp::Abs, Receivers::F64, 0)` is `["Abs", "F64", ""]`, and `"print"` is
/// `["print"]`.
fn const_rows(file: &syn::File, name: &str) -> Vec<Vec<String>> {
    let array = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Const(c) if c.ident == name => Some(&*c.expr),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the file should hold `const {name}`"));
    let syn::Expr::Array(array) = array else {
        panic!("`{name}` should be an array literal");
    };
    let text = |e: &syn::Expr| match e {
        syn::Expr::Path(p) => p
            .path
            .segments
            .last()
            .expect("a path should have a segment")
            .ident
            .to_string(),
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(s),
            ..
        }) => s.value(),
        _ => String::new(),
    };
    array
        .elems
        .iter()
        .map(|row| match row {
            syn::Expr::Tuple(t) => t.elems.iter().map(text).collect(),
            single => vec![text(single)],
        })
        .collect()
}

/// What the generated programs hold, where an inexact value reaches more than decision 11 lets
/// it, and which loops are not bounded as decision 5 has them.
#[derive(Default)]
struct Walk {
    seen: FxHashSet<String>,
    leaks: Vec<String>,
    unbounded: Vec<String>,
    /// Indexes neither in bounds nor the one meant to panic, `[e][1]` (decision 3).
    out_of_bounds: Vec<String>,
    /// The functions of the program being walked.
    funcs: FxHashSet<String>,
    /// The generic functions, classes and enums of it.
    generic_funcs: FxHashSet<String>,
    generic_classes: FxHashSet<String>,
    generic_enums: FxHashSet<String>,
    /// Whether the function being walked is generic.
    in_generic: bool,
    /// The function being walked, and its parameters.
    func: String,
    params: Vec<String>,
    /// The text of the program being walked.
    source: String,
    /// The length of each array local whose `let` writes it as a literal or a fill.
    lens: FxHashMap<String, i128>,
    /// The locals a `let` without `mut` declares.
    immutable: FxHashSet<String>,
    /// Where an index sits alone in an `if` branch after the first, as the one meant to panic.
    later: FxHashSet<usize>,
    /// Where the value of a `while let` that runs its body starts.
    some_values: FxHashSet<usize>,
    /// Where the value of a `while let` that walks an enum chain of a node or more starts.
    chains: FxHashSet<usize>,
    /// Each enum's variants, by the enum's name.
    variants: FxHashMap<String, Vec<String>>,
    /// The id of the prelude's `Result` in the typed tree.
    result: Option<usize>,
    /// The types each enum's payloads hold, by its id.
    payloads: Vec<Vec<Type>>,
    /// Field and element assignments of a value that may hold a class object (decision 4).
    heap_classes: Vec<String>,
    /// Where a `+=` on a name starts.
    adds: FxHashSet<usize>,
    /// Where the value of a plain `=` to a name starts.
    assigns: FxHashSet<usize>,
    /// Whether the walk is in the target of an assignment, which writes an element, not reads it.
    writing: bool,
    /// Where an arithmetic `+ - * / %` expression outside a constant starts.
    arithmetic: FxHashSet<usize>,
    /// Where a compound assignment starts.
    compounds: FxHashSet<usize>,
    /// Whether the walk is in a constant's value.
    in_const: bool,
    /// How many right operands of `f64` arithmetic and `op=` are literals, and how many are not.
    f64_rights: [usize; 2],
    /// Blocks in a constant's value or as the place of an assignment, and blocks that end in an
    /// `if false` copy of their last expression: what decision 2's rewrites must not write.
    misplaced: Vec<String>,
    /// The literal messages of `panic` and `todo`.
    messages: Vec<String>,
    /// The loop labels of the loops around the point walked, innermost last.
    labels: Vec<Option<Symbol>>,
    /// Where each `loop` that counts down starts.
    counted: FxHashSet<usize>,
}

impl Walk {
    fn mark(&mut self, key: &str) {
        if !self.seen.contains(key) {
            self.seen.insert(key.to_string());
        }
    }

    fn program(&mut self, source: &str) {
        let (ast, tir) = checked(source);
        source.clone_into(&mut self.source);
        self.lens.clear();
        self.immutable.clear();
        self.later.clear();
        self.some_values.clear();
        self.chains.clear();
        self.adds.clear();
        self.assigns.clear();
        self.arithmetic.clear();
        self.compounds.clear();
        self.counted.clear();
        self.ast(&ast);
        self.tir(&tir);
        for (feature, holds) in TEXT {
            if holds(source) {
                self.mark(feature);
            }
        }
    }

    fn ast(&mut self, program: &ast::Program) {
        self.funcs = program
            .items
            .iter()
            .filter_map(|item| match item {
                ast::Item::Function(f) => Some(f.name.name.as_str().to_string()),
                ast::Item::Class(_) | ast::Item::Enum(_) | ast::Item::Const(_) => None,
            })
            .collect();
        self.generic_funcs = generic_names(program, |item| match item {
            ast::Item::Function(f) => Some((&f.name, &f.type_params)),
            ast::Item::Class(_) | ast::Item::Enum(_) | ast::Item::Const(_) => None,
        });
        self.generic_classes = generic_names(program, |item| match item {
            ast::Item::Class(c) => Some((&c.name, &c.type_params)),
            ast::Item::Function(_) | ast::Item::Enum(_) | ast::Item::Const(_) => None,
        });
        self.generic_enums = generic_names(program, |item| match item {
            ast::Item::Enum(e) => Some((&e.name, &e.type_params)),
            ast::Item::Function(_) | ast::Item::Class(_) | ast::Item::Const(_) => None,
        });
        self.variants = program
            .items
            .iter()
            .filter_map(|item| match item {
                ast::Item::Enum(e) => Some((
                    e.name.name.as_str().to_string(),
                    e.variants
                        .iter()
                        .map(|v| v.name.name.as_str().to_string())
                        .collect(),
                )),
                ast::Item::Function(_) | ast::Item::Class(_) | ast::Item::Const(_) => None,
            })
            .collect();
        for broken in &program.broken {
            self.mark(match broken.kind {
                ast::BrokenKind::Function => "ast::BrokenKind::Function",
                ast::BrokenKind::Class => "ast::BrokenKind::Class",
                ast::BrokenKind::Enum => "ast::BrokenKind::Enum",
                ast::BrokenKind::Constant => "ast::BrokenKind::Constant",
            });
        }
        for item in &program.items {
            match item {
                ast::Item::Function(f) => {
                    self.mark("ast::Item::Function");
                    self.ast_function(f);
                }
                ast::Item::Class(c) => {
                    self.mark("ast::Item::Class");
                    for member in &c.members {
                        match member {
                            ast::Member::Field(field) => {
                                self.mark("ast::Member::Field");
                                self.mark(&format!("ast::FieldDecl.mutable = {}", field.mutable));
                                self.type_expr(&field.ty);
                            }
                            ast::Member::Method(f) => {
                                self.mark("ast::Member::Method");
                                self.ast_function(f);
                            }
                        }
                    }
                }
                ast::Item::Enum(e) => {
                    self.mark("ast::Item::Enum");
                    for field in e.variants.iter().flat_map(|v| v.payload.iter().flatten()) {
                        self.type_expr(&field.ty);
                    }
                    for f in &e.methods {
                        self.ast_function(f);
                    }
                }
                ast::Item::Const(c) => {
                    self.mark("ast::Item::Const");
                    if let Some(ty) = &c.ty {
                        self.type_expr(ty);
                    }
                    self.in_const = true;
                    self.ast_expr(&c.value);
                    self.in_const = false;
                }
            }
        }
    }

    /// Marks a call of `callee`, with its type arguments left out if `inferred`, that is generic.
    fn generic_call(&mut self, callee: &str, inferred: bool) {
        if self.generic_funcs.contains(callee) {
            self.mark(if inferred {
                "reach: a generic function is called with inferred type arguments"
            } else {
                "reach: a generic function is called with written type arguments"
            });
            if self.in_generic {
                self.mark("reach: a generic function calls a generic function");
            }
        }
        if self.generic_classes.contains(callee) {
            self.mark(if inferred {
                "reach: a generic class is built with inferred type arguments"
            } else {
                "reach: a generic class is built with written type arguments"
            });
        }
    }

    fn ast_function(&mut self, f: &ast::Function) {
        self.func = f.name.name.as_str().to_string();
        self.in_generic = !f.type_params.is_empty();
        self.params.clear();
        for param in &f.params {
            self.type_expr(&param.ty);
            self.params.push(param.name.name.as_str().to_string());
        }
        if let Some(ret) = &f.ret {
            self.type_expr(ret);
        }
        self.ast_block(&f.body);
    }

    fn type_expr(&mut self, ty: &ast::TypeExpr) {
        match ty.optional {
            0 => {}
            1 => self.mark("reach: a type has one `?` layer"),
            _ => self.mark("reach: a type has two or more `?` layers"),
        }
        match &ty.kind {
            ast::TypeKind::Name(_) => self.mark("ast::TypeKind::Name"),
            ast::TypeKind::Applied(applied) => {
                self.mark("ast::TypeKind::Applied");
                if applied.name.as_str() == "Option" {
                    self.mark("reach: a type is spelled `Option<..>`");
                }
                for arg in &applied.args {
                    if matches!(arg.kind, ast::TypeKind::Applied(_)) {
                        self.mark("reach: a generic type is a type argument");
                    }
                    if arg.optional > 0 {
                        self.mark("reach: an optional is a type argument");
                    }
                    self.type_expr(arg);
                }
            }
            ast::TypeKind::Array(element) => {
                self.mark("ast::TypeKind::Array");
                self.type_expr(element);
            }
        }
    }

    fn ast_block(&mut self, block: &ast::Block) {
        if let [.., last, dead] = &block.stmts[..]
            && let ast::StmtKind::Expr(e) = &dead.kind
            && let ast::ExprKind::If {
                cond,
                then_block,
                else_block: None,
            } = &e.kind
            && matches!(cond.kind, ast::ExprKind::Bool(false))
            && let [copy] = &then_block.stmts[..]
            && copy.to_string() == last.to_string()
        {
            self.misplaced.push(format!(
                "an `if false` after a block's last statement: {dead}"
            ));
        }
        for (i, stmt) in block.stmts.iter().enumerate() {
            let before = i.checked_sub(1).map(|at| &block.stmts[at]);
            if let ast::StmtKind::While { cond, body, .. } = &stmt.kind {
                self.while_bound(before, cond, body);
            }
            if let ast::StmtKind::WhileLet {
                pattern,
                value,
                body,
                ..
            } = &stmt.kind
            {
                self.while_let_bound(before, pattern, value, body);
            }
            if let ast::StmtKind::Expr(e) = &stmt.kind
                && let ast::ExprKind::Loop { body, .. } = &e.kind
            {
                self.loop_bound(before, e, body);
            }
            self.ast_stmt(stmt);
        }
    }

    /// Walks the target of an assignment.
    fn target(&mut self, target: &ast::Expr) {
        if matches!(target.kind, ast::ExprKind::Block(_)) {
            self.misplaced.push(format!("a block as a place: {target}"));
        }
        self.writing = true;
        self.ast_expr(target);
        self.writing = false;
    }

    /// Notes an index alone in the branch an `else` starts: the `else` block, or the first
    /// branch of the `if` an `else if` folds into it, which that `if` does not see as later.
    fn later_branch(&mut self, else_block: &ast::Block) {
        let block = match alone(else_block).map(|e| &e.kind) {
            Some(
                ast::ExprKind::If { then_block, .. } | ast::ExprKind::IfLet { then_block, .. },
            ) => then_block,
            Some(_) | None => else_block,
        };
        if let Some(e) = alone(block)
            && matches!(e.kind, ast::ExprKind::Index { .. })
        {
            self.later.insert(e.span.start());
        }
    }

    /// Marks the `Some` reach of a `while let` whose value `(if c { e } else { None })` the
    /// checker gives as a wrapped `e`: a `T`, so its first run is a `Some`; and the reach of one
    /// that walks an enum chain, which the checker lowers to a `match`.
    fn some_value(&mut self, scrutinee: &tir::Expr) {
        if self.chains.contains(&scrutinee.span.start()) {
            self.mark("reach: a `while let` walks an enum chain");
        }
        if self.some_values.contains(&scrutinee.span.start())
            && let tir::ExprKind::If { then_block, .. } = &scrutinee.kind
            && then_block
                .value
                .as_ref()
                .is_some_and(|value| matches!(value.kind, tir::ExprKind::Wrap(_)))
        {
            self.mark("reach: a `while let` takes its `Some` branch");
        }
    }

    /// Checks that a `while let` counts down as a `while` does, its value `(if cond { e } else
    /// { None })`, where the first run of `e` gives a `Some`, so a positive bound runs the body;
    /// or that it walks a chain, `while let V(t, ...) = c { c = t ... }` after
    /// `let mut c = E.V(E.V(... end ...), ...)`, which runs once a node.
    fn while_let_bound(
        &mut self,
        before: Option<&ast::Stmt>,
        pattern: &ast::Pattern,
        value: &ast::Expr,
        body: &ast::Block,
    ) {
        if let Some(nodes) = self.chain_bound(before, pattern, value, body) {
            if nodes > 0 {
                self.chains.insert(value.span.start());
            }
            return;
        }
        if let ast::ExprKind::If {
            cond,
            else_block: Some(other),
            ..
        } = &value.kind
            && let [none] = &other.stmts[..]
            && matches!(&none.kind, ast::StmtKind::Expr(e) if matches!(e.kind, ast::ExprKind::NoneLit))
        {
            // The TIR walk marks the reach once the checker wraps the first branch.
            if self.while_bound(before, cond, body) > 0 {
                self.some_values.insert(value.span.start());
            }
        } else {
            self.unbounded
                .push(format!("a `while let` at byte {}", value.span.start()));
        }
    }

    /// The nodes of the chain a `while let` walks, `None` unless it walks one of at most
    /// `MAX_BOUND` that ends in another variant of the enum.
    fn chain_bound(
        &self,
        before: Option<&ast::Stmt>,
        pattern: &ast::Pattern,
        value: &ast::Expr,
        body: &ast::Block,
    ) -> Option<i128> {
        let chain = name(value)?;
        let ast::StmtKind::Let {
            mutable: Some(_),
            name: declared,
            init,
            ..
        } = &before?.kind
        else {
            return None;
        };
        let ast::PatternKind::Variant {
            name: variant,
            fields,
            ..
        } = &pattern.kind
        else {
            return None;
        };
        let ast::PatternKind::Name(rest) = &fields.first()?.kind else {
            return None;
        };
        let steps = matches!(&body.stmts.first()?.kind, ast::StmtKind::Assign { target, value }
            if name(target) == Some(chain) && name(value) == Some(rest.name.as_str()));
        let nodes = self.nodes(init, variant.name.as_str())?;
        (declared.name.as_str() == chain && steps && nodes <= i128::from(MAX_BOUND))
            .then_some(nodes)
    }

    /// How many nodes of the variant `variant` `e` nests, each in the first field of the one
    /// before, `None` unless it ends in another variant of their enum.
    fn nodes(&self, e: &ast::Expr, variant: &str) -> Option<i128> {
        let (receiver, built, next) = match &e.kind {
            ast::ExprKind::MethodCall {
                receiver,
                method,
                args,
                ..
            } => (receiver, method, args.first().map(|arg| &arg.value)),
            ast::ExprKind::Field { base, name, .. } => (base, name, None),
            _ => return None,
        };
        let variants = self.variants.get(enum_name(receiver)?)?;
        let built = built.name.as_str();
        if !variants.iter().any(|v| v == built) {
            None
        } else if built == variant {
            Some(1 + self.nodes(next?, variant)?)
        } else {
            Some(0)
        }
    }

    /// Checks that a `while` counts down (decision 5): `let mut c = n` before it, with `n` at most
    /// `MAX_BOUND`, a condition that starts `c > 0`, and `c -= 1` first in its body. Gives `n`,
    /// 0 for a loop that does not count down.
    fn while_bound(
        &mut self,
        before: Option<&ast::Stmt>,
        cond: &ast::Expr,
        body: &ast::Block,
    ) -> i128 {
        let start = if let ast::ExprKind::Binary {
            op: ast::BinaryOp::And,
            lhs,
            ..
        } = &cond.kind
        {
            lhs
        } else {
            cond
        };
        let counted = if let ast::ExprKind::Compare { operands, ops } = &start.kind
            && ops[..] == [ast::CompareOp::Gt]
            && int(&operands[1]) == Some(0)
        {
            name(&operands[0])
        } else {
            None
        };
        let bound = counter(before, counted);
        let steps = steps(body.stmts.first(), counted);
        match bound {
            Some(n) if steps && (0..=i128::from(MAX_BOUND)).contains(&n) => {
                if n > 0 {
                    self.mark("reach: a loop runs its body");
                }
                n
            }
            _ => {
                self.unbounded
                    .push(format!("a `while` at byte {}", cond.span.start()));
                0
            }
        }
    }

    /// Checks that a `loop` counts down as a `while` does: `let mut c = n` before it, with `n`
    /// at most `MAX_BOUND`, `if c == 0 { break [v] }` first in its body and `c -= 1` next.
    fn loop_bound(&mut self, before: Option<&ast::Stmt>, e: &ast::Expr, body: &ast::Block) {
        let guard = body.stmts.first().and_then(|stmt| {
            if let ast::StmtKind::Expr(guard) = &stmt.kind
                && let ast::ExprKind::If {
                    cond,
                    then_block,
                    else_block: None,
                } = &guard.kind
                && let ast::ExprKind::Binary {
                    op: ast::BinaryOp::Eq,
                    lhs,
                    rhs,
                    ..
                } = &cond.kind
                && int(rhs) == Some(0)
                && let [jump] = &then_block.stmts[..]
                && let ast::StmtKind::Break { label: None, value } = &jump.kind
            {
                Some((name(lhs)?, value.is_some()))
            } else {
                None
            }
        });
        let counted = guard.map(|(counted, _)| counted);
        let steps = steps(body.stmts.get(1), counted);
        match (counter(before, counted), guard) {
            (Some(n), Some((_, value))) if steps && (0..=i128::from(MAX_BOUND)).contains(&n) => {
                if n > 0 {
                    self.mark("reach: a loop runs its body");
                }
                self.mark(if value {
                    "reach: a `loop` gives a value"
                } else {
                    "reach: a `loop` is a statement"
                });
                self.counted.insert(e.span.start());
            }
            _ => {}
        }
    }

    /// Walks a `kind` loop labeled `label`, if any, by `walk`, with its label open.
    fn ast_loop(&mut self, kind: &str, label: Option<&ast::Ident>, walk: impl FnOnce(&mut Self)) {
        if label.is_some() {
            self.mark(&format!("reach: a `{kind}` has a loop label"));
        }
        self.labels.push(label.map(|label| label.name));
        walk(self);
        self.labels.pop();
    }

    /// Records a `kind` jump, `break` or `continue`, that names `label`, if any.
    fn jump_label(&mut self, kind: &str, label: Option<&ast::Ident>) {
        if let Some(label) = label {
            self.mark(&format!("reach: a `{kind}` names a loop label"));
            if self.labels.last() != Some(&Some(label.name)) {
                self.mark("reach: a labeled jump leaves an inner loop");
            }
        }
    }

    /// Marks a `break` spelled `break l -1` that reads as the label `l` and the value `-1`, or
    /// spelled `break x -1` that reads as no label and the value `x - 1`.
    fn break_reading(&mut self, stmt: &ast::Stmt, label: Option<&ast::Ident>, value: &ast::Expr) {
        let text = &self.source[stmt.span.range()];
        match (label, &value.kind) {
            (
                Some(label),
                ast::ExprKind::Unary {
                    op: ast::UnaryOp::Neg,
                    operand,
                },
            ) if int(operand) == Some(1) && text == format!("break {} -1", label.name) => {
                self.mark("reach: `break l -1` is a label and `-1`");
            }
            (
                None,
                ast::ExprKind::Binary {
                    op: ast::BinaryOp::Sub,
                    lhs,
                    rhs,
                    ..
                },
            ) if int(rhs) == Some(1)
                && name(lhs).is_some_and(|x| text == format!("break {x} -1")) =>
            {
                self.mark("reach: `break x -1` is `x - 1`");
            }
            _ => {}
        }
    }

    fn ast_for(&mut self, source: &ast::ForSource, body: &ast::Block) {
        self.mark("ast::StmtKind::For");
        match source {
            ast::ForSource::Array(array) => {
                self.mark("ast::ForSource::Array");
                match array_len(array) {
                    Some(runs) if runs <= i128::from(MAX_BOUND) => {
                        if runs > 0 {
                            self.mark("reach: a `for` over an array runs its body");
                        }
                    }
                    _ => self
                        .unbounded
                        .push(format!("a `for` at byte {}", array.span.start())),
                }
                self.ast_expr(array);
            }
            ast::ForSource::Range {
                start,
                end,
                inclusive,
            } => {
                self.mark("ast::ForSource::Range");
                let runs = int(start)
                    .zip(int(end))
                    .map(|(start, end)| end - start + i128::from(*inclusive));
                match runs {
                    Some(runs) if runs <= i128::from(MAX_BOUND) => {
                        if runs > 0 {
                            self.mark("reach: a loop runs its body");
                        }
                    }
                    _ => self
                        .unbounded
                        .push(format!("a `for` at byte {}", start.span.start())),
                }
                self.mark(&format!("ast::ForSource::Range.inclusive = {inclusive}"));
                self.ast_expr(start);
                self.ast_expr(end);
            }
        }
        self.ast_block(body);
    }

    fn ast_let(&mut self, mutable: bool, name: &str, ty: Option<&ast::TypeExpr>, init: &ast::Expr) {
        self.mark("ast::StmtKind::Let");
        if mutable {
            self.immutable.remove(name);
        } else {
            self.immutable.insert(name.to_string());
        }
        if let Some(len) = array_len(init) {
            self.lens.insert(name.to_string(), len);
        }
        if let Some(ty) = ty {
            self.type_expr(ty);
        }
        self.ast_expr(init);
    }

    fn ast_stmt(&mut self, stmt: &ast::Stmt) {
        match &stmt.kind {
            ast::StmtKind::Let {
                ty,
                init,
                name,
                mutable,
            } => self.ast_let(mutable.is_some(), name.name.as_str(), ty.as_ref(), init),
            ast::StmtKind::While {
                label, cond, body, ..
            } => self.ast_loop("while", label.as_ref(), |walk| {
                walk.mark("ast::StmtKind::While");
                walk.ast_expr(cond);
                walk.ast_block(body);
            }),
            ast::StmtKind::WhileLet {
                label,
                pattern,
                value,
                body,
                ..
            } => self.ast_loop("while let", label.as_ref(), |walk| {
                walk.mark("ast::StmtKind::WhileLet");
                walk.ast_pattern(pattern);
                walk.ast_expr(value);
                walk.ast_block(body);
            }),
            ast::StmtKind::For {
                label,
                source,
                body,
                ..
            } => self.ast_loop("for", label.as_ref(), |walk| walk.ast_for(source, body)),
            ast::StmtKind::Return(value) => {
                self.mark("ast::StmtKind::Return");
                if let Some(value) = value {
                    self.ast_expr(value);
                }
            }
            ast::StmtKind::Break { label, value } => {
                self.mark("ast::StmtKind::Break");
                self.jump_label("break", label.as_ref());
                if let Some(value) = value {
                    if label.is_some() {
                        self.mark("reach: a labeled `break` gives a value");
                    }
                    self.break_reading(stmt, label.as_ref(), value);
                    self.ast_expr(value);
                }
            }
            ast::StmtKind::Continue { label } => {
                self.mark("ast::StmtKind::Continue");
                self.jump_label("continue", label.as_ref());
            }
            ast::StmtKind::Assign { target, value } => {
                self.mark("ast::StmtKind::Assign");
                if name(target).is_some() {
                    self.assigns.insert(value.span.start());
                }
                self.target(target);
                self.ast_expr(value);
            }
            ast::StmtKind::CompoundAssign {
                op, target, value, ..
            } => {
                self.mark("ast::StmtKind::CompoundAssign");
                self.compounds.insert(stmt.span.start());
                if *op == ast::BinaryOp::Add && name(target).is_some() {
                    self.adds.insert(stmt.span.start());
                }
                if matches!(target.kind, ast::ExprKind::Field { .. }) {
                    self.mark("reach: a compound assignment targets a field");
                }
                self.binary_op(*op);
                self.target(target);
                self.ast_expr(value);
            }
            ast::StmtKind::Expr(e) => {
                self.mark("ast::StmtKind::Expr");
                // A statement `if`: not a value `if`, written `(if ...)`, nor an `else if`,
                // which the parser folds into a statement of the `else` block.
                let before = self.source[..e.span.start()].trim_end();
                let bare = !before.ends_with('(') && !before.ends_with("else");
                if matches!(e.kind, ast::ExprKind::If { .. }) && bare && jumps(stmt) {
                    self.mark("reach: an `if` statement jumps in every branch");
                }
                self.ast_expr(e);
            }
        }
    }

    fn binary_op(&mut self, op: ast::BinaryOp) {
        self.mark(match op {
            ast::BinaryOp::Add => "ast::BinaryOp::Add",
            ast::BinaryOp::Sub => "ast::BinaryOp::Sub",
            ast::BinaryOp::Mul => "ast::BinaryOp::Mul",
            ast::BinaryOp::Div => "ast::BinaryOp::Div",
            ast::BinaryOp::Rem => "ast::BinaryOp::Rem",
            ast::BinaryOp::And => "ast::BinaryOp::And",
            ast::BinaryOp::Or => "ast::BinaryOp::Or",
            ast::BinaryOp::Eq => "ast::BinaryOp::Eq",
            ast::BinaryOp::Ne => "ast::BinaryOp::Ne",
        });
    }

    /// Records a `panic` or `todo` message written as a literal.
    fn message(&mut self, args: &[ast::Arg]) {
        if let [arg] = args
            && let ast::ExprKind::Str(text) = &arg.value.kind
        {
            self.messages.push(text.clone());
        }
    }

    fn ast_args(&mut self, args: &[ast::Arg]) {
        for arg in args {
            self.ast_expr(&arg.value);
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per node kind, so the walk reads in one place"
    )]
    fn ast_expr(&mut self, e: &ast::Expr) {
        match &e.kind {
            ast::ExprKind::Int { .. } => self.mark("ast::ExprKind::Int"),
            ast::ExprKind::Float(_) => self.mark("ast::ExprKind::Float"),
            ast::ExprKind::Bool(_) => self.mark("ast::ExprKind::Bool"),
            ast::ExprKind::Char(_) => self.mark("ast::ExprKind::Char"),
            ast::ExprKind::Str(text) => {
                self.mark("ast::ExprKind::Str");
                if text.chars().any(|c| "\t\\\"'{}\0\r\n\u{1F600}".contains(c)) {
                    self.mark("reach: a string literal holds an escape");
                }
            }
            ast::ExprKind::Interpolate(parts) => {
                self.mark("ast::ExprKind::Interpolate");
                for part in parts {
                    self.ast_expr(&part.value);
                    if let Some(align) = part.spec.and_then(|spec| spec.align) {
                        self.mark(match align {
                            ast::Align::Left => "ast::Align::Left",
                            ast::Align::Right => "ast::Align::Right",
                            ast::Align::Center => "ast::Align::Center",
                        });
                    }
                }
            }
            ast::ExprKind::Name(name) => {
                self.mark("ast::ExprKind::Name");
                if name.as_str() != "depth" && self.params.iter().any(|p| p == name.as_str()) {
                    self.mark("reach: a parameter other than `depth` is read");
                }
            }
            ast::ExprKind::Applied { types, .. } => {
                self.mark("ast::ExprKind::Applied");
                for ty in types {
                    self.type_expr(ty);
                }
            }
            ast::ExprKind::Call {
                callee,
                args,
                types,
                ..
            } => {
                self.mark("ast::ExprKind::Call");
                let callee = callee.name.as_str();
                if callee == "Some" {
                    self.mark("reach: `Some` builds an optional");
                }
                if callee == "Ok" || callee == "Err" {
                    self.mark(&format!("reach: `{callee}` builds a `Result`"));
                }
                self.generic_call(callee, types.is_empty());
                self.mark(&format!("prelude {callee}"));
                if callee == "panic" {
                    self.message(args);
                }
                if self.funcs.contains(callee) {
                    if callee == self.func {
                        self.mark("reach: a function calls itself");
                    }
                    let depth = args.first().and_then(|arg| int(&arg.value));
                    if self.func == "main" && depth.is_some_and(|depth| depth >= 1) {
                        self.mark("reach: `main` calls a function at a depth of 1 or more");
                    }
                }
                self.ast_args(args);
            }
            ast::ExprKind::Todo { args, .. } => {
                self.mark("ast::ExprKind::Todo");
                self.message(args);
                self.ast_args(args);
            }
            ast::ExprKind::SelfValue => self.mark("ast::ExprKind::SelfValue"),
            ast::ExprKind::NoneLit => self.mark("ast::ExprKind::NoneLit"),
            ast::ExprKind::Field { base, name, safe } => {
                self.mark("ast::ExprKind::Field");
                self.mark(&format!("ast::ExprKind::Field.safe = {safe}"));
                if enum_name(base) == Some("Option") && name.name.as_str() == "None" {
                    self.mark("reach: `Option.None` or `Option<..>.None`");
                }
                if matches!(&base.kind, ast::ExprKind::Name(n) if n.as_str() == "f64") {
                    self.mark(&format!("f64.{}", name.name.as_str()));
                }
                self.ast_expr(base);
            }
            ast::ExprKind::MethodCall {
                receiver,
                method,
                args,
                safe,
                ..
            } => {
                self.mark("ast::ExprKind::MethodCall");
                self.mark(&format!("ast::ExprKind::MethodCall.safe = {safe}"));
                if enum_name(receiver) == Some("Option") && method.name.as_str() == "Some" {
                    self.mark("reach: `Some` builds an optional");
                }
                let built = enum_name(receiver)
                    .and_then(|receiver| self.variants.get(receiver))
                    .is_some_and(|variants| variants.iter().any(|v| v == method.name.as_str()));
                if built && name(receiver).is_some_and(|n| self.generic_enums.contains(n)) {
                    self.mark("reach: a generic variant is built with inferred type arguments");
                }
                if built && !args.is_empty() {
                    self.mark(if args.iter().all(|arg| arg.label.is_some()) {
                        "reach: a variant is built with labeled fields"
                    } else {
                        "reach: a variant is built with positional fields"
                    });
                }
                self.ast_expr(receiver);
                self.ast_args(args);
            }
            ast::ExprKind::Unary { op, operand } => {
                self.mark("ast::ExprKind::Unary");
                self.mark(match op {
                    ast::UnaryOp::Neg => "ast::UnaryOp::Neg",
                    ast::UnaryOp::Not => "ast::UnaryOp::Not",
                });
                self.ast_expr(operand);
            }
            ast::ExprKind::Binary { op, lhs, rhs, .. } => {
                self.mark("ast::ExprKind::Binary");
                self.binary_op(*op);
                if !self.in_const
                    && matches!(
                        op,
                        ast::BinaryOp::Add
                            | ast::BinaryOp::Sub
                            | ast::BinaryOp::Mul
                            | ast::BinaryOp::Div
                            | ast::BinaryOp::Rem
                    )
                {
                    self.arithmetic.insert(e.span.start());
                }
                if matches!(op, ast::BinaryOp::Eq | ast::BinaryOp::Ne)
                    && [lhs, rhs]
                        .iter()
                        .any(|e| matches!(e.kind, ast::ExprKind::NoneLit))
                {
                    self.mark("reach: `==` or `!=` compares with `None`");
                }
                self.ast_expr(lhs);
                self.ast_expr(rhs);
            }
            ast::ExprKind::Compare { operands, ops } => {
                self.mark("ast::ExprKind::Compare");
                for op in ops {
                    self.mark(match op {
                        ast::CompareOp::Lt => "ast::CompareOp::Lt",
                        ast::CompareOp::Le => "ast::CompareOp::Le",
                        ast::CompareOp::Gt => "ast::CompareOp::Gt",
                        ast::CompareOp::Ge => "ast::CompareOp::Ge",
                    });
                }
                for operand in operands {
                    self.ast_expr(operand);
                }
            }
            ast::ExprKind::Coalesce { lhs, rhs } => {
                self.mark("ast::ExprKind::Coalesce");
                self.ast_expr(lhs);
                self.ast_expr(rhs);
            }
            ast::ExprKind::ArrayLit(elements) => {
                self.mark("ast::ExprKind::ArrayLit");
                for element in elements {
                    self.ast_expr(element);
                }
            }
            ast::ExprKind::ArrayFill { value, len } => {
                self.mark("ast::ExprKind::ArrayFill");
                self.ast_expr(value);
                self.ast_expr(len);
            }
            ast::ExprKind::Index { base, index } => {
                self.mark("ast::ExprKind::Index");
                let len =
                    name(base).map_or_else(|| array_len(base), |name| self.lens.get(name).copied());
                match (len, int(index)) {
                    (Some(len), Some(at)) if (0..len).contains(&at) => {
                        self.mark("reach: an index is in bounds");
                        if let Some(local) = name(base)
                            && !self.writing
                        {
                            self.mark("reach: an index reads a local");
                            if self.immutable.contains(local) {
                                self.mark("reach: an index reads an immutable local");
                            }
                        }
                        if name(base).is_none() {
                            self.mark("reach: an index reads an array literal in bounds");
                        }
                    }
                    (Some(1), Some(1))
                        if name(base).is_none() && self.later.contains(&e.span.start()) =>
                    {
                        self.mark("reach: an index is meant to panic");
                    }
                    _ => self
                        .out_of_bounds
                        .push(format!("an index at byte {}", e.span.start())),
                }
                self.ast_expr(base);
                self.ast_expr(index);
            }
            ast::ExprKind::Is { lhs, rhs } => {
                self.mark("ast::ExprKind::Is");
                self.ast_expr(lhs);
                self.ast_expr(rhs);
            }
            ast::ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                self.mark("ast::ExprKind::If");
                if let Some(block) = else_block {
                    self.later_branch(block);
                }
                self.ast_expr(cond);
                self.ast_block(then_block);
                if let Some(block) = else_block {
                    self.ast_block(block);
                }
            }
            ast::ExprKind::IfLet {
                pattern,
                value,
                then_block,
                else_block,
            } => {
                self.mark("ast::ExprKind::IfLet");
                if let Some(block) = else_block {
                    self.later_branch(block);
                }
                self.ast_pattern(pattern);
                self.ast_expr(value);
                self.ast_block(then_block);
                if let Some(block) = else_block {
                    self.ast_block(block);
                }
            }
            ast::ExprKind::Match {
                scrutinee, arms, ..
            } => {
                self.mark("ast::ExprKind::Match");
                if self.in_generic {
                    self.mark("reach: a generic function matches a generic enum");
                }
                self.ast_expr(scrutinee);
                for arm in arms {
                    self.ast_pattern(&arm.pattern);
                    if let Some(guard) = &arm.guard {
                        self.ast_expr(guard);
                    }
                    self.ast_expr(&arm.body);
                }
            }
            ast::ExprKind::Loop { label, body, .. } => {
                self.mark("ast::ExprKind::Loop");
                if !self.counted.contains(&e.span.start()) {
                    self.unbounded
                        .push(format!("a `loop` at byte {}", e.span.start()));
                }
                self.ast_loop("loop", label.as_ref(), |walk| walk.ast_block(body));
            }
            ast::ExprKind::Block(block) => {
                self.mark("ast::ExprKind::Block");
                if self.in_const {
                    self.misplaced
                        .push(format!("a block in a constant: {block}"));
                }
                self.ast_block(block);
            }
        }
    }

    fn ast_pattern(&mut self, pattern: &ast::Pattern) {
        match &pattern.kind {
            ast::PatternKind::Wild => self.mark("ast::PatternKind::Wild"),
            ast::PatternKind::Name(_) => self.mark("ast::PatternKind::Name"),
            ast::PatternKind::None => self.mark("ast::PatternKind::None"),
            ast::PatternKind::Variant { fields, .. } => {
                self.mark("ast::PatternKind::Variant");
                for field in fields {
                    self.ast_pattern(field);
                }
            }
            ast::PatternKind::Literal(literal) => {
                self.mark("ast::PatternKind::Literal");
                self.literal(literal);
            }
            ast::PatternKind::Range {
                start,
                end,
                inclusive,
            } => {
                self.mark("ast::PatternKind::Range");
                self.mark(&format!("ast::PatternKind::Range.inclusive = {inclusive}"));
                for bound in [start, end] {
                    match bound {
                        ast::Bound::Literal(literal) => {
                            self.mark("ast::Bound::Literal");
                            self.literal(literal);
                        }
                        ast::Bound::Name(_) => self.mark("ast::Bound::Name"),
                    }
                }
            }
            ast::PatternKind::Or(alternatives) => {
                self.mark("ast::PatternKind::Or");
                for alternative in alternatives {
                    self.ast_pattern(alternative);
                }
            }
        }
    }

    fn literal(&mut self, literal: &ast::Literal) {
        self.mark(match literal {
            ast::Literal::Int { .. } => "ast::Literal::Int",
            ast::Literal::Char(_) => "ast::Literal::Char",
            ast::Literal::Str(_) => "ast::Literal::Str",
            ast::Literal::Bool(_) => "ast::Literal::Bool",
        });
    }

    fn tir(&mut self, program: &tir::Program) {
        self.result = program.enums.iter().position(|e| e.name == "Result");
        self.payloads = program
            .enums
            .iter()
            .map(|e| {
                e.variants
                    .iter()
                    .flat_map(|v| v.fields.iter().map(|field| field.ty))
                    .collect()
            })
            .collect();
        for field in program
            .enums
            .iter()
            .flat_map(|e| e.variants.iter().flat_map(|v| &v.fields))
        {
            self.mark(match field.rank {
                tir::Rank::Plain => "tir::Rank::Plain",
                tir::Rank::Own => "tir::Rank::Own",
                tir::Rank::Open => "tir::Rank::Open",
                tir::Rank::Late => "tir::Rank::Late",
                tir::Rank::Never => "tir::Rank::Never",
            });
        }
        for (id, e) in program.enums.iter().enumerate() {
            self.option_shaped(id, e);
        }
        for function in &program.functions {
            self.ty(function.ret);
            for local in &function.locals {
                self.ty(local.ty);
                self.stored(local.ty);
            }
            self.tir_block(&function.body);
        }
    }

    /// Records an enum `id` of two variants, one with no payload and one with one field. The
    /// field's storage has a spare value: it is a box, or a `char`, `bool` or pointer type. This
    /// rule is a subset of codegen's `option_shape`, which also accepts an enum or optional
    /// field with a niche. Thus the walker can miss such an enum, but it never marks an enum
    /// that codegen lays out tagged.
    fn option_shaped(&mut self, id: usize, e: &tir::Enum) {
        let [first, second] = e.variants.as_slice() else {
            return;
        };
        let (([field], empty) | (empty, [field])) =
            (first.fields.as_slice(), second.fields.as_slice())
        else {
            return;
        };
        let niche = if field.boxed {
            !matches!(field.ty.kind(), TypeKind::Optional(_))
        } else {
            matches!(
                field.ty.kind(),
                TypeKind::Char
                    | TypeKind::Bool
                    | TypeKind::String
                    | TypeKind::Class(..)
                    | TypeKind::Array(_)
            )
        };
        if !empty.is_empty() || !niche {
            return;
        }
        self.mark("reach: an `Option`-shaped enum has a niche");
        if field.boxed && field.ty.inline_enum().is_some_and(|held| held.0 == id) {
            self.mark("reach: an `Option`-shaped enum holds itself");
        }
    }

    /// Records the optionals with a niche that a local of `ty` holds, itself or as its elements.
    fn stored(&mut self, mut ty: Type) {
        while let TypeKind::Array(element) = ty.kind() {
            ty = element;
        }
        let TypeKind::Optional(payload) = ty.kind() else {
            return;
        };
        match payload.kind() {
            TypeKind::Char => self.mark("reach: a `char?` is stored"),
            TypeKind::Bool => self.mark("reach: a `bool?` is stored"),
            TypeKind::Enum(..) => self.mark("reach: an optional of an enum is stored"),
            TypeKind::Optional(inner)
                if matches!(inner.kind(), TypeKind::Char | TypeKind::Bool) =>
            {
                self.mark("reach: an optional of an optional of `char` or `bool` is stored");
            }
            _ => {}
        }
    }

    /// Records `ty`'s kind, and the kinds inside it.
    fn ty(&mut self, ty: Type) {
        let (key, inner) = match ty.kind() {
            TypeKind::I64 => ("types::TypeKind::I64", None),
            TypeKind::U8 => ("types::TypeKind::U8", None),
            TypeKind::F64 => ("types::TypeKind::F64", None),
            TypeKind::Bool => ("types::TypeKind::Bool", None),
            TypeKind::Char => ("types::TypeKind::Char", None),
            TypeKind::String => ("types::TypeKind::String", None),
            TypeKind::Unit => ("types::TypeKind::Unit", None),
            TypeKind::Never => ("types::TypeKind::Never", None),
            TypeKind::Class(..) => ("types::TypeKind::Class", None),
            TypeKind::Enum(..) => ("types::TypeKind::Enum", None),
            TypeKind::Param(..) => ("types::TypeKind::Param", None),
            TypeKind::Array(element) => ("types::TypeKind::Array", Some(element)),
            TypeKind::Optional(inner) => ("types::TypeKind::Optional", Some(inner)),
        };
        self.mark(key);
        if let Some(inner) = inner {
            self.ty(inner);
        }
    }

    fn receiver(&mut self, variant: &str, receiver: &tir::Expr) {
        let kind = kind_name(receiver.ty);
        self.mark(&format!("{variant} on {kind}"));
    }

    fn tir_block(&mut self, block: &tir::Block) {
        self.ty(block.ty);
        for stmt in &block.stmts {
            match stmt {
                tir::Stmt::Let { init, .. } => {
                    self.mark("tir::Stmt::Let");
                    self.tir_expr(init, Parent::Other);
                }
                tir::Stmt::Assign { place, value } => {
                    self.mark("tir::Stmt::Assign");
                    if !matches!(place, tir::Place::Local(_))
                        && self.holds_class(value.ty, &mut Vec::new())
                    {
                        self.heap_classes
                            .push(format!("an assignment at byte {}", value.span.start()));
                    }
                    // A non-empty literal adds chars for certain.
                    if matches!(place, tir::Place::Local(_))
                        && let tir::ExprKind::Concat { rhs, .. } = &value.kind
                        && matches!(&rhs.kind, tir::ExprKind::Str(text) if !text.is_empty())
                        && self.adds.contains(&value.span.start())
                    {
                        self.mark("reach: a `+=` adds chars to a string local");
                    }
                    if matches!(place, tir::Place::Local(_))
                        && !matches!(value.ty.kind(), TypeKind::Array(_))
                        && self.assigns.contains(&value.span.start())
                    {
                        self.mark("reach: a plain `=` to a local that is not an array");
                    }
                    self.place(place);
                    self.tir_expr(value, Parent::Other);
                }
                tir::Stmt::Append { place, value } => {
                    self.mark("tir::Stmt::Append");
                    self.place(place);
                    self.tir_expr(value, Parent::Other);
                }
                tir::Stmt::Expr(e) => {
                    self.mark("tir::Stmt::Expr");
                    self.tir_expr(e, Parent::Discard);
                }
            }
        }
        if let Some(value) = &block.value {
            self.tir_expr(value, Parent::Other);
        }
    }

    /// Whether a value of `ty` may hold a class object, where `seen` are the enums looked into. A
    /// type parameter of a generic body may stand for a class.
    fn holds_class(&self, ty: Type, seen: &mut Vec<tir::EnumId>) -> bool {
        match ty.kind() {
            TypeKind::Class(..) | TypeKind::Param(..) => true,
            TypeKind::Array(inner) | TypeKind::Optional(inner) => self.holds_class(inner, seen),
            TypeKind::Enum(id, args) => {
                !seen.contains(&id) && {
                    seen.push(id);
                    // A payload names the enum's parameters, which its arguments stand for.
                    let args = args.get();
                    self.payloads[id.0]
                        .iter()
                        .any(|field| self.holds_class(field.subst(args), seen))
                }
            }
            TypeKind::I64
            | TypeKind::U8
            | TypeKind::F64
            | TypeKind::Bool
            | TypeKind::Char
            | TypeKind::String
            | TypeKind::Unit
            | TypeKind::Never => false,
        }
    }

    fn place(&mut self, place: &tir::Place) {
        match place {
            tir::Place::Local(_) => self.mark("tir::Place::Local"),
            tir::Place::Field { base, .. } => {
                self.mark("tir::Place::Field");
                self.tir_expr(base, Parent::Other);
            }
            tir::Place::Index { base, index } => {
                self.mark("tir::Place::Index");
                self.place(base);
                self.tir_expr(index, Parent::Other);
            }
        }
    }

    /// Marks the operands of `e`, a binary `lhs op rhs`, that are not literals: on the right, of
    /// arithmetic or a compound assignment of `f64`s or `i64`s; on the left, of arithmetic of
    /// `i64`s or `u8`s. A loop's own step, a constant's arithmetic, and the `depth - 1` of a
    /// call in a function, none of them a choice of `arithmetic`, are left out.
    fn operands(&mut self, e: &tir::Expr, lhs: &tir::Expr, rhs: &tir::Expr) {
        let text = |operand: &tir::Expr| &self.source[operand.span.start()..operand.span.end()];
        let written = self.arithmetic.contains(&e.span.start()) && text(e) != "depth - 1";
        let (left, right) = (!literal(text(lhs)), !literal(text(rhs)));
        if written || self.compounds.contains(&e.span.start()) {
            match e.ty.kind() {
                TypeKind::F64 => {
                    self.f64_rights[usize::from(right)] += 1;
                    if right {
                        self.mark(
                            "reach: `f64` arithmetic or `op=` takes a right operand that is not a literal",
                        );
                    }
                }
                TypeKind::I64 if right => self.mark(
                    "reach: `i64` arithmetic or `op=` takes a right operand that is not a literal",
                ),
                TypeKind::I64
                | TypeKind::U8
                | TypeKind::Bool
                | TypeKind::Char
                | TypeKind::String
                | TypeKind::Unit
                | TypeKind::Never
                | TypeKind::Array(_)
                | TypeKind::Optional(_)
                | TypeKind::Class(..)
                | TypeKind::Enum(..)
                | TypeKind::Param(..) => {}
            }
        }
        if written && left && matches!(e.ty.kind(), TypeKind::I64 | TypeKind::U8) {
            self.mark("reach: `i64` or `u8` arithmetic takes a left operand that is not a literal");
        }
    }

    fn tir_exprs(&mut self, exprs: &[tir::Expr]) {
        for e in exprs {
            self.tir_expr(e, Parent::Other);
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per node kind, so the walk reads in one place"
    )]
    fn tir_expr(&mut self, e: &tir::Expr, parent: Parent) {
        self.ty(e.ty);
        match &e.kind {
            tir::ExprKind::Int(_) => self.mark("tir::ExprKind::Int"),
            tir::ExprKind::Float(_) => self.mark("tir::ExprKind::Float"),
            tir::ExprKind::Bool(_) => self.mark("tir::ExprKind::Bool"),
            tir::ExprKind::Char(_) => self.mark("tir::ExprKind::Char"),
            tir::ExprKind::Str(_) => self.mark("tir::ExprKind::Str"),
            tir::ExprKind::None => self.mark("tir::ExprKind::None"),
            tir::ExprKind::Local(_) => self.mark("tir::ExprKind::Local"),
            tir::ExprKind::Call { types, args, .. } => {
                self.mark("tir::ExprKind::Call");
                if types
                    .get()
                    .iter()
                    .any(|t| matches!(t.kind(), TypeKind::Optional(_)))
                {
                    self.mark("reach: an optional is a type argument");
                }
                self.tir_exprs(args);
            }
            tir::ExprKind::New { args, .. } => {
                self.mark("tir::ExprKind::New");
                self.tir_exprs(args);
            }
            tir::ExprKind::Variant { args, .. } => {
                self.mark("tir::ExprKind::Variant");
                self.tir_exprs(args);
            }
            tir::ExprKind::Field { base, .. } => {
                self.mark("tir::ExprKind::Field");
                self.tir_expr(base, Parent::Other);
            }
            tir::ExprKind::Wrap(inner) => {
                self.mark("tir::ExprKind::Wrap");
                self.tir_expr(inner, Parent::Other);
            }
            tir::ExprKind::Convert(operand) => {
                self.mark("tir::ExprKind::Convert");
                // A conversion that can fail gives an optional.
                let to = if let TypeKind::Optional(to) = e.ty.kind() {
                    to
                } else {
                    e.ty
                };
                let (from, to) = (kind_name(operand.ty), kind_name(to));
                self.mark(&format!("tir::ExprKind::Convert from {from} to {to}"));
                self.tir_expr(operand, Parent::Other);
            }
            tir::ExprKind::Print { stream, operand } => {
                self.mark("tir::ExprKind::Print");
                self.mark(match stream {
                    tir::Stream::Stdout => "tir::Stream::Stdout",
                    tir::Stream::Stderr => "tir::Stream::Stderr",
                });
                self.tir_expr(operand, Parent::Other);
            }
            tir::ExprKind::Panic(message) => {
                self.mark("tir::ExprKind::Panic");
                self.tir_expr(message, Parent::Other);
            }
            tir::ExprKind::ReadLine => self.mark("tir::ExprKind::ReadLine"),
            tir::ExprKind::ArrayLit(elements) => {
                self.mark("tir::ExprKind::ArrayLit");
                self.tir_exprs(elements);
            }
            tir::ExprKind::ArrayFill { value, len } => {
                self.mark("tir::ExprKind::ArrayFill");
                self.tir_expr(value, Parent::Other);
                self.tir_expr(len, Parent::Other);
            }
            tir::ExprKind::Index { base, index } => {
                self.mark("tir::ExprKind::Index");
                self.tir_expr(base, Parent::Other);
                self.tir_expr(index, Parent::Other);
            }
            tir::ExprKind::Len(base) => self.receiver_call("tir::ExprKind::Len", base),
            tir::ExprKind::Chars(base) => self.receiver_call("tir::ExprKind::Chars", base),
            tir::ExprKind::Bytes(base) => self.receiver_call("tir::ExprKind::Bytes", base),
            tir::ExprKind::Words(base) => self.receiver_call("tir::ExprKind::Words", base),
            tir::ExprKind::FromChars(base) => {
                self.receiver_call("tir::ExprKind::FromChars", base);
            }
            tir::ExprKind::Sum(base) => self.receiver_call("tir::ExprKind::Sum", base),
            tir::ExprKind::Contains { base, value } => {
                self.receiver_call("tir::ExprKind::Contains", base);
                self.tir_expr(value, Parent::Other);
            }
            tir::ExprKind::Math { op, operands } => self.math(e, *op, operands, parent),
            tir::ExprKind::Unary { op, operand } => {
                self.mark("tir::ExprKind::Unary");
                self.mark(match op {
                    ast::UnaryOp::Neg => "ast::UnaryOp::Neg",
                    ast::UnaryOp::Not => "ast::UnaryOp::Not",
                });
                self.tir_expr(operand, Parent::Other);
            }
            tir::ExprKind::Binary { op, lhs, rhs } => {
                self.mark("tir::ExprKind::Binary");
                self.mark(match op {
                    tir::BinaryOp::Add => "tir::BinaryOp::Add",
                    tir::BinaryOp::AddUnchecked => "tir::BinaryOp::AddUnchecked",
                    tir::BinaryOp::Sub => "tir::BinaryOp::Sub",
                    tir::BinaryOp::Mul => "tir::BinaryOp::Mul",
                    tir::BinaryOp::Div => "tir::BinaryOp::Div",
                    tir::BinaryOp::Rem => "tir::BinaryOp::Rem",
                    tir::BinaryOp::And => "tir::BinaryOp::And",
                    tir::BinaryOp::Or => "tir::BinaryOp::Or",
                });
                self.operands(e, lhs, rhs);
                self.tir_expr(lhs, Parent::Other);
                self.tir_expr(rhs, Parent::Other);
            }
            tir::ExprKind::Interpolate { parts, .. } => {
                self.mark("tir::ExprKind::Interpolate");
                self.tir_exprs(parts);
            }
            tir::ExprKind::Concat { lhs, rhs } => {
                self.mark("tir::ExprKind::Concat");
                self.tir_expr(lhs, Parent::Other);
                self.tir_expr(rhs, Parent::Other);
            }
            tir::ExprKind::Compare { operands, .. } => {
                self.mark("tir::ExprKind::Compare");
                self.tir_exprs(operands);
            }
            tir::ExprKind::Equal { lhs, rhs, .. } => {
                self.mark("tir::ExprKind::Equal");
                self.tir_expr(lhs, Parent::Other);
                self.tir_expr(rhs, Parent::Other);
            }
            tir::ExprKind::Is { lhs, rhs } => {
                self.mark("tir::ExprKind::Is");
                self.tir_expr(lhs, Parent::Other);
                self.tir_expr(rhs, Parent::Other);
            }
            tir::ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                self.mark("tir::ExprKind::If");
                self.tir_expr(cond, Parent::Other);
                self.tir_block(then_block);
                if let Some(block) = else_block {
                    self.tir_block(block);
                }
            }
            tir::ExprKind::IfSome {
                scrutinee,
                then_block,
                else_block,
                ..
            } => {
                self.mark("tir::ExprKind::IfSome");
                // Only a `??` starts where its scrutinee does, but for the `(`s around it: an
                // `if let` starts at `if`, and a `?.` on an optional of an optional is an error
                // (E0062).
                let before = &self.source[e.span.start()..scrutinee.span.start()];
                if before.chars().all(|c| c == '(')
                    && matches!(scrutinee.ty.kind(), TypeKind::Optional(inner) if matches!(inner.kind(), TypeKind::Optional(_)))
                {
                    self.mark("reach: `??` on an optional of an optional");
                }
                self.some_value(scrutinee);
                self.tir_expr(scrutinee, Parent::Other);
                self.tir_block(then_block);
                if let Some(block) = else_block {
                    self.tir_block(block);
                }
            }
            tir::ExprKind::Match {
                scrutinee, arms, ..
            } => {
                self.mark("tir::ExprKind::Match");
                if matches!(scrutinee.ty.kind(), TypeKind::Enum(id, _) if Some(id.0) == self.result)
                {
                    self.mark("reach: a `match` on a `Result`");
                }
                self.some_value(scrutinee);
                self.tir_expr(scrutinee, Parent::Other);
                if let TypeKind::Optional(payload) = scrutinee.ty.kind() {
                    if let TypeKind::Optional(_) = payload.kind() {
                        self.mark("reach: a `match` on an optional of an optional");
                    }
                    // An `if let` lowers to a `match` whose `_` arm has the span of its pattern.
                    let first = arms.first().map(|arm| arm.pattern.span);
                    let covers = arms.iter().enumerate().any(|(i, arm)| {
                        matches!(
                            arm.pattern.kind,
                            tir::PatternKind::Wild | tir::PatternKind::Bind(_)
                        ) && (i == 0 || Some(arm.pattern.span) != first)
                    });
                    if covers {
                        self.mark(
                            "reach: a binding or `_` covers a whole optional in a `match` arm",
                        );
                    }
                }
                for arm in arms {
                    self.tir_pattern(&arm.pattern);
                    if let Some(guard) = &arm.guard {
                        self.tir_expr(guard, Parent::Other);
                    }
                    self.tir_block(&arm.body);
                }
            }
            tir::ExprKind::Loop(body) => {
                self.mark("tir::ExprKind::Loop");
                self.tir_block(body);
            }
            tir::ExprKind::Break { value, .. } => {
                self.mark("tir::ExprKind::Break");
                if let Some(value) = value {
                    self.tir_expr(value, Parent::Other);
                }
            }
            tir::ExprKind::Continue { .. } => self.mark("tir::ExprKind::Continue"),
            tir::ExprKind::Return(value) => {
                self.mark("tir::ExprKind::Return");
                if let Some(value) = value {
                    self.tir_expr(value, Parent::Other);
                }
            }
            tir::ExprKind::Block(block) => {
                self.mark("tir::ExprKind::Block");
                self.tir_block(block);
            }
        }
    }

    fn receiver_call(&mut self, variant: &str, base: &tir::Expr) {
        self.mark(variant);
        self.receiver(variant, base);
        self.tir_expr(base, Parent::Other);
    }

    /// A `Math` node, and decision 11: a libm result goes only into another libm method, a
    /// classification or a discarded `let _`, and a classification of one only into a `let _`.
    fn math(&mut self, e: &tir::Expr, op: tir::MathOp, operands: &[tir::Expr], parent: Parent) {
        self.mark("tir::ExprKind::Math");
        let name = format!("tir::MathOp::{op:?}");
        self.mark(&name);
        self.receiver(&name, &operands[0]);
        let own = kind(op);
        let leaks = match own {
            Parent::Libm => !matches!(parent, Parent::Libm | Parent::Classify | Parent::Discard),
            Parent::Classify => inexact(&operands[0]) && parent != Parent::Discard,
            Parent::Discard | Parent::Other => false,
        };
        if leaks {
            self.leaks
                .push(format!("{op:?} at byte {}", e.span.start()));
        }
        for operand in operands {
            self.tir_expr(operand, own);
        }
    }

    fn tir_pattern(&mut self, pattern: &tir::Pattern) {
        match &pattern.kind {
            tir::PatternKind::Wild => self.mark("tir::PatternKind::Wild"),
            tir::PatternKind::Bind(_) => self.mark("tir::PatternKind::Bind"),
            tir::PatternKind::Variant { fields, .. } => {
                self.mark("tir::PatternKind::Variant");
                for field in fields {
                    self.tir_pattern(field);
                }
            }
            tir::PatternKind::Bool(_) => self.mark("tir::PatternKind::Bool"),
            tir::PatternKind::Range { .. } => self.mark("tir::PatternKind::Range"),
            tir::PatternKind::Str(_) => self.mark("tir::PatternKind::Str"),
            tir::PatternKind::Or(alternatives) => {
                self.mark("tir::PatternKind::Or");
                for alternative in alternatives {
                    self.tir_pattern(alternative);
                }
            }
            tir::PatternKind::Some(inner) => {
                self.mark("tir::PatternKind::Some");
                // The checker gives the `Some` it writes out the span of the pattern inside.
                if inner.span == pattern.span {
                    self.mark(
                        "reach: a variant, literal or range pattern stands at an optional position",
                    );
                }
                self.tir_pattern(inner);
            }
            tir::PatternKind::None => self.mark("tir::PatternKind::None"),
        }
    }
}

/// A type's kind, as `RECEIVERS` and `CONVERSIONS` write it.
fn kind_name(ty: Type) -> &'static str {
    match ty.kind() {
        TypeKind::I64 => "I64",
        TypeKind::U8 => "U8",
        TypeKind::F64 => "F64",
        TypeKind::Bool => "Bool",
        TypeKind::Char => "Char",
        TypeKind::String => "String",
        TypeKind::Unit => "Unit",
        TypeKind::Never => "Never",
        TypeKind::Class(..) => "Class",
        TypeKind::Enum(..) => "Enum",
        TypeKind::Param(..) => "Param",
        TypeKind::Array(_) => "Array",
        TypeKind::Optional(_) => "Optional",
    }
}

/// The value of an integer literal, negated or not.
fn int(e: &ast::Expr) -> Option<i128> {
    if let ast::ExprKind::Int { value, .. } = &e.kind {
        Some(i128::from(*value))
    } else if let ast::ExprKind::Unary {
        op: ast::UnaryOp::Neg,
        operand,
    } = &e.kind
    {
        int(operand).map(|n| -n)
    } else {
        None
    }
}

/// The length of an array literal, or of a fill of a literal length.
fn array_len(e: &ast::Expr) -> Option<i128> {
    if let ast::ExprKind::ArrayLit(elements) = &e.kind {
        i128::try_from(elements.len()).ok()
    } else if let ast::ExprKind::ArrayFill { len, .. } = &e.kind {
        int(len)
    } else if let ast::ExprKind::Block(block) = &e.kind {
        // A block's value is its last statement.
        array_len(tail(block)?)
    } else {
        None
    }
}

/// The expression `block` holds, if it holds only that.
fn alone(block: &ast::Block) -> Option<&ast::Expr> {
    tail(block).filter(|_| block.stmts.len() == 1)
}

/// The expression `block` ends with, if it ends with one.
fn tail(block: &ast::Block) -> Option<&ast::Expr> {
    if let ast::StmtKind::Expr(e) = &block.stmts.last()?.kind {
        Some(e)
    } else {
        None
    }
}

/// The names of the items `pick` selects, with their type parameters, that have any.
fn generic_names<'a>(
    program: &'a ast::Program,
    pick: impl Fn(&'a ast::Item) -> Option<(&'a ast::Ident, &'a Vec<ast::Ident>)>,
) -> FxHashSet<String> {
    program
        .items
        .iter()
        .filter_map(pick)
        .filter(|(_, params)| !params.is_empty())
        .map(|(name, _)| name.name.as_str().to_string())
        .collect()
}

/// The enum a variant is built from, `E` of `E.V` and of `E<A>.V`.
fn enum_name(receiver: &ast::Expr) -> Option<&str> {
    match &receiver.kind {
        ast::ExprKind::Applied { name, .. } => Some(name.name.as_str()),
        _ => name(receiver),
    }
}

/// The name `e` is, if it is one.
fn name(e: &ast::Expr) -> Option<&str> {
    if let ast::ExprKind::Name(name) = &e.kind {
        Some(name.as_str())
    } else {
        None
    }
}

/// The `n` of `let mut c = n`, if `before` is that, of the counter `counted`.
fn counter(before: Option<&ast::Stmt>, counted: Option<&str>) -> Option<i128> {
    if let Some(ast::StmtKind::Let {
        mutable: Some(_),
        name: let_name,
        init,
        ..
    }) = before.map(|stmt| &stmt.kind)
        && Some(let_name.name.as_str()) == counted
    {
        int(init)
    } else {
        None
    }
}

/// Whether `stmt` is `c -= 1` of the counter `counted`.
fn steps(stmt: Option<&ast::Stmt>, counted: Option<&str>) -> bool {
    stmt.is_some_and(|stmt| {
        matches!(&stmt.kind, ast::StmtKind::CompoundAssign {
            op: ast::BinaryOp::Sub,
            target,
            value,
            ..
        } if counted.is_some() && name(target) == counted && int(value) == Some(1))
    })
}

/// Whether `stmt` never finishes: a jump, or an `if` whose every branch ends in one.
fn jumps(stmt: &ast::Stmt) -> bool {
    let ends = |block: &ast::Block| block.stmts.last().is_some_and(jumps);
    if matches!(
        stmt.kind,
        ast::StmtKind::Return(_) | ast::StmtKind::Break { .. } | ast::StmtKind::Continue { .. }
    ) {
        return true;
    }
    let ast::StmtKind::Expr(e) = &stmt.kind else {
        return false;
    };
    if let ast::ExprKind::If {
        then_block,
        else_block,
        ..
    } = &e.kind
    {
        ends(then_block) && else_block.as_ref().is_some_and(ends)
    } else {
        matches!(&e.kind, ast::ExprKind::Todo { .. })
            || matches!(&e.kind, ast::ExprKind::Call { callee, .. } if callee.name.as_str() == "panic")
    }
}

/// Where a typed expression's value goes, as decision 11 sees it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Parent {
    /// A discarded statement, `let _ = e`.
    Discard,
    /// An operand of a libm method.
    Libm,
    /// The operand of `is_nan`, `is_finite` or `is_infinite`.
    Classify,
    Other,
}

/// Whether `text`, an operand's source, is a literal: a number, negative or not, in parentheses
/// or not, or a `u8` written `(u8(n) ?? n)`.
fn literal(text: &str) -> bool {
    let number =
        |t: &str| t.starts_with(|c: char| c.is_ascii_digit()) && !t.contains([' ', '(', ')']);
    let text = text.trim_start_matches('(').trim_end_matches(')');
    let text = text.strip_prefix('-').unwrap_or(text);
    number(text)
        || text
            .strip_prefix("u8(")
            .and_then(|t| t.split_once(") ?? "))
            .is_some_and(|(a, b)| number(a) && number(b))
}

/// What a method's operands are to it.
fn kind(op: tir::MathOp) -> Parent {
    match op {
        tir::MathOp::Sin
        | tir::MathOp::Cos
        | tir::MathOp::Tan
        | tir::MathOp::Asin
        | tir::MathOp::Acos
        | tir::MathOp::Atan
        | tir::MathOp::Atan2
        | tir::MathOp::Exp
        | tir::MathOp::Ln
        | tir::MathOp::Log10
        | tir::MathOp::Log2
        | tir::MathOp::Pow
        | tir::MathOp::Hypot => Parent::Libm,
        tir::MathOp::IsNan | tir::MathOp::IsFinite | tir::MathOp::IsInfinite => Parent::Classify,
        tir::MathOp::Abs
        | tir::MathOp::Sqrt
        | tir::MathOp::Floor
        | tir::MathOp::Ceil
        | tir::MathOp::Trunc
        | tir::MathOp::Round
        | tir::MathOp::Min
        | tir::MathOp::Max
        | tir::MathOp::Clamp => Parent::Other,
    }
}

/// Whether `e` is a libm result.
fn inexact(e: &tir::Expr) -> bool {
    matches!(&e.kind, tir::ExprKind::Math { op, .. } if kind(*op) == Parent::Libm)
}

/// What the programs of every seed hold.
fn walk() -> Walk {
    let mut walk = Walk::default();
    for data in seeds() {
        walk.program(&generate(&data).plain);
    }
    walk
}

#[test]
fn every_generated_program_checks_clean() {
    for data in seeds() {
        let (_, program) = checked(&generate(&data).program);
        // In a debug build, `mono` asserts that no type parameter is left in what it gives.
        crate::mono::instantiate(program);
    }
}

#[test]
#[should_panic(expected = "W0010 at 1:4")]
fn checked_should_reject_a_program_that_calls_itself_on_every_path() {
    checked("fn f() {\n    f()\n}\nfn main() {}\n");
}

#[test]
#[should_panic(expected = "W0011 at 2:11")]
fn checked_should_reject_a_loop_whose_condition_never_changes() {
    checked("fn f(n: i64) {\n    while n > 0 { print(n) }\n}\nfn main() {}\n");
}

#[test]
fn every_short_input_checks_clean() {
    checked(&generate(&[]).program);
    for b in 0..=u8::MAX {
        checked(&generate(&[b]).program);
    }
}

#[test]
fn read_line_past_the_input_checks_clean() {
    // A program that calls `read_line` in more places than its stdin has lines can read past the
    // end, where `read_line` gives `None` (Review Focus 4), here one that keeps the optional
    // rather than replacing a `None` with `??` at once. Some seed writes one, and it checks.
    let past = seeds().map(|data| generate(&data)).find(|g| {
        let calls = g.program.matches("read_line()").count();
        calls > g.stdin.lines().count() && calls > g.program.matches("read_line() ??").count()
    });
    checked(
        &past
            .expect("some seed's program should read past its stdin")
            .program,
    );
}

#[test]
fn every_variant_is_generated() {
    let universe = universe();
    let named: Vec<&str> = EXCLUDED.iter().map(|(key, _)| *key).collect();
    let unknown: Vec<&&str> = named
        .iter()
        .filter(|key| !universe.contains(**key))
        .collect();
    assert!(
        unknown.is_empty(),
        "EXCLUDED names what does not exist: {unknown:?}"
    );
    let seen = walk().seen;
    let mut missing: Vec<&String> = universe
        .iter()
        .filter(|key| !seen.contains(*key) && !named.contains(&key.as_str()))
        .collect();
    missing.sort();
    assert!(
        missing.is_empty(),
        "no generated program holds: {missing:#?}"
    );
}

#[test]
fn main_holds_several_statements() {
    // Each top-level statement of `main` takes a share of its fuel, so the first cannot spend
    // it all, and `main` seldom stops before its first.
    let mut counts: Vec<usize> = seeds()
        .map(|data| {
            // The known sites (decision 3) are statements the generator adds at fixed places.
            let (program, _) = checked(&generate(&data).plain);
            program
                .items
                .into_iter()
                .find_map(|item| match item {
                    ast::Item::Function(f) if f.name.name.as_str() == "main" => {
                        Some(f.body.stmts.len())
                    }
                    ast::Item::Function(_)
                    | ast::Item::Class(_)
                    | ast::Item::Enum(_)
                    | ast::Item::Const(_) => None,
                })
                .expect("a program should have `main`")
        })
        .collect();
    counts.sort_unstable();
    let empty = counts.iter().filter(|&&n| n == 0).count();
    assert!(empty <= 2, "{empty} empty mains: {counts:?}");
    assert!(counts[counts.len() / 2] >= 4, "{counts:?}");
}

#[test]
fn a_generated_panic_is_rare() {
    // A panic ends the run, so a block ends in one only now and then where another jump can,
    // and seldom where none can; and an `if` expression's branch after the first, seldom. The
    // `return` of each function's depth guard is not a choice, so it is not counted.
    // A known panic site ends the run too, so it is kept in one seed in eight at most.
    let (mut jumps, mut panics, mut ifs, mut panicking, mut sites) = (0, 0, 0, 0, 0);
    for data in seeds() {
        let g = generate(&data);
        sites += g
            .known
            .iter()
            .filter(|known| matches!(known.answer, Answer::Panic(_)))
            .count();
        let mut guard = false;
        for line in g.program.lines().map(str::trim) {
            if line.starts_with("panic(") || line.starts_with("todo") {
                panics += 1;
            } else if line.starts_with("return") && !guard
                || line.starts_with("break")
                || line.starts_with("continue")
            {
                jumps += 1;
            }
            guard = line == "if depth <= 0 {";
        }
        ifs += g.program.matches("(if ").count();
        panicking += g.program.matches("{ panic(").count() + g.program.matches("{ todo").count();
    }
    assert!(
        jumps > 0 && ifs > 0,
        "the seeds should hold jumps and `if`s"
    );
    assert!(
        panics * 16 <= jumps,
        "{panics} panics and {jumps} other jumps end blocks"
    );
    assert!(
        panicking * 32 <= ifs,
        "{panicking} `if` branches panic in {ifs} `if`s"
    );
    assert!(
        sites * 8 <= seeds().count(),
        "{sites} known panic sites in {SEEDS} seeds"
    );
}

#[test]
fn generation_is_deterministic() {
    for data in seeds() {
        let (first, second) = (generate(&data), generate(&data));
        assert_eq!(first.program, second.program);
        assert_eq!(first.rewritten, second.rewritten);
        assert_eq!(first.stdin, second.stdin);
    }
}

#[test]
fn no_inexact_value_reaches_output() {
    let leaks = walk().leaks;
    assert!(leaks.is_empty(), "an inexact value escapes: {leaks:?}");
}

#[test]
fn generated_programs_stay_in_budget() {
    for data in seeds() {
        let g = generate(&data);
        assert!(
            g.cost <= BUDGET,
            "cost {} over budget:\n{}",
            g.cost,
            g.program
        );
    }
}

#[test]
fn cost_multiplies_nested_loops() {
    // Loops of 8, 5 and 2 runs around 3 statements: 8 * 5 * 2 * 3.
    let mut cost = Cost::default();
    for bound in [8, 5, 2] {
        cost.enter_loop(bound);
    }
    assert_eq!(cost.depth(), 3);
    for _ in 0..3 {
        cost.statement();
    }
    for _ in 0..3 {
        cost.exit_loop();
    }
    assert_eq!(cost.of_main(&[]), 240);
    // A statement after the loops runs once.
    assert_eq!(cost.depth(), 0);
    cost.statement();
    assert_eq!(cost.of_main(&[]), 241);
}

#[test]
fn cost_counts_call_fanout() {
    // `f` runs one statement and calls itself twice at `depth - 1`; at depth 0 it returns at
    // once, for 1. At depth d it costs its check, its statement and twice f at d - 1.
    let mut body = Cost::default();
    body.statement();
    body.call(0, None);
    body.call(0, None);
    // `f` is function 0, whose cost is not set yet.
    let f = body.of_function(&[[1; DEPTHS]], 0);
    assert_eq!(f, [1, 4, 10, 22, 46]);
    let mut main = Cost::default();
    main.call(0, Some(4));
    assert_eq!(main.of_main(&[f]), 46);
}

#[test]
fn cost_counts_calls_in_loops() {
    // Eight runs of a call costing 17, then one statement: 8 * 17 + 1.
    let mut main = Cost::default();
    main.enter_loop(8);
    main.call(0, Some(1));
    main.exit_loop();
    main.statement();
    assert_eq!(main.of_main(&[[1, 17, 17, 17, 17]]), 137);
}

#[test]
fn a_while_condition_runs_once_more_than_its_body() {
    // A statement in the condition of a `while` of 8 runs 9 times, one in its body 8 times.
    let mut cost = Cost::default();
    cost.enter_condition(8);
    cost.statement();
    cost.exit_loop();
    cost.enter_loop(8);
    cost.statement();
    cost.exit_loop();
    assert_eq!(cost.of_main(&[]), 17);
}

#[test]
fn the_costliest_statement_fits_the_reserve() {
    // A statement in the conditions of three nested `while`s of 8, each in a value block of the
    // condition around it, runs 9 * 9 * 9 times: the most any statement can, which
    // `Cost::statement` asserts the reserve for `main`'s statements covers.
    let mut cost = Cost::default();
    for _ in 0..3 {
        cost.enter_condition(MAX_BOUND);
    }
    cost.statement();
    assert_eq!(cost.of_main(&[]), 729);
}

#[test]
fn main_calls_fit_the_budget_left() {
    // `f1` costs 300,000 at depth 2, 354,201 at depth 3 and 100,000 at depth 1. Called twice in
    // a loop of 2, out of the 1,000,000 budget less 400 statements of 729 runs each: 708,400.
    // The first call fits depths 0 to 2 (2 * 354,201 is 708,402) and takes depth 2, leaving
    // 108,400; the second fits only depth 0 (2 * 100,000 is over).
    let data = [2, 1];
    let mut g = Gen::new(&data);
    g.scope.funcs.push(Func {
        name: "f1".to_string(),
        owner: None,
        params: Vec::new(),
        ret: Ty::Unit,
        ret_size: 0,
        cost: [1, 100_000, 300_000, 354_201, 1_000_000],
    });
    g.cost.enter_loop(2);
    g.call(&Ty::Unit, 0);
    g.call(&Ty::Unit, 0);
    assert_eq!(g.w.out.program, "f1(2)f1(0)");
}

/// A `Gen` over `data` in `main`, knowing `f1`, `f2` and `f3`: of an `i64`, of `unit` and of
/// an `[i64]`, each a call of cost 1 at every depth.
fn with_funcs(data: &[u8]) -> Gen<'_> {
    let mut g = Gen::new(data);
    for (name, ret) in [
        ("f1", Ty::I64),
        ("f2", Ty::Unit),
        ("f3", Ty::Array(Box::new(Ty::I64))),
    ] {
        g.scope.funcs.push(Func {
            name: name.to_string(),
            owner: None,
            params: Vec::new(),
            ret,
            ret_size: 0,
            cost: [1; DEPTHS],
        });
    }
    g
}

#[test]
fn a_return_gives_a_value_where_its_function_returns_one() {
    // Byte by byte: no statement, then the block's jump, a jump not an `if`, and of the jumps
    // a function's block has, `return`: in `f1`, of an `i64`, with a value; in `f2`, of
    // `unit`, without.
    for (func, text) in [(0, "\nreturn 0"), (1, "\nreturn")] {
        let mut g = with_funcs(&[0, 3, 0, 0]);
        g.scope.func = Some(func);
        g.jumps.ret = true;
        g.stmts(true);
        assert_eq!(g.w.out.program, text);
    }
}

#[test]
fn main_prints_or_discards_what_a_call_returns() {
    // Byte by byte: one statement in `main`, a call, of the function the third byte names, at
    // depth 0 once the bytes run out. A value is printed, an array discarded, and a `unit`
    // call stands alone; the statement gives back the fuel it did not spend.
    for (callee, line) in [(0, "print(f1(0))"), (1, "f2(0)"), (2, "let _ = f3(0)")] {
        let data = [1, 1, callee];
        let mut g = with_funcs(&data);
        g.main_stmts();
        assert_eq!(g.w.out.program, format!("\n{line}"));
        assert_eq!(g.fuel, FUEL - 1);
    }
}

#[test]
fn a_call_statement_names_any_function_the_budget_lets_it() {
    // In `main`, every function, of any type, while the loops around the call run no more
    // times than the budget left, so that a call at depth 0 fits; in a function, itself and
    // the earlier ones, whatever the loops.
    let mut g = with_funcs(&[]);
    g.budget = 8;
    g.cost.enter_loop(8);
    assert_eq!(g.callees_any(), 3);
    g.cost.enter_loop(2);
    assert_eq!(g.callees_any(), 0);
    g.scope.func = Some(1);
    assert_eq!(g.callees_any(), 2);
}

#[test]
fn fuel_runs_out() {
    let mut g = Gen::new(&[]);
    let spent = (0..=FUEL).filter(|_| g.spend(1)).count();
    assert_eq!(u64::try_from(spent).expect("the count should fit"), FUEL);
}

/// A `Gen` over `data` in `main`, holding `let mut s0: string` with `base` and `room`, inside
/// a loop of `bound` runs if any.
fn with_string(data: &[u8], base: u64, room: u64, bound: Option<u64>) -> Gen<'_> {
    let mut g = Gen::new(data);
    let var = Var::new("s0".to_string(), Ty::String, true, base, room, 0);
    g.scope.vars.push(var);
    if let Some(bound) = bound {
        g.cost.enter_loop(bound);
    }
    g
}

#[test]
fn appending_keeps_a_strings_most_size() {
    // Room for 500 chars: a `+=` of 300 leaves room for 200, and the string can hold 500.
    let mut var = Var::new("s0".to_string(), Ty::String, true, 0, 500, 0);
    var.append(300, 1);
    assert_eq!((var.size, var.room), (500, 200));
    // In a loop of 8, each run may add an eighth of that room; a run of 25 takes it all.
    assert_eq!(var.append_max(8), 25);
    var.append(25, 8);
    assert_eq!((var.size, var.room), (500, 0));
}

#[test]
fn a_string_appends_itself_only_within_its_room() {
    // `s0 += s0` adds all `s0` holds: with room 500 and nothing held yet, once fits ...
    let appended = |bound| {
        (0..=u8::MAX).any(|b| {
            let data = [b; 16];
            let mut g = with_string(&data, 0, 500, bound);
            g.compound();
            g.w.out.program.starts_with("s0 += s0\n") || g.w.out.program == "s0 += s0"
        })
    };
    assert!(appended(None));
    // ... and in a loop of 8 it does not: each run may add only 62.
    assert!(!appended(Some(8)));
}

#[test]
fn a_string_at_half_the_cap_does_not_double() {
    // `let mut s0 = <500 chars>` with room 500 may reach 1,000 chars: `s0 += s0` could reach
    // 2,000 and `s0 + s0` 4,000.
    for b in 0..=u8::MAX {
        let data = [b; 64];
        let mut g = with_string(&data, 500, 500, None);
        g.compound();
        g.w.line();
        g.expr(&Ty::String, MAX_SIZE);
        let text = &g.w.out.program;
        assert!(
            !text.contains("s0 += s0") && !text.contains("(s0 + s0"),
            "{text}"
        );
    }
}

#[test]
fn a_constant_string_stays_within_its_most() {
    // `c0` holds 600 chars, so `(c0 + c0)` would hold 1,200, over the 1,000 of a constant: the
    // right operand, chosen as a name where one fits, is not `c0`.
    let data = [2, 1, 1];
    let mut g = Gen::new(&data);
    g.scope.consts.push(Const {
        name: "c0".to_string(),
        ty: Ty::String,
        value: 0,
        size: 600,
    });
    g.const_expr(&Ty::String, MAX_SIZE);
    assert!(!g.w.out.program.contains("c0 + c0"), "{}", g.w.out.program);
}

#[test]
fn printed_sizes_are_the_longest_values() {
    let chars = |text: &str| u64::try_from(text.chars().count()).expect("the count should fit");
    assert_eq!(Ty::I64.printed(0, None), chars(&i64::MIN.to_string()));
    assert_eq!(Ty::U8.printed(0, None), chars("255"));
    assert_eq!(Ty::Bool.printed(0, None), chars("false"));
    assert_eq!(Ty::Char.printed(0, None), 1);
    assert_eq!(Ty::String.printed(7, None), 7);
    // `print` writes an `f64` as Rust's `{:?}`, and with a precision as `{:.p$}`
    // (`crates/runtime/src/float.rs`).
    assert_eq!(
        Ty::F64.printed(0, None),
        chars(&format!("{:?}", -2.225_073_858_507_201_4e-308))
    );
    // With no decimals, the point goes too: one char under the bound.
    assert_eq!(
        Ty::F64.printed(0, Some(0)),
        chars(&format!("{:.0}", -f64::MAX)) + 1
    );
    for p in 1..=6 {
        let longest = format!(
            "{:.p$}",
            -f64::MAX,
            p = usize::try_from(p).expect("a precision should fit a `usize`")
        );
        assert_eq!(Ty::F64.printed(0, Some(p)), chars(&longest));
    }
}

#[test]
fn a_part_whose_spec_does_not_fit_is_written_without_it() {
    // One `f64` part, `0.0`, with 6 decimals: up to 317 chars, over a room of 30. The part is
    // written without its spec and counts the 24 chars of the longest `f64`.
    let data = [0, 0, 2, 0, 0, 0, 1, 6];
    let mut g = Gen::new(&data);
    let info = g.interpolate(30);
    assert_eq!((g.w.out.program.as_str(), info.size), ("\"{0.0}\"", 24));
}

#[test]
fn an_int_literal_is_worth_what_it_says() {
    // The folder reads the value, the compiler the text: if they differ, a const the folder
    // takes for safe can overflow (E0088). Every first byte picks a form; the rest, its value.
    assert_eq!(
        Gen::new(&[]).int_literal(),
        ("0".to_string(), 0),
        "the smallest choice"
    );
    for form in 0..=u8::MAX {
        for rest in [&[][..], &[1], &[7, 200, 3, 99, 250, 18, 77, 5]] {
            let data = [&[form][..], rest].concat();
            let (text, value) = Gen::new(&data).int_literal();
            let digits = text.trim_matches(['(', ')']).replace('_', "");
            let (negative, digits) = match digits.strip_prefix('-') {
                Some(digits) => (true, digits),
                None => (false, digits.as_str()),
            };
            let (radix, digits) = match digits.get(..2) {
                Some("0x") => (16, &digits[2..]),
                Some("0b") => (2, &digits[2..]),
                Some("0o") => (8, &digits[2..]),
                _ => (10, digits),
            };
            let magnitude = i128::from_str_radix(digits, radix).expect("a literal should parse");
            let said = if negative { -magnitude } else { magnitude };
            assert_eq!(said, i128::from(value), "{text} on {data:?}");
        }
    }
}

#[test]
fn rare_constant_forms_check_clean() {
    // Each input chooses, byte by byte, no class or enum, no generic type, one instance of
    // `Result` over two `i64`s, no generic function, an item, a constant, its type, no
    // annotation, and then its form, ending once the bytes run out, where every choice is the
    // first.
    for (data, form) in [
        // `-(e)` of the smallest `i64`, which has no negation: `e` alone.
        (
            &[0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 63][..],
            "const c1 = (-9223372036854775808)\n",
        ),
        // `a - b` of the smallest `i64` and a negative literal, which fits where `a + b` would
        // overflow: the fold must know the literal's sign.
        (
            &[0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 3, 0, 63, 0, 6, 0, 0, 0][..],
            "const c1 = ((-9223372036854775808) - (-1))\n",
        ),
        // `(a && b)` and `(a || b)` of `bool`s.
        (
            &[0, 0, 0, 0, 0, 0, 1, 0, 3, 0, 3, 1][..],
            "const c1 = (false && false)\n",
        ),
        (
            &[0, 0, 0, 0, 0, 0, 1, 0, 3, 0, 3, 1, 0, 0, 1][..],
            "const c1 = (false || false)\n",
        ),
    ] {
        let g = generate(data);
        assert!(g.program.starts_with(form), "{}", g.program);
        checked(&g.program);
    }
}

#[test]
fn no_generated_program_assigns_a_class_into_the_heap() {
    // Decision 4: an object stored in a field or an element could make a cycle.
    let stores = walk().heap_classes;
    assert!(
        stores.is_empty(),
        "a class object goes into the heap: {stores:?}"
    );
}

#[test]
fn every_loop_is_bounded() {
    let unbounded = walk().unbounded;
    assert!(unbounded.is_empty(), "loops not bounded: {unbounded:?}");
}

#[test]
fn every_index_is_in_bounds_or_meant_to_panic() {
    let out_of_bounds = walk().out_of_bounds;
    assert!(out_of_bounds.is_empty(), "{out_of_bounds:?}");
}

/// What the walker marks for `source`, which must check.
fn marked(source: &str) -> FxHashSet<String> {
    let mut walk = Walk::default();
    walk.program(source);
    walk.seen
}

#[test]
fn the_walker_should_mark_a_generic_call_by_how_it_names_its_type_arguments() {
    let program = |body: &str| {
        format!(
            "fn id<T>(x: T) -> T {{
    x
}}
fn again<T>(x: T) -> T {{
    {body}
}}
fn main() {{
    print(id(1))
}}
"
        )
    };
    let key =
        |kind: &str| format!("reach: a generic function is called with {kind} type arguments");
    // `main` infers, and a generic function that calls `id` is one that calls a generic one.
    let seen = marked(&program("id(x)"));
    assert!(seen.contains(&key("inferred")));
    assert!(!seen.contains(&key("written")));
    assert!(seen.contains("reach: a generic function calls a generic function"));
    let seen = marked(&program("id<T>(x)"));
    assert!(seen.contains(&key("written")));
    // A call from `main` is no call from a generic function.
    let seen = marked(&program("x"));
    assert!(!seen.contains("reach: a generic function calls a generic function"));
}

#[test]
fn the_walker_should_mark_an_inferred_class_and_variant_apart_from_a_written_one() {
    let program = |class: &str, variant: &str| {
        format!(
            "class B<T> {{
    v: T
}}
enum E<T> {{
    Z
    C(T)
}}
fn main() {{
    let _ = {class}
    let _ = {variant}
}}
"
        )
    };
    let keys = [
        "reach: a generic class is built with written type arguments",
        "reach: a generic class is built with inferred type arguments",
        "reach: a generic variant is built with inferred type arguments",
    ];
    let marks =
        |class: &str, variant: &str| keys.map(|k| marked(&program(class, variant)).contains(k));
    assert_eq!(marks("B(v: 1)", "E.C(1)"), [false, true, true]);
    assert_eq!(marks("B<i64>(v: 1)", "E<i64>.C(1)"), [true, false, false]);
}

#[test]
fn the_walker_should_mark_a_result_built_by_bare_names_and_matched() {
    let program = |build: &str| {
        format!(
            "fn main() {{
    let r: Result<i64, string> = {build}
    match r {{
        Ok(n) => print(n)
        Err(e) => print(e)
    }}
}}
"
        )
    };
    let keys = [
        "reach: `Ok` builds a `Result`",
        "reach: `Err` builds a `Result`",
        "reach: a `match` on a `Result`",
    ];
    let marks = |build: &str| keys.map(|k| marked(&program(build)).contains(k));
    assert_eq!(marks("Ok(1)"), [true, false, true]);
    assert_eq!(marks("Err(\"x\")"), [false, true, true]);
    // The qualified spelling is no bare name.
    assert_eq!(marks("Result.Ok(1)"), [false, false, true]);
}

#[test]
fn the_walker_should_mark_a_match_in_a_generic_function_only() {
    let key = "reach: a generic function matches a generic enum";
    let program = |generic: &str| {
        format!(
            "enum E<T> {{
    Z
    C(T)
}}
fn unwrap{generic}(e: E<i64>, d: i64) -> i64 {{
    match e {{
        E.C(p) => p
        _ => d
    }}
}}
fn main() {{}}
"
        )
    };
    assert!(!marked(&program("")).contains(key));
    // `unwrap<T>` has a type parameter, though `E<i64>` does not name it.
    assert!(marked(&program("<T>")).contains(key));
}

#[test]
fn the_walker_should_mark_a_type_argument_that_is_itself_applied() {
    let key = "reach: a generic type is a type argument";
    let program = |ty: &str| {
        format!(
            "class B<T> {{
    v: T
}}
fn take(b: {ty}) {{}}
fn main() {{}}
"
        )
    };
    assert!(marked(&program("B<B<i64>>")).contains(key));
    assert!(!marked(&program("B<i64>")).contains(key));
}

#[test]
fn the_walker_should_mark_the_rank_of_each_payload_field() {
    let seen = marked(
        "enum Two<T> {
    Both(T, T)
}
enum Expr {
    Lit(i64)
    Pair(Two<Expr>)
}
enum Tree<T> {
    Leaf
    Node(Tree<T>, T, Tree<T>)
}
fn main() {}
",
    );
    for rank in ["Plain", "Own", "Never"] {
        assert!(seen.contains(&format!("tir::Rank::{rank}")), "{rank}");
    }
    for rank in ["Open", "Late"] {
        assert!(!seen.contains(&format!("tir::Rank::{rank}")), "{rank}");
    }
}

/// An enum is `Option`-shaped in either order of its variants. It is `Option`-shaped only when
/// its field's storage has a spare value: a `Link?` box and an `i64` have none.
#[test]
fn the_walker_should_mark_an_option_shaped_enum_by_its_niche() {
    let shaped = "enum Rev {\n    Just(char)\n    Nothing\n}\nenum Chain {\n    End\n    Next(Chain)\n}\nfn main() {}\n";
    let tagged = "enum Link {\n    End\n    Next(Link?)\n}\nenum Wide {\n    A\n    B(i64)\n}\nenum Two {\n    A(char)\n    B(char)\n}\nfn main() {}\n";
    let keys = [
        "reach: an `Option`-shaped enum has a niche",
        "reach: an `Option`-shaped enum holds itself",
    ];
    let (seen, not_seen) = (marked(shaped), marked(tagged));
    for key in keys {
        assert!(seen.contains(key), "{key}");
        assert!(!not_seen.contains(key), "{key}");
    }
}

#[test]
fn the_walker_should_see_a_type_parameter_before_mono_makes_copies() {
    let seen = marked("fn id<T>(x: T) -> T {\n    x\n}\nfn main() {}\n");
    assert!(seen.contains("types::TypeKind::Param"));
    assert!(!marked("fn main() {}\n").contains("types::TypeKind::Param"));
}

#[test]
fn the_walker_should_read_the_arguments_of_a_generic_enum_for_a_class() {
    // `W<i64>` holds none, though `W`'s payload names a parameter, and `W<K>` holds one.
    let stores = |arg: &str, value: &str| {
        let mut walk = Walk::default();
        walk.program(&format!(
            "class K {{
    x: i64
}}
enum W<T> {{
    A(T)
    B
}}
fn main() {{
    let mut a: [W<{arg}>] = [W.B]
    a[0] = {value}
}}
"
        ));
        walk.heap_classes.len()
    };
    assert_eq!(stores("i64", "W<i64>.A(1)"), 0);
    assert_eq!(stores("K", "W<K>.A(K(x: 1))"), 1);
}

#[test]
fn the_walker_marks_both_readings_of_break_x_minus_1_by_spelling() {
    // `break l -1` and `break v -1` are the two readings; with parentheses, neither is.
    let marks = |breaks: [&str; 2]| {
        let mut walk = Walk::default();
        walk.program(&format!(
            "fn main() {{
    let v = 2
    let mut c = 1
    let _ = l: loop {{
        if c == 0 {{ {} }}
        c -= 1
    }}
    let mut d = 1
    let _ = loop {{
        if d == 0 {{ {} }}
        d -= 1
    }}
}}
",
            breaks[0], breaks[1]
        ));
        [
            "reach: `break l -1` is a label and `-1`",
            "reach: `break x -1` is `x - 1`",
        ]
        .map(|key| walk.seen.contains(key))
    };
    assert_eq!(marks(["break l -1", "break v -1"]), [true, true]);
    assert_eq!(marks(["break l (-1)", "break (v - 1)"]), [false, false]);
    assert_eq!(marks(["break l -2", "break v -2"]), [false, false]);
}

#[test]
fn the_walker_reports_unbounded_loops_and_stray_indexes() {
    // Indexes past the end, `[e][1]` among them where it is not a later branch of an `if`, and
    // a `while`, a `while let`, a `for` and a `loop` of 9 runs or no bound at all, and `loop`s
    // with no counter or a guard on no counter: the walker lists each.
    // `b[1]` is in bounds through the block, and `[1][1]` in a later branch is meant to panic.
    let program = "fn main() {
    let a = [1, 2]
    let _ = a[2]
    let _ = [1, 2][2]
    let _ = [1][1]
    let b: [i64] = ({
        [1, 2]
    })
    let _ = b[1]
    let _ = (if true { [1][1] } else { [1][1] })
    let _ = (if true { 0 } else if false { [1][1] } else { [1][1] })
    let mut c = 9
    while c > 0 {
        c -= 1
    }
    while let v = read_line() {
        print(v)
    }
    for v in [1, 2, 3, 4, 5, 6, 7, 8, 9] {
        print(v)
    }
    let mut d = 9
    loop {
        if d == 0 { break }
        d -= 1
    }
    loop {
        break
    }
    let mut e = 1
    loop {
        if e + 1 == 0 { break }
        e -= 1
    }
}
";
    let mut walk = Walk::default();
    walk.program(program);
    let listed: Vec<&str> = walk
        .out_of_bounds
        .iter()
        .filter_map(|index| {
            let at: usize = index.rsplit_once(' ')?.1.parse().ok()?;
            Some(program[at..].split_once('\n')?.0)
        })
        .collect();
    assert_eq!(
        listed,
        ["a[2]", "[1, 2][2]", "[1][1]", "[1][1] } else { [1][1] })"],
        "{:?}",
        walk.out_of_bounds
    );
    assert!(walk.seen.contains("reach: an index is meant to panic"));
    assert!(walk.seen.contains("reach: an index reads a local"));
    let unbounded: Vec<&str> = walk
        .unbounded
        .iter()
        .filter_map(|loop_| Some(loop_.split_once(" at byte")?.0))
        .collect();
    assert_eq!(
        unbounded,
        [
            "a `while`",
            "a `while let`",
            "a `for`",
            "a `loop`",
            "a `loop`",
            "a `loop`"
        ]
    );
}

/// What `expr` writes for `ty` within `max`, nested `nesting` deep, the size it counts and the
/// elements it knows of, over 4,096 short inputs.
fn written(ty: &Ty, max: u64, nesting: u32) -> impl Iterator<Item = (String, u64, u64)> {
    (0..4_096).map(move |mut state| {
        let data: Vec<u8> = (0..4)
            .flat_map(|_| splitmix64(&mut state).to_le_bytes())
            .collect();
        let mut g = Gen::new(&data);
        g.nesting = nesting;
        let info = g.expr(ty, max);
        (g.w.out.program, info.size, info.len)
    })
}

/// The chars of a string literal at the start of `text` with no escape or interpolation, and
/// the text after it.
fn plain(text: &str) -> Option<(u64, &str)> {
    let (literal, rest) = text.strip_prefix('"')?.split_once('"')?;
    let chars = u64::try_from(literal.chars().count()).expect("the count should fit");
    (!literal.contains(['\\', '{'])).then_some((chars, rest))
}

/// The chars of each element of `text`, an array literal of string literals with no escape or
/// interpolation.
fn plain_list(text: &str) -> Option<Vec<u64>> {
    let mut rest = text.strip_prefix('[')?;
    let mut parts = Vec::new();
    while let Some((chars, after)) = plain(rest) {
        parts.push(chars);
        rest = after.strip_prefix(", ").unwrap_or(after);
    }
    (rest == "]").then_some(parts)
}

#[test]
fn an_array_literal_counts_its_slots_and_elements() {
    // `["ab", "c"]` is 2 slots and 3 chars: 5. Nested as deep as can be, the elements are
    // literals.
    let mut seen = 0;
    for (text, size, known) in written(&Ty::Array(Box::new(Ty::String)), 40, MAX_NESTING) {
        let parts = plain_list(&text).unwrap_or_default();
        if parts.len() >= 2 && parts.iter().sum::<u64>() > 0 {
            let len = u64::try_from(parts.len()).expect("the count should fit");
            assert_eq!(size, len + parts.iter().sum::<u64>(), "{text}");
            assert_eq!(known, len, "{text}");
            seen += 1;
        }
    }
    assert!(
        seen > 0,
        "no input should fail to write a literal of plain strings"
    );
}

#[test]
fn a_fill_counts_each_copy() {
    // `["ab"; 3]` is 3 slots of 2 chars each: 9.
    let mut seen = 0;
    for (text, size, known) in written(&Ty::Array(Box::new(Ty::String)), 100, 0) {
        let fill = text
            .strip_prefix('[')
            .and_then(plain)
            .and_then(|(chars, rest)| {
                let len = rest
                    .strip_prefix("; ")?
                    .strip_suffix(']')?
                    .parse::<u64>()
                    .ok()?;
                Some((chars, len))
            });
        if let Some((chars, len)) = fill.filter(|&(chars, len)| chars > 0 && len >= 2) {
            assert_eq!(size, len + len * chars, "{text}");
            assert_eq!(known, len, "{text}");
            seen += 1;
        }
    }
    assert!(
        seen > 0,
        "no input should fail to write a fill of a plain string"
    );
}

#[test]
fn a_split_counts_what_its_parts_can_take() {
    // Of `n` chars, `chars` gives `n` chars, `bytes` up to `4 * n` bytes, `words` up to `n`
    // chars in `n + 1` words at most.
    type Size = fn(u64) -> u64;
    let splits: [(Ty, &str, Size); 3] = [
        (Ty::Char, "chars", |n| n),
        (Ty::U8, "bytes", |n| 4 * n),
        (Ty::String, "words", |n| n + 1),
    ];
    for (element, method, size_of) in splits {
        let mut seen = 0;
        for (text, size, _) in written(&Ty::Array(Box::new(element.clone())), 100, 0) {
            let suffix = format!(").{method}())");
            let split = text.strip_prefix("((").and_then(plain);
            if let Some((chars, rest)) = split.filter(|(chars, rest)| *chars > 0 && *rest == suffix)
            {
                assert_eq!(size, size_of(chars), "{text} {rest}");
                seen += 1;
            }
        }
        assert!(
            seen > 0,
            "no input should fail to write `{method}` of a plain string"
        );
    }
}

/// A `Gen` over `data` in `main`, holding `let mut s0: [string]` of `len` elements with `base`
/// and `room`, inside a loop of `bound` runs if any.
fn with_strings(data: &[u8], len: u64, base: u64, room: u64, bound: Option<u64>) -> Gen<'_> {
    let mut g = with_string(data, base, room, bound);
    let var = &mut g.scope.vars[0];
    var.ty = Ty::Array(Box::new(Ty::String));
    var.len = len;
    var.size = super::expr::array_size(len, base) + room;
    g
}

#[test]
fn an_element_takes_at_most_its_base() {
    // `s0` holds 3 strings of 2 chars at most: 9 in all, but `s0[i] = e` may write 2 chars only.
    let mut seen = 0;
    for mut state in 0..1_024 {
        let data: Vec<u8> = (0..32)
            .flat_map(|_| splitmix64(&mut state).to_le_bytes())
            .collect();
        let mut g = with_strings(&data, 3, 2, 0, None);
        g.stmts(false);
        for line in g.w.out.program.lines() {
            let assigned = ["s0[0] = ", "s0[1] = ", "s0[2] = "]
                .into_iter()
                .find_map(|prefix| line.trim_start().strip_prefix(prefix))
                .and_then(plain);
            if let Some((chars, _)) = assigned {
                assert!(chars <= 2, "{line}");
                seen += 1;
            }
        }
    }
    assert!(
        seen > 0,
        "no input should fail to assign a plain string to an element"
    );
}

#[test]
fn an_element_appends_another_only_within_the_room() {
    // `s0[i] += (s0[j])` adds all an element can hold, its base and the room: with room 500 and
    // no base, once fits ...
    let appended = |bound| {
        (0..=u8::MAX).any(|b| {
            let data = [b; 16];
            let mut g = with_strings(&data, 2, 0, 500, bound);
            g.compound();
            let text = &g.w.out.program;
            text.starts_with("s0[") && text.contains("] += (s0[")
        })
    };
    assert!(appended(None));
    // ... and in a loop of 8 it does not: each run may add only 62.
    assert!(!appended(Some(8)));
}

#[test]
fn an_expression_stays_within_its_most() {
    // Every type, written where it has an expected type and where it has none, within a most
    // from its least value's to a few elements' worth, holds no more than that most: a `K0`
    // optional below the 9 of a `K0` among them.
    let string = || Box::new(Ty::String);
    let class = || Ty::class(0, 9);
    let types = [
        Ty::I64,
        Ty::Char,
        Ty::String,
        Ty::Array(string()),
        Ty::Array(Box::new(Ty::Array(string()))),
        Ty::Array(Box::new(Ty::Char)),
        Ty::Array(Box::new(Ty::U8)),
        Ty::Optional(string()),
        Ty::Optional(Box::new(Ty::Array(string()))),
        class(),
        E0,
        Ty::Optional(Box::new(class())),
        Ty::Optional(Box::new(E0)),
        Ty::Array(Box::new(E0)),
    ];
    for ty in &types {
        for max in [0, 1, 2, 3, 7, 9, 40]
            .into_iter()
            .filter(|max| ty.floor() <= *max)
        {
            for synth in [false, true]
                .into_iter()
                .filter(|&synth| !synth || ty.least() <= max)
            {
                for mut state in 0..64 {
                    let data: Vec<u8> = (0..8)
                        .flat_map(|_| splitmix64(&mut state).to_le_bytes())
                        .collect();
                    let mut g = Gen::new(&data);
                    declare(&mut g);
                    g.synth = synth;
                    let size = g.expr(ty, max).size;
                    let text = &g.w.out.program;
                    assert!(size <= max, "{ty} within {max}: {size} for {text}");
                }
            }
        }
    }
}

#[test]
fn a_program_spends_no_more_than_its_fuel() {
    // Holding fuel back for `main` keeps the total `FUEL`: the cost reserve for `main`'s
    // statements assumes no more.
    for data in seeds() {
        let mut g = Gen::new(&data);
        g.program();
        assert!(g.fuel <= FUEL, "{} left of {FUEL}", g.fuel);
    }
}

#[test]
fn an_array_of_class_objects_is_never_a_fill() {
    // `[k; n]` would share one object among the elements (E0067); a `string` fill is fine.
    let mut fills = FxHashSet::default();
    let class = Ty::class(0, 9);
    let elements = [
        ("K0", class.clone()),
        ("K0?", Ty::Optional(Box::new(class))),
        ("string", Ty::String),
    ];
    for mut state in 0..512 {
        let data: Vec<u8> = (0..8)
            .flat_map(|_| splitmix64(&mut state).to_le_bytes())
            .collect();
        for (name, element) in &elements {
            let mut g = Gen::new(&data);
            declare(&mut g);
            let ty = Ty::Array(Box::new(element.clone()));
            g.array_let("v0".to_string(), ty, element);
            let program = &g.w.out.program;
            let fill = program.trim_end().strip_suffix(']').and_then(|p| {
                let (_, len) = p.rsplit_once("; ")?;
                len.parse::<u64>().ok()
            });
            if fill.is_some() {
                fills.insert(*name);
            }
        }
    }
    assert!(fills.contains("string"), "no `string` array is a fill");
    assert!(
        !fills.contains("K0") && !fills.contains("K0?"),
        "an array of class objects is a fill: {fills:?}"
    );
}

#[test]
fn a_mutable_array_counts_its_slots_bases_and_room() {
    // Each `let mut` array's most size covers a slot and a base per element and the room left
    // for `+=` on them, within `MAX_SIZE`: 8 strings of 64 chars, 520 in all, leave room for at
    // most 480 more, so 16 or 64 but not the 500 a `string` local may have. Only `string`
    // elements grow, so only their arrays get room.
    let mut widest = 0;
    for element in [Ty::String, Ty::I64, Ty::Array(Box::new(Ty::String))] {
        for mut state in 0..1_024 {
            let data: Vec<u8> = (0..8)
                .flat_map(|_| splitmix64(&mut state).to_le_bytes())
                .collect();
            let mut g = Gen::new(&data);
            let ty = Ty::Array(Box::new(element.clone()));
            g.array_let("v0".to_string(), ty, &element);
            let var = &g.scope.vars[0];
            let least = var.len + var.len * var.base + var.room;
            assert!(
                least <= var.size && var.size <= MAX_SIZE,
                "{}",
                g.w.out.program
            );
            assert!(
                element == Ty::String || var.room == 0,
                "{}",
                g.w.out.program
            );
            if element == Ty::String && var.len == MAX_BOUND && var.base == 64 && var.room > 0 {
                widest += 1;
            }
        }
    }
    assert!(
        widest > 0,
        "no input should fail to write 8 strings of 64 chars with room"
    );
}

#[test]
fn an_index_of_a_local_stays_below_its_length() {
    // `a0` holds 3 `i64`s: `(a0[i])` reads one with `i` from 0 to 2.
    let mut seen = [false; 3];
    for mut state in 0..1_024 {
        let data: Vec<u8> = (0..8)
            .flat_map(|_| splitmix64(&mut state).to_le_bytes())
            .collect();
        let mut g = Gen::new(&data);
        let mut var = Var::new(
            "a0".to_string(),
            Ty::Array(Box::new(Ty::I64)),
            false,
            0,
            0,
            0,
        );
        var.len = 3;
        g.scope.vars.push(var);
        g.expr(&Ty::I64, 0);
        for read in g.w.out.program.split("(a0[").skip(1) {
            let at: usize = read
                .split_once(']')
                .and_then(|(at, _)| at.parse().ok())
                .expect("an index of `a0` should be a literal");
            assert!(at < 3, "{}", g.w.out.program);
            seen[at] = true;
        }
    }
    assert_eq!(seen, [true; 3]);
}

#[test]
fn a_type_nests_at_most_twice() {
    // Inputs of one repeated byte pick an array or an optional at every level they can.
    fn depth(ty: &Ty) -> u32 {
        match ty {
            Ty::Array(inner) | Ty::Optional(inner) => 1 + depth(inner),
            Ty::I64
            | Ty::U8
            | Ty::F64
            | Ty::Bool
            | Ty::Char
            | Ty::String
            | Ty::Unit
            | Ty::Class { .. }
            | Ty::Enum { .. } => 0,
        }
    }
    let mut deepest = 0;
    let repeated = (0..=u8::MAX).map(|byte| vec![byte; 64]);
    let mixed = (0..2_048).map(|mut state| {
        (0..2)
            .flat_map(|_| splitmix64(&mut state).to_le_bytes())
            .collect()
    });
    for data in repeated.chain(mixed) {
        let ty = Gen::new(&data).any_type();
        assert!(depth(&ty) <= 2, "{ty}");
        deepest = deepest.max(depth(&ty));
    }
    assert_eq!(deepest, 2);
}

#[test]
fn a_panicking_index_builds_its_array_within_the_cap() {
    // `([e][1])` builds `[e]` before it panics, a slot and `e`: a `string` local of 1,000 chars
    // does not fit, though one of 999 does.
    let mut panics = [0, 0];
    for (i, size) in [MAX_SIZE, MAX_SIZE - 1].into_iter().enumerate() {
        for mut state in 0..4_096 {
            let data: Vec<u8> = (0..8)
                .flat_map(|_| splitmix64(&mut state).to_le_bytes())
                .collect();
            let mut g = Gen::new(&data);
            g.scope
                .vars
                .push(Var::new("s0".to_string(), Ty::String, false, size, 0, 0));
            g.expr(&Ty::String, 0);
            panics[i] += g.w.out.program.matches("([s0][1])").count();
        }
    }
    assert_eq!(
        panics[0], 0,
        "a string of 1,000 chars should not be put in an array"
    );
    assert!(
        panics[1] > 0,
        "some input should put the string of 999 in one"
    );
}

#[test]
fn the_walker_bounds_a_while_let_only_on_a_chain_it_steps_down() {
    // `a` is a chain of 2 nodes that its `while let` steps down a node a run. Each other
    // `while let` falls short of that in one way, so the walker lists it as unbounded: no `let`
    // before it, an immutable one, a pattern that is not a variant's or binds no rest first, a
    // body that does not step to the rest, a chain of 9 nodes, and a value that is not a chain
    // of literals.
    let program = "enum E {
    A(E, i64)
    B
}

class O {
    c: E
}

fn make() -> E {
    return E.B
}

fn mk() -> O {
    return O(c: E.B)
}

fn walk(e: E) {
    while let E.A(t, _) = e {
        print(1)
    }
}

fn main() {
    let mut a = E.A(E.A(E.B, 1), 2)
    while let E.A(t, _) = a {
        a = t
    }
    let b = E.A(E.B, 1)
    while let E.A(t, _) = b {
        print(1)
    }
    let mut n: i64? = 1
    while let v = n {
        n = None
    }
    let mut c = E.B
    while let E.B = c {
        c = E.B
    }
    let mut d = E.A(E.B, 1)
    while let E.A(E.B, _) = d {
        d = E.B
    }
    let mut f = E.A(E.B, 1)
    while let E.A(t, _) = f {
    }
    let mut g = E.A(E.B, 1)
    while let E.A(t, _) = g {
        g = E.B
    }
    let mut h = make()
    while let E.A(t, _) = h {
        h = t
    }
    let mut i = mk().c
    while let E.A(t, _) = i {
        i = t
    }
    let o = mk()
    let mut j = o.c
    while let E.A(t, _) = j {
        j = t
    }
    let mut k = E.A(make(), 1)
    while let E.A(t, _) = k {
        k = t
    }
    let mut m = E.A(E.A(E.A(E.A(E.A(E.A(E.A(E.A(E.A(E.B, 1), 1), 1), 1), 1), 1), 1), 1), 1)
    while let E.A(t, _) = m {
        m = t
    }
}
";
    let mut walk = Walk::default();
    walk.program(program);
    let chain = program.find("a {").expect("the program should walk `a`");
    assert_eq!(walk.chains.iter().copied().collect::<Vec<_>>(), [chain]);
    assert!(
        walk.seen
            .contains("reach: a `while let` walks an enum chain")
    );
    let unbounded: Vec<&str> = walk
        .unbounded
        .iter()
        .filter_map(|loop_| {
            let at: usize = loop_.strip_prefix("a `while let` at byte ")?.parse().ok()?;
            Some(&program[at..=at])
        })
        .collect();
    assert_eq!(
        unbounded,
        ["e", "b", "n", "c", "d", "f", "g", "h", "i", "j", "k", "m"],
        "{:?}",
        walk.unbounded
    );
}

#[test]
fn the_walker_lists_each_class_object_stored_in_a_field_or_an_element() {
    // Decision 4's test reads this list: it finds an object put in a field and one put in an
    // element, but not one bound to a local or a number put in a field.
    let program = "class P {
    mut q: P?
    mut r: [P]
    mut n: i64
}

fn main() {
    let a = P(q: None, r: [], n: 0)
    let b = P(q: None, r: [], n: 0)
    a.q = b
    a.r[0] = b
    a.n = 1
    let mut c = a
    c = b
    print(c.n)
}
";
    let mut walk = Walk::default();
    walk.program(program);
    let stored: Vec<&str> = walk
        .heap_classes
        .iter()
        .filter_map(|store| {
            let at: usize = store.strip_prefix("an assignment at byte ")?.parse().ok()?;
            Some(program[..at].lines().last()?.trim())
        })
        .collect();
    assert_eq!(stored, ["a.q =", "a.r[0] ="], "{:?}", walk.heap_classes);
}

/// What `declare` puts in a scope, as a program declares it.
const DECLARED: &str = "class K0 {
    mut g0: string
    g1: i64
}

enum E0 {
    V0
    V1(E0, string, K0)
    V2(u8, char, bool)
    V3(string, K0)
}
";

/// `E0`, as `declare` declares it.
const E0: Ty = Ty::enumeration(0, 1);

/// `K0`, a class of 9, and `E0`, which holds it, in each variant but the first two, and in
/// `V1` itself.
fn declare(g: &mut Gen<'_>) {
    let class = Ty::class(0, 9);
    let field = |name: &str, ty, mutable, cap| Field {
        name: name.to_string(),
        ty,
        mutable,
        cap,
    };
    g.scope.classes.push(Class {
        fields: vec![
            field("g0", Ty::String, true, 8),
            field("g1", Ty::I64, false, 0),
        ],
        size: 9,
        applied: None,
        inferable: false,
    });
    let variant = |name: &str, fields: Vec<Ty>| Variant {
        name: name.to_string(),
        recursive: fields.iter().any(|f| matches!(f, Ty::Enum { id: 0, .. })),
        fields,
        labels: Vec::new(),
        inferable: false,
    };
    g.scope.enums.push(Enum {
        variants: vec![
            variant("V0", Vec::new()),
            variant("V1", vec![E0, Ty::String, class.clone()]),
            variant("V2", vec![Ty::U8, Ty::Char, Ty::Bool]),
            variant("V3", vec![Ty::String, class.clone()]),
        ],
        least: 1,
        has_class: true,
        applied: None,
    });
    g.scope.types.extend([class, E0]);
}

/// The expression `let v = ...` or `let mut v = ...` starts `text` with.
fn first_init(text: &str) -> ast::Expr {
    let source = format!("fn main() {{\n{text}\n}}\n");
    let lexed = lexer::lex(&source).expect("a generated expression should lex");
    let mut items = parser::parse(lexed, false).program.items.into_iter();
    let Some(ast::Item::Function(main)) = items.next() else {
        panic!("`main` should be the only item: {source}")
    };
    let Some(ast::StmtKind::Let { init, .. }) = main.body.stmts.into_iter().next().map(|s| s.kind)
    else {
        panic!("`main` should start with a `let`: {source}")
    };
    init
}

/// The size of a value of `K0`s, `E0`s, strings and scalars written as literals: a slot per
/// node, each string's chars, and each `K0`'s 9, which its fields may grow to; none for anything
/// else.
fn held(e: &ast::Expr) -> Option<u64> {
    let all = |es: &mut dyn Iterator<Item = &ast::Expr>| es.map(held).sum::<Option<u64>>();
    match &e.kind {
        ast::ExprKind::Int { .. }
        | ast::ExprKind::Float(_)
        | ast::ExprKind::Bool(_)
        | ast::ExprKind::Char(_) => Some(0),
        ast::ExprKind::Str(text) => u64::try_from(text.chars().count()).ok(),
        ast::ExprKind::Call { callee, .. } if callee.name.as_str() == "K0" => Some(9),
        ast::ExprKind::Field { base, .. } if name(base) == Some("E0") => Some(1),
        ast::ExprKind::MethodCall { receiver, args, .. } if name(receiver) == Some("E0") => {
            Some(1 + all(&mut args.iter().map(|arg| &arg.value))?)
        }
        _ => None,
    }
}

#[test]
fn a_variant_counts_its_node_and_fields() {
    // Nested as deep as can be, a variant's fields are literals, and it holds no `E0` itself:
    // what it holds is no more than it counts, which is no more than its most. Above that, one
    // holds `E0` itself.
    let mut measured = 0;
    let mut recursive = false;
    for max in [1, 2, 9, 10, 11, 40, 111] {
        for mut state in 0..256 {
            let data: Vec<u8> = (0..8)
                .flat_map(|_| splitmix64(&mut state).to_le_bytes())
                .collect();
            for nesting in [0, MAX_NESTING] {
                let mut g = Gen::new(&data);
                declare(&mut g);
                g.nesting = nesting;
                let size = g.variant(&E0, max, None).size;
                let text = format!("let v = {}", g.w.out.program);
                assert!(size <= max, "{size} within {max}: {text}");
                let init = first_init(&text);
                let own = matches!(&init.kind, ast::ExprKind::MethodCall { method, .. }
                    if method.name.as_str() == "V1");
                assert!(nesting < MAX_NESTING || !own, "{text}");
                recursive |= own;
                if let Some(holds) = held(&init) {
                    assert!(holds <= size, "{holds} counted as {size}: {text}");
                    measured += 1;
                }
            }
        }
    }
    assert!(
        recursive,
        "no variant above the deepest nesting should hold `E0`"
    );
    assert!(measured > 1_000, "only {measured} variants were literals");
}

#[test]
fn a_chain_counts_each_node_within_its_room() {
    // `let mut c = E0.V1(E0.V1(... end ...), s, k)`: at most `MAX_BOUND` nodes, then another
    // variant, each within a ninth of `MAX_SIZE`, so all of them within it, as `c` counts them;
    // and `c` counts no less than they hold. Nested as deep as can be, their fields are literals,
    // which show what they hold; at the top, they count the most.
    const ROOM: u64 = MAX_SIZE / (MAX_BOUND + 1);
    let mut longest = 0;
    let mut measured = 0;
    for (nesting, mut state) in [0, MAX_NESTING]
        .into_iter()
        .flat_map(|n| (0..512).map(move |s| (n, s)))
    {
        let data: Vec<u8> = (0..32)
            .flat_map(|_| splitmix64(&mut state).to_le_bytes())
            .collect();
        let mut g = Gen::new(&data);
        declare(&mut g);
        g.nesting = nesting;
        g.while_chain(0, 1);
        let text = &g.w.out.program;
        let chain = text
            .strip_prefix("let mut ")
            .and_then(|rest| rest.split_once(' '))
            .expect("a chain should start with its `let mut`")
            .0;
        let size = g
            .scope
            .vars
            .iter()
            .find(|var| var.name == chain)
            .expect("the chain should be in scope")
            .size;
        assert!(size <= MAX_SIZE, "{size}: {text}");
        let init = first_init(text);
        let (mut node, mut nodes, mut holds) = (&init, 0, Some(0));
        while let ast::ExprKind::MethodCall { method, args, .. } = &node.kind
            && method.name.as_str() == "V1"
        {
            let own = args[1..]
                .iter()
                .map(|arg| held(&arg.value))
                .sum::<Option<u64>>()
                .map(|parts| 1 + parts);
            assert!(own.is_none_or(|own| own <= ROOM), "{own:?}: {text}");
            holds = holds.zip(own).map(|(sum, own)| sum + own);
            node = &args[0].value;
            nodes += 1;
        }
        let end = held(node);
        assert!(end.is_none_or(|end| end <= ROOM), "{end:?}: {text}");
        assert!(nodes <= MAX_BOUND, "{nodes} nodes: {text}");
        assert!(
            size <= (nodes + 1) * ROOM,
            "{size} for {nodes} nodes: {text}"
        );
        longest = longest.max(nodes);
        if let Some(holds) = holds.zip(end).map(|(sum, end)| sum + end) {
            assert!(holds <= size, "{holds} counted as {size}: {text}");
            measured += 1;
        }
    }
    assert_eq!(
        longest, MAX_BOUND,
        "no chain should fail to reach {MAX_BOUND} nodes"
    );
    assert!(measured > 300, "only {measured} chains were literals");
}

#[test]
fn a_chain_ends_in_a_variant_within_its_room() {
    // `E0.V2(K0())` holds 112, past the ninth of `MAX_SIZE` each node of a chain has, so a
    // chain of `E0.V1` ends in `E0.V0`.
    for mut state in 0..64 {
        let data: Vec<u8> = (0..32)
            .flat_map(|_| splitmix64(&mut state).to_le_bytes())
            .collect();
        let mut g = Gen::new(&data);
        let class = Ty::class(0, 111);
        g.scope.classes.push(Class {
            fields: Vec::new(),
            size: 111,
            applied: None,
            inferable: false,
        });
        let variant = |name: &str, fields: Vec<Ty>| Variant {
            name: name.to_string(),
            recursive: fields.iter().any(|f| matches!(f, Ty::Enum { id: 0, .. })),
            fields,
            labels: Vec::new(),
            inferable: false,
        };
        g.scope.enums.push(Enum {
            variants: vec![
                variant("V0", Vec::new()),
                variant("V1", vec![E0, Ty::I64]),
                variant("V2", vec![class.clone()]),
            ],
            least: 1,
            has_class: true,
            applied: None,
        });
        g.scope.types.extend([class, E0]);
        g.while_chain(0, 1);
        let text = &g.w.out.program;
        let init = first_init(text);
        let mut node = &init;
        while let ast::ExprKind::MethodCall { method, args, .. } = &node.kind
            && method.name.as_str() == "V1"
        {
            node = &args[0].value;
        }
        let ast::ExprKind::Field { name, .. } = &node.kind else {
            panic!("the chain should end in `E0.V0`: {text}")
        };
        assert_eq!(name.name.as_str(), "V0", "{text}");
    }
}

#[test]
fn every_match_checks_without_an_unreachable_arm() {
    // Matches of each tested type, `E0` among them, with guards, alternatives and nested
    // patterns.
    let program = matches_check_clean(DECLARED, declare);
    // And each kind of pattern comes: a letter, a negative number, an inclusive range, a lone
    // `bool`, a word alone and with another, a variant named with its enum, and a variant
    // within a variant within a variant, but no deeper; and a guard.
    let arms: Vec<&str> = program
        .lines()
        .filter(|line| line.contains(" => "))
        .collect();
    assert!(
        arms.iter().any(|arm| arm.contains(" if ")),
        "no arm has a guard"
    );
    let patterns: Vec<&str> = arms
        .iter()
        .filter_map(|arm| arm.trim().split(" => ").next()?.split(" if ").next())
        .collect();
    let kinds: [(&str, Is); 10] = [
        ("a letter", |p| p.contains('\'')),
        ("a lone number", |p| p.parse::<i64>().is_ok()),
        ("a negative number", |p| p.starts_with('-')),
        ("an inclusive range", |p| p.contains("..=")),
        ("a lone `bool`", |p| p == "true" || p == "false"),
        ("a lone word", |p| p.starts_with('"') && !p.contains(" | ")),
        ("two words", |p| p.starts_with('"') && p.contains(" | ")),
        ("a qualified variant", |p| p.starts_with("E0.V")),
        ("a nested variant", |p| {
            p.contains("(V") || p.contains("(E0.V")
        }),
        ("a variant three deep", |p| depth(p) == 3),
    ];
    for (kind, is) in kinds {
        assert!(patterns.iter().any(|p| is(p)), "no pattern is {kind}");
    }
    let deepest = patterns.iter().map(|p| depth(p)).max();
    assert_eq!(deepest, Some(3), "a variant pattern nests too deep");
}

/// `main` of 4096 matches after `header`, whose types `declare` gives the generator, asserted
/// to check clean: no arm overlaps one above it, and `_` or a name comes only where the arms
/// leave a value unmatched (W0006 otherwise, and E0130 where it does not come but should).
fn matches_check_clean(header: &str, declare: fn(&mut Gen<'_>)) -> String {
    let mut program = format!("{header}\nfn main() {{\n");
    for mut state in 0..4_096 {
        let data: Vec<u8> = (0..8)
            .flat_map(|_| splitmix64(&mut state).to_le_bytes())
            .collect();
        let mut g = Gen::new(&data);
        declare(&mut g);
        // Literals leave the input to the arms.
        g.nesting = MAX_NESTING;
        g.blocks = 1;
        g.match_expr(&Ty::I64, 0, false);
        // Its arms are a block, which it leaves.
        assert_eq!(g.blocks, 1);
        writeln!(program, "    let _ = {}", g.w.out.program)
            .expect("writing to a `String` should succeed");
    }
    program.push_str("}\n");
    let (_, diagnostics) = front_end(&program);
    let wrong: Vec<String> = diagnostics
        .iter()
        .filter(|d| d.severity() == Severity::Error || d.code().as_str() == "W0006")
        .map(|d| {
            let line = program[..d.span().start()].matches('\n').count();
            let text = program.lines().nth(line).unwrap_or_default();
            format!("{} at line {}: {text}", d.code().as_str(), line + 1)
        })
        .collect();
    assert!(wrong.is_empty(), "{wrong:#?}");
    program
}

#[test]
fn an_enum_match_ends_in_a_catch_all_only_where_its_arms_leave_a_value() {
    // Of an enum small enough that its arms often cover it, alone or two together, with or
    // without a guard: some matches end in a variant's arm and others in `_` or a name.
    const SMALL: &str = "enum E0 {\n    V0\n    V1(bool)\n    V2\n}\n";
    let program = matches_check_clean(SMALL, |g| {
        let variant = |name: &str, fields: Vec<Ty>| Variant {
            name: name.to_string(),
            recursive: fields.iter().any(|f| matches!(f, Ty::Enum { id: 0, .. })),
            fields,
            labels: Vec::new(),
            inferable: false,
        };
        g.scope.enums.push(Enum {
            variants: vec![
                variant("V0", Vec::new()),
                variant("V1", vec![Ty::Bool]),
                variant("V2", Vec::new()),
            ],
            least: 1,
            has_class: false,
            applied: None,
        });
        g.scope.types.push(E0);
    });
    let variant = |p: &str| p.starts_with('V') || p.starts_with("E0.");
    let (mut covered, mut open) = (0, 0);
    for one in program.split("let _ = (match ").skip(1) {
        let patterns: Vec<&str> = one
            .lines()
            .filter_map(|line| Some(line.trim().split_once(" => ")?.0))
            .collect();
        if patterns.first().is_some_and(|p| variant(p)) {
            if patterns.last().is_some_and(|p| variant(p)) {
                covered += 1;
            } else {
                open += 1;
            }
        }
    }
    assert!(covered > 0 && open > 0, "{covered} covered, {open} open");
}

/// Whether a pattern's text is of some kind.
type Is = fn(&str) -> bool;

/// How deep the variants of `pattern` nest: its parentheses, but those of a `Some`, which is
/// a layer of an optional.
fn depth(pattern: &str) -> usize {
    // Whether each open parenthesis is a variant's.
    let mut open: Vec<bool> = Vec::new();
    let mut deepest = 0;
    for (at, c) in pattern.char_indices() {
        match c {
            '(' => {
                open.push(!pattern[..at].ends_with("Some"));
                deepest = deepest.max(open.iter().filter(|&&variant| variant).count());
            }
            ')' => {
                open.pop();
            }
            _ => {}
        }
    }
    deepest
}

#[test]
fn an_element_takes_an_assignment_only_where_it_holds_no_class() {
    // Decision 4: `ks[i] = e` would store a `K0` in the heap, and `es[i] = e` an `E0`, which
    // may hold one; `ss[i] = e` stores a `string`.
    let mut assigned = FxHashSet::default();
    for mut state in 0..4_096 {
        let data: Vec<u8> = (0..2)
            .flat_map(|_| splitmix64(&mut state).to_le_bytes())
            .collect();
        let mut g = Gen::new(&data);
        declare(&mut g);
        for (name, element, base) in [
            ("ks", Ty::class(0, 9), 9),
            ("es", E0, 10),
            ("ss", Ty::String, 8),
        ] {
            let ty = Ty::Array(Box::new(element));
            let mut var = Var::new(name.to_string(), ty, true, base, 0, 0);
            (var.len, var.elem, var.size) = (2, base, 2 + 2 * base);
            g.scope.vars.push(var);
        }
        g.stmt();
        for line in g.w.out.program.lines() {
            let line = line.trim_start();
            for name in ["ks", "es", "ss"] {
                if line.starts_with(&format!("{name}[")) && line.contains("] = ") {
                    assigned.insert(name);
                }
            }
        }
    }
    assert!(
        assigned.contains("ss"),
        "no `string` element takes an assignment"
    );
    assert!(
        !assigned.contains("ks") && !assigned.contains("es"),
        "an element that holds a class takes an assignment: {assigned:?}"
    );
}

#[test]
fn a_declared_type_leaves_arrays_among_the_types() {
    // With `K0` and `E0` declared, they share one choice with the values, and arrays still
    // come, of them as well.
    let (mut arrays, mut of_declared) = (0, 0);
    for mut state in 0..1_024 {
        let data: Vec<u8> = splitmix64(&mut state).to_le_bytes().to_vec();
        let mut g = Gen::new(&data);
        declare(&mut g);
        if let Ty::Array(element) = g.any_type() {
            arrays += 1;
            of_declared += usize::from(matches!(*element, Ty::Class { .. } | Ty::Enum { .. }));
        }
    }
    assert!(arrays > 0, "no array with `K0` and `E0` declared");
    assert!(of_declared > 0, "no array of `K0` or `E0`");
}

#[test]
fn a_variant_should_be_recursive_when_it_holds_its_enum() {
    let (mut recursive, mut other) = (0, 0);
    for data in seeds() {
        let mut g = Gen::new(&data);
        g.program();
        for (id, e) in g.scope.enums.iter().enumerate() {
            let own = |ty: &Ty| matches!(ty, Ty::Enum { id: own, .. } if *own == id);
            for variant in &e.variants {
                // An instance of a generic enum says it by its declaration, not by its fields.
                if e.applied.is_none() {
                    assert_eq!(variant.recursive, variant.fields.iter().any(own));
                }
                recursive += usize::from(variant.recursive);
                other += usize::from(!variant.recursive);
            }
        }
    }
    assert!(
        recursive > 0 && other > 0,
        "{recursive} recursive, {other} not"
    );
}

#[test]
fn each_count_reaches_its_most_and_stays_within_it() {
    // 3 fields of a class or a variant, 2 methods, 2 to 4 variants.
    let (mut fields, mut methods, mut variants, mut payload) = (0, 0, (usize::MAX, 0), 0);
    for mut state in 0..1_024 {
        let data: Vec<u8> = (0..8)
            .flat_map(|_| splitmix64(&mut state).to_le_bytes())
            .collect();
        let mut g = Gen::new(&data);
        g.class();
        g.enumeration();
        let program = &g.w.out.program;
        fields = fields.max(g.scope.classes[0].fields.len());
        methods = methods.max(program.lines().filter(|l| l.starts_with("    fn ")).count());
        let count = g.scope.enums[0].variants.len();
        variants = (variants.0.min(count), variants.1.max(count));
        let most = g.scope.enums[0]
            .variants
            .iter()
            .map(|v| v.fields.len())
            .max();
        payload = payload.max(most.unwrap_or(0));
    }
    assert_eq!((fields, methods, variants, payload), (3, 2, (2, 4), 3));
}

#[test]
fn a_cap_holds_the_least_value_of_its_field() {
    // A `K0` of 50, or a `K0` optional, takes its 50 and more; a number, or a number optional,
    // takes none.
    let class = Ty::class(0, 50);
    for byte in 0..3 {
        let data = [byte];
        let mut g = Gen::new(&data);
        assert_eq!(g.size_for(&class, &[0, 8, 32]), 50);
        let mut g = Gen::new(&data);
        assert!(g.size_for(&Ty::Optional(Box::new(class.clone())), &[0, 8, 32]) >= 50);
        let mut g = Gen::new(&data);
        assert_eq!(g.size_for(&Ty::I64, &[0, 8, 32]), 0);
        let mut g = Gen::new(&data);
        assert_eq!(g.size_for(&Ty::Optional(Box::new(Ty::I64)), &[0, 8, 32]), 0);
    }
}

#[test]
fn a_part_holds_at_most_its_whole_or_a_class_its_own() {
    // A class object counts its size wherever it is, so its own is the tighter of the two.
    assert_eq!(Ty::String.within(40), 40);
    assert_eq!(Ty::Optional(Box::new(Ty::String)).within(40), 40);
    assert_eq!(Ty::class(0, 9).within(40), 9);
}

#[test]
fn a_field_type_past_most_is_an_i64() {
    // A field of a class of 50 fits a holder of 50 within `MOST`, but not one of 51.
    for (size, fits) in [(50, true), (51, false)] {
        let mut g = Gen::new(&[6]);
        let class = Ty::class(0, 50);
        g.scope.types.push(class.clone());
        let ty = g.field_type(size);
        assert_eq!(ty, if fits { class } else { Ty::I64 }, "within {size}");
    }
}

#[test]
fn a_field_cap_past_most_is_an_i64() {
    // `class K1 { g: K0, g: string }`, the string's cap 32: after a `K0` of 67 it reaches
    // `MOST`, after one of 68 it is past it, and an `i64` takes its place.
    for (held, fits) in [(67, true), (68, false)] {
        let mut g = Gen::new(&[2, 6, 0, 5, 2]);
        g.scope.classes.push(Class {
            fields: Vec::new(),
            size: held,
            applied: None,
            inferable: false,
        });
        g.scope.types.push(Ty::class(0, held));
        g.class();
        let class = &g.scope.classes[1];
        let last = &class.fields[1];
        let (ty, cap) = if fits { (Ty::String, 32) } else { (Ty::I64, 0) };
        assert_eq!((&last.ty, last.cap), (&ty, cap), "{}", g.w.out.program);
        assert_eq!(class.size, 1 + held + cap, "{}", g.w.out.program);
    }
}

/// Checks seed `n`'s declared types as `declared_types_keep_their_sizes` says, and gives how
/// many class fields hold a class optional and how many enums hold a class.
fn check_declared_types(n: u64) -> (usize, usize) {
    let (mut optional_classes, mut enum_classes) = (0, 0);
    let data = seed(n);
    let mut g = Gen::new(&data);
    g.program();
    let text = &g.w.out.program;
    for class in &g.scope.classes {
        let caps: u64 = class.fields.iter().map(|field| field.cap).sum();
        assert_eq!(class.size, 1 + caps, "{text}");
        assert!(class.size <= MOST, "{text}");
        for field in &class.fields {
            let least = match &field.ty {
                Ty::Optional(payload) => payload.floor(),
                ty => ty.floor(),
            };
            assert!(field.cap >= least, "{}: {text}", field.name);
            optional_classes += usize::from(
                matches!(&field.ty, Ty::Optional(payload) if matches!(**payload, Ty::Class { .. })),
            );
        }
    }
    for (id, e) in g.scope.enums.iter().enumerate() {
        let own = |ty: &Ty| matches!(ty, Ty::Enum { id: of, .. } if *of == id);
        let first = &e.variants[0].fields;
        assert!(!first.iter().any(own), "{text}");
        assert_eq!(
            e.least,
            1 + first.iter().map(Ty::floor).sum::<u64>(),
            "{text}"
        );
        let holds = e
            .variants
            .iter()
            .flat_map(|variant| &variant.fields)
            .any(|field| !own(field) && g.scope.contains_class(field));
        assert_eq!(e.has_class, holds, "{text}");
        enum_classes += usize::from(holds);
    }
    (optional_classes, enum_classes)
}

/// Checks the declared types of the `seeds` (`check_declared_types`), of which some enum should
/// hold a class.
fn declared_types_keep_their_sizes(seeds: std::ops::Range<u64>) {
    // A class's size is a node and its fields' caps, within `MOST`, and each cap holds its
    // field's least value, or the least value of the type an optional field holds. An enum's
    // least is its first variant's, which holds no value of the enum, and it holds a class
    // where a field of a variant may.
    let enum_classes: usize = seeds.map(|n| check_declared_types(n).1).sum();
    assert!(enum_classes > 0, "no enum should fail to hold a class");
}

// Two batches of 256 seeds, so nextest runs them in parallel within the time budget.
#[test]
fn declared_types_keep_their_sizes_0() {
    declared_types_keep_their_sizes(0..256);
}

#[test]
fn declared_types_keep_their_sizes_256() {
    declared_types_keep_their_sizes(256..512);
}

#[test]
fn a_class_optional_field_should_keep_its_size() {
    // A class field that holds a class optional is rare: seed 0 has one.
    let (optional_classes, _) = check_declared_types(0);
    assert!(
        optional_classes > 0,
        "no class should fail to hold a class optional"
    );
}

#[test]
fn a_mutable_array_of_large_elements_holds_fewer_of_them() {
    // `E0`'s least value is 100, a node and a `K0` of 99: with a base of 116 or 164, fewer than
    // `MAX_BOUND` elements fit `MAX_SIZE`.
    let mut fewer = 0;
    for mut state in 0..1_024 {
        let data: Vec<u8> = (0..8)
            .flat_map(|_| splitmix64(&mut state).to_le_bytes())
            .collect();
        let mut g = Gen::new(&data);
        let class = Ty::class(0, 99);
        g.scope.classes.push(Class {
            fields: vec![Field {
                name: "g0".to_string(),
                ty: Ty::String,
                mutable: true,
                cap: 98,
            }],
            size: 99,
            applied: None,
            inferable: false,
        });
        let variant = |name: &str, fields: Vec<Ty>| Variant {
            name: name.to_string(),
            recursive: fields.iter().any(|f| matches!(f, Ty::Enum { id: 0, .. })),
            fields,
            labels: Vec::new(),
            inferable: false,
        };
        g.scope.enums.push(Enum {
            variants: vec![
                variant("V0", vec![class.clone()]),
                variant("V1", Vec::new()),
            ],
            least: 100,
            has_class: true,
            applied: None,
        });
        let element = Ty::enumeration(0, 100);
        g.scope.types.extend([class, element.clone()]);
        g.array_let(
            "v0".to_string(),
            Ty::Array(Box::new(element.clone())),
            &element,
        );
        let var = &g.scope.vars[0];
        assert!(
            var.len + var.len * var.base <= var.size && var.size <= MAX_SIZE,
            "{}",
            g.w.out.program
        );
        fewer += usize::from(var.len > 0 && (var.len + 1) * (1 + var.base) > MAX_SIZE);
    }
    assert!(
        fewer > 0,
        "no array should fail to fill all the elements that fit"
    );
}

#[test]
fn an_f64_right_operand_is_any_expression() {
    // `right_operand` writes its small literal only for an integer type. Float arithmetic, where
    // optimizers miscompile, takes any `f64` expression on its right, a literal about as often as
    // not. Were the small literal taken for an `f64` too, literals would outnumber the rest many
    // times over.
    let [literal, other] = walk().f64_rights;
    assert!(
        other * 2 > literal,
        "{literal} right operands of `f64` arithmetic or `op=` are literals and {other} are not"
    );
}

#[test]
fn every_rewritten_copy_checks_clean() {
    for data in seeds() {
        checked(&generate(&data).rewritten);
    }
}

#[test]
fn no_message_breaks_a_line() {
    // A panic's record is the last stderr line, which `judge_pair` reads as one line.
    let mut messages = Vec::new();
    for data in seeds() {
        // The rewritten copy writes the same messages.
        let source = generate(&data).program;
        let (ast, _) = checked(&source);
        let mut walk = Walk {
            source,
            ..Walk::default()
        };
        walk.ast(&ast);
        messages.extend(walk.messages);
    }
    assert!(!messages.is_empty(), "no seed writes a message");
    let broken: Vec<&String> = messages
        .iter()
        .filter(|m| m.contains(['\n', '\r']))
        .collect();
    assert!(broken.is_empty(), "messages that break a line: {broken:?}");
}

#[test]
fn rewrites_happen() {
    let mut made = FxHashMap::default();
    for data in seeds() {
        for rewrite in generate(&data).made {
            *made.entry(format!("{rewrite:?}")).or_insert(0) += 1;
        }
    }
    let missing: Vec<&str> = ["Block", "DeadIf", "Swap", "Let", "Const"]
        .into_iter()
        .filter(|rewrite| !made.contains_key(*rewrite))
        .collect();
    assert!(missing.is_empty(), "no seed's copy holds: {missing:?}");
}

/// Whether `name` is a local the Let rewrite declares: `r` and a number.
fn bound(name: Option<&str>) -> bool {
    name.and_then(|name| name.strip_prefix('r'))
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The `let r<n>` statements of a function of a rewritten copy, each checked against the Let
/// rewrite's condition (decision 2) from the typed tree alone.
struct Lets<'f> {
    locals: &'f [tir::Local],
    /// The values of the `r<n>` whose value is a known integer.
    values: FxHashMap<usize, i64>,
    /// While the use of an `r<n>` is sought: the local.
    sought: Option<tir::LocalId>,
    /// Where it was found: under a condition, after an effect.
    found: Option<(bool, bool)>,
    /// How many conditions the walk is under, and whether an effect ran before this point.
    conditional: usize,
    effect: bool,
    /// How many `let`s were checked, and what broke the condition.
    checked: usize,
    broken: Vec<String>,
}

impl<'f> Lets<'f> {
    fn new(locals: &'f [tir::Local]) -> Self {
        Lets {
            locals,
            values: FxHashMap::default(),
            sought: None,
            found: None,
            conditional: 0,
            effect: false,
            checked: 0,
            broken: Vec::new(),
        }
    }

    fn name(&self, local: tir::LocalId) -> Option<&str> {
        self.locals[local.0].name.as_deref()
    }

    fn block(&mut self, block: &tir::Block) {
        for (i, stmt) in block.stmts.iter().enumerate() {
            if self.sought.is_none() {
                self.check(&block.stmts[i..], block.value.as_deref());
            }
            self.stmt(stmt);
        }
        if let Some(value) = &block.value {
            self.expr(value);
        }
    }

    /// Checks the `let` that starts `stmts`, if the Let rewrite wrote it: its value is pure,
    /// and its statement, which the other such `let`s after it may hold, uses it first where it
    /// runs unconditionally and after no effect. A block's last statement is its `value`.
    fn check(&mut self, stmts: &[tir::Stmt], value: Option<&tir::Expr>) {
        let [tir::Stmt::Let { local, init }, rest @ ..] = stmts else {
            return;
        };
        if !bound(self.name(*local)) {
            return;
        }
        self.checked += 1;
        let name = self.name(*local).unwrap_or_default().to_string();
        if !self.pure(init, false) {
            self.broken.push(format!("`{name}` holds what is not pure"));
        }
        if let Some(value) = self.value(init) {
            self.values.insert(local.0, value);
        }
        let outer = (self.conditional, self.effect);
        self.sought = Some(*local);
        for host in rest {
            (self.conditional, self.effect) = (0, false);
            self.stmt(host);
            let next = matches!(host, tir::Stmt::Let { local, .. } if bound(self.name(*local)));
            if self.found.is_some() || !next {
                break;
            }
        }
        if let Some(value) = value
            && self.found.is_none()
            && rest.iter().all(
                |stmt| matches!(stmt, tir::Stmt::Let { local, .. } if bound(self.name(*local))),
            )
        {
            (self.conditional, self.effect) = (0, false);
            self.expr(value);
        }
        match self.found.take() {
            None => self
                .broken
                .push(format!("`{name}`'s statement does not use it")),
            Some((true, _)) => self
                .broken
                .push(format!("`{name}` is used under a condition")),
            Some((false, true)) => self
                .broken
                .push(format!("`{name}` is used after an effect")),
            Some((false, false)) => {}
        }
        self.sought = None;
        (self.conditional, self.effect) = outer;
    }

    fn stmt(&mut self, stmt: &tir::Stmt) {
        match stmt {
            tir::Stmt::Let { init, .. } | tir::Stmt::Expr(init) => self.expr(init),
            tir::Stmt::Assign { place, value } | tir::Stmt::Append { place, value } => {
                self.place(place);
                self.expr(value);
            }
        }
    }

    fn place(&mut self, place: &tir::Place) {
        match place {
            tir::Place::Local(_) => {}
            tir::Place::Field { base, .. } => self.expr(base),
            tir::Place::Index { base, index } => {
                self.place(base);
                self.expr(index);
            }
        }
    }

    fn conditionally(&mut self, walk: impl FnOnce(&mut Self)) {
        self.conditional += 1;
        walk(self);
        self.conditional -= 1;
    }

    /// Walks `e` in the order it runs.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per kind of expression, with no `_` arm to miss a new one"
    )]
    fn expr(&mut self, e: &tir::Expr) {
        match &e.kind {
            tir::ExprKind::Int(_)
            | tir::ExprKind::Float(_)
            | tir::ExprKind::Bool(_)
            | tir::ExprKind::Char(_)
            | tir::ExprKind::Str(_)
            | tir::ExprKind::None
            | tir::ExprKind::ReadLine
            | tir::ExprKind::Break { value: None, .. }
            | tir::ExprKind::Continue { .. } => {}
            tir::ExprKind::Local(local) => {
                if self.sought == Some(*local) && self.found.is_none() {
                    self.found = Some((self.conditional > 0, self.effect));
                }
            }
            tir::ExprKind::Call { args, .. }
            | tir::ExprKind::New { args, .. }
            | tir::ExprKind::Variant { args, .. }
            | tir::ExprKind::ArrayLit(args)
            | tir::ExprKind::Math { operands: args, .. }
            | tir::ExprKind::Interpolate { parts: args, .. } => {
                for arg in args {
                    self.expr(arg);
                }
            }
            tir::ExprKind::Field { base: x, .. }
            | tir::ExprKind::Wrap(x)
            | tir::ExprKind::Convert(x)
            | tir::ExprKind::Print { operand: x, .. }
            | tir::ExprKind::Panic(x)
            | tir::ExprKind::Len(x)
            | tir::ExprKind::Chars(x)
            | tir::ExprKind::Bytes(x)
            | tir::ExprKind::Words(x)
            | tir::ExprKind::FromChars(x)
            | tir::ExprKind::Sum(x)
            | tir::ExprKind::Unary { operand: x, .. } => self.expr(x),
            tir::ExprKind::ArrayFill { value: a, len: b }
            | tir::ExprKind::Index { base: a, index: b }
            | tir::ExprKind::Contains { base: a, value: b }
            | tir::ExprKind::Concat { lhs: a, rhs: b }
            | tir::ExprKind::Equal { lhs: a, rhs: b, .. }
            | tir::ExprKind::Is { lhs: a, rhs: b } => {
                self.expr(a);
                self.expr(b);
            }
            tir::ExprKind::Binary { op, lhs, rhs } => {
                self.expr(lhs);
                if matches!(op, tir::BinaryOp::And | tir::BinaryOp::Or) {
                    self.conditionally(|walk| walk.expr(rhs));
                } else {
                    self.expr(rhs);
                }
            }
            // A chain stops at its first false comparison.
            tir::ExprKind::Compare { operands, .. } => {
                for (i, operand) in operands.iter().enumerate() {
                    if i < 2 {
                        self.expr(operand);
                    } else {
                        self.conditionally(|walk| walk.expr(operand));
                    }
                }
            }
            tir::ExprKind::If {
                cond: scrutinee,
                then_block,
                else_block,
            }
            | tir::ExprKind::IfSome {
                scrutinee,
                then_block,
                else_block,
                ..
            } => {
                self.expr(scrutinee);
                self.conditionally(|walk| {
                    walk.block(then_block);
                    if let Some(block) = else_block {
                        walk.block(block);
                    }
                });
            }
            tir::ExprKind::Match {
                scrutinee, arms, ..
            } => {
                self.expr(scrutinee);
                self.conditionally(|walk| {
                    for arm in arms {
                        if let Some(guard) = &arm.guard {
                            walk.expr(guard);
                        }
                        walk.block(&arm.body);
                    }
                });
            }
            tir::ExprKind::Loop(block) => self.conditionally(|walk| walk.block(block)),
            tir::ExprKind::Break {
                value: Some(value), ..
            } => self.expr(value),
            tir::ExprKind::Return(value) => {
                if let Some(value) = value {
                    self.expr(value);
                }
            }
            tir::ExprKind::Block(block) => self.block(block),
        }
        if !self.pure(e, true) {
            self.effect = true;
        }
    }

    /// Whether `e` has no effect and cannot panic; and, unless `reads`, reads no field and no
    /// mutable local either.
    fn pure(&self, e: &tir::Expr, reads: bool) -> bool {
        let int = matches!(e.ty.kind(), TypeKind::I64 | TypeKind::U8);
        let all = |parts: &[tir::Expr]| parts.iter().all(|part| self.pure(part, reads));
        match &e.kind {
            tir::ExprKind::Int(_)
            | tir::ExprKind::Float(_)
            | tir::ExprKind::Bool(_)
            | tir::ExprKind::Char(_)
            | tir::ExprKind::Str(_)
            | tir::ExprKind::None => true,
            tir::ExprKind::Local(local) => reads || !self.locals[local.0].mutable,
            tir::ExprKind::Field { base, .. } => reads && self.pure(base, reads),
            tir::ExprKind::Call { .. }
            | tir::ExprKind::Print { .. }
            | tir::ExprKind::Panic(_)
            | tir::ExprKind::ReadLine
            | tir::ExprKind::Index { .. }
            | tir::ExprKind::Loop(_)
            | tir::ExprKind::Break { .. }
            | tir::ExprKind::Continue { .. }
            | tir::ExprKind::Return(_) => false,
            // An `i64` sum may overflow, and so may an `i64` negation or arithmetic whose value
            // is not known.
            tir::ExprKind::Sum(x) => !int && self.pure(x, reads),
            tir::ExprKind::Unary { op, operand } => {
                (!int || *op == tir::UnaryOp::Not || self.value(e).is_some())
                    && self.pure(operand, reads)
            }
            tir::ExprKind::Binary { lhs, rhs, .. } => {
                (!int || self.value(e).is_some()) && self.pure(lhs, reads) && self.pure(rhs, reads)
            }
            tir::ExprKind::Math { op, operands } => {
                !(int && *op == tir::MathOp::Abs) && all(operands)
            }
            tir::ExprKind::ArrayFill { value, len } => {
                self.value(len).is_some_and(|n| n >= 0) && self.pure(value, reads)
            }
            tir::ExprKind::New { args, .. }
            | tir::ExprKind::Variant { args, .. }
            | tir::ExprKind::ArrayLit(args)
            | tir::ExprKind::Interpolate { parts: args, .. }
            | tir::ExprKind::Compare { operands: args, .. } => all(args),
            tir::ExprKind::Wrap(x)
            | tir::ExprKind::Convert(x)
            | tir::ExprKind::Len(x)
            | tir::ExprKind::Chars(x)
            | tir::ExprKind::Bytes(x)
            | tir::ExprKind::Words(x)
            | tir::ExprKind::FromChars(x) => self.pure(x, reads),
            tir::ExprKind::Contains { base: a, value: b }
            | tir::ExprKind::Concat { lhs: a, rhs: b }
            | tir::ExprKind::Equal { lhs: a, rhs: b, .. }
            | tir::ExprKind::Is { lhs: a, rhs: b } => self.pure(a, reads) && self.pure(b, reads),
            tir::ExprKind::If {
                cond: scrutinee,
                then_block,
                else_block,
            }
            | tir::ExprKind::IfSome {
                scrutinee,
                then_block,
                else_block,
                ..
            } => {
                self.pure(scrutinee, reads)
                    && self.pure_block(then_block, reads)
                    && else_block
                        .as_ref()
                        .is_none_or(|block| self.pure_block(block, reads))
            }
            tir::ExprKind::Match {
                scrutinee, arms, ..
            } => {
                self.pure(scrutinee, reads)
                    && arms.iter().all(|arm| {
                        arm.guard.as_ref().is_none_or(|g| self.pure(g, reads))
                            && self.pure_block(&arm.body, reads)
                    })
            }
            tir::ExprKind::Block(block) => self.pure_block(block, reads),
        }
    }

    /// Whether `block` is pure: no statement but the `let`s the checker writes, of a chain's
    /// operands for one, and a pure value.
    fn pure_block(&self, block: &tir::Block, reads: bool) -> bool {
        block
            .stmts
            .iter()
            .all(|stmt| matches!(stmt, tir::Stmt::Let { init, .. } if self.pure(init, reads)))
            && block
                .value
                .as_deref()
                .is_none_or(|value| self.pure(value, reads))
    }

    /// The value of an integer `e` of literals, negations, arithmetic and the `r<n>` whose
    /// value is known, if no step overflows or divides by zero.
    fn value(&self, e: &tir::Expr) -> Option<i64> {
        let value = if let tir::ExprKind::Int(n) = &e.kind {
            Some(*n)
        } else if let tir::ExprKind::Local(local) = &e.kind {
            self.values.get(&local.0).copied()
        } else if let tir::ExprKind::Unary {
            op: tir::UnaryOp::Neg,
            operand,
        } = &e.kind
        {
            self.value(operand)?.checked_neg()
        } else if let tir::ExprKind::Binary { op, lhs, rhs } = &e.kind {
            let (l, r) = (self.value(lhs)?, self.value(rhs)?);
            match op {
                tir::BinaryOp::Add | tir::BinaryOp::AddUnchecked => l.checked_add(r),
                tir::BinaryOp::Sub => l.checked_sub(r),
                tir::BinaryOp::Mul => l.checked_mul(r),
                tir::BinaryOp::Div => l.checked_div(r),
                tir::BinaryOp::Rem => l.checked_rem(r),
                tir::BinaryOp::And | tir::BinaryOp::Or => None,
            }
        } else if let tir::ExprKind::Block(block) = &e.kind
            && block.stmts.is_empty()
        {
            self.value(block.value.as_deref()?)
        } else {
            None
        };
        value.filter(|n| e.ty != Type::U8 || (0..=255).contains(n))
    }
}

#[test]
fn every_rewrite_meets_its_condition() {
    // Each condition is checked on the rewritten copy itself, apart from the flags the
    // generator chose it by. A `C<n>` that does not fold, or folds through an overflowing step,
    // fails to check (E0086, E0088), so checking clean covers the Const rewrite.
    let (mut lets_seen, mut broken, mut misplaced) = (0, Vec::new(), Vec::new());
    for data in seeds() {
        let g = generate(&data);
        let (ast, program) = checked(&g.rewritten);
        let mut walk = Walk {
            source: g.rewritten,
            ..Walk::default()
        };
        walk.ast(&ast);
        misplaced.extend(walk.misplaced);
        for f in &program.functions {
            let mut lets = Lets::new(&f.locals);
            lets.block(&f.body);
            lets_seen += lets.checked;
            broken.extend(lets.broken.into_iter().map(|b| format!("{}: {b}", f.name)));
        }
    }
    assert!(lets_seen > 0, "no copy holds a `let r<n>`");
    assert!(broken.is_empty(), "{broken:#?}");
    assert!(misplaced.is_empty(), "{misplaced:#?}");
}

/// A site where every rewrite keeps the meaning: each test below breaks one condition.
const ANY: Site = Site {
    pure: true,
    unconditional: true,
    folds: true,
    in_const: false,
    place: false,
    last: false,
};

/// Asserts that `rewrite` is allowed at `ANY` but not at `site`.
fn refused(rewrite: Rewrite, site: &Site) {
    assert!(
        allowed(rewrite, &ANY),
        "{rewrite:?} should be allowed where it keeps the meaning"
    );
    assert!(
        !allowed(rewrite, site),
        "{rewrite:?} should not be allowed here"
    );
}

#[test]
fn let_binding_skips_a_mutated_name() {
    // `print({ n = 2; n })`: a `let` before the statement would read `n` before it is 2.
    // `allowed` sees only an impure site; that the generator marks a `mut` local impure is
    // `every_rewrite_meets_its_condition`'s to check.
    refused(Rewrite::Let, &Site { pure: false, ..ANY });
}

#[test]
fn let_binding_skips_a_short_circuit() {
    // `false && (1 / 0 == 0)`: a `let` before the statement would divide by zero.
    refused(
        Rewrite::Let,
        &Site {
            unconditional: false,
            ..ANY
        },
    );
}

#[test]
fn let_binding_skips_a_field_read() {
    // `print({ p.x = 2; p.x })`: a field is a read of what may be assigned, as a `mut` local is.
    // `allowed` sees only an impure site; that the generator marks a field read impure is
    // `every_rewrite_meets_its_condition`'s to check.
    refused(Rewrite::Let, &Site { pure: false, ..ANY });
}

#[test]
fn no_block_in_a_const() {
    refused(
        Rewrite::Block,
        &Site {
            in_const: true,
            ..ANY
        },
    );
}

#[test]
fn no_block_as_a_place() {
    refused(Rewrite::Block, &Site { place: true, ..ANY });
}

#[test]
fn dead_branch_never_ends_a_block() {
    // `fn f() -> i64 { 4 }`: an `if false` after the `4` would end the block in a unit.
    refused(Rewrite::DeadIf, &Site { last: true, ..ANY });
}

#[test]
fn no_const_with_an_overflowing_step() {
    refused(
        Rewrite::Const,
        &Site {
            folds: false,
            ..ANY
        },
    );
}

#[test]
fn let_binding_keeps_the_type() {
    // `255` where a `u8` is expected: unannotated, `let r = 255` would be an `i64`.
    let mut w = Writer::default();
    w.text("fn main() {");
    w.line();
    let statement = w.begin();
    w.text("let v1: u8 = ");
    let value = w.mark();
    w.text("255");
    w.bind(value, &Ty::U8);
    w.end(statement, false);
    w.line();
    w.text("print(v1)\n}\n");
    assert_eq!(
        w.out.rewritten,
        "fn main() {\nlet r1: u8 = 255\nlet v1: u8 = r1\nprint(v1)\n}\n"
    );
    checked(&w.out.rewritten);
}

#[test]
fn known_answers_happen() {
    // Over the seeds, a known site of each type gives a value, and one panics (decision 3). A
    // `u8` site that panics is rare, so this test reads 50% more seeds than the others.
    let mut kinds = FxHashSet::default();
    for data in (0..SEEDS * 3 / 2).map(seed) {
        let g = generate(&data);
        for known in &g.known {
            let head = format!("fn {}(a: ", known.marker);
            let at = g
                .program
                .find(&head)
                .expect("a known site should have its function");
            let ty = &g.program[at + head.len()..]
                .split(',')
                .next()
                .unwrap_or_default();
            let panics = matches!(known.answer, Answer::Panic(_));
            kinds.insert((ty.to_string(), panics));
        }
    }
    for kind in [("i64", false), ("i64", true), ("u8", false), ("u8", true)] {
        assert!(
            kinds.contains(&(kind.0.to_string(), kind.1)),
            "no seed has a known site of `{}` that {}",
            kind.0,
            if kind.1 { "panics" } else { "gives a value" }
        );
    }
}

#[test]
fn known_sites_take_every_count_and_negative_arguments() {
    // Up to `MAX_KNOWN` sites, whose `i64` arguments include `-1`, the least `i64` and drawn
    // negatives, written `(-n)`.
    let (mut most, mut negatives) = (0, FxHashSet::default());
    for data in seeds() {
        let g = generate(&data);
        most = most.max(g.known.len());
        let main = g.program.split("\nfn k").next().unwrap_or_default();
        let calls = main
            .lines()
            .filter(|line| line.contains("print(\"@k") || line.contains("print(k"));
        for call in calls {
            for arg in call.split("(-").skip(1) {
                let digits = arg.split(')').next().unwrap_or_default();
                negatives.extend(digits.parse::<i128>().map(|n| -n));
            }
        }
    }
    assert_eq!(most, MAX_KNOWN, "no seed has {MAX_KNOWN} known sites");
    assert!(
        negatives.contains(&-1),
        "no known call takes `(-1)`: {negatives:?}"
    );
    assert!(
        negatives.contains(&i128::from(i64::MIN)),
        "no known call takes the least `i64`: {negatives:?}"
    );
    assert!(
        negatives.iter().any(|n| (-1_000..-1).contains(n)),
        "no known call takes a drawn negative: {negatives:?}"
    );
}

#[test]
fn no_panic_site_follows_a_return() {
    // A `return` in `main` may end the run before the site's call, with no panic to show that
    // it was never reached (decision 3). Only about one seed in 50 both returns from `main` and
    // draws a panic site, so this takes 256.
    let mut returning = 0;
    for n in 0..256 {
        let g = generate(&seed(n));
        let main = g.program.split("fn main() ").nth(1).unwrap_or_default();
        let main = main.split("\nfn k").next().unwrap_or_default();
        if main.contains("return") {
            returning += 1;
            let panics = g.known.iter().any(|k| matches!(k.answer, Answer::Panic(_)));
            assert!(!panics, "a panic site follows a `return`:\n{}", g.program);
        }
    }
    assert!(returning > 0, "no seed's `main` returns");
}

#[test]
fn rewrites_are_rare() {
    // Each expression and statement is rewritten about one time in 8 (decision 2), so the copy
    // costs its builds not much more than the program: about 1.6 times its size, against over
    // 2 when every one is.
    let (mut program, mut rewritten, mut swaps, mut ifs) = (0, 0, 0, 0);
    for data in seeds() {
        let g = generate(&data);
        program += g.program.len();
        rewritten += g.rewritten.len();
        swaps += g.made.iter().filter(|&&made| made == Rewrite::Swap).count();
        ifs += g.program.matches("} else {").count();
    }
    assert!(
        rewritten < 2 * program,
        "{rewritten} bytes rewritten of {program}"
    );
    // About 80 of 4,000 `else`s swap, against over 500 when each `if` that may swap does.
    assert!(swaps * 16 < ifs, "{swaps} swaps of {ifs} `else`s");
}

#[test]
fn known_answers_are_integer() {
    // Decision 11: a known answer depends on no inexact value. A known function's parameters,
    // locals and result are integers, and each call of it takes integer literals.
    let mut calls = 0;
    for data in seeds() {
        let g = generate(&data);
        let (_, program) = checked(&g.program);
        let known: Vec<usize> = g
            .known
            .iter()
            .map(|k| {
                program
                    .functions
                    .iter()
                    .position(|f| f.name == k.marker)
                    .expect("a known site should have its function")
            })
            .collect();
        for &f in &known {
            let f = &program.functions[f];
            let types = f.locals.iter().map(|local| local.ty).chain([f.ret]);
            for ty in types {
                assert!(
                    matches!(ty.kind(), TypeKind::I64 | TypeKind::U8),
                    "`{}` holds a {ty:?}",
                    f.name
                );
            }
        }
        // Each site prints its call in a statement of `main`: `print("@k<n> {k<n>(x, y)}")`
        // or `print(k<n>(x, y))`.
        let main = program
            .functions
            .iter()
            .find(|f| f.name == "main")
            .expect("a program should have `main`");
        let lets = Lets::new(&main.locals);
        let printed = main.body.stmts.iter().filter_map(|stmt| match stmt {
            tir::Stmt::Expr(e) => Some(e),
            tir::Stmt::Let { .. } | tir::Stmt::Assign { .. } | tir::Stmt::Append { .. } => None,
        });
        let mut sites = 0;
        for e in printed.chain(main.body.value.as_deref()) {
            let tir::ExprKind::Print { operand, .. } = &e.kind else {
                continue;
            };
            let parts = if let tir::ExprKind::Interpolate { parts, .. } = &operand.kind {
                &parts[..]
            } else {
                std::slice::from_ref(&**operand)
            };
            for part in parts {
                if let tir::ExprKind::Call { func, args, .. } = &part.kind
                    && known.contains(&func.0)
                {
                    sites += 1;
                    for arg in args {
                        assert!(lets.value(arg).is_some(), "a known call takes {arg:?}");
                    }
                }
            }
        }
        assert_eq!(
            sites,
            g.known.len(),
            "each known site should call its function once"
        );
        calls += sites;
    }
    assert!(calls > 0, "no seed calls a known function");
}
