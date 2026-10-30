use super::*;
use crate::check::check_alone;
use crate::mir::tests::assert_near_linear;
use crate::parser::parse_ok;
use crate::payload_limit;
use crate::tir::BinaryOp;
use inkwell::values::{AnyValue, CallSiteValue, InstructionOpcode, InstructionValue, Operand};
use std::panic::{self, AssertUnwindSafe};

/// Lexes, parses, checks (no errors allowed), builds the mir of and compiles `src`, then
/// verifies the module.
fn compiled<'ctx>(context: &'ctx Context, src: &str) -> Module<'ctx> {
    compiled_with(context, src, false)
}

/// `compiled`, with the RC checks if `rc_checks`.
fn compiled_with<'ctx>(context: &'ctx Context, src: &str, rc_checks: bool) -> Module<'ctx> {
    compiled_from(context, src, rc_checks, |_| {})
}

/// `compiled_with`, after `edit` changes the mir that `build` gives: a test uses it to put in
/// mir that lowering cannot produce.
fn compiled_from<'ctx>(
    context: &'ctx Context,
    src: &str,
    rc_checks: bool,
    edit: impl FnOnce(&mut mir::Program),
) -> Module<'ctx> {
    compiled_over(
        context,
        crate::front_end::PRELUDE_SOURCE,
        src,
        rc_checks,
        edit,
    )
}

/// `compiled_from` over the prelude `prelude`, which the program may call.
fn compiled_over<'ctx>(
    context: &'ctx Context,
    prelude: &str,
    src: &str,
    rc_checks: bool,
    edit: impl FnOnce(&mut mir::Program),
) -> Module<'ctx> {
    let program = parse_ok(src);
    let (program, diagnostics) = crate::check::check(
        &program,
        &crate::front_end::prelude_program(prelude),
        src,
        &[],
    );
    let program = program.unwrap_or_else(|| panic!("{diagnostics:?}"));
    let program = crate::mono::instantiate(program).unwrap_or_else(|found| panic!("{found:?}"));
    let mut mir = mir::build(&program);
    edit(&mut mir);
    let machine = target_machine(OptLevel::O0).expect("the host is a target");
    let module = compile(
        context,
        &program,
        &mir,
        "test.sisu",
        src,
        &machine,
        rc_checks,
    );
    if let Err(e) = module.verify() {
        panic!("{e}\n{}", module.print_to_string());
    }
    module
}

/// A program of enums for `payload_bound_should_cover_the_llvm_layout_of_each_enum`: scalars of
/// each alignment, strings, arrays, classes, optionals and optionals of optionals, enums that
/// hold enums, recursive enums with a box, generic enums and the prelude's `Result`.
const ENUM_LAYOUTS: &str = "class Node {
    v: i64
}

enum Scalars {
    A(u8, i64, f64, bool, char)
    B(string, [i64], Node)
    C(u8?, i64?, string?, char?, bool?)
    D(i64??, [u8]?, Node?)
    E
}

enum Mixed {
    Leaf(Scalars, u8)
    Two(Scalars, Scalars?)
    Lone(Inner, u8)
}

enum Inner {
    P(Scalars?)
}

enum List {
    Nil
    Cons(i64, List)
}

enum Tree {
    Leaf
    Node(Tree, u8, Tree, f64?)
}

enum Shaped {
    No
    Has(string)
}

enum Pair<T> {
    End
    Both(T, T)
}

enum Wrap<T> {
    W(Pair<T>, u8)
}

enum Tags {
    A
    B(u8)
}

fn main() {
    let s = Scalars.E
    let m = Mixed.Leaf(s, 1)
    let i = Inner.P(None)
    let l = List.Cons(1, List.Nil)
    let t = Tree.Node(Tree.Leaf, 1, Tree.Leaf, None)
    let sh = Shaped.No
    let w: Wrap<Pair<u8>> = Wrap.W(Pair.End, 1)
    let r: Result<Scalars, string> = Result.Ok(s)
    let r2: Result<u8, Mixed> = Result.Err(m)
    let o: Option<Tags> = None
    print(i)
    print(l)
    print(t)
    print(sh)
    print(w)
    print(r)
    print(r2)
    print(o)
}
";

/// 300 variants, so that the tag is two bytes.
const MANY_VARIANTS: &str = "enum Many {
    V0
    V1
    V2
    V3
    V4
    V5
    V6
    V7
    V8
    V9
    V10
    V11
    V12
    V13
    V14
    V15
    V16
    V17
    V18
    V19
    V20
    V21
    V22
    V23
    V24
    V25
    V26
    V27
    V28
    V29
    V30
    V31
    V32
    V33
    V34
    V35
    V36
    V37
    V38
    V39
    V40
    V41
    V42
    V43
    V44
    V45
    V46
    V47
    V48
    V49
    V50
    V51
    V52
    V53
    V54
    V55
    V56
    V57
    V58
    V59
    V60
    V61
    V62
    V63
    V64
    V65
    V66
    V67
    V68
    V69
    V70
    V71
    V72
    V73
    V74
    V75
    V76
    V77
    V78
    V79
    V80
    V81
    V82
    V83
    V84
    V85
    V86
    V87
    V88
    V89
    V90
    V91
    V92
    V93
    V94
    V95
    V96
    V97
    V98
    V99
    V100
    V101
    V102
    V103
    V104
    V105
    V106
    V107
    V108
    V109
    V110
    V111
    V112
    V113
    V114
    V115
    V116
    V117
    V118
    V119
    V120
    V121
    V122
    V123
    V124
    V125
    V126
    V127
    V128
    V129
    V130
    V131
    V132
    V133
    V134
    V135
    V136
    V137
    V138
    V139
    V140
    V141
    V142
    V143
    V144
    V145
    V146
    V147
    V148
    V149
    V150
    V151
    V152
    V153
    V154
    V155
    V156
    V157
    V158
    V159
    V160
    V161
    V162
    V163
    V164
    V165
    V166
    V167
    V168
    V169
    V170
    V171
    V172
    V173
    V174
    V175
    V176
    V177
    V178
    V179
    V180
    V181
    V182
    V183
    V184
    V185
    V186
    V187
    V188
    V189
    V190
    V191
    V192
    V193
    V194
    V195
    V196
    V197
    V198
    V199
    V200
    V201
    V202
    V203
    V204
    V205
    V206
    V207
    V208
    V209
    V210
    V211
    V212
    V213
    V214
    V215
    V216
    V217
    V218
    V219
    V220
    V221
    V222
    V223
    V224
    V225
    V226
    V227
    V228
    V229
    V230
    V231
    V232
    V233
    V234
    V235
    V236
    V237
    V238
    V239
    V240
    V241
    V242
    V243
    V244
    V245
    V246
    V247
    V248
    V249
    V250
    V251
    V252
    V253
    V254
    V255
    V256
    V257
    V258
    V259
    V260
    V261
    V262
    V263
    V264
    V265
    V266
    V267
    V268
    V269
    V270
    V271
    V272
    V273
    V274
    V275
    V276
    V277
    V278
    V279
    V280
    V281
    V282
    V283
    V284
    V285
    V286
    V287
    V288
    V289
    V290
    V291
    V292
    V293
    V294
    V295
    V296
    V297
    V298
    V299
}

fn main() {
    print(Many.V0)
}
";

/// The name of an enum, its bound, and its largest payload, value size and alignment in LLVM.
type Compared = (String, payload_limit::Measured, Option<(u64, u64, u64)>);

/// Compiles `src` and returns, for each enum of the program that `payload_limit::measure` bounds,
/// that enum's name, the size of its largest variant and its value size and alignment in LLVM,
/// each `None` when the enum is `Option`-shaped, which has no struct of its own.
fn enum_layouts(src: &str) -> Vec<Compared> {
    let context = Context::create();
    let parsed = parse_ok(src);
    let prelude = crate::front_end::prelude_program(crate::front_end::PRELUDE_SOURCE);
    let (program, diagnostics) = crate::check::check(&parsed, &prelude, src, &[]);
    let program = program.unwrap_or_else(|| panic!("{diagnostics:?}"));
    let program = crate::mono::instantiate(program).unwrap_or_else(|found| panic!("{found:?}"));
    let measured = payload_limit::measure(&program.enums);
    let mir = mir::build(&program);
    let machine = target_machine(OptLevel::O0).expect("the host is a target");
    let module = compile(&context, &program, &mir, "test.sisu", src, &machine, false);
    let data = machine.get_target_data();
    let mut found = Vec::new();
    for (e, bound) in program.enums.iter().zip(measured) {
        let bound = bound.unwrap_or_else(|| panic!("`{}` should have a bound", e.name));
        let value = module.get_struct_type(&format!("sisu.{}", e.name));
        let value = value.expect("every enum should have a named struct type");
        let laid_out = (!value.is_opaque()).then(|| {
            // The size of the largest variant: each payload struct has a body.
            let payload = (e.variants.iter())
                .map(|v| {
                    let name = format!("sisu.{}.{}", e.name, v.name);
                    let payload = module.get_struct_type(&name).expect("a payload struct");
                    data.get_abi_size(&payload)
                })
                .max()
                .unwrap_or(0);
            let (size, align) = (data.get_abi_size(&value), data.get_abi_alignment(&value));
            (payload, size, u64::from(align))
        });
        found.push((e.name.clone(), bound, laid_out));
    }
    found
}

#[test]
fn payload_bound_should_cover_the_llvm_layout_of_each_enum() {
    let programs = [ENUM_LAYOUTS, MANY_VARIANTS];
    let mut compared = Vec::new();
    for src in programs {
        for (name, bound, laid_out) in enum_layouts(src) {
            // An `Option`-shaped enum has the storage of its field: the bound is not checked
            // against a struct. Its holders are, which hold its value.
            let Some((payload, size, align)) = laid_out else {
                continue;
            };
            compared.push(name.clone());
            assert!(
                bound.payload >= payload,
                "{name}: {bound:?} < payload {payload}"
            );
            assert!(bound.value.size >= size, "{name}: {bound:?} < size {size}");
            assert_eq!(
                bound.value.align, align,
                "{name}: the alignment should be exact"
            );
        }
    }
    // Scalars, strings, arrays, classes and optionals; enums in enums; a box; a tag of two
    // bytes; generic instances, nested ones too; and the prelude's `Result`.
    let required = [
        "Scalars",
        "Mixed",
        "List",
        "Tree",
        "Tags",
        "Many",
        "Pair<u8>",
        "Wrap<Pair<u8>>",
        "Pair<Pair<u8>>",
        "Result<Scalars, string>",
        "Result<u8, Mixed>",
    ];
    for name in required {
        assert!(
            compared.iter().any(|n| n == name),
            "{name} was not compared: {compared:?}"
        );
    }
}

#[test]
fn payload_bound_should_be_exact_for_enums_with_no_optional_and_no_shape() {
    let layouts = enum_layouts(ENUM_LAYOUTS);
    let exact = |name: &str| {
        let (_, bound, laid_out) = layouts.iter().find(|(n, ..)| n == name).expect("the enum");
        let (payload, size, _) = laid_out.expect("a struct");
        (bound.payload, bound.value.size, payload, size)
    };
    for name in ["Tags", "List", "Tree"] {
        let (bound_payload, bound_size, payload, size) = exact(name);
        assert_eq!((bound_payload, bound_size), (payload, size), "{name}");
    }
}

const FIB: &str = "fn fib(n: i64) -> i64 {\n    if n < 2 { n } else { fib(n - 1) + fib(n - 2) }\n}\n\nfn main() {\n    print(fib(30))\n}\n";

/// `compiled` verifies each module, so a source that lowers to a broken one fails its row. The
/// panic message names the row.
#[test]
fn compile_should_verify_every_shape_of_control_flow() {
    let rows = [
        (
            "all paths return",
            "fn sign(n: i64) -> i64 {\n    if n < 0 { return -1 } else { return 1 }\n}\nfn main() { print(sign(2)) }",
        ),
        (
            "chain and short circuit",
            "fn main() {\n    print(1 < 2 <= 3)\n    print(true && false || true)\n}",
        ),
        (
            "loop body ending in return",
            "fn f() -> i64 {\n    loop { return 1 }\n    2\n}\nfn main() { print(f()) }",
        ),
        (
            "early return in main",
            "fn main() {\n    if true { return }\n    print(1)\n}",
        ),
        (
            "discarded body value",
            "fn g() { 1 }\nfn main() {\n    g()\n    if true { 1 } else { 2 }\n}",
        ),
        (
            "else if with short circuit in branch",
            "fn f(a: i64) -> i64 {\n    if a < 0 { -1 } else if a == 0 && true { 0 } else { 1 }\n}\nfn main() { print(f(2)) }",
        ),
    ];
    for (name, src) in rows {
        let context = Context::create();
        panic::catch_unwind(AssertUnwindSafe(|| compiled(&context, src)))
            .unwrap_or_else(|_| panic!("row {name:?} should compile to a verified module"));
    }
}

/// An `if` with one returning branch: that branch returns on its own, and the block after the
/// `if` is reached only from the other.
#[test]
fn verifies_one_returning_branch() {
    let context = Context::create();
    let module = compiled(
        &context,
        "fn a(c: bool) -> i64 {\n    if c { return 1 } else { 2 }\n}\nfn b(c: bool) -> i64 {\n    if c { 1 } else { return 2 }\n}\nfn main() {\n    print(a(true))\n}",
    );
    let ir = module.print_to_string().to_string();
    for name in ["a", "b"] {
        let body = function_body(&ir, &format!("define internal i64 @sisu.{name}("));
        assert_eq!(body.matches("\n  ret i64 ").count(), 2, "{body}");
        let preds: Vec<_> = body
            .lines()
            .filter_map(|l| l.split_once("; preds = ").map(|(_, preds)| preds))
            .collect();
        assert!(!preds.is_empty(), "{body}");
        assert!(preds.iter().all(|p| !p.contains(',')), "{body}");
    }
}

#[test]
fn print_bool_passes_a_zero_extended_i1() {
    let context = Context::create();
    let ir = compiled(&context, "fn main() { print(true) }")
        .print_to_string()
        .to_string();
    for wanted in [
        "declare void @sisu_print_bool(i1 zeroext)",
        "call void @sisu_print_bool(i1 zeroext",
    ] {
        assert!(ir.contains(wanted), "missing {wanted:?} in\n{ir}");
    }
}

/// `eprint` calls the runtime's standard error functions, one for each operand type, and
/// `print` still calls the standard output ones.
#[test]
fn eprint_calls_the_stderr_functions() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "fn main() {\n    eprint(1)\n    eprint(true)\n    eprint('a')\n    eprint(\"s\")\n    print(2)\n}",
    )
    .print_to_string()
    .to_string();
    for wanted in [
        "call void @sisu_eprint_int(",
        "call void @sisu_eprint_bool(i1 zeroext",
        "call void @sisu_eprint_char(",
        "call void @sisu_eprint_str(",
        "call void @sisu_print_int(",
    ] {
        assert!(ir.contains(wanted), "missing {wanted:?} in\n{ir}");
    }
    assert!(!ir.contains("call void @sisu_print_str("), "{ir}");
}

/// Interpolation passes a `bool` to the runtime as `print` does.
#[test]
fn write_bool_passes_a_zero_extended_i1() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "fn f(b: bool) -> string {\n    \"{b}\"\n}\nfn main() {}",
    )
    .print_to_string()
    .to_string();
    for wanted in [
        "declare i64 @sisu_write_bool(i1 zeroext, ptr)",
        "call i64 @sisu_write_bool(i1 zeroext",
    ] {
        assert!(ir.contains(wanted), "missing {wanted:?} in\n{ir}");
    }
}

/// The precision and the alignment cross to the runtime as a C `u8`, which the caller extends
/// with zeros, as for a `bool`; a `char` is already 32 bits and gets no extension.
#[test]
fn format_specs_pass_zero_extended_i8s() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "fn f(x: f64, c: char) -> string {\n    \"{x:>8.2}{c}\"\n}\nfn main() {}",
    )
    .print_to_string()
    .to_string();
    for wanted in [
        "declare i64 @sisu_write_f64_fixed(double, i8 zeroext, ptr)",
        "call i64 @sisu_write_f64_fixed(double %",
        ", i8 zeroext 2, ptr",
        "declare i64 @sisu_pad(ptr, i64, i64, i8 zeroext)",
        ", i64 8, i8 zeroext 1)",
        "declare i64 @sisu_write_char(i32, ptr)",
    ] {
        assert!(ir.contains(wanted), "missing {wanted:?} in\n{ir}");
    }
}

#[test]
fn arithmetic_calls_overflow_intrinsics() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "fn f(a: i64, b: i64) -> i64 { a * b - a / b }\n\nfn main() {\n    print(-f(1, 2))\n}",
    )
    .print_to_string()
    .to_string();
    for wanted in [
        "@llvm.smul.with.overflow.i64",
        "@llvm.ssub.with.overflow.i64",
        "sdiv",
        "call void @sisu_panic",
    ] {
        assert!(ir.contains(wanted), "missing {wanted:?} in\n{ir}");
    }
    // `a * b` and `a * b - a / b` start at column 31, `a / b` at 39, `-f(1, 2)` at 4:11.
    for message in [
        "test.sisu:1:31: integer overflow",
        "test.sisu:1:39: division by zero",
        "test.sisu:1:39: integer overflow",
        "test.sisu:4:11: integer overflow",
    ] {
        assert_panics_with(&ir, message);
    }
}

#[test]
fn division_guards_precede_the_division() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "fn d(a: i64, b: i64) -> i64 { a / b }\nfn r(a: i64, b: i64) -> i64 { a % b }\nfn main() {}",
    )
    .print_to_string()
    .to_string();
    for (name, op) in [("d", "sdiv"), ("r", "srem")] {
        let header = format!("define internal i64 @sisu.{name}(");
        let body = function_body(&ir, &header);
        let lines: Vec<&str> = body.lines().map(str::trim).collect();
        let find = |wanted: &dyn Fn(&str) -> bool, what: &str| {
            instruction(&lines, wanted).unwrap_or_else(|| panic!("no {what} in\n{body}"))
        };
        let (op_at, _, op_text) = find(&|t| t.starts_with(&format!("{op} i64 ")), op);
        let (a, b) = op_text[op.len() + " i64 ".len()..]
            .split_once(", ")
            .unwrap_or_else(|| panic!("two operands in {op_text:?}"));
        // The zero guard tests the divisor; the `MIN / -1` guard tests both operands.
        let zero_test = format!("icmp eq i64 {b}, 0");
        let min_test = format!("icmp eq i64 {a}, -9223372036854775808");
        let neg_test = format!("icmp eq i64 {b}, -1");
        let (zero_at, zero, _) = find(&|t| t == zero_test, &zero_test);
        let (min_at, min, _) = find(&|t| t == min_test, &min_test);
        let (neg_at, neg, _) = find(&|t| t == neg_test, &neg_test);
        let (and_at, overflow, _) = find(
            &|t| t == format!("and i1 {min}, {neg}") || t == format!("and i1 {neg}, {min}"),
            "the `and` of the `MIN / -1` tests",
        );
        // Each guard branches on its own test: true to a panic block, false on to `ok`.
        let branch = |cond: &str| {
            let at = lines
                .iter()
                .position(|l| l.starts_with(&format!("br i1 {cond}, ")))
                .unwrap_or_else(|| panic!("no branch on {cond} in\n{body}"));
            let labels: Vec<_> = lines[at].split("label %").skip(1).collect();
            assert!(
                matches!(labels.as_slice(), [t, f] if t.starts_with("panic") && f.starts_with("ok")),
                "{}",
                lines[at]
            );
            at
        };
        // The zero guard, then the `MIN / -1` guard, then the operation itself.
        let order = [
            zero_at,
            branch(zero),
            min_at.min(neg_at),
            min_at.max(neg_at),
            and_at,
            branch(overflow),
            op_at,
        ];
        assert!(order.is_sorted(), "{order:?} in\n{body}");
        assert_eq!(
            lines.iter().filter(|l| l.starts_with("br i1 ")).count(),
            2,
            "{body}"
        );
    }
    for message in [
        "test.sisu:1:31: division by zero",
        "test.sisu:1:31: integer overflow",
        "test.sisu:2:31: division by zero",
        "test.sisu:2:31: integer overflow",
    ] {
        assert_panics_with(&ir, message);
    }
}

/// The body of the function `header` starts: up to its closing `}`.
fn function_body<'a>(ir: &'a str, header: &str) -> &'a str {
    ir.split_once(header)
        .and_then(|(_, rest)| rest.split("\n}").next())
        .unwrap_or_else(|| panic!("no {header} in\n{ir}"))
}

/// The first instruction `<value> = <text>` in `lines` whose text is `wanted`: its line
/// index, the value it defines and its text.
fn instruction<'a>(
    lines: &[&'a str],
    wanted: &dyn Fn(&str) -> bool,
) -> Option<(usize, &'a str, &'a str)> {
    lines.iter().enumerate().find_map(|(i, line)| {
        let (value, text) = line.split_once(" = ")?;
        wanted(text).then_some((i, value, text))
    })
}

/// Asserts that `ir` has a `sisu_panic` call that passes `message` and its byte length.
fn assert_panics_with(ir: &str, message: &str) {
    let constant = format!("c\"{message}\\00\"");
    let global = ir
        .lines()
        .find(|l| l.ends_with(&constant) || l.contains(&format!("{constant},")))
        .and_then(|l| l.split(" = ").next())
        .unwrap_or_else(|| panic!("no constant {constant} in\n{ir}"));
    let call = format!("@sisu_panic(ptr {global}, i64 {})", message.len());
    assert!(ir.contains(&call), "missing {call:?} in\n{ir}");
}

