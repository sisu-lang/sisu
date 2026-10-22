//! The coverage test of ADR 0031: programs generated from fixed seeds check clean, and between
//! them hold every variant of the syntax tree's, the typed tree's and the types' enums, every
//! built-in method on each receiver, every prelude function and `f64` constant, each value of
//! the syntax tree's `bool` fields, and the literal forms only the text shows. Each enum is read
//! with `syn`, so a new variant fails the test until a rule writes it. The walkers match without
//! a `_` arm, so a new variant does not compile until they record it.

use std::fmt::Write as _;

use rustc_hash::{FxHashMap, FxHashSet};

use super::scope::{Const, Func, Ty, Var};
use super::{
    BUDGET, Cost, DEPTHS, FUEL, Gen, Generated, MAX_BOUND, MAX_NESTING, MAX_SIZE, Shape, generate,
};
use crate::diagnostic::Severity;
use crate::front_end::front_end;
use crate::types::{Type, TypeKind};
use crate::{ast, lexer, parser, tir};

/// How many seeds, and how many bytes each holds.
const SEEDS: u64 = 64;
const SEED_LEN: usize = 4_096;

/// What no generated program holds, and why.
const EXCLUDED: [(&str, &str); 6] = [
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
        "ast::PatternKind::None",
        "a `None` pattern is an error in every position (E0127)",
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
const REACH: [&str; 17] = [
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
];

/// Whether a program's text holds a form.
type Holds = fn(&str) -> bool;

/// The forms only a program's text shows, each with how to find it.
const TEXT: [(&str, Holds); 7] = [
    ("text: `_` between digits", |text| {
        text.as_bytes()
            .windows(3)
            .any(|w| w[0].is_ascii_digit() && w[1] == b'_' && w[2].is_ascii_digit())
    }),
    ("text: the smallest `i64`", |text| {
        text.contains("-9223372036854775808")
    }),
    ("text: `0x`", |text| text.contains("0x")),
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
    (0..SEEDS).map(|seed| {
        let mut state = seed;
        (0..SEED_LEN / 8)
            .flat_map(|_| splitmix64(&mut state).to_le_bytes())
            .collect()
    })
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The program's syntax tree and typed tree, after asserting it checks without an error.
fn checked(g: &Generated) -> (ast::Program, tir::Program) {
    let (program, diagnostics) = front_end(&g.program);
    let errors: Vec<String> = diagnostics
        .iter()
        .filter(|d| d.severity() == Severity::Error)
        .map(|d| {
            let before = &g.program[..d.span().start()];
            let line = 1 + before.matches('\n').count();
            let column = 1 + before.len() - before.rfind('\n').map_or(0, |at| at + 1);
            format!("{} at {line}:{column}", d.code().as_str())
        })
        .collect();
    let program = program.filter(|_| errors.is_empty()).unwrap_or_else(|| {
        let mut numbered = String::new();
        for (i, line) in g.program.lines().enumerate() {
            writeln!(numbered, "{:>4} {line}", i + 1)
                .expect("writing to a `String` should succeed");
        }
        panic!("a generated program fails to check: {errors:?}\n{numbered}")
    });
    let lexed = lexer::lex(&g.program).expect("a program that checks should lex");
    (parser::parse(lexed).program, program)
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
}

impl Walk {
    fn mark(&mut self, key: &str) {
        if !self.seen.contains(key) {
            self.seen.insert(key.to_string());
        }
    }

    fn program(&mut self, g: &Generated) {
        let (ast, tir) = checked(g);
        self.source.clone_from(&g.program);
        self.lens.clear();
        self.immutable.clear();
        self.later.clear();
        self.some_values.clear();
        self.chains.clear();
        self.adds.clear();
        self.assigns.clear();
        self.ast(&ast);
        self.tir(&tir);
        for (feature, holds) in TEXT {
            if holds(&g.program) {
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
                    self.ast_expr(&c.value);
                }
            }
        }
    }

    fn ast_function(&mut self, f: &ast::Function) {
        self.func = f.name.name.as_str().to_string();
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
        self.mark(&format!("ast::TypeExpr.optional = {}", ty.optional));
        match &ty.kind {
            ast::TypeKind::Name(_) => self.mark("ast::TypeKind::Name"),
            ast::TypeKind::Array(element) => {
                self.mark("ast::TypeKind::Array");
                self.type_expr(element);
            }
        }
    }

    fn ast_block(&mut self, block: &ast::Block) {
        for (i, stmt) in block.stmts.iter().enumerate() {
            let before = i.checked_sub(1).map(|at| &block.stmts[at]);
            if let ast::StmtKind::While { cond, body } = &stmt.kind {
                self.while_bound(before, cond, body);
            }
            if let ast::StmtKind::WhileLet {
                pattern,
                value,
                body,
            } = &stmt.kind
            {
                self.while_let_bound(before, pattern, value, body);
            }
            self.ast_stmt(stmt);
        }
    }

    /// Walks the target of an assignment.
    fn target(&mut self, target: &ast::Expr) {
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
        let variants = self.variants.get(name(receiver)?)?;
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
        let bound = before.and_then(|stmt| {
            if let ast::StmtKind::Let {
                mutable: Some(_),
                name: let_name,
                init,
                ..
            } = &stmt.kind
                && Some(let_name.name.as_str()) == counted
            {
                int(init)
            } else {
                None
            }
        });
        let steps = body.stmts.first().is_some_and(|stmt| {
            matches!(&stmt.kind, ast::StmtKind::CompoundAssign {
                op: ast::BinaryOp::Sub,
                target,
                value,
                ..
            } if counted.is_some() && name(target) == counted && int(value) == Some(1))
        });
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
            ast::StmtKind::While { cond, body } => {
                self.mark("ast::StmtKind::While");
                self.ast_expr(cond);
                self.ast_block(body);
            }
            ast::StmtKind::WhileLet {
                pattern,
                value,
                body,
            } => {
                self.mark("ast::StmtKind::WhileLet");
                self.ast_pattern(pattern);
                self.ast_expr(value);
                self.ast_block(body);
            }
            ast::StmtKind::For { source, body, .. } => self.ast_for(source, body),
            ast::StmtKind::Return(value) => {
                self.mark("ast::StmtKind::Return");
                if let Some(value) = value {
                    self.ast_expr(value);
                }
            }
            ast::StmtKind::Break => self.mark("ast::StmtKind::Break"),
            ast::StmtKind::Continue => self.mark("ast::StmtKind::Continue"),
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
                if *op == ast::BinaryOp::Add && name(target).is_some() {
                    self.adds.insert(stmt.span.start());
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
            ast::ExprKind::Call { callee, args } => {
                self.mark("ast::ExprKind::Call");
                let callee = callee.name.as_str();
                self.mark(&format!("prelude {callee}"));
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
                self.ast_args(args);
            }
            ast::ExprKind::SelfValue => self.mark("ast::ExprKind::SelfValue"),
            ast::ExprKind::NoneLit => self.mark("ast::ExprKind::NoneLit"),
            ast::ExprKind::Field { base, name, safe } => {
                self.mark("ast::ExprKind::Field");
                self.mark(&format!("ast::ExprKind::Field.safe = {safe}"));
                if matches!(&base.kind, ast::ExprKind::Name(n) if n.as_str() == "f64") {
                    self.mark(&format!("f64.{}", name.name.as_str()));
                }
                self.ast_expr(base);
            }
            ast::ExprKind::MethodCall {
                receiver,
                args,
                safe,
                ..
            } => {
                self.mark("ast::ExprKind::MethodCall");
                self.mark(&format!("ast::ExprKind::MethodCall.safe = {safe}"));
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
                self.ast_expr(scrutinee);
                for arm in arms {
                    self.ast_pattern(&arm.pattern);
                    if let Some(guard) = &arm.guard {
                        self.ast_expr(guard);
                    }
                    self.ast_expr(&arm.body);
                }
            }
            ast::ExprKind::Block(block) => {
                self.mark("ast::ExprKind::Block");
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
        for function in &program.functions {
            self.ty(function.ret);
            for local in &function.locals {
                self.ty(local.ty);
            }
            self.tir_block(&function.body);
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
            TypeKind::Class(_) => ("types::TypeKind::Class", None),
            TypeKind::Enum(_) => ("types::TypeKind::Enum", None),
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

    /// Whether a value of `ty` may hold a class object, where `seen` are the enums looked into.
    fn holds_class(&self, ty: Type, seen: &mut Vec<tir::EnumId>) -> bool {
        match ty.kind() {
            TypeKind::Class(_) => true,
            TypeKind::Array(inner) | TypeKind::Optional(inner) => self.holds_class(inner, seen),
            TypeKind::Enum(id) => {
                !seen.contains(&id) && {
                    seen.push(id);
                    self.payloads[id.0]
                        .iter()
                        .any(|field| self.holds_class(*field, seen))
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
            tir::ExprKind::Call { args, .. } => {
                self.mark("tir::ExprKind::Call");
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
                self.some_value(scrutinee);
                self.tir_expr(scrutinee, Parent::Other);
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
            tir::ExprKind::Break => self.mark("tir::ExprKind::Break"),
            tir::ExprKind::Continue => self.mark("tir::ExprKind::Continue"),
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
        TypeKind::Class(_) => "Class",
        TypeKind::Enum(_) => "Enum",
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

/// The name `e` is, if it is one.
fn name(e: &ast::Expr) -> Option<&str> {
    if let ast::ExprKind::Name(name) = &e.kind {
        Some(name.as_str())
    } else {
        None
    }
}

/// Whether `stmt` never finishes: a jump, or an `if` whose every branch ends in one.
fn jumps(stmt: &ast::Stmt) -> bool {
    let ends = |block: &ast::Block| block.stmts.last().is_some_and(jumps);
    if matches!(
        stmt.kind,
        ast::StmtKind::Return(_) | ast::StmtKind::Break | ast::StmtKind::Continue
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
        walk.program(&generate(&data));
    }
    walk
}

#[test]
fn every_generated_program_checks_clean() {
    for data in seeds() {
        checked(&generate(&data));
    }
}

#[test]
fn every_short_input_checks_clean() {
    checked(&generate(&[]));
    for b in 0..=u8::MAX {
        checked(&generate(&[b]));
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
    checked(&past.expect("some seed's program should read past its stdin"));
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
fn sizes_are_counted_and_capped() {
    // 20 strings of 49 chars: 20 elements and 980 chars.
    let strings = |chars| Shape::Array {
        len: 20,
        element: Box::new(Shape::Str(chars)),
    };
    assert_eq!(strings(49).size(), 1_000);
    assert!(strings(49).fits());
    assert!(!strings(50).fits());
    // A chain of enum nodes, each holding the next.
    let chain = |nodes| (1..nodes).fold(Shape::Node(vec![]), |next, _| Shape::Node(vec![next]));
    assert!(chain(1_000).fits());
    assert!(!chain(1_001).fits());
    // A class object holding an array of numbers.
    let class = |len| {
        Shape::Node(vec![Shape::Array {
            len,
            element: Box::new(Shape::Scalar),
        }])
    };
    assert!(class(999).fits());
    assert!(!class(1_000).fits());
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
fn rare_constant_forms_check_clean() {
    // Each input chooses, byte by byte, no class or enum, an item, a constant, its type, no
    // annotation, and then its form, ending once the bytes run out, where every choice is the
    // first.
    for (data, form) in [
        // `-(e)` of the smallest `i64`, which has no negation: `e` alone.
        (
            &[0, 1, 0, 0, 0, 1, 0, 7, 1][..],
            "const c1 = (-9223372036854775808)\n",
        ),
        // `(a && b)` and `(a || b)` of `bool`s.
        (&[0, 1, 0, 3, 0, 3, 1][..], "const c1 = (false && false)\n"),
        (
            &[0, 1, 0, 3, 0, 3, 1, 0, 0, 1][..],
            "const c1 = (false || false)\n",
        ),
    ] {
        let g = generate(data);
        assert!(g.program.starts_with(form), "{}", g.program);
        checked(&g);
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

#[test]
fn the_walker_reports_unbounded_loops_and_stray_indexes() {
    // Indexes past the end, `[e][1]` among them where it is not a later branch of an `if`, and
    // a `while`, a `while let` and a `for` of 9 runs or no bound at all: the walker lists each.
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
}
";
    let mut walk = Walk::default();
    walk.program(&Generated {
        program: program.to_string(),
        rewritten: program.to_string(),
        stdin: String::new(),
        known: Vec::new(),
        cost: 0,
    });
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
    assert_eq!(unbounded, ["a `while`", "a `while let`", "a `for`"]);
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
    for (element, method, size_of) in [
        (Ty::Char, "chars", (|n| n) as fn(u64) -> u64),
        (Ty::U8, "bytes", |n| 4 * n),
        (Ty::String, "words", |n| n + 1),
    ] {
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
    // from none to a few elements' worth, holds no more than that most.
    let string = || Box::new(Ty::String);
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
    ];
    for ty in &types {
        for max in [0, 1, 2, 3, 7, 40] {
            for synth in [false, true].into_iter().filter(|_| ty.least() <= max) {
                for mut state in 0..64 {
                    let data: Vec<u8> = (0..8)
                        .flat_map(|_| splitmix64(&mut state).to_le_bytes())
                        .collect();
                    let mut g = Gen::new(&data);
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