#[test]
fn class_layout_comes_from_the_data_layout() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "class Q {}\nclass P {\n    x: i64\n    mut y: bool\n    q: Q\n}\nfn main() {\n    let p = P(x: 1, y: true, q: Q())\n    print(p.x)\n}",
    )
    .print_to_string()
    .to_string();
    for wanted in [
        "%sisu.P = type { i64, i64, ptr, i1 }",
        "call ptr @sisu_alloc(i64 32, i64 8)",
    ] {
        assert!(ir.contains(wanted), "missing {wanted:?} in\n{ir}");
    }
}

/// Each constructor argument is stored through the struct element of its field, after the
/// fields are sorted by alignment: `S`'s `b` moves ahead of `a`.
#[test]
fn class_fields_follow_their_elements() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "class S {\n    a: bool\n    b: i64\n    c: bool\n}\nfn main() {\n    let s = S(a: true, b: 7, c: false)\n    print(s.b)\n}",
    )
    .print_to_string()
    .to_string();
    for wanted in [
        "%sisu.S = type { i64, i64, i1, i1 }",
        "call ptr @sisu_alloc(i64 24, i64 8)",
    ] {
        assert!(ir.contains(wanted), "missing {wanted:?} in\n{ir}");
    }
    let main = function_body(&ir, "define internal void @sisu.main()");
    let lines: Vec<&str> = main.lines().map(str::trim).collect();
    // The struct element each field GEP in `main` reaches, by the value it defines.
    let elements: FxHashMap<&str, &str> = lines
        .iter()
        .filter_map(|line| {
            let (value, text) = line.split_once(" = ")?;
            let (_, operands) = text.split_once("%sisu.S, ptr ")?;
            let (_, element) = operands.rsplit_once(", i32 0, i32 ")?;
            Some((value, element))
        })
        .collect();
    // Each value goes to the element its field moved to: `b` first, then `a` and `c`.
    for (stored, element) in [("i64 7", "1"), ("i1 true", "2"), ("i1 false", "3")] {
        let store = format!("store {stored}, ptr ");
        let field = lines
            .iter()
            .find_map(|l| l.strip_prefix(&store)?.split(',').next())
            .unwrap_or_else(|| panic!("no {store:?} in\n{main}"));
        assert_eq!(elements.get(field), Some(&element), "{stored} in\n{main}");
    }
}

#[test]
fn empty_class_allocates_its_count() {
    let context = Context::create();
    let ir = compiled(&context, "class E {}\nfn main() { E() }")
        .print_to_string()
        .to_string();
    let lines: Vec<&str> = ir.lines().map(str::trim).collect();
    let (_, object, _) = instruction(&lines, &|t| t == "call ptr @sisu_alloc(i64 8, i64 8)")
        .unwrap_or_else(|| panic!("no 8-byte allocation in\n{ir}"));
    let count = format!("store i64 1, ptr {object},");
    assert!(
        lines.iter().any(|l| l.starts_with(&count)),
        "missing {count:?} in\n{ir}"
    );
}

#[test]
fn method_symbols() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "class P {\n    x: i64\n\n    fn m(self) { print(self.x) }\n}\nfn main() { P(x: 1).m() }",
    )
    .print_to_string()
    .to_string();
    assert!(ir.contains("define internal void @sisu.P.m(ptr"), "{ir}");
}

#[test]
fn verifies_field_assignment_through_a_call() {
    compiled(
        &Context::create(),
        "class C {\n    mut n: i64\n}\nfn make() -> C { C(n: 0) }\nfn main() {\n    make().n = 1\n}",
    );
}

#[test]
fn helpers_are_generated() {
    let context = Context::create();
    let ir = compiled(&context, "class P {}\nfn main() { P() }")
        .print_to_string()
        .to_string();
    for wanted in [
        "define internal void @sisu_rc.retain(ptr",
        "define internal void @sisu_rc.release.P(ptr",
        "define internal void @sisu_rc.drop.P(ptr",
    ] {
        assert!(ir.contains(wanted), "missing {wanted:?} in\n{ir}");
    }
}

/// `T` is deep through `l`, which is not its chain field (`r`), and has a `string` field too.
#[test]
fn class_drop_should_call_no_drop_or_release_of_its_own_class() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "class Node {\n    value: i64\n    next: Node?\n}\nclass T {\n    l: T?\n    r: T?\n    name: string\n}\nfn main() {}",
    )
    .print_to_string()
    .to_string();
    for name in ["Node", "T"] {
        let body = function_body(
            &ir,
            &format!("define internal void @sisu_rc.drop.{name}(ptr"),
        );
        assert!(body.contains("phi ptr"), "no loop in\n{body}");
        assert_eq!(body.matches("@sisu_free(").count(), 1, "{body}");
        for unwanted in [
            format!("@sisu_rc.drop.{name}("),
            format!("@sisu_rc.release.{name}("),
        ] {
            assert!(!body.contains(&unwanted), "{unwanted:?} in\n{body}");
        }
    }
    let tree = function_body(&ir, "define internal void @sisu_rc.drop.T(ptr");
    assert!(tree.contains("@sisu_rc.release.string("), "{tree}");
}

/// `Node` holds itself in a `Node??`, and `A` and `B` hold each other in a `B??` and an `A???`,
/// so each has a loop, not a call of the drop of the class that its field holds.
#[test]
fn class_drop_should_loop_on_fields_with_more_than_one_optional() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "class Node {\n    next: Node??\n}\nclass A {\n    b: B??\n}\nclass B {\n    a: A???\n}\nfn main() {}",
    )
    .print_to_string()
    .to_string();
    let node = function_body(&ir, "define internal void @sisu_rc.drop.Node(ptr");
    assert!(node.contains("phi ptr"), "no loop in\n{node}");
    // The drop tests the flag of the outer `?` before the link. The `true` branch of the flag
    // takes the payload out and tests it for null.
    let lines: Vec<&str> = node.lines().map(str::trim).collect();
    let flag = lines
        .iter()
        .position(|line| line.contains("= extractvalue { i1, ptr }") && line.ends_with(", 0"))
        .unwrap_or_else(|| panic!("no flag test in\n{node}"));
    let branch = lines[flag + 1]
        .strip_prefix("br i1 ")
        .unwrap_or_else(|| panic!("no branch on the flag in\n{node}"));
    let some = branch
        .split("label %")
        .nth(1)
        .and_then(|label| label.split(',').next())
        .unwrap_or_else(|| panic!("no target in {branch:?}"));
    let some = node
        .split_once(&format!("\n{some}:"))
        .and_then(|(_, rest)| rest.split("\n\n").next())
        .unwrap_or_else(|| panic!("no block {some} in\n{node}"));
    assert!(
        some.contains("= extractvalue { i1, ptr }") && some.contains("icmp eq ptr"),
        "no payload null test in\n{some}"
    );
    let a = function_body(&ir, "define internal void @sisu_rc.drop.A(ptr");
    assert!(a.contains("@sisu_rc.drain.A("), "{a}");
    let drain = function_body(&ir, "define internal void @sisu_rc.drain.A(ptr");
    assert_eq!(drain.matches("@sisu_free(").count(), 2, "{drain}");
    for body in [&node, &a, &drain] {
        for unwanted in ["@sisu_rc.drop.", "@sisu_rc.release."] {
            assert!(!body.contains(unwanted), "{unwanted:?} in\n{body}");
        }
    }
}

/// `A` and `B` hold each other, so they are one cycle with one `drain` helper. `H` holds `A`
/// but is in no cycle, so its drop releases `A` as before.
#[test]
fn class_drop_should_call_no_drop_or_release_of_the_other_class_in_its_cycle() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "class A {\n    b: B?\n}\nclass B {\n    a: A?\n    c: C\n    name: string\n}\nclass C {}\nclass H {\n    a: A?\n}\nfn main() {}",
    )
    .print_to_string()
    .to_string();
    let drop = |name| {
        function_body(
            &ir,
            &format!("define internal void @sisu_rc.drop.{name}(ptr"),
        )
    };
    for name in ["A", "B"] {
        let body = drop(name);
        assert!(body.contains("@sisu_rc.drain.A("), "{body}");
        for unwanted in ["@sisu_free(", "@sisu_rc.drop.", "@sisu_rc.release."] {
            assert!(!body.contains(unwanted), "{unwanted:?} in\n{body}");
        }
    }
    let drain = function_body(&ir, "define internal void @sisu_rc.drain.A(ptr");
    assert_eq!(drain.matches("@sisu_free(").count(), 2, "{drain}");
    for wanted in ["@sisu_rc.release.string(", "@sisu_rc.release.C("] {
        assert!(drain.contains(wanted), "no {wanted:?} in\n{drain}");
    }
    for unwanted in [
        "@sisu_rc.drop.",
        "@sisu_rc.release.A(",
        "@sisu_rc.release.B(",
    ] {
        assert!(!drain.contains(unwanted), "{unwanted:?} in\n{drain}");
    }
    assert!(drop("H").contains("@sisu_rc.release.A("));
}

/// The helpers of an enum and the drop of a class hand an inline enum field to its own helper by
/// address. A copy in a stack slot makes each frame as large as the field, so a chain of
/// nested enums needs stack quadratic in its depth.
#[test]
fn helpers_should_pass_inline_enum_fields_by_address() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "enum Inner {\n    P(string, i64, i64, i64)\n    Q\n}\nenum Outer {\n    W(Inner)\n    V\n}\nenum Tree {\n    Leaf\n    Node(Tree, Inner)\n}\nclass Holder {\n    inner: Inner\n}\nfn same(a: Outer, b: Outer, e: Tree, f: Tree) -> bool { a == b && e == f }\nfn keep(o: Outer) -> [Outer] { [o, o] }\nfn drop(o: Outer, t: Tree, h: Holder) {\n    let k = o\n    let u = t\n    let v = h\n}\nfn main() {}",
    )
    .print_to_string()
    .to_string();
    for (header, callee) in [
        (
            "define internal i1 @sisu_rc.eq.Outer(ptr",
            "@sisu_rc.eq.Inner(",
        ),
        (
            "define internal void @sisu_rc.release.Outer(ptr",
            "@sisu_rc.release.Inner(",
        ),
        (
            "define internal void @sisu_rc.retain.Outer(ptr",
            "@sisu_rc.retain.Inner(",
        ),
        (
            "define internal void @sisu_rc.drop.Holder(ptr",
            "@sisu_rc.release.Inner(",
        ),
        (
            "define internal void @sisu_rc.walk.Tree(ptr",
            "@sisu_rc.release.Inner(",
        ),
        (
            "define internal i1 @sisu_rc.eq_component.Tree(i32",
            "@sisu_rc.eq.Inner(",
        ),
    ] {
        let body = function_body(&ir, header);
        assert!(body.contains(callee), "{header}: no {callee}\n{body}");
        assert!(!body.contains("load %sisu.Inner"), "{header}:\n{body}");
        assert!(!body.contains("alloca %sisu.Inner"), "{header}:\n{body}");
    }
}

/// The operands of the one `sisu_free` call in `body`, as `(pointer, size, align)`.
fn free_operands(body: &str) -> (String, String, String) {
    let line = body
        .lines()
        .find_map(|l| l.trim().strip_prefix("call void @sisu_free(ptr "))
        .unwrap_or_else(|| panic!("no free in\n{body}"));
    let operands: Vec<&str> = line.trim_end_matches(')').split(", i64 ").collect();
    let [pointer, size, align] = operands[..] else {
        panic!("free operands in {line:?}")
    };
    (pointer.into(), size.into(), align.into())
}

/// The text of the instruction `<value> = <text>` in `body`.
fn defined<'a>(body: &'a str, value: &str) -> &'a str {
    body.lines()
        .find_map(|l| l.trim().strip_prefix(&format!("{value} = ")))
        .unwrap_or_else(|| panic!("no {value} in\n{body}"))
}

/// A drop frees a block with the size and alignment it was allocated with. `sisu_free` hands
/// both to the allocator, which here ignores them, so Valgrind and the RC checks cannot notice
/// a wrong value.
#[test]
fn drop_should_free_with_the_allocated_layout() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "class Q {}\nclass P {\n    x: i64\n    mut y: bool\n    q: Q\n}\nclass S {\n    a: bool\n    b: i64\n    c: bool\n}\nfn main() {\n    let p = P(x: 1, y: true, q: Q())\n    let s = S(a: true, b: 7, c: false)\n    print(s.b)\n    let a = [1, 2, 3]\n    let b = [true, false]\n    let c: [i64?] = [None, 1]\n    print(p.x + a[0])\n}",
    )
    .print_to_string()
    .to_string();
    // The classes: `P` is 32 bytes aligned to 8 (see `class_layout_comes_from_the_data_layout`),
    // and `S` 24, with its fields sorted (see `class_fields_follow_their_elements`); the block
    // is the drop function's own argument.
    for (name, bytes) in [("P", "32"), ("S", "24")] {
        assert!(
            ir.contains(&format!("call ptr @sisu_alloc(i64 {bytes}, i64 8)")),
            "{name}: no alloc of {bytes} bytes in\n{ir}"
        );
        let class = function_body(
            &ir,
            &format!("define internal void @sisu_rc.drop.{name}(ptr"),
        );
        let (pointer, size, align) = free_operands(class);
        assert!(
            class.trim_start().starts_with(&format!("{pointer})")),
            "{class}"
        );
        assert_eq!((size.as_str(), align.as_str()), (bytes, "8"), "{class}");
    }
    // An array: the 24-byte header plus `cap` elements, aligned to 8; the element sizes are
    // those of `literal_should_allocate_header_and_elements`, recomputed from the capacity. The
    // literal of each type in `main` allocates `24 + len * element_size` bytes, the size the drop
    // frees.
    for (helper, element_size, len) in [("[i64]", 8, 3), ("[bool]", 1, 2), ("[i64?]", 16, 2)] {
        let allocated = 24 + len * element_size;
        assert!(
            ir.contains(&format!("call ptr @sisu_alloc(i64 {allocated}, i64 8)")),
            "{helper}: no alloc of {allocated} bytes in\n{ir}"
        );
        let body = function_body(
            &ir,
            &format!("define internal void @\"sisu_rc.drop.{helper}\"(ptr"),
        );
        let (pointer, size, align) = free_operands(body);
        assert!(
            body.trim_start().starts_with(&format!("{pointer})")),
            "{body}"
        );
        assert_eq!(align, "8", "{body}");
        let elements = defined(body, &size)
            .strip_prefix("add nuw i64 ")
            .and_then(|operands| operands.strip_suffix(", 24"))
            .unwrap_or_else(|| panic!("size {size} is not elements + 24 in\n{body}"));
        let length = defined(body, elements)
            .strip_prefix("mul nuw i64 ")
            .and_then(|operands| operands.strip_suffix(&format!(", {element_size}")))
            .unwrap_or_else(|| panic!("{elements} is not cap * {element_size} in\n{body}"));
        assert!(
            defined(body, length).starts_with("load i64, ptr "),
            "{body}"
        );
    }
}

/// A block can have room beyond its length. It is freed with the size it was allocated with.
/// So the drop sizes the block from the header's capacity, which is field 2, not field 1.
#[test]
fn array_drop_should_free_the_size_of_its_capacity() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "fn main() {\n    let a = [1, 2, 3]\n    print(a[0])\n}",
    )
    .print_to_string()
    .to_string();
    let body = function_body(&ir, "define internal void @\"sisu_rc.drop.[i64]\"(ptr");
    let (block, size, _) = free_operands(body);
    let elements = defined(body, &size)
        .strip_prefix("add nuw i64 ")
        .and_then(|operands| operands.split(", ").next())
        .unwrap_or_else(|| panic!("size {size} is not an add in\n{body}"));
    let capacity = defined(body, elements)
        .strip_prefix("mul nuw i64 ")
        .and_then(|operands| operands.split(", ").next())
        .unwrap_or_else(|| panic!("{elements} is not a multiplication in\n{body}"));
    let field = defined(body, capacity)
        .strip_prefix("load i64, ptr ")
        .and_then(|operands| operands.split(',').next())
        .unwrap_or_else(|| panic!("{capacity} is not a load in\n{body}"));
    assert!(
        defined(body, field).ends_with(&format!("{{ i64, i64, i64 }}, ptr {block}, i32 0, i32 2")),
        "{field} is not field 2 of the header in\n{body}"
    );
}

/// A new block has room for exactly its length: `[v; n]` stores `n` into both the length and
/// the capacity, whatever `n` is.
#[test]
fn alloc_array_should_set_the_capacity_to_the_length() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "fn make(n: i64) -> [i64] { [0; n] }\nfn main() {}",
    )
    .print_to_string()
    .to_string();
    let body = function_body(&ir, "define internal ptr @sisu.make(i64");
    // The pointer that the one allocation returns.
    let block = body
        .lines()
        .find_map(|l| {
            let (value, text) = l.trim().split_once(" = ")?;
            text.starts_with("call ptr @sisu_alloc(").then_some(value)
        })
        .unwrap_or_else(|| panic!("no allocation in\n{body}"));
    // The value stored into field `index` of an array header.
    let stored = |index: u32| {
        let field = body
            .lines()
            .find_map(|l| {
                let (value, text) = l.trim().split_once(" = ")?;
                text.contains(&format!(
                    "{{ i64, i64, i64 }}, ptr {block}, i32 0, i32 {index}"
                ))
                .then_some(value)
            })
            .unwrap_or_else(|| panic!("no field {index} in\n{body}"));
        body.lines()
            .find_map(|l| {
                let operands = l.trim().strip_prefix("store i64 ")?;
                let (value, rest) = operands.split_once(", ptr ")?;
                rest.starts_with(field).then_some(value)
            })
            .unwrap_or_else(|| panic!("no store to field {index} in\n{body}"))
    };
    assert_eq!(stored(2), stored(1), "{body}");
}

const STRING_PROGRAM: &str =
    "fn main() {\n    let mut s = \"a\" + \"b\"\n    s += \"c\"\n    print(s)\n}";

/// Asserts that `size`, a value in `body`, is a string block's header plus its capacity: `add
/// nuw i64 24, <cap>`, with `<cap>` loaded from the capacity field of `block`.
fn assert_header_plus_capacity(body: &str, block: &str, size: &str) {
    // The value defined by the instruction `<value> = <text>`.
    let text = |value: &str| {
        body.lines()
            .find_map(|l| l.trim().strip_prefix(&format!("{value} = ")))
            .unwrap_or_else(|| panic!("no {value} in\n{body}"))
    };
    let cap = text(size)
        .strip_prefix("add nuw i64 24, ")
        .unwrap_or_else(|| panic!("size {size} is not 24 + cap in\n{body}"));
    let field = text(cap)
        .strip_prefix("load i64, ptr ")
        .and_then(|operands| operands.strip_suffix(", align 8"))
        .unwrap_or_else(|| panic!("{cap} is not a load in\n{body}"));
    assert!(
        text(field).ends_with(&format!("ptr {block}, i32 0, i32 {}", layout::CAP)),
        "{field} is not {block}'s capacity field in\n{body}"
    );
}

/// A string's drop frees its header plus its capacity, the size of the last `sisu_alloc` or
/// `sisu_realloc`. Valgrind and the RC checks cannot see a wrong size.
#[test]
fn string_drop_frees_header_and_capacity() {
    let context = Context::create();
    let ir = compiled(&context, STRING_PROGRAM)
        .print_to_string()
        .to_string();
    let body = function_body(&ir, "define internal void @sisu_rc.drop.string(ptr");
    let line = body
        .lines()
        .find_map(|l| l.trim().strip_prefix("call void @sisu_free(ptr "))
        .unwrap_or_else(|| panic!("no free in\n{body}"));
    let operands: Vec<&str> = line.trim_end_matches(')').split(", i64 ").collect();
    let [pointer, size, align] = operands[..] else {
        panic!("free operands in {line:?}")
    };
    assert!(
        body.trim_start().starts_with(&format!("{pointer})")),
        "{body}"
    );
    assert_eq!(align, "8", "{body}");
    assert_header_plus_capacity(body, pointer, size);
}

/// A string append that outgrows its block reallocates it from the size it was allocated with:
/// the header plus the old capacity, not the new one.
#[test]
fn string_grow_reallocates_from_the_old_capacity() {
    let context = Context::create();
    let ir = compiled(&context, STRING_PROGRAM)
        .print_to_string()
        .to_string();
    let body = function_body(&ir, "define internal void @sisu.main(");
    let line = body
        .split_once("call ptr @sisu_realloc(ptr ")
        .and_then(|(_, rest)| rest.lines().next())
        .unwrap_or_else(|| panic!("no realloc in\n{body}"));
    let operands: Vec<&str> = line.trim_end_matches(')').split(", i64 ").collect();
    let [block, old_size, align, _] = operands[..] else {
        panic!("realloc operands in {line:?}")
    };
    assert_eq!(align, "8", "{body}");
    assert_header_plus_capacity(body, block, old_size);
}

/// A `push` that cannot fit another element panics at the call, not at its argument. The call
/// starts at `xs` on line 3, column 5. Its argument starts at column 13.
#[test]
fn push_should_report_array_is_too_large_at_the_call() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "fn main() {\n    let mut xs = [1]\n    xs.push(2)\n    print(xs.len())\n}",
    )
    .print_to_string()
    .to_string();
    assert_panics_with(&ir, "test.sisu:3:5: array is too large");
}

/// A full block grows to the larger of one more element and its double capacity. The double is
/// capped at the most elements a block can have. So the IR takes `umin` of the double and that
/// count, then `umax` with `len + 1`. An `[i64]` block has a 24-byte header and 8-byte elements.
/// Its size stays at most `i64::MAX` rounded down to the alignment of 8.
#[test]
fn push_should_grow_to_the_larger_of_one_more_and_the_capped_double() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "fn main() {\n    let mut xs = [1]\n    xs.push(2)\n    print(xs.len())\n}",
    )
    .print_to_string()
    .to_string();
    let body = function_body(&ir, "define internal void @sisu.main(");
    let lines: Vec<&str> = body.lines().map(str::trim).collect();
    let largest = (9_223_372_036_854_775_807_u64 - 8 + 1 - 24) / 8;
    let (capped_at, capped, text) =
        instruction(&lines, &|t| t.starts_with("call i64 @llvm.umin.i64(i64 "))
            .unwrap_or_else(|| panic!("no umin in\n{body}"));
    let operands = text
        .strip_prefix("call i64 @llvm.umin.i64(i64 ")
        .and_then(|rest| rest.strip_suffix(')'))
        .and_then(|rest| rest.split_once(", i64 "))
        .unwrap_or_else(|| panic!("umin operands in {text:?}"));
    assert_eq!(operands.1, largest.to_string(), "{body}");
    let Some((_, double, _)) = instruction(&lines, &|t| {
        t.starts_with("mul nuw i64 ") && t.ends_with(", 2")
    }) else {
        panic!("no double in\n{body}")
    };
    assert_eq!(operands.0, double, "{body}");
    let (extreme_at, _, text) = instruction(&lines, &|t| t.starts_with("call i64 @llvm.umax.i64("))
        .unwrap_or_else(|| panic!("no umax in\n{body}"));
    let (min_operand, needed) = text
        .strip_prefix("call i64 @llvm.umax.i64(i64 ")
        .and_then(|rest| rest.strip_suffix(')'))
        .and_then(|rest| rest.split_once(", i64 "))
        .unwrap_or_else(|| panic!("umax operands in {text:?}"));
    assert_eq!(min_operand, capped, "{body}");
    assert!(capped_at < extreme_at, "{body}");
    let defines = |line: &&str| {
        line.strip_prefix(needed)
            .and_then(|rest| rest.strip_prefix(" = add nuw i64 "))
            .is_some_and(|rest| rest.ends_with(", 1"))
    };
    assert!(
        lines.iter().any(defines),
        "{needed} is not a length plus 1 in\n{body}"
    );
}

/// A class gets no generated `eq` helper: it compares by identity. A class with a written `Eq`
/// gets the helper that calls its `eq`, which the helper of an array of it calls.
#[test]
fn class_should_get_no_eq_helper_and_its_array_should_call_the_written_eq() {
    let context = Context::create();
    let plain = compiled(
        &context,
        "class C {\n    v: i64\n}\n\nfn main() {\n    let c = C(v: 1)\n    print(c is c)\n}\n",
    );
    assert!(plain.get_function("sisu_rc.eq.C").is_none());
    let written = compiled(
        &context,
        "class C: Eq {\n    v: i64\n    fn eq(self, other: Self) -> bool { self.v == other.v }\n}\n\nfn main() {\n    print([C(v: 1)] == [C(v: 2)])\n}\n",
    )
    .print_to_string()
    .to_string();
    let array = function_body(&written, "define internal i1 @\"sisu_rc.eq.[C]\"(ptr");
    assert!(array.contains("call i1 @sisu_rc.eq.C("), "{array}");
    let helper = function_body(&written, "define internal i1 @sisu_rc.eq.C(ptr");
    assert!(helper.contains("call i1 @sisu.C.eq("), "{helper}");
}

/// `x == None` on a counted optional tests whether it holds a value, so it needs no `eq` helper,
/// which a class has none of: the pointer is tested against `null`.
#[test]
fn comparing_an_optional_class_with_none_should_test_the_pointer() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "class Node {\n    next: Node?\n}\nfn present(n: Node) -> bool { n.next != None }\nfn absent(n: Node) -> bool { None == n.next }\nfn main() {}",
    )
    .print_to_string()
    .to_string();
    assert!(!ir.contains("sisu_rc.eq."), "{ir}");
    // `!=` is the test itself, and `==` is its negation.
    for (name, negated) in [("present", false), ("absent", true)] {
        let body = function_body(&ir, &format!("define internal i1 @sisu.{name}(ptr"));
        let test = body
            .lines()
            .find(|line| line.contains("icmp ne ptr") && line.ends_with(", null"));
        let result = test
            .and_then(|line| line.trim().split_once(" = "))
            .map_or_else(
                || panic!("{name}: no test against null in\n{body}"),
                |(result, _)| result,
            );
        // The negation takes the result of that test, whatever LLVM calls it.
        let negation = format!("xor i1 {result}, true");
        assert_eq!(body.contains(&negation), negated, "{name}:\n{body}");
    }
}

/// `a is b` on a `T: AnyObject` costs nothing at run time. With `T = Node` it is one pointer
/// comparison with no helper. A `T?` local is a bare pointer, as any `Node?` is.
#[test]
fn is_on_a_t_any_object_should_be_one_pointer_compare() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "class Node {}\nfn same<T: AnyObject>(a: T, b: T) -> bool { a is b }\nfn kept<T: AnyObject>(a: T) -> bool {\n    let held: T? = a\n    held is a\n}\nfn main() {\n    let n = Node()\n    print(same(n, n))\n    print(kept(n))\n}",
    )
    .print_to_string()
    .to_string();
    let compares = |body: &str| {
        body.lines()
            .filter(|line| line.contains("icmp eq ptr ") && !line.ends_with(", null"))
            .count()
    };
    let same = function_body(&ir, "define internal i1 @\"sisu.same<Node>\"(ptr");
    assert_eq!(compares(same), 1, "{same}");
    assert!(!same.contains("call"), "{same}");
    let kept = function_body(&ir, "define internal i1 @\"sisu.kept<Node>\"(ptr");
    assert_eq!(compares(kept), 1, "{kept}");
    // The `T?` local is one of the allocas: a pair `{ i1, ptr }` there would be a flag and a
    // pointer. The only other type is the `i1` of the result.
    let slots: Vec<&str> = kept
        .lines()
        .filter_map(|line| line.split_once(" = alloca ")?.1.split(',').next())
        .collect();
    assert!(!slots.is_empty(), "{kept}");
    assert!(slots.iter().all(|ty| matches!(*ty, "ptr" | "i1")), "{kept}");
}

/// An `eq` helper answers "equal" for one pointer on both sides only when its type's `==` is
/// reflexive; where an `f64` is reachable, or a class has a written `eq`, which need not be
/// reflexive, one block or object may differ from itself, so only two `null`s are equal at once.
#[test]
fn same_pointer_shortcut_should_be_taken_only_when_equality_is_reflexive() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "class Node: Eq {\n    value: i64\n    fn eq(self, other: Self) -> bool { self.value == other.value }\n}\nfn same(e: [i64], f: [i64], g: [f64], h: [f64], i: [Node], j: [Node], k: [Node?], l: [Node?]) -> bool { e == f && g == h && i == j && k == l }\nfn main() {}",
    )
    .print_to_string()
    .to_string();
    for (helper, reflexive) in [
        ("@\"sisu_rc.eq.[i64]\"(ptr", true),
        ("@\"sisu_rc.eq.[f64]\"(ptr", false),
        ("@\"sisu_rc.eq.[Node]\"(ptr", false),
        ("@\"sisu_rc.eq.[Node?]\"(ptr", false),
    ] {
        let body = function_body(&ir, &format!("define internal i1 {helper}"));
        // The shortcut is a pointer comparison against another pointer, not against `null`.
        let between_pointers = body
            .lines()
            .any(|line| line.contains("icmp eq ptr ") && !line.ends_with(", null"));
        assert_eq!(between_pointers, reflexive, "{helper}:\n{body}");
        // Every helper still tests for `null`, which is all a non-reflexive one may stop at.
        let against_null = body
            .lines()
            .any(|line| line.contains("icmp eq ptr ") && line.ends_with(", null"));
        assert!(against_null, "{helper}:\n{body}");
    }
}

/// An enum's reflexivity is looked up apart from the classes': the box of a recursive enum
/// is compared by pointer first only when no `f64` is reachable from the enum, whatever the
/// classes before it in the program hold.
#[test]
fn enum_box_shortcut_should_be_taken_only_when_equality_is_reflexive() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "class Pt {\n    x: f64\n}\nclass Other {\n    n: i64\n}\nenum Ints {\n    Nil\n    Cons(i64, Ints)\n}\nenum Floats {\n    Nil\n    Cons(f64, Floats)\n}\nfn main() {\n    let _ = Pt(x: 1.0)\n    let _ = Other(n: 1)\n    print(Ints.Nil == Ints.Nil)\n    print(Floats.Nil == Floats.Nil)\n}",
    )
    .print_to_string()
    .to_string();
    for (helper, reflexive) in [
        ("@sisu_rc.eq_component.Ints(i32", true),
        ("@sisu_rc.eq_component.Floats(i32", false),
    ] {
        let body = function_body(&ir, &format!("define internal i1 {helper}"));
        let between_pointers = body
            .lines()
            .any(|line| line.contains("icmp eq ptr ") && !line.ends_with(", null"));
        assert_eq!(between_pointers, reflexive, "{helper}:\n{body}");
    }
}

/// `eq_component` queues the links it still has to compare, so it never calls itself: a call
/// per level of a left-deep value would overflow the stack.
#[test]
fn eq_component_should_not_call_an_eq_component_helper() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "enum E {\n    Leaf\n    Node(E, E)\n}\nfn main() {\n    print(E.Node(E.Leaf, E.Leaf) == E.Leaf)\n}",
    )
    .print_to_string()
    .to_string();
    let body = function_body(&ir, "define internal i1 @sisu_rc.eq_component.E(i32");
    assert!(!body.contains("@sisu_rc.eq_component."), "{body}");
}

/// `Pair` has a link and then a class field with a written `eq`. `eq_component` compares in field
/// order: it queues the link, enters it, and calls the class's `eq` only after it pops the queue,
/// in the `resume` block. So no call of it comes before that block.
#[test]
fn eq_component_should_compare_a_field_after_a_link_only_when_it_resumes() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "class C: Eq {\n    n: i64\n    fn eq(self, other: Self) -> bool { self.n == other.n }\n}\nenum E {\n    Leaf\n    Pair(E, C)\n}\nfn main() {\n    print(E.Pair(E.Leaf, C(n: 1)) == E.Leaf)\n}",
    )
    .print_to_string()
    .to_string();
    let body = function_body(&ir, "define internal i1 @sisu_rc.eq_component.E(i32");
    assert_eq!(body.matches("@sisu_rc.eq.C(").count(), 1, "{body}");
    let (before, after) = body
        .split_once("\nresume:")
        .unwrap_or_else(|| panic!("no resume block in\n{body}"));
    assert!(!before.contains("@sisu_rc.eq.C("), "{body}");
    assert!(after.contains("@sisu_rc.eq.C("), "{body}");
}

#[test]
fn helper_names_cannot_collide() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "fn retain() {}\nclass release {\n    fn Tree(self) {}\n}\nclass Tree {}\nfn main() {\n    retain()\n    release().Tree()\n}",
    )
    .print_to_string()
    .to_string();
    // The method `release.Tree` and the helper for class `Tree` must both keep their names:
    // LLVM renames the second of two equal names with a numeric suffix.
    for wanted in [
        "define internal void @sisu.retain()",
        "define internal void @sisu.release.Tree(ptr",
        "define internal void @sisu_rc.release.Tree(ptr",
    ] {
        assert!(ir.contains(wanted), "missing {wanted:?} in\n{ir}");
    }
    assert!(
        !ir.contains("@sisu.release.Tree."),
        "a renamed symbol in\n{ir}"
    );
}

#[test]
fn scalar_optional_is_a_pair() {
    let context = Context::create();
    let ir = compiled(&context, "fn f(x: i64?) -> i64? { x }\nfn main() {}")
        .print_to_string()
        .to_string();
    assert!(
        ir.contains("define internal { i1, i64 } @sisu.f({ i1, i64 }"),
        "{ir}"
    );
}

/// A tag is the smallest of 8, 16 and 32 bits that holds the variant count.
#[test]
fn tag_bits_should_hold_the_variant_count() {
    for (count, bits) in [(1, 8), (256, 8), (257, 16), (65_536, 16), (65_537, 32)] {
        assert_eq!(enums::tag_bits(count), bits, "{count} variants");
    }
}

/// Each layer of a niche takes the next spare value while its storage holds it: a `bool` with
/// 254 layers is an `i8` whose empty value is 255, and the 255th layer is a pair. An enum of 255
/// variants has one spare tag, so `E?` is `E` and `E??` is a pair.
#[test]
fn niche_should_run_out_past_its_storage() {
    let context = Context::create();
    let variants = (0..255)
        .map(|v| format!("    V{v}\n"))
        .collect::<Vec<_>>()
        .concat();
    let src = format!("enum E {{\n{variants}}}\nfn main() {{\n    let _ = E.V0\n}}\n");
    let (program, diagnostics) = check_alone(&parse_ok(&src), &src, &[]);
    let program = program.unwrap_or_else(|| panic!("{diagnostics:?}"));
    let program = crate::mono::instantiate(program).unwrap_or_else(|found| panic!("{found:?}"));
    let machine = target_machine(OptLevel::O0).expect("the host is a target");
    let mir = mir::build(&program);
    let mut codegen = Codegen::new(&context, &program, &mir, "test.sisu", &src, &machine, false);
    codegen.lay_out_types();
    let layers = |ty, k| (0..k).fold(ty, |ty, _| Type::optional(ty));
    let pair = |value| {
        Some(
            context
                .struct_type(&[context.bool_type().into(), value], false)
                .into(),
        )
    };
    let i8_type = context.i8_type().into();
    assert_eq!(codegen.llvm_type(layers(Type::BOOL, 254)), Some(i8_type));
    assert_eq!(codegen.llvm_type(layers(Type::BOOL, 255)), pair(i8_type));
    let e = Type::intern(TypeKind::Enum(EnumId(0), TypeArgs::NONE));
    let e_type = codegen.enum_layout(EnumId(0)).ty;
    assert_eq!(codegen.llvm_type(layers(e, 1)), Some(e_type));
    assert_eq!(codegen.llvm_type(layers(e, 2)), pair(e_type));
}

/// The LLVM type of each local, from its `alloca`, and the value that its first value stores.
/// A call that takes the local leads back to its slot, so the test does not depend on the order
/// of the `alloca`s.
/// An optional is a spare value of the payload where it has one, and `null` for one layer around
/// a pointer. Every other optional is a `{ i1, T }` pair. An `Option`-shaped enum has one variant
/// with no payload and one with one field. It is that field's storage in either order of its
/// variants, and its payload-free variant is the field's spare value. Every other enum is tagged.
#[test]
fn niche_layouts_should_follow_the_payload() {
    let rows = [
        ("char?", "None", "i32", Some("i32 1114112")),
        ("char??", "None", "i32", Some("i32 1114113")),
        ("bool?", "None", "i8", Some("i8 2")),
        ("bool??", "None", "i8", Some("i8 3")),
        ("E?", "None", "%sisu.E", Some("%sisu.E { i8 2 }")),
        ("Node?", "None", "ptr", Some("ptr null")),
        (
            "Node??",
            "None",
            "{ i1, ptr }",
            Some("{ i1, ptr } zeroinitializer"),
        ),
        (
            "i64?",
            "None",
            "{ i1, i64 }",
            Some("{ i1, i64 } zeroinitializer"),
        ),
        (
            "u8?",
            "None",
            "{ i1, i8 }",
            Some("{ i1, i8 } zeroinitializer"),
        ),
        (
            "f64?",
            "None",
            "{ i1, double }",
            Some("{ i1, double } zeroinitializer"),
        ),
        ("Maybe", "Maybe.Nothing", "i32", Some("i32 1114112")),
        ("Flag", "Flag.Unset", "i8", Some("i8 2")),
        ("Chain", "Chain.End", "ptr", Some("ptr null")),
        ("Rev", "Rev.Nothing", "i32", Some("i32 1114112")),
        ("Maybe?", "None", "i32", Some("i32 1114113")),
        ("Link", "Link.End", "%sisu.Link", None),
        ("Wide", "Wide.A", "%sisu.Wide", None),
        ("Two", "Two.A('a')", "%sisu.Two", None),
    ];
    let context = Context::create();
    let lets = (rows.iter().enumerate())
        .map(|(i, (ty, init, ..))| {
            format!("    let x{i}: {ty} = {init}\n    print(x{i} == {init})\n    take{i}(x{i})\n")
        })
        .collect::<Vec<_>>()
        .concat();
    let takers = (rows.iter().enumerate())
        .map(|(i, (ty, ..))| format!("fn take{i}(x: {ty}) {{}}\n"))
        .collect::<Vec<_>>()
        .concat();
    let enums = "enum Maybe {\n    Nothing\n    Just(char)\n}\n\
        enum Flag {\n    Unset\n    Set(bool)\n}\n\
        enum Chain {\n    End\n    Next(Chain)\n}\n\
        enum Rev {\n    Just(char)\n    Nothing\n}\n\
        enum Link {\n    End\n    Next(Link?)\n}\n\
        enum Wide {\n    A\n    B(i64)\n}\n\
        enum Two {\n    A(char)\n    B(char)\n}\n";
    let src = format!(
        "class Node {{\n    n: i64\n}}\nenum E {{\n    A\n    B\n}}\n{enums}{takers}fn main() {{\n    let _ = Node(n: 1)\n    let _ = E.A\n    let _ = E.B\n{lets}}}\n"
    );
    let module = compiled(&context, &src);
    let main = module
        .get_function("sisu.main")
        .expect("the module should define `main`");
    let code = instructions(main);
    for (i, (ty, _, llvm, first)) in rows.iter().enumerate() {
        // `take{i}` gets `x{i}` through its slot, maybe through a temporary: follow the copies
        // back to the slot that holds the value of the `let`.
        let call = (code.iter())
            .find(|c| {
                CallSiteValue::try_from(**c)
                    .ok()
                    .and_then(CallSiteValue::get_called_fn_value)
                    .is_some_and(|f| f.get_name().to_str() == Ok(&format!("sisu.take{i}")))
            })
            .unwrap_or_else(|| panic!("no call of `take{i}` in\n{}", module.print_to_string()));
        let (initial, slot) = stored_trail(&code, operand_of(*call, 0));
        let slot = slot.unwrap_or_else(|| panic!("the argument of `take{i}` should be a load"));
        let allocated = slot
            .get_allocated_type()
            .expect("an `alloca` should have a type")
            .print_to_string()
            .to_string();
        // A named struct prints as its definition: `%sisu.E = type { .. }`.
        let allocated = allocated.split(" = type ").next().unwrap_or(&allocated);
        assert_eq!(allocated, *llvm, "{ty} should be {llvm}");
        if let Some(first) = first {
            assert_eq!(
                &initial.print_to_string().to_string(),
                first,
                "the first value of {ty}"
            );
        }
    }
}

/// `!=` on an `i64` or a `bool` is one `icmp ne`, not an `icmp eq` and a `not`.
#[test]
fn scalar_not_equal_is_one_icmp_ne() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "fn f(a: i64, b: i64) -> bool { a != b }\nfn g(a: bool, b: bool) -> bool { a != b }\nfn main() {}",
    )
    .print_to_string()
    .to_string();
    for (name, ty) in [("f", "i64"), ("g", "i1")] {
        let body = function_body(&ir, &format!("define internal i1 @sisu.{name}("));
        assert!(body.contains(&format!("icmp ne {ty} ")), "{body}");
        assert!(!body.contains("icmp eq"), "{body}");
        assert!(!body.contains("xor"), "{body}");
    }
}

#[test]
fn field_read_retains_before_releasing_base() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "class N {}\nclass C {\n    next: N\n}\nfn make() -> C { C(next: N()) }\nfn main() { make().next }",
    )
    .print_to_string()
    .to_string();
    let main = function_body(&ir, "define internal void @sisu.main()");
    let lines: Vec<&str> = main.lines().map(str::trim).collect();
    let at = |wanted: &dyn Fn(&str) -> bool, what: &str| {
        lines
            .iter()
            .position(|l| wanted(l))
            .unwrap_or_else(|| panic!("no {what} in\n{main}"))
    };
    // The field's load carries its TBAA tag; a local's slot has none.
    let (_, next, _) = instruction(&lines, &|t| {
        t.starts_with("load ptr, ptr ") && t.contains("!tbaa")
    })
    .unwrap_or_else(|| panic!("no load of `next` in\n{main}"));
    let order = [
        at(&|l| l.starts_with(&format!("{next} = load ptr")), "load"),
        at(
            &|l| l == format!("call void @sisu_rc.retain(ptr {next})"),
            "retain of the field",
        ),
        at(
            &|l| l.starts_with("call void @sisu_rc.release.C(ptr "),
            "release of the base",
        ),
    ];
    assert!(order.is_sorted(), "{order:?} in\n{main}");
}

/// The object whose count `text`, an instruction `<op> i64 <count>, ...`, uses: the pointer
/// `<count>` is loaded from in `lines`.
fn count_owner<'a>(lines: &[&'a str], text: &str) -> Option<&'a str> {
    let count = text.split_once(" i64 ")?.1.split(", ").next()?;
    let load = format!("{count} = load i64, ptr ");
    lines
        .iter()
        .find_map(|l| l.strip_prefix(load.as_str()))?
        .split(", ")
        .next()
}

/// Asserts that in `body` the RC check of `kind` guards `step`, the first instruction
/// `<op> i64 <count>, ...` that uses an object's count: before `step`, the count of that object
/// is loaded and compared signed against 1, and the branch on the result goes to a block that
/// calls `sisu_rc_fault(i32 <kind>)`.
fn assert_count_checked(body: &str, kind: u32, step: &dyn Fn(&str) -> bool) {
    let lines: Vec<&str> = body.lines().map(str::trim).collect();
    let (step_at, _, step_text) =
        instruction(&lines, step).unwrap_or_else(|| panic!("no count step in\n{body}"));
    let object = count_owner(&lines, step_text)
        .unwrap_or_else(|| panic!("{step_text:?} should use a loaded count in\n{body}"));
    let (test_at, dead, _) = instruction(&lines, &|t| {
        t.starts_with("icmp slt i64 ")
            && t.ends_with(", 1")
            && count_owner(&lines, t) == Some(object)
    })
    .unwrap_or_else(|| panic!("no signed test of {object}'s count against 1 in\n{body}"));
    let branch_at = assert_faults_on(&lines, dead, kind, body);
    assert!([test_at, branch_at, step_at].is_sorted(), "{body}");
}

/// Asserts that the branch on `dead` in `lines` goes, when `dead` holds, to a block that
/// calls `sisu_rc_fault(i32 <kind>)`; returns the branch's line index.
fn assert_faults_on(lines: &[&str], dead: &str, kind: u32, body: &str) -> usize {
    let branch = format!("br i1 {dead}, label %");
    let branch_at = lines
        .iter()
        .position(|l| l.starts_with(&branch))
        .unwrap_or_else(|| panic!("no branch on {dead} in\n{body}"));
    let fault = lines[branch_at][branch.len()..]
        .split(',')
        .next()
        .expect("a branch should name its true label");
    let fault_at = lines
        .iter()
        .position(|l| l.starts_with(&format!("{fault}:")))
        .unwrap_or_else(|| panic!("no block {fault} in\n{body}"));
    assert_eq!(
        lines.get(fault_at + 1).copied(),
        Some(format!("call void @sisu_rc_fault(i32 {kind})").as_str()),
        "{body}"
    );
    branch_at
}

#[test]
fn rc_checks_should_guard_retain_release_drop_and_exit() {
    let context = Context::create();
    // `next` holds the class itself, so the drop lists the next object after a checked decrement.
    // `fill` shares one array among its slots; `write` makes a shared array unique.
    let src = format!(
        "class P {{\n    next: P?\n}}\nfn fill(n: i64) -> [[i64]] {{ [[1]; n] }}\nfn write(xs: [i64]) -> [i64] {{\n    let mut ys = xs\n    ys[0] = 1\n    ys\n}}\n{FIB}"
    );
    let ir = compiled_with(&context, &src, true)
        .print_to_string()
        .to_string();
    let retain = function_body(&ir, "define internal void @sisu_rc.retain(ptr");
    assert_count_checked(retain, 0, &|t| {
        t.starts_with("add i64 ") && t.ends_with(", 1")
    });
    let release = function_body(&ir, "define internal void @sisu_rc.release.P(ptr");
    assert_count_checked(release, 1, &|t| {
        t.starts_with("add i64 ") && t.ends_with(", -1")
    });
    let drop = function_body(&ir, "define internal void @sisu_rc.drop.P(ptr");
    assert_count_checked(drop, 1, &|t| {
        t.starts_with("add i64 ") && t.ends_with(", -1")
    });
    let fill = function_body(&ir, "@sisu.fill(");
    // The loop's counter steps with `add nuw`; the count gains its extra references with `add`.
    assert_count_checked(fill, 0, &|t| t.starts_with("add i64 "));
    let unique = function_body(&ir, "define internal ptr @\"sisu_rc.unique.[i64]\"(ptr");
    assert_count_checked(unique, 1, &|t| {
        t.starts_with("icmp eq i64 ") && t.ends_with(", 1")
    });
    let main = function_body(&ir, "define i32 @main(i32 %argc, ptr %argv)");
    let (_, after_check) = main
        .split_once("call void @sisu_rc_check_exit()")
        .unwrap_or_else(|| panic!("no exit check in\n{main}"));
    assert!(after_check.contains("ret i32 0"), "{main}");
}

/// `fn main() { let s = "hi"; take(move s); print(s) }`: the mir of a read after a move, which
/// the certifier would reject.
fn print_after_move() -> mir::Function {
    let whole = |local| Place {
        local,
        projections: Vec::new(),
        borrow: false,
        guard: false,
    };
    let assign = |dest, value| Statement::Assign { dest, value };
    let mut b = mir::tests::Builder::new("main");
    let s = b.local("s", Type::STRING);
    b.block(
        vec![
            assign(Some(s), Rvalue::CopyStr("hi".to_owned())),
            assign(
                None,
                Rvalue::Call {
                    func: FuncId(0),
                    args: vec![mir::Operand::Move(s)],
                    borrowed: vec![false],
                    span: Span::new(0, 0),
                    quiet: false,
                },
            ),
            assign(
                None,
                Rvalue::Print {
                    stream: Stream::Stdout,
                    operand: mir::Operand::Read(whole(s)),
                    calls: false,
                },
            ),
            Statement::ScopeRelease(s),
        ],
        Terminator::Return(None),
    );
    b.finish()
}

#[test]
fn read_after_move_faults() {
    let context = Context::create();
    let module = compiled_from(
        &context,
        "fn take(s: string) {}
fn main() {}",
        true,
        |mir| {
            let main = mir.functions.iter_mut().find(|f| f.name == "main");
            *main.expect("the source should define `main`") = print_after_move();
        },
    );
    let ir = module.print_to_string().to_string();
    let main = function_body(&ir, "@sisu.main(");
    let lines: Vec<&str> = main.lines().map(str::trim).collect();
    // The move empties `s`'s slot; the print then reads the slot, and null tests what it loaded.
    let moved_at = lines
        .iter()
        .position(|l| l.starts_with("store ptr null, ptr "))
        .unwrap_or_else(|| panic!("no move in\n{main}"));
    let slot = lines[moved_at]["store ptr null, ptr ".len()..]
        .split(',')
        .next()
        .expect("a store should name its address");
    let print_at = lines
        .iter()
        .position(|l| l.starts_with("call void @sisu_print_str("))
        .unwrap_or_else(|| panic!("no print in\n{main}"));
    let between = &lines[moved_at..print_at];
    let (_, loaded, _) = instruction(between, &|t| {
        t.starts_with(&format!("load ptr, ptr {slot},"))
    })
    .unwrap_or_else(|| panic!("the print should read {slot} in\n{main}"));
    let (_, null, _) = instruction(between, &|t| t == format!("icmp eq ptr {loaded}, null"))
        .unwrap_or_else(|| panic!("no null test of {loaded} in\n{main}"));
    let branch_at = assert_faults_on(&lines, null, 2, main);
    assert!(branch_at < print_at, "{main}");
}

const MOVED_ROOT_SOURCE: &str = "enum Shape {\n    Dot\n    Circle(i64)\n}\nclass Holder {\n    mut n: i64\n    mut xs: [i64]\n    mut shape: Shape\n}\nfn make_holder() -> Holder { Holder(n: 1, xs: [1, 2], shape: Shape.Dot) }\nfn make_array() -> [i64] { [1, 2] }\nfn take_holder(h: Holder) {}\nfn take_array(xs: [i64]) {}\nfn main() {}";

/// Builds `main` as `r = make(); take(move r); <what>(r)`, which the certifier would reject, and
/// asserts that `<what>` reaches `r`'s root slot only after a null test that calls
/// `sisu_rc_fault(2)`: the first instructions after the call to `take` are that test.
/// `what` gives the statements after the move and the terminator of `bb0`; `bb1` returns.
fn assert_root_read_faults(
    root: Type,
    (make, take): (&str, &str),
    what: impl FnOnce(LocalId) -> (Vec<Statement>, Terminator),
) {
    let context = Context::create();
    let module = compiled_from(&context, MOVED_ROOT_SOURCE, true, |mir| {
        let func = |name: &str| {
            let at = mir.functions.iter().position(|f| f.name == name);
            FuncId(at.expect("the source should define the function"))
        };
        let call = |func, args: Vec<mir::Operand>| Statement::Assign {
            dest: None,
            value: Rvalue::Call {
                func,
                borrowed: vec![false; args.len()],
                args,
                span: Span::new(0, 0),
                quiet: false,
            },
        };
        let mut b = mir::tests::Builder::new("main");
        let r = b.local("r", root);
        let (after, term) = what(r);
        let mut stmts = vec![
            Statement::Assign {
                dest: Some(r),
                value: Rvalue::Call {
                    func: func(make),
                    args: Vec::new(),
                    borrowed: Vec::new(),
                    span: Span::new(0, 0),
                    quiet: false,
                },
            },
            call(func(take), vec![mir::Operand::Move(r)]),
        ];
        stmts.extend(after);
        b.block(stmts, term);
        b.block(Vec::new(), Terminator::Return(None));
        let main = mir.functions.iter_mut().find(|f| f.name == "main");
        *main.expect("the source should define `main`") = b.finish();
    });
    let ir = module.print_to_string().to_string();
    let main = function_body(&ir, "@sisu.main(");
    let lines: Vec<&str> = main.lines().map(str::trim).collect();
    let take_at = lines
        .iter()
        .position(|l| l.starts_with("call void @sisu.take_"))
        .unwrap_or_else(|| panic!("no call to take in\n{main}"));
    let slot = lines[..take_at]
        .iter()
        .rev()
        .find_map(|l| l.strip_prefix("store ptr null, ptr "))
        .and_then(|rest| rest.split(',').next())
        .unwrap_or_else(|| panic!("no move in\n{main}"));
    let after = &lines[take_at + 1..];
    let (load_at, loaded, _) =
        instruction(after, &|t| t.starts_with(&format!("load ptr, ptr {slot},")))
            .unwrap_or_else(|| panic!("no load of {slot} after the move in\n{main}"));
    let (null_at, null, _) = instruction(after, &|t| t == format!("icmp eq ptr {loaded}, null"))
        .unwrap_or_else(|| panic!("no null test of {loaded} in\n{main}"));
    assert_eq!((load_at, null_at), (0, 1), "{main}");
    let branch_at = assert_faults_on(&lines, null, 2, main);
    assert_eq!(branch_at, take_at + 3, "{main}");
}

fn whole_place(local: LocalId) -> Place {
    Place {
        local,
        projections: Vec::new(),
        borrow: false,
        guard: false,
    }
}

#[test]
fn store_to_a_moved_root_faults() {
    let holder = Type::intern(TypeKind::Class(ClassId(0), TypeArgs::NONE));
    assert_root_read_faults(holder, ("make_holder", "take_holder"), |r| {
        let place = Place {
            projections: vec![Projection::Field {
                index: 0,
                mutable: true,
            }],
            ..whole_place(r)
        };
        let value = mir::Operand::Const(Const::Int(5, Type::I64));
        (
            vec![Statement::Store { place, value }],
            Terminator::Goto(BlockId(1)),
        )
    });
}

#[test]
fn make_unique_of_a_moved_array_faults() {
    let array = Type::intern(TypeKind::Array(Type::I64));
    assert_root_read_faults(array, ("make_array", "take_array"), |r| {
        (
            vec![Statement::MakeUnique(whole_place(r))],
            Terminator::Goto(BlockId(1)),
        )
    });
}

#[test]
fn check_of_a_moved_array_faults() {
    let array = Type::intern(TypeKind::Array(Type::I64));
    assert_root_read_faults(array, ("make_array", "take_array"), |r| {
        let element = Place {
            projections: vec![Projection::Index {
                index: mir::Operand::Const(Const::Int(0, Type::I64)),
                span: Span::new(0, 0),
            }],
            ..whole_place(r)
        };
        (
            vec![Statement::Check(element)],
            Terminator::Goto(BlockId(1)),
        )
    });
}

/// An enum local is not a pointer, so a moved one is not checked; a switch on the tag of an
/// enum field reads the class that holds it.
#[test]
fn switch_on_a_field_of_a_moved_class_faults() {
    let holder = Type::intern(TypeKind::Class(ClassId(0), TypeArgs::NONE));
    assert_root_read_faults(holder, ("make_holder", "take_holder"), |r| {
        let shape = Place {
            projections: vec![Projection::Field {
                index: 2,
                mutable: true,
            }],
            ..whole_place(r)
        };
        let term = Terminator::SwitchTag {
            place: shape,
            arms: vec![(0, BlockId(1))],
            otherwise: Some(BlockId(1)),
        };
        (Vec::new(), term)
    });
}

#[test]
fn main_flushes_stdout_before_the_exit_check_and_the_return() {
    let context = Context::create();
    for rc_checks in [false, true] {
        let ir = compiled_with(&context, FIB, rc_checks)
            .print_to_string()
            .to_string();
        let main = function_body(&ir, "define i32 @main(i32 %argc, ptr %argv)");
        let flush = main
            .find("call void @sisu_flush_stdout()")
            .unwrap_or_else(|| panic!("no flush in\n{main}"));
        let after = main.find("ret i32 0").expect("main returns 0");
        assert!(flush < after, "{main}");
        if rc_checks {
            let check = main
                .find("call void @sisu_rc_check_exit()")
                .expect("exit check");
            assert!(flush < check, "{main}");
        }
    }
}

/// A build with the RC checks calls `sisu_rc_fault` and `sisu_rc_check_exit`, and the same source
/// without them calls neither.
#[test]
fn rc_check_symbols_should_appear_only_in_an_rc_checks_build() {
    let context = Context::create();
    let src = format!("class P {{}}\n{FIB}");
    for (rc_checks, present) in [(false, false), (true, true)] {
        let ir = compiled_with(&context, &src, rc_checks)
            .print_to_string()
            .to_string();
        for symbol in ["sisu_rc_fault", "sisu_rc_check_exit"] {
            assert_eq!(
                ir.contains(symbol),
                present,
                "{symbol:?} with rc_checks {rc_checks} in\n{ir}"
            );
        }
    }
}

/// Only the C entry point is visible outside the module: every other function, helpers
/// included, is internal, so `-O2` can inline and specialise it.
#[test]
fn only_main_is_external() {
    let context = Context::create();
    let ir = compiled(
        &context,
        &format!(
            "class P {{\n    next: P?\n}}\nfn f(xs: [[i64]], p: P) -> [[i64]] {{ xs }}\n{FIB}"
        ),
    )
    .print_to_string()
    .to_string();
    let defined: Vec<&str> = ir.lines().filter(|l| l.starts_with("define ")).collect();
    assert!(defined.len() > 4, "{ir}");
    for line in defined {
        assert!(
            line.starts_with("define i32 @main(") || line.starts_with("define internal "),
            "{line}"
        );
    }
}

#[test]
fn user_function_names_are_prefixed() {
    let context = Context::create();
    let ir = compiled(&context, FIB).print_to_string().to_string();
    for wanted in [
        "define internal i64 @sisu.fib(i64",
        "define internal void @sisu.main()",
        "define i32 @main(i32 %argc, ptr %argv)",
    ] {
        assert!(ir.contains(wanted), "missing {wanted:?} in\n{ir}");
    }
}

#[test]
fn u8_add_uses_unsigned_overflow() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "fn f(a: u8, b: u8) -> u8 { a + b }\nfn main() { print(f(1, 2)) }",
    )
    .print_to_string()
    .to_string();
    for wanted in [
        "define internal i8 @sisu.f(i8",
        "@llvm.uadd.with.overflow.i8",
    ] {
        assert!(ir.contains(wanted), "missing {wanted:?} in\n{ir}");
    }
}

#[test]
fn u8_compare_is_unsigned() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "fn f(a: u8, b: u8) -> bool { a < b }\nfn main() { print(f(1, 2)) }",
    )
    .print_to_string()
    .to_string();
    assert!(ir.contains("icmp ult i8"), "missing `icmp ult i8` in\n{ir}");
}

/// A block is the 24-byte header, then the elements at their ABI size.
#[test]
fn literal_should_allocate_header_and_elements() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "fn main() {\n    let a = [1, 2, 3]\n    let b = [true, false]\n    let c: [i64?] = [None, 1]\n}",
    )
    .print_to_string()
    .to_string();
    // 24 + 3 * 8, 24 + 2 * 1 and 24 + 2 * 16 bytes, aligned to the header's 8.
    for wanted in [
        "call ptr @sisu_alloc(i64 48, i64 8)",
        "call ptr @sisu_alloc(i64 26, i64 8)",
        "call ptr @sisu_alloc(i64 56, i64 8)",
    ] {
        assert!(ir.contains(wanted), "missing {wanted:?} in\n{ir}");
    }
}

/// Reading `xs[i]` from a local neither retains nor releases the array.
#[test]
fn fast_path_skips_base_count() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "fn f(xs: [i64]) -> [i64] {\n    print(xs[0] + xs[1])\n    xs\n}\nfn main() {}",
    )
    .print_to_string()
    .to_string();
    let body = function_body(&ir, "define internal ptr @sisu.f(ptr");
    let (before, _) = body
        .split_once("call void @sisu_print_int(")
        .unwrap_or_else(|| panic!("no print in\n{body}"));
    for unwanted in ["@sisu_rc.retain(", "@\"sisu_rc.release.[i64]\"("] {
        assert!(
            !before.contains(unwanted),
            "{unwanted:?} before the print in\n{body}"
        );
    }
}

/// A string pattern compares the scrutinee with its literal's global borrowed: the test calls
/// the string `eq` helper and retains nothing. A retained global is never freed, so neither
/// Valgrind nor `--rc-checks` would see the extra count; only the IR shows it.
#[test]
fn string_pattern_borrows_its_literal() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "fn f(s: string) -> i64 {\n    match s {\n        \"hi\" => 1\n        _ => 0\n    }\n}\nfn main() {}",
    )
    .print_to_string()
    .to_string();
    let body = function_body(&ir, "define internal i64 @sisu.f(ptr");
    assert!(
        body.contains("@sisu_rc.eq.string("),
        "no string comparison in\n{body}"
    );
    assert!(!body.contains("@sisu_rc.retain("), "a retain in\n{body}");
}

/// The runtime's print functions touch only memory the module cannot reach (their buffer,
/// `errno`) and what a pointer argument points to, which they read, and never unwind, but
/// they can exit the process, so they are not `willreturn`. `sisu_str_compare` only reads its
/// arguments' bytes.
#[test]
fn print_declarations_carry_attributes() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "fn main() { print(1)\n    print(true)\n    print(\"s\")\n    print(\"a\" < \"b\") }",
    )
    .print_to_string()
    .to_string();
    let own = "memory(inaccessiblemem: readwrite, errnomem: readwrite)";
    let reads = "memory(argmem: read, inaccessiblemem: readwrite, errnomem: readwrite)";
    for (name, memory) in [
        ("sisu_print_int", own),
        ("sisu_print_bool", own),
        ("sisu_print_str", reads),
        ("sisu_eprint_int", own),
        ("sisu_eprint_bool", own),
        ("sisu_eprint_char", own),
        ("sisu_eprint_str", reads),
        ("sisu_str_compare", "memory(argmem: read)"),
    ] {
        let attributes = attributes(&ir, name);
        for wanted in ["nounwind", memory] {
            assert!(attributes.contains(wanted), "{name}: {attributes}");
        }
        assert!(!attributes.contains("willreturn"), "{name}: {attributes}");
    }
}

/// `print(n)` of an `i64` stays the runtime's `sisu_print_int` call: no sink, no `show` helper.
#[test]
fn printing_an_i64_should_call_sisu_print_int_alone() {
    let context = Context::create();
    let ir = compiled(&context, "fn main() {\n    let n = 5\n    print(n)\n}")
        .print_to_string()
        .to_string();
    assert!(ir.contains("call void @sisu_print_int("), "{ir}");
    assert!(!ir.contains("sisu_show"), "{ir}");
}

/// The text of the function `name` in `ir`, from its `define` line to its closing brace.
fn definition<'a>(ir: &'a str, name: &str) -> &'a str {
    let start = ir
        .match_indices("define ")
        .map(|(at, _)| at)
        .find(|&at| {
            let line = ir[at..].lines().next().unwrap_or_default();
            line.contains(&format!("@{name}("))
        })
        .unwrap_or_else(|| panic!("no definition of {name} in\n{ir}"));
    let end = ir[start..]
        .find("\n}\n")
        .expect("a definition ends with a brace");
    &ir[start..start + end]
}

/// A right-deep enum list prints in one frame: its component helper loops on the last link, and
/// never calls itself or another `show` helper of its own enum.
#[test]
fn printing_a_deep_enum_should_loop_instead_of_recursing() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "enum List {\n    Nil\n    Cons(i64, List)\n}\nfn main() {\n    print(List.Cons(1, List.Nil))\n}",
    )
    .print_to_string()
    .to_string();
    let body = definition(&ir, "sisu_show_component.List");
    let calls = |callee: &str| {
        body.lines()
            .any(|l| l.contains("call ") && l.contains(callee))
    };
    assert!(!calls("@sisu_show_component.List("), "{body}");
    assert!(!calls("@sisu_show.List("), "{body}");
    // The helper of the type is a call of the component helper.
    assert!(
        definition(&ir, "sisu_show.List").contains("@sisu_show_component.List("),
        "{ir}"
    );
}

/// `eprint` of a compound value builds the text in a buffer sink and writes it with one
/// `sisu_eprint_str`; `print` writes through a sink over standard output, with no buffer for the
/// text.
#[test]
fn printing_a_compound_value_should_use_the_sink_of_its_stream() {
    let context = Context::create();
    let ir = compiled(&context, "fn main() {\n    print([1])\n    eprint([1])\n}")
        .print_to_string()
        .to_string();
    let main = definition(&ir, "sisu.main");
    for wanted in [
        "@sisu_show_stdout(",
        "@sisu_show_line_end(",
        "@sisu_show_buffer(",
        "@sisu_show_finish(",
        "@sisu_eprint_str(",
    ] {
        assert!(main.contains(wanted), "{wanted} in\n{main}");
    }
    assert_eq!(main.matches("@sisu_show_buffer(").count(), 1, "{main}");
    // The sink slot is the runtime's `SINK_SIZE` bytes, as six words.
    let words = abi::SINK_SIZE / 8;
    assert!(main.contains(&format!("alloca [{words} x i64]")), "{main}");
}

/// An interpolation with a compound part writes every part through one buffer sink, which is the
/// result string, and pads a part from its mark; one with only text and numbers keeps the block
/// of known size and never touches a sink.
#[test]
fn interpolating_a_compound_part_should_use_one_buffer_sink() {
    let context = Context::create();
    let enum_part = "enum E {\n    A\n    B(E)\n}\nfn f(e: E, n: i64, x: f64) -> string {\n    \"{n}{e:>8}{x:.2}\"\n}\nfn main() {\n    print(f(E.B(E.A), 1, 2.5))\n}";
    let ir = compiled(&context, enum_part).print_to_string().to_string();
    let f = definition(&ir, "sisu.f");
    for wanted in [
        "@sisu_show_buffer(",
        "@sisu_show_int(",
        "@sisu_show_mark(",
        "@sisu_show_pad(",
        "@sisu_show_f64_fixed(",
        "@sisu_show_finish(",
    ] {
        assert!(f.contains(wanted), "{wanted} in\n{f}");
    }
    assert_eq!(f.matches("@sisu_show_buffer(").count(), 1, "{f}");
    assert_eq!(f.matches("@sisu_show_mark(").count(), 1, "{f}");
    assert!(!f.contains("@sisu_alloc("), "the sink makes the block: {f}");
    let plain = "fn f(s: string, n: i64) -> string {\n    \"{s}{n:>4}\"\n}\nfn main() {\n    print(f(\"a\", 1))\n}";
    let ir = compiled(&context, plain).print_to_string().to_string();
    let f = definition(&ir, "sisu.f");
    assert!(f.contains("@sisu_write_int("), "{f}");
    assert!(!f.contains("@sisu_show_"), "{f}");
}

/// A `#` part whose type makes a group is written through a pretty sink over the buffer sink.
/// The sink is started and finished once.
/// A `#` part whose type writes one piece emits the IR of the same part with no `#`.
/// That holds for a concrete type and for the argument of a type parameter.
/// An array part beside it makes every interpolation go through a sink.
#[test]
fn a_hash_part_should_use_a_pretty_sink_only_when_its_type_makes_a_group() {
    let context = Context::create();
    let ir_of = |src: &str, name: &str| {
        let ir = compiled(&context, src).print_to_string().to_string();
        definition(&ir, name).to_owned()
    };
    // (row, declarations, parameter type, argument, whether a group is laid out)
    for (row, decls, ty, arg, grouped) in [
        ("an `i64`", "", "i64", "1", false),
        ("a class", "class C {}\n", "C", "C()", false),
        ("an array", "", "[i64]", "[1]", true),
        (
            "an enum",
            "enum E {\n    A\n    B(E)\n}\n",
            "E",
            "E.B(E.A)",
            true,
        ),
    ] {
        for generic in [false, true] {
            let (params, name) = if generic {
                ("<T>", format!("\"sisu.f<{ty}>\""))
            } else {
                ("", "sisu.f".to_owned())
            };
            let param = if generic { "T" } else { ty };
            let src = |spec: &str| {
                format!(
                    "{decls}fn f{params}(x: {param}, ys: [i64]) -> string {{\n    \"[{{x{spec}}}]{{ys}}\"\n}}\nfn main() {{\n    print(f({arg}, [1]))\n}}"
                )
            };
            let (hashed, flat) = (ir_of(&src(":#"), &name), ir_of(&src(""), &name));
            let row = format!("{row}, generic: {generic}");
            if grouped {
                assert_eq!(
                    hashed.matches("@sisu_show_pretty(").count(),
                    1,
                    "{row}: {hashed}"
                );
                assert_eq!(
                    hashed.matches("@sisu_show_pretty_finish(").count(),
                    1,
                    "{row}: {hashed}"
                );
                assert!(!flat.contains("@sisu_show_pretty"), "{row}: {flat}");
            } else {
                assert_eq!(hashed, flat, "{row}");
            }
        }
    }
}

/// The number of calls of `sisu_show_*` functions that printing `[x]` emits, for an `x` of a
/// `char` under `layers` optionals that holds `None` in the outermost.
fn show_calls_of_nested_optionals(layers: usize) -> usize {
    let src = format!(
        "fn f(x: char{}) {{\n    print([x])\n}}\nfn main() {{\n    f(None)\n}}\n",
        "?".repeat(layers)
    );
    let context = Context::create();
    let ir = compiled(&context, &src).print_to_string().to_string();
    ir.lines()
        .filter(|line| line.contains(" call ") && line.contains("@sisu_show_"))
        .count()
}

/// Printing an optional of `n` layers writes the `Some(` and `)` of a `None` under `k` `Some`s
/// with one rendering for every `k`, so the calls emitted grow with `n` and not with its square.
#[test]
fn printing_nested_optionals_should_emit_calls_in_linear_number() {
    let calls = show_calls_of_nested_optionals;
    assert_near_linear(calls(20), calls(40), calls(60));
}

/// The calls that printing `[x]` emits for a `char` under 20 optionals: three for each layer (the
/// loop of `Some(`, `None` and the loop of `)`) and four others, so that a change in the
/// rendering of a `None` shows as a different count.
#[test]
fn printing_twenty_nested_optionals_should_emit_three_calls_for_each_layer() {
    assert_eq!(show_calls_of_nested_optionals(20), 3 * 20 + 4);
}

/// The attribute group line of the declaration of `name` in `ir`.
fn attributes<'a>(ir: &'a str, name: &str) -> &'a str {
    let declaration = ir
        .lines()
        .find(|line| line.starts_with("declare ") && line.contains(&format!("@{name}(")))
        .unwrap_or_else(|| panic!("no declaration of {name} in\n{ir}"));
    let (_, group) = declaration
        .rsplit_once(" #")
        .unwrap_or_else(|| panic!("{declaration:?} has no attribute group"));
    ir.lines()
        .find(|line| line.starts_with(&format!("attributes #{group} = ")))
        .unwrap_or_else(|| panic!("no attribute group #{group} in\n{ir}"))
}

/// `sisu_words_count` only reads the string it is given; `sisu_word_next` also writes the
/// start slot. Neither unwinds, and neither is declared when no `words()` is called.
#[test]
fn words_declarations_carry_memory_effects() {
    let context = Context::create();
    let ir = compiled(&context, "fn main() {\n    let a = \"a b\".words()\n}")
        .print_to_string()
        .to_string();
    for (name, memory) in [
        ("sisu_words_count", "memory(argmem: read)"),
        ("sisu_word_next", "memory(argmem: readwrite)"),
    ] {
        let attributes = attributes(&ir, name);
        for wanted in ["nounwind", memory] {
            assert!(attributes.contains(wanted), "{name}: {attributes}");
        }
    }
    let plain = compiled(&context, "fn main() {}")
        .print_to_string()
        .to_string();
    assert!(!plain.contains("sisu_word"), "{plain}");
}

/// `s.words().len()` is one `sisu_words_count` call: no `[string]` is built, so no
/// `sisu_word_next` call fills one. A `[string]` kept in a local is still built, word by word.
#[test]
fn len_of_words_only_counts() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "fn main() {\n    let n = (\"a \" + \"b\").words().len()\n}",
    )
    .print_to_string()
    .to_string();
    assert!(ir.contains("call i64 @sisu_words_count("), "{ir}");
    assert!(!ir.contains("sisu_word_next"), "{ir}");
    let kept = compiled(
        &context,
        "fn main() {\n    let ws = (\"a \" + \"b\").words()\n    let n = ws.len()\n}",
    )
    .print_to_string()
    .to_string();
    assert!(kept.contains("call i64 @sisu_word_next("), "{kept}");
}

/// `sisu_read_line` never unwinds, and carries no `memory` attribute: the call overwrites the
/// line an earlier call returned, so LLVM must not move a read of those bytes past it. It may
/// exit (a failed read panics), so it is not `willreturn` either.
#[test]
fn read_line_declaration_has_no_memory_attribute() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "fn main() {\n    let a = read_line()\n    let b = read_line()\n}",
    )
    .print_to_string()
    .to_string();
    let attributes = attributes(&ir, "sisu_read_line");
    assert!(attributes.contains("nounwind"), "{attributes}");
    assert!(!attributes.contains("memory("), "{attributes}");
    assert!(!attributes.contains("willreturn"), "{attributes}");
}

/// `sisu_args_len` reads no memory. `sisu_arg` has no `memory` attribute, for
/// `sisu_read_line`'s reason: a call can overwrite the bytes an earlier call returned. Neither
/// unwinds, and neither is declared when `main` takes no `args`.
#[test]
fn args_declarations_carry_memory_effects() {
    let context = Context::create();
    let ir = compiled(&context, "fn main(args: [string]) {}")
        .print_to_string()
        .to_string();
    let attributes_len = attributes(&ir, "sisu_args_len");
    for wanted in ["nounwind", "memory(none)"] {
        assert!(attributes_len.contains(wanted), "{attributes_len}");
    }
    let attributes_arg = attributes(&ir, "sisu_arg");
    assert!(attributes_arg.contains("nounwind"), "{attributes_arg}");
    assert!(!attributes_arg.contains("memory("), "{attributes_arg}");
    let plain = compiled(&context, "fn main() {}")
        .print_to_string()
        .to_string();
    assert!(!plain.contains("sisu_arg"), "{plain}");
}

/// `xs.len()` and `o.n` on a local neither retain nor release the base: the local keeps it
/// alive. A counted field read that escapes still retains the field: `counted` hands `a` to
/// `keep`. A copy that is only read is a pair, which `mir/pairs.rs` makes a borrow.
#[test]
fn len_and_field_of_a_local_skip_base_count() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "class O {\n    n: i64\n    a: [i64]\n}\n\
         fn len(xs: [i64]) -> [i64] {\n    print(xs.len() + xs.len())\n    xs\n}\n\
         fn field(o: O) -> O {\n    print(o.n + o.n)\n    o\n}\n\
         fn keep(xs: [i64]) {\n    let kept = xs\n}\n\
         fn counted(o: O) -> O {\n    let a = o.a\n    print(a[0])\n    keep(a)\n    o\n}\n\
         fn main() {}",
    )
    .print_to_string()
    .to_string();
    for (header, release) in [
        (
            "define internal ptr @sisu.len(ptr",
            "@\"sisu_rc.release.[i64]\"(",
        ),
        ("define internal ptr @sisu.field(ptr", "@sisu_rc.release.O("),
    ] {
        let body = function_body(&ir, header);
        let (before, _) = body
            .split_once("call void @sisu_print_int(")
            .unwrap_or_else(|| panic!("no print in\n{body}"));
        for unwanted in ["@sisu_rc.retain(", release] {
            assert!(
                !before.contains(unwanted),
                "{unwanted:?} before the print in\n{body}"
            );
        }
    }
    // `a` takes a reference to the field's array, and nothing else is counted before the print.
    let body = function_body(&ir, "define internal ptr @sisu.counted(ptr");
    let (before, _) = body
        .split_once("call void @sisu_print_int(")
        .unwrap_or_else(|| panic!("no print in\n{body}"));
    assert_eq!(before.matches("@sisu_rc.retain(").count(), 1, "{body}");
    assert!(!before.contains("@sisu_rc.release.O("), "{body}");
}

/// A base that is not a local is still released after `len` and after a field read.
#[test]
fn len_and_field_of_a_value_release_the_base() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "class O {\n    n: i64\n}\nfn make() -> [i64] { [1] }\nfn o() -> O { O(n: 1) }\n\
         fn len() -> i64 { make().len() }\nfn field() -> i64 { o().n }\nfn main() {}",
    )
    .print_to_string()
    .to_string();
    for (header, release) in [
        (
            "define internal i64 @sisu.len()",
            "@\"sisu_rc.release.[i64]\"(",
        ),
        ("define internal i64 @sisu.field()", "@sisu_rc.release.O("),
    ] {
        let body = function_body(&ir, header);
        assert!(body.contains(release), "no {release:?} in\n{body}");
    }
}

/// `o.x = o.x + 1` on a local neither retains nor releases `o`: the value cannot replace or
/// move the local, which keeps the object alive. `tests/run/field_assign_local_base.sisu` runs
/// the values that can.
#[test]
fn field_assignment_to_a_local_skips_base_count() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "class O {\n    mut x: i64\n}\n\
         fn f(o: O) -> O {\n    o.x = o.x + 1\n    print(o.x)\n    o\n}\nfn main() {}",
    )
    .print_to_string()
    .to_string();
    let body = function_body(&ir, "define internal ptr @sisu.f(ptr");
    let (before, _) = body
        .split_once("call void @sisu_print_int(")
        .unwrap_or_else(|| panic!("no print in\n{body}"));
    assert!(before.contains("store i64"), "no field store in\n{body}");
    for unwanted in ["@sisu_rc.retain(", "@sisu_rc.release.O("] {
        assert!(
            !before.contains(unwanted),
            "{unwanted:?} before the print in\n{body}"
        );
    }
}

/// The name of the TBAA type that the instruction `line` is tagged with, from the metadata in
/// `ir`: `!tbaa !t`, then `!t = !{!ty, !ty, i64 0}`, then `!ty = !{!"<name>", ...}`.
fn tbaa_name<'a>(ir: &'a str, line: &str) -> &'a str {
    let node = |id: &str| {
        let head = format!("{id} = !{{");
        ir.lines()
            .find_map(|l| l.strip_prefix(&head))
            .unwrap_or_else(|| panic!("no {id} in\n{ir}"))
    };
    let (_, rest) = line
        .split_once("!tbaa ")
        .unwrap_or_else(|| panic!("no tag on {line}"));
    let tag = rest.split(',').next().unwrap_or(rest);
    let (ty, _) = node(tag)
        .split_once(", ")
        .unwrap_or_else(|| panic!("no type in tag {tag}"));
    let name = node(ty)
        .strip_prefix("!\"")
        .unwrap_or_else(|| panic!("no name in {ty}"));
    name.split_once('"')
        .unwrap_or_else(|| panic!("no name in {ty}"))
        .0
}

/// A field read's load and a field assignment's store carry the tag of their own class and
/// field, so a store to one field leaves LLVM free to keep another in a register.
#[test]
fn field_accesses_carry_their_fields_tbaa_tag() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "class P {\n    mut x: f64\n    mut y: f64\n}\nclass Q {\n    x: f64\n}\n\
         fn f(p: P, q: Q) {\n    p.x = p.y\n    p.y = q.x\n}\nfn main() {}",
    )
    .print_to_string()
    .to_string();
    let body = function_body(&ir, "define internal void @sisu.f(ptr");
    let lines: Vec<&str> = body.lines().map(str::trim).collect();
    // The loads and stores of `double`s, less those of the locals' slots.
    let accesses: Vec<_> = lines
        .iter()
        .filter(|l| l.contains(" double") && !l.contains(" alloca "))
        .filter(|l| {
            let address = l
                .split_once(", ptr ")
                .and_then(|(_, rest)| rest.split(',').next())
                .unwrap_or_else(|| panic!("no address in {l}"));
            !lines
                .iter()
                .any(|m| m.starts_with(&format!("{address} = alloca ")))
        })
        .map(|l| {
            (
                l.split(' ').find(|w| *w == "load" || *w == "store"),
                tbaa_name(&ir, l),
            )
        })
        .collect();
    assert_eq!(
        accesses,
        [
            (Some("load"), "P.y"),
            (Some("store"), "P.x"),
            (Some("load"), "Q.x"),
            (Some("store"), "P.y"),
        ],
        "{body}"
    );
}

/// The instructions of `function`'s entry block, in order.
fn entry_instructions(function: FunctionValue<'_>) -> Vec<InstructionValue<'_>> {
    let first = function
        .get_first_basic_block()
        .and_then(BasicBlock::get_first_instruction);
    std::iter::successors(first, |i| i.get_next_instruction()).collect()
}

/// Every instruction of `function`, block by block, in order.
fn instructions(function: FunctionValue<'_>) -> Vec<InstructionValue<'_>> {
    function
        .get_basic_block_iter()
        .flat_map(|block| {
            std::iter::successors(block.get_first_instruction(), |i| i.get_next_instruction())
        })
        .collect()
}

/// Where `value` came from through the slots of locals in `code`, straight-line instructions in
/// order: while it is a load of an `alloca`, the value of the last store into that slot before
/// the load.
fn stored_origin<'ctx>(
    code: &[InstructionValue<'ctx>],
    value: BasicValueEnum<'ctx>,
) -> BasicValueEnum<'ctx> {
    stored_trail(code, value).0
}

/// `stored_origin`, with the last slot that it went through: the one whose store gave the
/// origin.
fn stored_trail<'ctx>(
    code: &[InstructionValue<'ctx>],
    mut value: BasicValueEnum<'ctx>,
) -> (BasicValueEnum<'ctx>, Option<InstructionValue<'ctx>>) {
    let mut last_slot = None;
    while let Some(load) = value.as_instruction_value().filter(|i| {
        i.get_opcode() == InstructionOpcode::Load
            && operand_of(*i, 0)
                .into_pointer_value()
                .as_instruction()
                .is_some_and(|slot| slot.get_opcode() == InstructionOpcode::Alloca)
    }) {
        let slot = operand_of(load, 0);
        last_slot = slot.into_pointer_value().as_instruction();
        let before = code
            .iter()
            .position(|i| *i == load)
            .expect("the load should be among the instructions");
        let store = code[..before]
            .iter()
            .rev()
            .find(|i| i.get_opcode() == InstructionOpcode::Store && operand_of(**i, 1) == slot)
            .expect("a slot should be stored before it is loaded");
        value = operand_of(*store, 0);
    }
    (value, last_slot)
}

/// Operand `n` of `instruction`, which should be a value, not a block.
fn operand_of(instruction: InstructionValue<'_>, n: u32) -> BasicValueEnum<'_> {
    let Some(Operand::Value(value)) = instruction.get_operand(n) else {
        panic!("operand {n} of {instruction:?} should be a value")
    };
    value
}

/// The instruction whose result is pointer operand `n` of `instruction`.
fn address_of(instruction: InstructionValue<'_>, n: u32) -> InstructionValue<'_> {
    operand_of(instruction, n)
        .into_pointer_value()
        .as_instruction()
        .expect("the pointer should be an instruction's result")
}

/// `binary` takes `&&` and `||` as the eager `and` and `or` of two computed `bool`s, which
/// is what a range pattern's test is in mir: no branch, no second block.
#[test]
fn binary_should_take_and_and_or_as_the_eager_operations_of_two_computed_bools() {
    let context = Context::create();
    let src = "fn main() {}";
    let (program, diagnostics) = check_alone(&parse_ok(src), src, &[]);
    let program = program.unwrap_or_else(|| panic!("{diagnostics:?}"));
    let program = crate::mono::instantiate(program).unwrap_or_else(|found| panic!("{found:?}"));
    let machine = target_machine(OptLevel::O0).expect("the host is a target");
    let mir = mir::build(&program);
    let codegen = Codegen::new(&context, &program, &mir, "test.sisu", src, &machine, false);
    let bool_type = context.bool_type();
    let fn_type = bool_type.fn_type(&[bool_type.into(), bool_type.into()], false);
    let function = codegen.module.add_function("both", fn_type, None);
    codegen
        .builder
        .position_at_end(context.append_basic_block(function, "entry"));
    let param = |n| {
        function
            .get_nth_param(n)
            .expect("the function should have two parameters")
    };
    let span = Span::new(0, 0);
    let both = codegen.binary(BinaryOp::And, Type::BOOL, param(0), param(1), span);
    let either = codegen.binary(BinaryOp::Or, Type::BOOL, both, param(1), span);
    codegen
        .builder
        .build_return(Some(&either))
        .expect(POSITIONED);
    let instructions = entry_instructions(function);
    let opcodes: Vec<_> = instructions.iter().map(|i| i.get_opcode()).collect();
    assert_eq!(
        opcodes,
        [
            InstructionOpcode::And,
            InstructionOpcode::Or,
            InstructionOpcode::Return
        ]
    );
    let [and, or, ret] = instructions[..] else {
        unreachable!("the opcodes should have three entries")
    };
    assert_eq!(
        [operand_of(and, 0), operand_of(and, 1)],
        [param(0), param(1)]
    );
    assert_eq!([operand_of(or, 0), operand_of(or, 1)], [both, param(1)]);
    assert_eq!(operand_of(ret, 0), either);
}

/// A string's length is field 1 of its header, read without a tag; an array's is field 1 of
/// its own header, the `i64` that carries the "array header" tag. Each load reads the
/// field of the pointer its own parameter gave, through the slots it moved through: `len`
/// picks by the base's type.
#[test]
fn len_should_read_a_string_by_its_field_and_an_array_by_its_header() {
    let context = Context::create();
    let module = compiled(
        &context,
        "fn size(s: string, a: [i64]) -> i64 {\n    s.len() + a.len()\n}\nfn main() {}",
    );
    let ir = module.print_to_string().to_string();
    let function = module
        .get_function("sisu.size")
        .expect("the module should define `size`");
    let param = |n| {
        function
            .get_nth_param(n)
            .expect("the function should have two parameters")
    };
    let entry = entry_instructions(function);
    let loads: Vec<_> = entry
        .iter()
        .filter(|i| {
            i.get_opcode() == InstructionOpcode::Load
                && address_of(**i, 0).get_opcode() == InstructionOpcode::GetElementPtr
        })
        .map(|&load| {
            let gep = address_of(load, 0);
            let text = load.print_to_string().to_string();
            (
                gep.get_gep_source_element_type()
                    .expect("a GEP should have a source element type")
                    .print_to_string()
                    .to_string(),
                operand_of(gep, 2)
                    .into_int_value()
                    .get_zero_extended_constant(),
                stored_origin(&entry, operand_of(gep, 0)),
                text.contains("!tbaa").then(|| tbaa_name(&ir, &text)),
            )
        })
        .collect();
    assert_eq!(
        loads,
        [
            (
                "{ i64, i64, i64, [0 x i8] }".to_string(),
                Some(1),
                param(0),
                None
            ),
            (
                "{ i64, i64, i64 }".to_string(),
                Some(1),
                param(1),
                Some("array header")
            ),
        ],
        "{ir}"
    );
}

/// `words()` of a string that is not a local releases it, once the words are copied out; of a
/// local it leaves the string to its owner.
#[test]
fn words_releases_a_base_that_is_not_a_local() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "fn make() -> string { \"a \" + \"b\" }\nfn value() -> [string] { make().words() }\n\
         fn local() -> i64 {\n    let s = make()\n    let ws = s.words()\n    s.len()\n}\nfn main() {}",
    )
    .print_to_string()
    .to_string();
    let value = function_body(&ir, "define internal ptr @sisu.value()");
    let local = function_body(&ir, "define internal i64 @sisu.local()");
    let release = "@sisu_rc.release.string(";
    assert_eq!(value.matches(release).count(), 1, "{value}");
    // The local's one release is its scope end: `words()` itself released nothing.
    assert_eq!(local.matches(release).count(), 1, "{local}");
}

/// Only `drop.[[Node]]` asks for `release.[Node]`: the worklist must define it too.
#[test]
fn nested_array_helpers_are_defined() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "class Node {}\nfn main() {\n    let g = [[Node()]]\n}",
    )
    .print_to_string()
    .to_string();
    for wanted in [
        "define internal void @\"sisu_rc.drop.[[Node]]\"(ptr",
        "define internal void @\"sisu_rc.release.[Node]\"(ptr",
    ] {
        assert!(ir.contains(wanted), "missing {wanted:?} in\n{ir}");
    }
}

/// A base that is not a local is released, after the element is loaded.
#[test]
fn general_path_releases_base() {
    let context = Context::create();
    let module = compiled(
        &context,
        "fn make() -> [i64] { [1] }\nfn f() -> i64 { make()[0] }\nfn main() {}",
    );
    let ir = module.print_to_string().to_string();
    let f = module
        .get_function("sisu.f")
        .expect("the module should define `f`");
    let all = instructions(f);
    let ret = all
        .iter()
        .find(|i| i.get_opcode() == InstructionOpcode::Return)
        .unwrap_or_else(|| panic!("no `ret` in\n{ir}"));
    // `f` returns the element it loaded, through its locals' slots.
    let element = stored_origin(&all, operand_of(*ret, 0))
        .as_instruction_value()
        .unwrap_or_else(|| panic!("`f` should return a loaded value in\n{ir}"));
    assert_eq!(
        address_of(element, 0).get_opcode(),
        InstructionOpcode::GetElementPtr,
        "{ir}"
    );
    let at = |wanted: &dyn Fn(&InstructionValue<'_>) -> bool| all.iter().position(wanted);
    let load = at(&|i| *i == element);
    let release = at(&|i| {
        i.get_opcode() == InstructionOpcode::Call
            && i.print_to_string()
                .to_string()
                .contains("@\"sisu_rc.release.[i64]\"(")
    })
    .unwrap_or_else(|| panic!("no release of the base in\n{ir}"));
    assert!(load.is_some_and(|load| load < release), "{ir}");
}

/// An index built only from whitelisted nodes reads the base in place; one with a call copies
/// the base, which is released before the element is printed. `call` indexes a `mut` field, `q.xs`:
/// a borrow of a whole local survives a call, so pair removal (`mir/pairs.rs`) would make a
/// copy of a whole local a borrow. In `moved` the index's read of `ys` is its last use, which
/// the ownership pass makes a move, and `xs` is still read in place:
/// `base_borrowed_when_the_sibling_moves_another_local` (`mir/tests/lower.rs`) covers that rule.
#[test]
fn fast_path_follows_the_index_whitelist() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "class O {\n    n: i64\n}\nclass Q {\n    mut xs: [i64]\n}\nfn g() -> i64 { 0 }\n\
         fn unary(xs: [i64], i: i64) -> [i64] {\n    print(xs[-i + 1])\n    xs\n}\n\
         fn convert(xs: [i64], b: u8) -> [i64] {\n    print(xs[i64(b)])\n    xs\n}\n\
         fn field(xs: [i64], o: O) -> [i64] {\n    print(xs[o.n])\n    print(o.n)\n    xs\n}\n\
         fn length(xs: [i64], ys: [bool]) -> [i64] {\n    print(xs[ys.len()])\n    print(ys.len())\n    xs\n}\n\
         fn call(q: Q) -> Q {\n    print(q.xs[1 + g()])\n    q\n}\n\
         fn moved(xs: [i64]) -> [i64] {\n    let ys = [true]\n    print(xs[ys.len()])\n    xs\n}\nfn main() {}",
    )
    .print_to_string()
    .to_string();
    for (name, fast) in [
        ("unary", true),
        ("convert", true),
        ("field", true),
        ("length", true),
        ("call", false),
        ("moved", true),
    ] {
        let body = function_body(&ir, &format!("define internal ptr @sisu.{name}("));
        let (before, _) = body
            .split_once("call void @sisu_print_int(")
            .unwrap_or_else(|| panic!("no print in\n{body}"));
        let released = before.contains("@\"sisu_rc.release.[i64]\"(");
        assert_eq!(released, !fast, "{name}:\n{body}");
    }
}

/// An element write checks the index before `unique` copies the array, so an
/// out-of-bounds write copies nothing, and stores the value into the unique array.
#[test]
fn write_checks_bounds_before_unique() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "fn f(xs: [i64]) -> [i64] {\n    let mut ys = xs\n    ys[0] = 1\n    ys\n}\nfn main() {}",
    )
    .print_to_string()
    .to_string();
    let body = function_body(&ir, "@sisu.f(");
    let lines: Vec<&str> = body.lines().map(str::trim).collect();
    let find = |wanted: &dyn Fn(&str) -> bool, what: &str| {
        instruction(&lines, wanted)
            .unwrap_or_else(|| panic!("no {what} in\n{body}"))
            .0
    };
    // `check_bounds` panics when `index uge len`: `0 <= index < len` negated.
    let bounds = find(&|t| t.starts_with("icmp uge i64 0, "), "bounds check");
    let unique = find(
        &|t| t.starts_with("call ptr @\"sisu_rc.unique.[i64]\"(ptr "),
        "`unique` call",
    );
    let store = lines
        .iter()
        .position(|l| l.starts_with("store i64 1, ptr "))
        .unwrap_or_else(|| panic!("no store of 1 in\n{body}"));
    assert!([bounds, unique, store].is_sorted(), "{body}");
}

/// The slot of the first parameter of the function whose text is `body`: where the entry block
/// stores that parameter.
fn first_parameter_slot(body: &str) -> &str {
    let (params, _) = body
        .split_once(')')
        .unwrap_or_else(|| panic!("no parameter list in\n{body}"));
    let param = params
        .trim_start_matches("ptr ")
        .split(',')
        .next()
        .expect("a split should yield a first part");
    let entry_store = format!("store ptr {param}, ptr ");
    body.lines()
        .find_map(|l| l.trim().strip_prefix(entry_store.as_str()))
        .and_then(|rest| rest.split(',').next())
        .unwrap_or_else(|| panic!("no store of {param} in\n{body}"))
}

/// `Move` takes the local's reference and leaves `null` in its slot only where a conditional
/// release reads the slot (or under the RC checks, which test it): `xs` has no `release?`, so
/// the plain build stores nothing.
#[test]
fn move_stores_null_only_for_a_flag() {
    let context = Context::create();
    let src = "fn f(xs: [i64]) -> [i64] { xs }\nfn main() {}";
    for (rc_checks, stores) in [(false, false), (true, true)] {
        let ir = compiled_with(&context, src, rc_checks)
            .print_to_string()
            .to_string();
        let body = function_body(&ir, "@sisu.f(");
        let slot = first_parameter_slot(body);
        assert_eq!(
            body.contains(&format!("store ptr null, ptr {slot},")),
            stores,
            "{body}"
        );
    }
}

/// `xs` is moved on one branch and released at the join by a `release?`, which reads the zero
/// that the move left: the plain build keeps the store.
#[test]
fn conditional_release_keeps_null_store() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "fn keep(p: [i64]) {\n    let kept = p\n}\nfn f(xs: [i64], c: bool) {\n    if c {\n        keep(xs)\n    }\n}\nfn main() {}",
    )
    .print_to_string()
    .to_string();
    let body = function_body(&ir, "@sisu.f(");
    let slot = first_parameter_slot(body);
    assert!(
        body.contains(&format!("store ptr null, ptr {slot},")),
        "{body}"
    );
}

/// A static `release s` tests its slot under the RC checks: a null slot is a read after a move,
/// kind 2, and the test comes before the release helper's call. The plain build has no check.
#[test]
fn static_release_checks_its_slot_under_rc_checks() {
    let context = Context::create();
    let src = "fn name() -> string { \"n\" }\nfn f() {\n    let s = name()\n}\nfn main() {}";
    let plain = compiled(&context, src).print_to_string().to_string();
    let body = function_body(&plain, "@sisu.f(");
    assert!(!body.contains("@sisu_rc_fault"), "{body}");
    let checked = compiled_with(&context, src, true)
        .print_to_string()
        .to_string();
    let body = function_body(&checked, "@sisu.f(");
    let lines: Vec<&str> = body.lines().map(str::trim).collect();
    let release_at = lines
        .iter()
        .position(|l| l.starts_with("call void @sisu_rc.release.string("))
        .unwrap_or_else(|| panic!("no release in\n{body}"));
    // `s`'s slot is where the call to `name` stores its result.
    let (_, value, _) = instruction(&lines, &|t| t.starts_with("call ptr @sisu.name("))
        .unwrap_or_else(|| panic!("no call to name in\n{body}"));
    let store = format!("store ptr {value}, ptr ");
    let slot = lines
        .iter()
        .find_map(|l| l.strip_prefix(store.as_str()))
        .and_then(|rest| rest.split(',').next())
        .unwrap_or_else(|| panic!("no store of {value} in\n{body}"));
    let (_, loaded, _) = instruction(&lines, &|t| {
        t.starts_with(&format!("load ptr, ptr {slot},"))
    })
    .unwrap_or_else(|| panic!("the release should read {slot} in\n{body}"));
    let (_, null, _) = instruction(&lines, &|t| t == format!("icmp eq ptr {loaded}, null"))
        .unwrap_or_else(|| panic!("no null test of {loaded} in\n{body}"));
    let branch_at = assert_faults_on(&lines, null, 2, body);
    assert!(branch_at < release_at, "{body}");
}

/// `.config/nextest.toml` kills this test after 6 s (12 s in the `ci` profile), matching it by
/// name: a rename must update the override there. Each `+=` is an overflow panic site, and finding
/// its line and column from the start of the text, or building a line index for each site, made
/// codegen quadratic in the number of sites. No step counter sees such a new scan.
#[test]
fn many_panic_sites_should_compile_in_linear_time() {
    let src = format!(
        "fn main() {{\n    let mut x = 0\n{}    print(x)\n}}\n",
        "    x += 1\n".repeat(25_000)
    );
    compiled(&Context::create(), &src);
}

/// No float instruction carries a fast-math flag, so `-O2` keeps every result exact to IEEE
/// 754. The operands come from the arguments' count, so `-O2` cannot fold the operations away,
/// and each comparison is printed alone: together, `s <= b || s >= b || s != b` folds to true.
/// `a` is a quotient, NaN when the count is 0: from LLVM 23, the `instcombine` pass adds `nnan`
/// or `ninf` where it proves no operand or result is NaN or infinite, as for the product of two
/// counts.
#[test]
fn float_ops_have_no_fast_math() {
    let context = Context::create();
    let module = compiled(
        &context,
        "fn ops(a: f64, b: f64) -> bool {\n    let s = -(a + b - a * b / a % b)\n    print(s < a)\n    print(s <= b)\n    print(s > a)\n    print(s >= b)\n    print(s == a)\n    s != b\n}\n\nfn main(args: [string]) {\n    let n = args.len()\n    print(ops(f64(n) / f64(n), f64(n + 1)))\n}",
    );
    let machine = target_machine(OptLevel::O2).expect("the host is a target");
    optimize(&module, &machine, OptLevel::O2).expect("the module should optimize");
    let ir = module.print_to_string().to_string();
    for wanted in ["fadd", "frem", "fcmp une"] {
        assert!(ir.contains(wanted), "missing {wanted:?} in\n{ir}");
    }
    let opcodes = ["fadd", "fsub", "fmul", "fdiv", "frem", "fneg", "fcmp"];
    let flags = [
        "fast", "nnan", "ninf", "nsz", "arcp", "contract", "afn", "reassoc",
    ];
    let mut seen = 0;
    for line in ir.lines() {
        let Some((_, text)) = line.split_once(" = ") else {
            continue;
        };
        let mut words = text.split_whitespace();
        if !words.next().is_some_and(|op| opcodes.contains(&op)) {
            continue;
        }
        seen += 1;
        // The words between the opcode and the type: `fcmp`'s predicate, or a flag.
        for word in words.take_while(|w| *w != "double") {
            assert!(!flags.contains(&word), "{line}");
        }
    }
    assert!(seen >= 7, "{ir}");
}

/// `i64(x)` from an `f64` converts with the saturating `llvm.fptosi.sat`, never a plain
/// `fptosi`, which is poison out of range: the range check and the round trip decide the flag.
#[test]
fn float_to_int_saturates() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "fn f(x: f64) -> i64? { i64(x) }\n\nfn main() {\n    print(f(2.0) ?? 0)\n}",
    )
    .print_to_string()
    .to_string();
    let body = function_body(&ir, "define internal { i1, i64 } @sisu.f(");
    assert!(body.contains("@llvm.fptosi.sat.i64.f64"), "{body}");
    assert!(
        !body.lines().any(|l| l.contains(" fptosi ")),
        "a plain `fptosi` in\n{body}"
    );
    // The range check is ordered, so `nan` is out of range, and the round trip is `oeq`.
    for wanted in [
        "fcmp oge double",
        "fcmp olt double", // spellchecker:disable-line
        "sitofp",
        "fcmp oeq double",
    ] {
        assert!(body.contains(wanted), "missing {wanted:?} in\n{body}");
    }
}

/// The names of the `@sisu_` functions `ir` mentions.
fn runtime_symbols(ir: &str) -> std::collections::BTreeSet<&str> {
    ir.split(|c: char| !(c.is_alphanumeric() || matches!(c, '@' | '_' | '.')))
        .filter(|word| word.starts_with("@sisu_"))
        .collect()
}

/// An `f64` method is an LLVM intrinsic, except `hypot`, which is the one `libm` call, and none
/// adds a runtime function. `min`, `max` and `clamp` are `minimumnum` and `maximumnum`: `minnum`
/// and `maxnum` leave the sign of a zero result unspecified, and LLVM picks it differently at
/// `-O0` and `-O2`.
#[test]
fn methods_lower_to_intrinsics_and_libm() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "fn f(x: f64, y: f64) -> f64 {\n    x.sqrt() + x.round() + x.min(y) + x.max(y) + x.clamp(y, y) + x.hypot(y) + x.abs() + x.ln() + x.pow(y) + x.atan2(y)\n}\n\nfn main() {\n    let _ = f(1.0, 2.0)\n}",
    )
    .print_to_string()
    .to_string();
    for wanted in [
        "@llvm.sqrt.f64",
        "@llvm.round.f64",
        "@llvm.minimumnum.f64",
        "@llvm.maximumnum.f64",
        "@llvm.fabs.f64",
        "@llvm.log.f64",
        "@llvm.pow.f64",
        "@llvm.atan2.f64",
        "declare double @hypot(double, double)",
    ] {
        assert!(ir.contains(wanted), "missing {wanted:?} in\n{ir}");
    }
    for unwanted in ["minnum", "maxnum"] {
        assert!(!ir.contains(unwanted), "{unwanted} in\n{ir}");
    }
    let baseline = compiled(
        &context,
        "fn f(x: f64, y: f64) -> f64 { x + y }\n\nfn main() {\n    let _ = f(1.0, 2.0)\n}",
    )
    .print_to_string()
    .to_string();
    assert_eq!(runtime_symbols(&ir), runtime_symbols(&baseline));
}

/// A module that calls no method that needs `hypot` does not declare it.
#[test]
fn hypot_is_declared_on_first_use() {
    let context = Context::create();
    let ir = compiled(&context, "fn main() {\n    let _ = 1.0.sqrt()\n}")
        .print_to_string()
        .to_string();
    assert!(!ir.contains("hypot"), "{ir}");
}

/// The classification methods are float compares, not calls: `is_nan` is `uno x, x`, and the
/// others compare `fabs x` with infinity.
#[test]
fn classification_is_float_compares() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "fn nan(x: f64) -> bool { x.is_nan() }\nfn finite(x: f64) -> bool { x.is_finite() }\nfn infinite(x: f64) -> bool { x.is_infinite() }\nfn main() {\n    let _ = nan(1.0) || finite(1.0) || infinite(1.0)\n}",
    )
    .print_to_string()
    .to_string();
    let nan = function_body(&ir, "define internal i1 @sisu.nan(");
    assert!(nan.contains("fcmp uno double"), "{nan}");
    assert!(!nan.contains("call"), "{nan}");
    let finite = function_body(&ir, "define internal i1 @sisu.finite(");
    assert!(finite.contains("@llvm.fabs.f64"), "{finite}");
    assert!(finite.contains("fcmp olt double"), "{finite}"); // spellchecker:disable-line
    assert!(finite.contains("+inf"), "{finite}");
    let infinite = function_body(&ir, "define internal i1 @sisu.infinite(");
    assert!(infinite.contains("@llvm.fabs.f64"), "{infinite}");
    assert!(infinite.contains("fcmp oeq double"), "{infinite}");
    assert!(infinite.contains("+inf"), "{infinite}");
}

/// `i64` `min`, `max` and `clamp` are the prelude's methods, and at `-O2` the inliner makes each
/// an `llvm.smin.i64` or `llvm.smax.i64` call, with no call of a prelude function left.
#[test]
fn integer_min_max_clamp_should_be_smin_and_smax_at_o2() {
    let context = Context::create();
    let module = compiled_over(
        &context,
        crate::front_end::PRELUDE_SOURCE,
        "fn lo(a: i64, b: i64) -> i64 { a.min(b) }\nfn hi(a: i64, b: i64) -> i64 { a.max(b) }\nfn both(a: i64, b: i64, c: i64) -> i64 { a.clamp(b, c) }\nfn main(args: [string]) {\n    let a = 5 - args.len()\n    let b = args.len() - 7\n    print(lo(a, b) + hi(a, b) + both(a, b, 3 - a))\n}",
        false,
        |_| {},
    );
    keep_functions(&module, "lo,hi,both");
    let machine = target_machine(OptLevel::O2).expect("the host is a target");
    optimize(&module, &machine, OptLevel::O2).expect("the module should optimize");
    let ir = module.print_to_string().to_string();
    let lo = function_body(&ir, "@sisu.lo(");
    let hi = function_body(&ir, "@sisu.hi(");
    let both = function_body(&ir, "@sisu.both(");
    assert!(
        lo.contains("@llvm.smin.i64") && !lo.contains("@llvm.smax.i64"),
        "{lo}"
    );
    assert!(
        hi.contains("@llvm.smax.i64") && !hi.contains("@llvm.smin.i64"),
        "{hi}"
    );
    assert!(
        both.contains("@llvm.smin.i64") && both.contains("@llvm.smax.i64"),
        "{both}"
    );
    for body in [lo, hi, both] {
        assert!(!body.contains("@sisu.i64."), "{body}");
    }
}

/// The names of the two hidden last parameters of the function that starts with `header` in
/// `ir`, a location pointer and its length, after its one array parameter.
fn hidden_location(ir: &str, header: &str) -> (String, String) {
    let (_, rest) = ir
        .split_once(header)
        .unwrap_or_else(|| panic!("no {header} in\n{ir}"));
    let (params, _) = rest.split_once(')').expect("the header should close");
    let names: Vec<&str> = params
        .split(", ")
        .map(|param| param.rsplit(' ').next().expect("a parameter has a name"))
        .collect();
    let [_, at, at_len] = names[..] else {
        panic!("the function should take its array and the location: {params}");
    };
    assert!(params.contains("ptr") && params.contains("i64"), "{params}");
    (at.to_string(), at_len.to_string())
}

/// A prelude function takes its caller's location as two hidden last parameters, a pointer and a
/// length. Its overflow check and its bounds check pass them to the runtime, and a call from the
/// user's `main` passes the location of the call.
#[test]
fn a_prelude_function_should_report_the_location_it_got_from_its_caller() {
    let context = Context::create();
    let module = compiled(&context, "fn main() {\n    print([1, 2].sum())\n}");
    let ir = module.print_to_string().to_string();
    let header = "define internal i64 @\"sisu.[i64].sum\"(";
    let (at, at_len) = hidden_location(&ir, header);
    let sum = function_body(&ir, header);
    let overflow =
        format!("call void @sisu_panic_message(ptr {at}, i64 {at_len}, ptr @report.text");
    assert!(sum.contains(&overflow), "{overflow} in\n{sum}");
    let bounds = format!("call void @sisu_panic_bounds(ptr {at}, i64 {at_len}, i64 ");
    assert!(sum.contains(&bounds), "{bounds} in\n{sum}");
    // The call starts at the `[` on line 2, column 11 of the source.
    let location = "test.sisu:2:11";
    let constant = format!("c\"{location}\\00\"");
    let global = ir
        .lines()
        .find(|l| l.contains(&format!("{constant},")))
        .and_then(|l| l.split(" = ").next())
        .unwrap_or_else(|| panic!("no constant {constant} in\n{ir}"));
    let main = function_body(&ir, "define internal void @sisu.main(");
    let call = format!(", ptr {global}, i64 {})", location.len());
    assert!(
        main.contains("call i64 @\"sisu.[i64].sum\"(ptr ") && main.contains(&call),
        "{call} in\n{main}"
    );
}

/// The prelude's `panic` names the location of the user's call, like its checks do. A location
/// baked from a span of the prelude would be an internal error.
#[test]
fn panic_in_a_prelude_function_should_report_the_location_of_its_caller() {
    let context = Context::create();
    let prelude =
        "extend [i64] {\n    fn probe(self) -> i64 {\n        panic(\"boom\")\n    }\n}\n";
    let module = compiled_over(
        &context,
        prelude,
        "fn main() {\n    print([1, 2].probe())\n}",
        false,
        |_| {},
    );
    let ir = module.print_to_string().to_string();
    let header = "define internal i64 @\"sisu.[i64].probe\"(";
    let (at, at_len) = hidden_location(&ir, header);
    let panic = format!("call void @sisu_panic_message(ptr {at}, i64 {at_len}, ");
    let probe = function_body(&ir, header);
    assert!(probe.contains(&panic), "{panic} in\n{probe}");
}

/// The prelude's `dbg` names the location of the user's call. The checker cannot read the text of
/// a `dbg` in the prelude, so this marks a user function as the prelude's in the mir.
#[test]
fn dbg_in_a_prelude_function_should_report_the_location_of_its_caller() {
    let context = Context::create();
    let module = compiled_from(
        &context,
        "fn probe(xs: [i64]) -> i64 {\n    dbg(xs)\n    1\n}\nfn main() {\n    print(probe([1, 2]))\n}",
        false,
        |mir| {
            let probe = (mir.functions.iter_mut())
                .find(|f| f.name == "probe")
                .expect("the program should have `probe`");
            probe.prelude = true;
        },
    );
    let ir = module.print_to_string().to_string();
    let header = "define internal i64 @sisu.probe(";
    let (at, at_len) = hidden_location(&ir, header);
    let probe = function_body(&ir, header);
    let shows_location = |line: &&str| {
        line.contains("@sisu_show_str(") && line.contains(&format!("ptr {at}, i64 {at_len})"))
    };
    assert!(
        probe.lines().any(|line| shows_location(&line)),
        "a `sisu_show_str` of ({at}, {at_len}) in\n{probe}"
    );
}

/// At `-O2` the inliner removes the call of `sum` and its location parameters.
#[test]
fn sum_should_be_inlined_at_o2() {
    let context = Context::create();
    let module = compiled(&context, "fn main() {\n    print([1, 2].sum())\n}");
    let machine = target_machine(OptLevel::O2).expect("the host is a target");
    optimize(&module, &machine, OptLevel::O2).expect("the module should optimize");
    let ir = module.print_to_string().to_string();
    assert!(!ir.contains("[i64].sum"), "{ir}");
    assert!(!ir.contains("sisu_panic_message"), "{ir}");
}

/// A swap in a loop, as quicksort's partition does it, makes its array unique once: `-O2` keeps
/// one copy path, the hoisted `unique` before the loop, inlined, and at most the bounds checks
/// of `a[j]` and `a[i]`, but drops those of the writes. It needs the TBAA tags, so that an
/// element store keeps the length; without them `-O2` keeps three bounds checks. The loop has
/// no copy path, so the order in which `unique` releases and allocates is guarded by
/// `a_barred_partition_makes_its_array_unique_once` instead.
#[test]
fn partition_makes_its_array_unique_once() {
    let context = Context::create();
    let module = compiled(
        &context,
        "fn partition(xs: [i64], pivot: i64) -> [i64] {\n    let mut a = xs\n    let mut i = 0\n    for j in 0..a.len() {\n        if a[j] < pivot {\n            let t = a[i]\n            a[i] = a[j]\n            a[j] = t\n            i += 1\n        }\n    }\n    a\n}\n\nfn main() {\n    print(partition([3, 1, 2], 2).sum())\n}",
    );
    keep_functions(&module, "partition");
    let machine = target_machine(OptLevel::O2).expect("the host is a target");
    optimize(&module, &machine, OptLevel::O2).expect("the module should optimize");
    let ir = module.print_to_string().to_string();
    let partition = function_body(&ir, "@sisu.partition(");
    assert_eq!(
        partition.matches("call ptr @sisu_alloc(").count(),
        1,
        "{partition}"
    );
    let bounds = partition.matches("call void @sisu_panic_bounds(").count();
    assert!(bounds <= 2, "{bounds} bounds checks in\n{partition}");
}

/// `ys[i % ys.len()]` with a non-negative `i` is in range only because a length is never
/// negative: the `!range` on the length load lets `-O2` drop its bounds check.
#[test]
fn an_index_modulo_the_length_should_have_no_bounds_check_at_o2() {
    let context = Context::create();
    let module = compiled(
        &context,
        "fn wrap(ys: [i64], n: i64) -> i64 {\n    let mut s = 0\n    for i in 0..n {\n        s += ys[i % ys.len()]\n    }\n    s\n}\n\nfn main(args: [string]) {\n    print(wrap([1, 2, 3], args.len()))\n}",
    );
    keep_functions(&module, "wrap");
    let machine = target_machine(OptLevel::O2).expect("the host is a target");
    optimize(&module, &machine, OptLevel::O2).expect("the module should optimize");
    let ir = module.print_to_string().to_string();
    let wrap = function_body(&ir, "@sisu.wrap(");
    assert!(wrap.contains("rem i64"), "{wrap}");
    assert!(!wrap.contains("@sisu_panic_bounds("), "{wrap}");
}

/// `binary_trees`' `Node.check` borrows `self.left` and `self.right` and passes each payload in
/// place: its `-O2` IR calls no counting helper.
#[test]
fn optional_field_method_counts_nothing() {
    let context = Context::create();
    let module = compiled(
        &context,
        "class Node {\n    left: Node?\n    right: Node?\n\n    fn check(self) -> i64 {\n        1 + (self.left?.check() ?? 0) + (self.right?.check() ?? 0)\n    }\n}\n\n\
         fn make(depth: i64) -> Node {\n    if depth == 0 {\n        Node(left: None, right: None)\n    } else {\n        Node(left: make(depth - 1), right: make(depth - 1))\n    }\n}\n\n\
         fn main() {\n    print(make(4).check())\n}",
    );
    keep_functions(&module, "Node.check");
    let machine = target_machine(OptLevel::O2).expect("the host is a target");
    optimize(&module, &machine, OptLevel::O2).expect("the module should optimize");
    let ir = module.print_to_string().to_string();
    let check = function_body(&ir, "define internal fastcc i64 @sisu.Node.check(");
    assert!(!check.contains("@sisu_rc."), "{check}");
}

/// `partition_makes_its_array_unique_once` with a `return a` in the loop, which bars the hoist:
/// the writes in the loop call `unique` again, so the copy path that the TBAA tags and
/// `unique`'s early release fold away is still there to guard. Same assertions.
#[test]
fn a_barred_partition_makes_its_array_unique_once() {
    let context = Context::create();
    let module = compiled(
        &context,
        "fn partition(xs: [i64], pivot: i64, stop: i64) -> [i64] {\n    let mut a = xs\n    let mut i = 0\n    for j in 0..a.len() {\n        if j == stop {\n            return a\n        }\n        if a[j] < pivot {\n            let t = a[i]\n            a[i] = a[j]\n            a[j] = t\n            i += 1\n        }\n    }\n    a\n}\n\nfn main() {\n    print(partition([3, 1, 2], 2, 5).sum())\n}",
    );
    keep_functions(&module, "partition");
    let machine = target_machine(OptLevel::O2).expect("the host is a target");
    optimize(&module, &machine, OptLevel::O2).expect("the module should optimize");
    let ir = module.print_to_string().to_string();
    let partition = function_body(&ir, "@sisu.partition(");
    assert_eq!(
        partition.matches("call ptr @sisu_alloc(").count(),
        1,
        "{partition}"
    );
    let bounds = partition.matches("call void @sisu_panic_bounds(").count();
    assert!(bounds <= 2, "{bounds} bounds checks in\n{partition}");
}

/// Objects and an array that never leave a loop iteration are not allocated at `-O2`:
/// `sisu_alloc` and `sisu_free` are declared as an allocator pair.
#[test]
fn objects_that_stay_local_are_not_allocated_at_o2() {
    let context = Context::create();
    let module = compiled(
        &context,
        "class Point {\n    x: i64\n    y: i64\n}\n\nfn dist(a: Point, b: Point) -> i64 {\n    (a.x - b.x) * (a.x - b.x) + (a.y - b.y) * (a.y - b.y)\n}\n\nfn f(n: i64) -> i64 {\n    let mut total = 0\n    for i in 0..n {\n        let p = Point(x: i, y: i + 1)\n        let q = Point(x: 2 * i, y: 3)\n        total += dist(p, q) % 7\n        let xs = [i, i + 1, i + 2]\n        total += xs[1] % 5\n    }\n    total\n}\n\nfn main() {\n    print(f(10))\n}",
    );
    keep_functions(&module, "f");
    let ir = module.print_to_string().to_string();
    let f = function_body(&ir, "@sisu.f(");
    assert_eq!(f.matches("@sisu_alloc(").count(), 3, "{f}");
    let machine = target_machine(OptLevel::O2).expect("the host is a target");
    optimize(&module, &machine, OptLevel::O2).expect("the module should optimize");
    let ir = module.print_to_string().to_string();
    let f = function_body(&ir, "@sisu.f(");
    assert_eq!(f.matches("@sisu_alloc(").count(), 0, "{f}");
}

/// `[true; n]` and `[b; n]` fill with one `memset` at `-O2`, as `[0; n]` does: the fill stores
/// a byte, which LLVM's loop-idiom pass turns into a `memset`, and an `i1` store it does not.
#[test]
fn a_bool_fill_is_a_memset_at_o2() {
    let context = Context::create();
    let module = compiled(
        &context,
        "fn f(n: i64) -> [bool] {\n    [true; n]\n}\n\nfn g(b: bool, n: i64) -> [bool] {\n    [b; n]\n}\n\nfn main(args: [string]) {\n    let n = args.len()\n    print(f(n).len() + g(n > 1, n).len())\n}",
    );
    keep_functions(&module, "f,g");
    let machine = target_machine(OptLevel::O2).expect("the host is a target");
    optimize(&module, &machine, OptLevel::O2).expect("the module should optimize");
    let ir = module.print_to_string().to_string();
    for name in ["f", "g"] {
        let body = function_body(&ir, &format!("@sisu.{name}("));
        assert!(body.contains("@llvm.memset"), "{body}");
    }
}

/// `compare`'s `repeats`: pair removal borrows both elements that the inner loop compares, so
/// at `-O2` the function retains nothing. Before it, each comparison retained both. `-O2`
/// inlines `@sisu_rc.retain`, so the test also rejects its inlined blocks (`sisu_rc.retain.exit`).
#[test]
fn element_comparison_in_a_loop_retains_nothing() {
    let context = Context::create();
    let module = compiled(
        &context,
        "fn repeats(xs: [string]) -> i64 {\n    let mut n = 0\n    for i in 0..xs.len() {\n        for j in 0..i {\n            if xs[i] == xs[j] { n += 1 }\n        }\n    }\n    n\n}\n\nfn main() {\n    print(repeats([\"a\", \"b\", \"a\"]))\n}",
    );
    keep_functions(&module, "repeats");
    let machine = target_machine(OptLevel::O2).expect("the host is a target");
    optimize(&module, &machine, OptLevel::O2).expect("the module should optimize");
    let ir = module.print_to_string().to_string();
    let repeats = function_body(&ir, "@sisu.repeats(");
    assert!(!repeats.contains("sisu_rc.retain"), "{repeats}");
}

/// `look` passes its borrowed array to `size`, which borrows it too: at `-O2` neither function
/// counts it. Before inference, `look` moved `xs` into `size`, which released it.
#[test]
fn size_of_a_borrowed_array_counts_nothing() {
    let context = Context::create();
    let module = compiled(
        &context,
        "fn size(xs: [i64]) -> i64 {\n    xs.len()\n}\nfn look(xs: [i64]) -> i64 { size(xs) }\n\
         fn main() {\n    print(look([1, 2]))\n}",
    );
    keep_functions(&module, "look,size");
    let machine = target_machine(OptLevel::O2).expect("the host is a target");
    optimize(&module, &machine, OptLevel::O2).expect("the module should optimize");
    let ir = module.print_to_string().to_string();
    for name in ["look", "size"] {
        let body = function_body(&ir, &format!("@sisu.{name}("));
        assert!(!body.contains("sisu_rc."), "{body}");
    }
}

/// A loop makes the arrays it writes unique before it starts. `f`'s raw IR, split at each
/// `loop.body` label: the count of `unique` calls before the first loop body, then in each
/// piece from a loop body on. The blocks are in mir's order, where a loop's exit block may sit
/// in another loop's piece, so no `f` writes after a loop.
#[test]
fn a_loop_makes_its_written_arrays_unique_before_it_starts() {
    use crate::mir::tests::hoist::{
        APPEND, B6, BOTH_WRITE, GRID, OUTER_BARRED_INNER_HOISTS, PARTITION,
    };
    let cases: [(&str, &str, &[usize]); 6] = [
        ("partition", PARTITION, &[1, 0]),
        ("grid", GRID, &[1, 1]),
        ("nested_both_write", BOTH_WRITE, &[1, 0, 0]),
        ("outer_barred", OUTER_BARRED_INNER_HOISTS, &[0, 1, 0]),
        ("append", APPEND, &[1, 0]),
        ("b6", B6, &[1, 0]),
    ];
    let mut found = Vec::new();
    let mut irs = Vec::new();
    for (name, src, _) in cases {
        let context = Context::create();
        let module = compiled(&context, &format!("{src}\nfn main() {{}}"));
        let ir = module.print_to_string().to_string();
        let f = function_body(&ir, "@sisu.f(");
        let mut pieces = vec![0];
        for line in f.lines() {
            if line.starts_with("loop.body") {
                pieces.push(0);
            }
            *pieces.last_mut().unwrap() += line.matches("call ptr @\"sisu_rc.unique.").count();
        }
        irs.push(format!("{name}:\n{f}"));
        found.push((name, pieces));
    }
    let expected: Vec<_> = cases
        .iter()
        .map(|&(name, _, counts)| (name, counts.to_vec()))
        .collect();
    let irs = irs.join("\n");
    assert_eq!(found, expected, "{irs}");
}

/// Each block is named after its mir index (`bbN`), or after its loop role: `loop.pre` for a
/// pre-header, `loop.body` for a header. Loop blocks compare by role prefix, since the digits
/// LLVM appends to repeated names are its own.
#[test]
fn blocks_are_named_after_their_index_or_loop_role() {
    let context = Context::create();
    let module = compiled(
        &context,
        "fn f() {\n    for i in 0..2 {\n        for j in 0..3 {\n            print(j)\n        }\n    }\n\
             for k in 0..4 {\n        print(k)\n    }\n}\nfn main() {}",
    );
    let f = module
        .get_function("sisu.f")
        .expect("`f` should be defined");
    let names: Vec<String> = f
        .get_basic_block_iter()
        .map(|b| b.get_name().to_string_lossy().into_owned())
        .map(|n| {
            if n.starts_with("loop.") {
                n.trim_end_matches(|c: char| c.is_ascii_digit()).to_owned()
            } else {
                n
            }
        })
        .collect();
    let expected = [
        "loop.pre",
        "loop.body",
        "loop.pre",
        "bb3",
        "loop.body",
        "bb5",
        "bb6",
        "bb7",
        "bb8",
        "bb9",
        "loop.pre",
        "loop.body",
        "bb12",
        "bb13",
        "bb14",
        "bb15",
    ];
    assert_eq!(names, expected, "{}", f.print_to_string());
}

/// Whether `==` on an enum is reflexive is worked out once for the program: a walk from each
/// `eq` helper's type reads a chain of enums quadratically.
#[test]
fn reflexivity_should_read_linearly() {
    let reads = |n: usize| {
        let enums = (0..n)
            .map(|i| format!("enum C{i} {{\n    End\n    Next(C{})\n}}\n", i + 1))
            .collect::<Vec<_>>()
            .concat();
        let src = format!(
            "{enums}enum C{n} {{\n    End\n}}\nfn main() {{\n    print(C0.End == C0.End)\n}}\n"
        );
        TYPE_READS.set(0);
        let context = Context::create();
        compiled(&context, &src);
        TYPE_READS.get()
    };
    assert_near_linear(reads(100), reads(200), reads(300));
}

/// The `show` of an enum in a component with boxes is written by one loop over the component. The
/// loop gives each member its index in the `kind` switch. It gives each link the index of the
/// enum it enters. A search of the component for each member or link reads a ring of enums
/// quadratically. A search in the `show` helper of each enum reads it quadratically too. So each
/// enum of the ring links to the next, and `print` writes every enum of the ring.
#[test]
fn show_of_a_component_should_find_each_member_in_linear_steps() {
    let scans = |n: usize| {
        let enums = (0..n)
            .map(|i| format!("enum R{i} {{\n    End\n    Link(R{})\n}}\n", (i + 1) % n))
            .collect::<Vec<_>>()
            .concat();
        let prints = (0..n)
            .map(|i| format!("    print(R{i}.End)\n"))
            .collect::<Vec<_>>()
            .concat();
        let src = format!("{enums}fn main() {{\n{prints}}}\n");
        MEMBER_SCANS.set(0);
        let context = Context::create();
        compiled(&context, &src);
        MEMBER_SCANS.get()
    };
    assert_near_linear(scans(100), scans(200), scans(300));
}

/// The ring of `n` enums of #245. Each enum links to the next. `main` compares every `End` with
/// itself, so each enum gets its `walk`, `release` and `eq` helpers.
fn enum_ring(n: usize) -> String {
    let enums = (0..n)
        .map(|i| format!("enum R{i} {{\n    End\n    Link(R{})\n}}\n", (i + 1) % n))
        .collect::<Vec<_>>()
        .concat();
    let compares = (0..n)
        .map(|i| format!("    print(R{i}.End == R{i}.End)\n"))
        .collect::<Vec<_>>()
        .concat();
    format!("{enums}fn main() {{\n{compares}}}\n")
}

/// `==` on an enum of a component with boxes finds the index of each member and of each link
/// in the component. The `walk` helper of each member finds the index of each boxed field the
/// same way. Each of these is one lookup in the table of the component. A search of the
/// component for any of them reads a ring of enums quadratically.
///
/// The count is exact, so a call site that stops using the lookup helper makes the count fall
/// and fails the test.
#[test]
fn eq_and_walk_of_a_component_should_find_each_member_in_linear_steps() {
    let scans = |n: usize| {
        MEMBER_SCANS.set(0);
        let context = Context::create();
        compiled(&context, &enum_ring(n));
        MEMBER_SCANS.get()
    };
    // Each enum counts 4. The table counts 1. The `walk` of the enum counts 1 lookup for its
    // boxed field. Its `eq` counts 1 lookup for the enum itself and 1 for the link.
    assert_eq!(scans(100), 4 * 100);
    assert_eq!(scans(200), 4 * 200);
}

/// The `walk`, `release` and `drain` helper of each member of a component with boxes uses the
/// type of the heads of the component. A struct type with one element for each member, built
/// and printed at each use, makes the module quadratic in the size of the ring. The heads type
/// is an array, so each use prints it in constant size.
#[test]
fn drop_of_an_enum_component_should_print_the_heads_type_in_linear_size() {
    let size = |n: usize| {
        let context = Context::create();
        compiled(&context, &enum_ring(n))
            .print_to_string()
            .to_bytes()
            .len()
    };
    assert_near_linear(size(100), size(200), size(300));
}

/// A cycle of `n` classes. Each class is held by a function of its own, so each gets a `drop`.
fn class_cycle(n: usize) -> String {
    let classes = (0..n)
        .map(|i| {
            format!(
                "class C{i} {{\n    next: C{}?\n}}\nfn hold{i}(c: C{i}) {{}}\n",
                (i + 1) % n
            )
        })
        .collect::<Vec<_>>()
        .concat();
    format!("{classes}fn main() {{}}\n")
}

/// `drop` and `drain` of each class of a cycle use the type of the heads of the cycle, as the
/// helpers of enums do. Each class is dropped in a function of its own, so each gets a `drop`.
#[test]
fn drop_of_a_class_cycle_should_print_the_heads_type_in_linear_size() {
    let size = |n: usize| {
        let context = Context::create();
        compiled(&context, &class_cycle(n))
            .print_to_string()
            .to_bytes()
            .len()
    };
    assert_near_linear(size(100), size(200), size(300));
}

/// The slot for the heads of a component is zeroed by one `memset` of constant size. A store of
/// a zero array would be one store for each member, so the code of `release` would grow with
/// the ring.
#[test]
fn release_of_an_enum_ring_should_zero_the_heads_with_one_memset() {
    let context = Context::create();
    let ir = compiled(&context, &enum_ring(10))
        .print_to_string()
        .to_string();
    let release = function_body(&ir, "define internal void @sisu_rc.release.R0(ptr");
    assert_eq!(
        release.matches("call void @llvm.memset.").count(),
        1,
        "{release}"
    );
    assert!(!release.contains("store ["), "{release}");
}

#[test]
fn drop_of_a_class_cycle_should_zero_the_heads_with_one_memset() {
    let context = Context::create();
    let ir = compiled(&context, &class_cycle(10))
        .print_to_string()
        .to_string();
    let drop = function_body(&ir, "define internal void @sisu_rc.drop.C0(ptr");
    assert_eq!(drop.matches("call void @llvm.memset.").count(), 1, "{drop}");
    assert!(!drop.contains("store ["), "{drop}");
}

/// A `take_apart` finds each place's token and kept value once: a scan of every token, or of
/// every rest, per binding would visit quadratically many places.
#[test]
fn take_apart_probes_linearly() {
    let probes = |n: usize| {
        let fields = vec!["E"; n].join(", ");
        let parts = (0..n)
            .map(|i| format!("Pair(a{i}, _)"))
            .collect::<Vec<_>>()
            .join(", ");
        let args = (0..n)
            .map(|i| format!("a{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let src = format!(
            "enum E {{\n    Leaf\n    Pair(E, E)\n    Big({fields})\n}}\n\
             fn f(e: E) -> E {{\n    match e {{\n        Big({parts}) => E.Big({args})\n        other => other\n    }}\n}}\n\
             fn main() {{}}"
        );
        TAKE_APART_PROBES.set(0);
        let context = Context::create();
        compiled(&context, &src);
        TAKE_APART_PROBES.get()
    };
    assert_near_linear(probes(100), probes(200), probes(300));
}

/// `swap` takes its scrutinee apart: an unbox of each box, which retains nothing when the box
/// has count 1, where the copying commit retained each binding.
#[test]
fn take_apart_of_swap_retains_nothing() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "enum Expr {\n    Num(i64)\n    Add(Expr, Expr)\n}\n\
         fn swap(e: Expr) -> Expr {\n    match e {\n        Add(l, r) => Expr.Add(r, l)\n        other => other\n    }\n}\n\
         fn main() {}",
    )
    .print_to_string()
    .to_string();
    let header = ir
        .lines()
        .find(|line| line.starts_with("define") && line.contains("@sisu.swap("))
        .unwrap_or_else(|| panic!("no swap in\n{ir}"));
    let swap = function_body(&ir, header);
    assert_eq!(
        swap.matches("call ptr @sisu_rc.unbox.Expr(").count(),
        2,
        "{swap}"
    );
    assert!(!swap.contains("@sisu_rc.retain.Expr("), "{swap}");
}

/// An instantiation's symbol names its type arguments, which LLVM quotes, and each
/// instantiation is defined once however many calls reach it.
#[test]
fn instantiation_symbols_should_name_their_arguments() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "fn first<T>(xs: [T]) -> T? {\n    if xs.len() > 0 { xs[0] } else { None }\n}\n\
         class Stack<T> {\n    mut items: [T]\n    fn push(self, x: T) { self.items = [x] }\n}\n\
         enum E<T> {\n    A(T)\n    fn map<U>(self, u: U) -> [U] { [u] }\n}\n\
         fn main() {\n    print(first([1]) ?? 0)\n    print(first([2]) ?? 0)\n    let s = Stack(items: [\"a\"])\n    s.push(\"b\")\n\
             print(E.A(1).map(\"a\").len())\n}",
    )
    .print_to_string()
    .to_string();
    for symbol in [
        "@\"sisu.first<i64>\"(",
        "@\"sisu.Stack<string>.push\"(",
        "@\"sisu.E<i64>.map<string>\"(",
    ] {
        let defined = ir
            .lines()
            .filter(|line| line.starts_with("define") && line.contains(symbol))
            .count();
        assert_eq!(defined, 1, "{symbol} in\n{ir}");
    }
}

/// The program of the chain `G0<T>` ... `G{n-1}<T>`: `main` builds one `Gk<i64>` for each k and
/// compares it with itself, `G0` first.
fn chain_program(n: usize) -> String {
    chain_program_using(n, |k| format!("print(g{k} == g{k})"))
}

/// The program of the chain `G0<T>` ... `G{n-1}<T>` whose `main` runs `statement(k)` for each
/// `Gk<i64>` that it builds.
fn chain_program_using(n: usize, statement: impl Fn(usize) -> String) -> String {
    let mut src = String::new();
    for k in 0..n - 1 {
        writeln!(src, "enum G{k}<T> {{\n    A(G{}<T>)\n    End\n}}", k + 1)
            .expect("writing to a `String` should succeed");
    }
    writeln!(src, "enum G{}<T> {{\n    End(T)\n}}\nfn main() {{", n - 1)
        .expect("writing to a `String` should succeed");
    for k in 0..n - 1 {
        writeln!(src, "    let g{k}: G{k}<i64> = G{k}.End")
            .expect("writing to a `String` should succeed");
    }
    writeln!(src, "    let g{} = G{}.End(1)", n - 1, n - 1)
        .expect("writing to a `String` should succeed");
    for k in 0..n {
        writeln!(src, "    {}", statement(k)).expect("writing to a `String` should succeed");
    }
    src + "}\n"
}

/// What `mono` counts for `src`: the keys `holds` decided and the pairs the record walk entered;
/// and what the counts are after codegen built the program.
fn mono_counts(src: &str) -> ((usize, usize), (usize, usize)) {
    use crate::conformance::HOLDS_STEPS;
    use crate::mono::WALK_VISITS;

    HOLDS_STEPS.set(0);
    WALK_VISITS.set(0);
    let (program, diagnostics) = check_alone(&parse_ok(src), src, &[]);
    let program = program.unwrap_or_else(|| panic!("{diagnostics:?}"));
    let program = crate::mono::instantiate(program).unwrap_or_else(|found| panic!("{found:?}"));
    let after_mono = (HOLDS_STEPS.get(), WALK_VISITS.get());
    let mir = mir::build(&program);
    let context = Context::create();
    let machine = target_machine(OptLevel::O0).expect("the host is a target");
    let module = compile(&context, &program, &mir, "test.sisu", src, &machine, false);
    module.verify().expect("the module should verify");
    (after_mono, (HOLDS_STEPS.get(), WALK_VISITS.get()))
}

/// `mono` decides each class or enum type once, and its record walk enters each `(type, interface)`
/// pair once, however many operands reach it. The chain makes `mono` take its slow path, where
/// output ids differ from input ids. From `G0<i64>` the walk enters `G0<i64>` ... `G49<i64>` and
/// then `i64`: 51 pairs, and `holds` decides the 50 enum keys once. Without the visited set the
/// walk from `Gk<i64>` enters 51 - k pairs (1325 in all), and without the memo `holds(Gk<i64>)`
/// follows the generated rule through `Gk` ... `G49` (1275 misses). Codegen asks no conformance
/// query: it finds a written conformance in the output's records.
#[test]
fn mono_should_decide_each_type_once_and_codegen_should_ask_none() {
    let (after_mono, after_codegen) = mono_counts(&chain_program(50));
    assert_eq!(after_mono, (50, 51), "`mono` should decide each key once");
    assert_eq!(after_codegen, after_mono, "codegen should ask no query");
}

/// The counts of `mono` are linear in the number of types: 20, 40 and 60 enums, ten times the
/// smallest chain that a quadratic walk shows.
#[test]
fn mono_counts_should_grow_in_proportion_to_the_types() {
    let counts = |n| mono_counts(&chain_program(n)).0;
    let (a, b, c) = (counts(20), counts(40), counts(60));
    assert_near_linear(a.0, b.0, c.0);
    assert_near_linear(a.1, b.1, c.1);
}

/// The walk of an interpolation is the walk of a `print`: it enters each `(type, interface)` pair
/// once, and `holds` decides each enum key once, however many parts reach them.
#[test]
fn mono_should_walk_each_interpolated_type_once() {
    let interpolated = |n| chain_program_using(n, |k| format!("print(\"{{g{k}}}\")"));
    let (after_mono, after_codegen) = mono_counts(&interpolated(50));
    assert_eq!(
        after_mono,
        (50, 51),
        "`mono` should decide each key once and enter each pair once"
    );
    assert_eq!(after_codegen, after_mono, "codegen should ask no query");
    let counts = |n| mono_counts(&interpolated(n)).0;
    let (a, b, c) = (counts(20), counts(40), counts(60));
    assert_near_linear(a.0, b.0, c.0);
    assert_near_linear(a.1, b.1, c.1);
}

/// `largest<i64>` and a hand-written `largest_i64` emit the same function body, apart from their
/// names, at O0 and at `-O2`: a generic `>` on `i64` is a `Compare`, not a call of a `less`. Both
/// use `best` and `x` for their locals, so only the function name differs.
#[test]
fn generic_largest_should_emit_the_hand_written_body() {
    let body = |ty: &str| {
        format!(
            "(xs: [{ty}]) -> {ty}? {{\n    let mut best: {ty}? = None\n    for x in xs {{\n        if let b = best {{\n            if x > b {{\n                best = x\n            }}\n        }} else {{\n            best = x\n        }}\n    }}\n    best\n}}\n"
        )
    };
    let src = format!(
        "fn largest<T: Ord>{}\nfn largest_i64{}\nfn main() {{\n    print(largest([3, 9, 2]) ?? 0)\n    print(largest_i64([3, 9, 2]) ?? 0)\n}}",
        body("T"),
        body("i64")
    );
    let machine = target_machine(OptLevel::O2).expect("the host is a target");
    for (level, optimized) in [("O0", false), ("O2", true)] {
        let context = Context::create();
        let module = compiled(&context, &src);
        keep_functions(&module, "largest<i64>,largest_i64");
        if optimized {
            optimize(&module, &machine, OptLevel::O2).expect("the module should optimize");
        }
        let ir = module.print_to_string().to_string();
        // A panic names its position in the source, which differs between the two functions: the
        // message global and its length are placeholders. So is a loop's metadata number.
        let sited = |body: &str| -> String {
            (body.lines())
                .map(|line| match line.split_once("(ptr @report.text") {
                    Some((call, rest)) => {
                        let after = rest.splitn(3, ',').nth(2).unwrap_or_default();
                        format!("{call}(ptr @placeholder,{after}")
                    }
                    // A loop's metadata node is numbered by the functions before it.
                    None => match line.split_once("!llvm.loop !") {
                        Some((before, _)) => format!("{before}!llvm.loop !loop"),
                        None => line.to_string(),
                    },
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        // The text of `ir` from the header line of the function, up to its name's end.
        let header = |name: &str| {
            let line = (ir.lines())
                .find(|line| line.starts_with("define") && line.contains(name))
                .unwrap_or_else(|| panic!("no {name} in\n{ir}"));
            line[..line.find(name).expect("the line should hold the name") + name.len()].to_string()
        };
        let generic = function_body(&ir, &header("@\"sisu.largest<i64>\"("));
        let written = function_body(&ir, &header("@sisu.largest_i64("));
        let (generic, written) = (sited(generic), sited(written));
        for (a, b) in generic.lines().zip(written.lines()) {
            assert_eq!(a, b, "at {level}");
        }
        assert_eq!(
            generic.lines().count(),
            written.lines().count(),
            "at {level}"
        );
    }
}

/// The names of the `dbg`-mode show helpers that `ir` defines, such as `Node` for
/// `@sisu_show.dbg.Node`.
fn dbg_helpers(ir: &str) -> Vec<&str> {
    ir.lines()
        .filter(|line| line.starts_with("define "))
        .filter_map(|line| line.split_once("@sisu_show.dbg.").map(|(_, rest)| rest))
        .filter_map(|rest| rest.split_once('(').map(|(name, _)| name))
        .collect()
}

/// A program that prints a `[i64]` and `dbg`s a `Node` has a `dbg`-mode helper for `Node` only.
/// The array holds no class, so `print` and `dbg` share its helper. The helper of `Node` writes
/// the `next` field by the `print` helper of `Node`, and never calls itself.
#[test]
fn dbg_should_make_a_helper_only_for_a_type_that_holds_a_class() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "class Node {\n    value: i64\n    next: Node?\n}\nfn main() {\n    let xs = [1]\n    print(xs)\n    let n = Node(value: 1, next: None)\n    dbg(n)\n    print(n.value)\n    print(n.next == None)\n}",
    )
    .print_to_string()
    .to_string();
    assert_eq!(dbg_helpers(&ir), ["Node"], "{ir}");
    assert_eq!(
        ir.matches("define internal void @\"sisu_show.[i64]\"(")
            .count(),
        1,
        "{ir}"
    );
    let body = definition(&ir, "sisu_show.dbg.Node");
    let calls =
        |callee: &str| (body.lines()).any(|line| line.contains("call ") && line.contains(callee));
    assert!(calls("@sisu_show.Node("), "{body}");
    assert!(!calls("@sisu_show.dbg.Node("), "{body}");
}

/// A class with a written `to_string` is written by its text, in `print` and in `dbg`. So a value
/// that holds only such a class has no `dbg` helper of its own: `print` and `dbg` share one.
#[test]
fn dbg_should_share_the_helper_of_a_type_that_holds_a_class_with_a_written_to_string() {
    let context = Context::create();
    let decls = "class Leaf {\n    v: i64\n}\nextend Leaf: Display {\n    fn to_string(self) -> string {\n        \"leaf\"\n    }\n}\nenum Pick {\n    One(Leaf)\n}\n";
    for (row, value) in [
        ("a class", "Leaf(v: 1)"),
        ("an array", "[Leaf(v: 1)]"),
        ("an optional", "Some(Leaf(v: 1))"),
        ("an enum payload", "Pick.One(Leaf(v: 1))"),
    ] {
        let src = format!(
            "{decls}fn main() {{\n    let value = {value}\n    print(value)\n    dbg(value)\n}}"
        );
        let ir = compiled(&context, &src).print_to_string().to_string();
        assert!(dbg_helpers(&ir).is_empty(), "{row}: {ir}");
        assert!(!ir.contains("sisu_show_component.dbg"), "{row}: {ir}");
    }
}

/// An enum with a written `to_string` shows that text, so `dbg` writes no field of a class in its
/// payload. The helper of a value that holds only such an enum is the `print` helper.
#[test]
fn dbg_should_share_the_helper_of_an_enum_with_a_written_to_string() {
    let context = Context::create();
    let decls = "class Plain {\n    v: i64\n}\nenum Slot {\n    Full(Plain)\n}\nextend Slot: Display {\n    fn to_string(self) -> string {\n        \"slot\"\n    }\n}\nenum Pick {\n    One(Slot)\n}\n";
    for (row, value) in [
        ("an enum", "Slot.Full(Plain(v: 1))"),
        ("an array", "[Slot.Full(Plain(v: 1))]"),
        ("an optional", "Some(Slot.Full(Plain(v: 1)))"),
        ("an enum payload", "Pick.One(Slot.Full(Plain(v: 1)))"),
    ] {
        let src = format!(
            "{decls}fn main() {{\n    let value = {value}\n    print(value)\n    dbg(value)\n}}"
        );
        let ir = compiled(&context, &src).print_to_string().to_string();
        assert!(dbg_helpers(&ir).is_empty(), "{row}: {ir}");
    }
}

/// A recursive enum that holds no class is written by one helper, whether `print` or `dbg` asks.
/// So is a pair of enums that hold each other: no class is behind either of them.
#[test]
fn dbg_should_share_the_helper_of_a_recursive_enum_that_holds_no_class() {
    let context = Context::create();
    for (row, decls, value) in [
        (
            "a list",
            "enum List {\n    Nil\n    Cons(i64, List)\n}\n",
            "List.Cons(1, List.Nil)",
        ),
        (
            "a pair of enums",
            "enum C {\n    D(D)\n}\nenum D {\n    C(C)\n    End\n}\n",
            "C.D(D.End)",
        ),
    ] {
        let src = format!(
            "{decls}fn main() {{\n    let value = {value}\n    print(value)\n    dbg(value)\n}}"
        );
        let ir = compiled(&context, &src).print_to_string().to_string();
        assert!(dbg_helpers(&ir).is_empty(), "{row}: {ir}");
        assert!(!ir.contains("sisu_show_component.dbg"), "{row}: {ir}");
    }
}

/// A recursive enum that holds a class has a `dbg`-mode component helper, which writes the class
/// by its `dbg` helper.
#[test]
fn dbg_should_write_a_class_in_the_payload_of_a_recursive_enum_by_its_helper() {
    let context = Context::create();
    let ir = compiled(
        &context,
        "class Node {\n    value: i64\n}\nenum Tree {\n    Leaf(Node)\n    Join(Tree, Tree)\n}\nfn main() {\n    let leaf = Tree.Leaf(Node(value: 1))\n    dbg(Tree.Join(leaf, leaf))\n}",
    )
    .print_to_string()
    .to_string();
    let body = definition(&ir, "sisu_show_component.dbg.Tree");
    assert!(body.contains("@sisu_show.dbg.Node("), "{body}");
    assert!(!body.contains("@sisu_show.Node("), "{body}");
}

/// The `dbg`-mode helper of a class reads a field in place. It retains a counted `mut` field
/// whose helper may run a written `to_string` before the field's helper, and releases it after.
/// A `mut` field that holds a `[i64]` runs no written method, so nothing is
/// retained.
#[test]
fn dbg_should_retain_only_a_mut_field_whose_helper_may_run_a_written_method() {
    let context = Context::create();
    let ir = compiled(
        &context,
        &std::fs::read_to_string(format!(
            "{}/tests/run/dbg_field_snapshot.sisu",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("the run case should be readable"),
    )
    .print_to_string()
    .to_string();
    let body = definition(&ir, "sisu_show.dbg.Owner");
    let retain = body
        .find("@sisu_rc.retain(")
        .unwrap_or_else(|| panic!("no retain in\n{body}"));
    let branch = body.find("br ").expect("the helper should branch");
    let release = body
        .find("@sisu_rc.release.Inner(")
        .unwrap_or_else(|| panic!("no release in\n{body}"));
    let done = body.find("show.done:").expect("the helper should join");
    assert!(
        retain < branch,
        "the retain is before the field is shown:\n{body}"
    );
    assert!(
        done < release,
        "the release is after the field is shown:\n{body}"
    );

    let ir = compiled(
        &context,
        "class Bag {\n    mut xs: [i64]\n    mut tags: [Tag]\n}\nclass Tag {\n    n: i64\n}\nfn main() {\n    let bag = Bag(xs: [1], tags: [])\n    let _ = dbg(bag)\n    bag.xs = [2]\n    print(bag.xs.len() + bag.tags.len())\n}",
    )
    .print_to_string()
    .to_string();
    let body = definition(&ir, "sisu_show.dbg.Bag");
    assert!(!body.contains("@sisu_rc."), "{body}");
}

/// `tir::layout_enums` boxes a field on every cycle, so the checker cannot give codegen an
/// enum that holds itself inline. This test unboxes the one field of a recursive enum, so the
/// walk of `lay_out_enums` must stop on the cycle and not loop.
#[test]
#[should_panic(expected = "tir::layout_enums should box a field on every cycle")]
fn lay_out_enums_should_panic_on_an_enum_that_holds_itself_inline() {
    let src = "enum L {\n    End\n    A(L)\n}\nfn main() {\n    let _ = L.A(L.End)\n}";
    let program = parse_ok(src);
    let (program, diagnostics) = check_alone(&program, src, &[]);
    let program = program.unwrap_or_else(|| panic!("{diagnostics:?}"));
    let mut program = crate::mono::instantiate(program).unwrap_or_else(|found| panic!("{found:?}"));
    let enums = program.enums.iter_mut().filter(|e| e.name == "L");
    for field in enums
        .flat_map(|e| &mut e.variants)
        .flat_map(|v| &mut v.fields)
    {
        assert!(field.boxed, "the checker should box the field of `L.A`");
        field.boxed = false;
    }
    let mir = mir::build(&program);
    let machine = target_machine(OptLevel::O0).expect("the host is a target");
    let context = Context::create();
    compile(&context, &program, &mir, "test.sisu", src, &machine, false);
}
