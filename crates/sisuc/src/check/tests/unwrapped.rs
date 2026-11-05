//! A `.` on an optional reports E0062, and the rest of the chain is checked as if the optional
//! were unwrapped. A value that E0062 typed this way is no judge of whether it is optional.

use super::*;

/// `body` on line 11 of `fn g` over `c: Outer?`, `i: Inner?`, `o: [i64]?` and the plain `p: Outer`.
/// The classes have no `mut` field, so a write through them is E0001.
fn with_body(body: &str) -> String {
    format!(
        "class Inner {{\n    v: i64\n    ys: [i64]\n    fn one(self) -> i64 {{ self.v }}\n}}\nclass Outer {{\n    inner: Inner\n    oys: [[i64]]?\n}}\nfn g(c: Outer?, i: Inner?, o: [i64]?, p: Outer) {{\n    {body}\n}}\nfn main() {{}}"
    )
}

/// Each row is a body and the codes that it reports, in order, beside E0062 at its first place.
fn rows(rows: &[(&str, &[Wanted])]) {
    for &(body, wanted) in rows {
        codes_row(body, &with_body(body), wanted);
    }
}

/// A field of an optional has its type on the payload, so a mismatch or a missing member after it
/// is reported: `Poisoned::typed` in `Checker::field`.
#[test]
fn field_of_an_optional_should_go_on_with_the_type_of_the_field() {
    rows(&[
        (
            "let s: string = i.v",
            &[(Code::MayBeNone, (11, 21)), (Code::Mismatch, (11, 21))],
        ),
        (
            "let s: string = c.inner.v",
            &[(Code::MayBeNone, (11, 21)), (Code::Mismatch, (11, 21))],
        ),
        (
            "print(c.inner.nope)",
            &[(Code::MayBeNone, (11, 11)), (Code::NoMember, (11, 19))],
        ),
    ]);
}

/// A method of an optional has the type of the call on the payload: `method_of_payload`.
#[test]
fn method_of_an_optional_should_go_on_with_the_type_of_the_call() {
    rows(&[
        (
            "let s: string = i.one()",
            &[(Code::MayBeNone, (11, 21)), (Code::Mismatch, (11, 21))],
        ),
        (
            "i.one().nope()",
            &[(Code::MayBeNone, (11, 5)), (Code::NoMember, (11, 13))],
        ),
    ]);
}

/// A value that E0062 typed is optional under the `?.` fix of the E0062, so E0048, E0051, E0044
/// and E0049 do not report that it is not.
#[test]
fn value_typed_by_e0062_should_get_no_report_that_it_is_not_optional() {
    rows(&[
        ("print(c.inner.v ?? 0)", &[(Code::MayBeNone, (11, 11))]),
        ("print(o.len() ?? 0)", &[(Code::MayBeNone, (11, 11))]),
        (
            "if let x = c.inner.v { print(x) }",
            &[(Code::MayBeNone, (11, 16))],
        ),
        (
            "let x = c.inner.v else { return }",
            &[(Code::MayBeNone, (11, 13))],
        ),
        ("print(c.inner.v == None)", &[(Code::MayBeNone, (11, 11))]),
        ("print(c.inner?.v)", &[(Code::MayBeNone, (11, 11))]),
    ]);
}

/// The same checks on a plain value, which is not optional, so each reports.
#[test]
fn value_that_is_not_optional_should_get_the_report_that_it_is_not_optional() {
    rows(&[
        (
            "print(p.inner.v ?? 0)",
            &[(Code::CoalesceNotOptional, (11, 11))],
        ),
        (
            "if let x = p.inner.v { print(x) }",
            &[(Code::UnwrapNeedsOptional, (11, 16))],
        ),
        (
            "let x = p.inner.v else { return }",
            &[(Code::UnwrapNeedsOptional, (11, 13))],
        ),
        ("print(p.inner.v == None)", &[(Code::NeverNone, (11, 11))]),
        (
            "print(p.inner?.v)",
            &[(Code::SafeAccessNotOptional, (11, 11))],
        ),
    ]);
}

/// A `let` or a `for` that binds a value E0062 typed binds `Error`: its type depends on the
/// remedy. The plain value gives the name its type, so a member that it lacks is reported.
#[test]
fn name_bound_to_a_value_typed_by_e0062_should_have_no_type() {
    rows(&[
        (
            "let x = c.inner\n    x.nope",
            &[(Code::MayBeNone, (11, 13))],
        ),
        (
            "for n in c.inner.ys { n.nope() }",
            &[(Code::MayBeNone, (11, 14))],
        ),
        ("let x = p.inner\n    x.nope", &[(Code::NoMember, (12, 7))]),
        (
            "for n in p.inner.ys { n.nope() }",
            &[(Code::NoMember, (11, 29))],
        ),
    ]);
}

/// An index of an optional array reports E0062, and goes on with the element of the payload.
#[test]
fn index_of_an_optional_array_should_go_on_with_the_element() {
    rows(&[
        (
            "let s: string = o[0]",
            &[(Code::MayBeNone, (11, 21)), (Code::Mismatch, (11, 21))],
        ),
        (
            "c.oys[0].push(\"a\")",
            &[
                (Code::MayBeNone, (11, 5)),
                (Code::MayBeNone, (11, 5)),
                (Code::AssignToImmutable, (11, 5)),
                (Code::Mismatch, (11, 19)),
            ],
        ),
    ]);
}

/// A method of an optional checks its type arguments on the payload. A field is no method.
#[test]
fn type_arguments_on_a_method_of_an_optional_should_be_reported() {
    rows(&[
        (
            "print(i.one<i64>())",
            &[(Code::MayBeNone, (11, 11)), (Code::TypeArguments, (11, 16))],
        ),
        (
            "print(o.len<i64>())",
            &[(Code::MayBeNone, (11, 11)), (Code::TypeArguments, (11, 16))],
        ),
        ("print(i.v<i64>())", &[(Code::TypeArguments, (11, 11))]),
    ]);
}

/// At one start, E0062 comes before E0001, whichever check runs first: an element assignment
/// through `?.`, whose field reports E0001 before the index reports E0062.
#[test]
fn e0062_should_come_before_e0001_for_an_element_assignment_through_safe_access() {
    rows(&[(
        "c?.oys[0] = [1]",
        &[
            (Code::MayBeNone, (11, 5)),
            (Code::AssignToImmutable, (11, 5)),
        ],
    )]);
}

/// `body` on line 14 of `fn r` over `c: C?`, `k: bool` and `p: E`. `C` holds an `E`, an optional
/// of itself and an array, `mut w`, and a generic method `opt`. `id` and `Bx2` follow `main`.
fn with_cell(body: &str) -> String {
    format!(
        "enum E {{\n    A\n    B\n}}\nclass C {{\n    v: i64\n    e: E\n    ys: [i64]\n    n: C?\n    mut w: i64\n    fn opt<T>(self, t: T) -> T? {{ Some(t) }}\n}}\nfn r(c: C?, k: bool, p: E) {{\n    {body}\n}}\nfn main() {{}}\nfn id<T>(t: T) -> T {{ t }}\nenum Bx2<T> {{\n    Full(T)\n    Empty\n}}"
    )
}

/// `rows` over `with_cell`: each row is a body and the codes that it reports.
fn cell_rows(rows: &[(&str, &[Wanted])]) {
    for &(body, wanted) in rows {
        codes_row(body, &with_cell(body), wanted);
    }
}

/// An operator keeps the flag of its operands, so its result gets no report that it is not
/// optional. The right side of `??` keeps it; the left side does not need to.
#[test]
fn unwrapped_flag_should_spread_through_an_operator() {
    cell_rows(&[
        ("print((c.v + 1) ?? 0)", &[(Code::MayBeNone, (14, 12))]),
        ("print((c.v + 1) == None)", &[(Code::MayBeNone, (14, 12))]),
        (
            "print((c.v + 1)?.to_string())",
            &[(Code::MayBeNone, (14, 12))],
        ),
        (
            "let o: i64? = None\n    print((o ?? c.v) ?? 0)",
            &[(Code::MayBeNone, (15, 17))],
        ),
    ]);
}

/// A block and a join keep the flag of the value they end in.
#[test]
fn unwrapped_flag_should_spread_through_a_block_and_a_join() {
    cell_rows(&[
        ("print({ c.v } ?? 0)", &[(Code::MayBeNone, (14, 13))]),
        (
            "print((if k { c.v } else { c.v }) ?? 0)",
            &[(Code::MayBeNone, (14, 19)), (Code::MayBeNone, (14, 32))],
        ),
        (
            "print((if k { c.v } else { 1 }) ?? 0)",
            &[(Code::MayBeNone, (14, 19))],
        ),
    ]);
}

/// A generic call that returns a type parameter keeps the flag of its arguments. So does a slice
/// of a flagged array.
#[test]
fn unwrapped_flag_should_spread_through_a_generic_call_and_a_slice() {
    cell_rows(&[
        ("print(id(c.v) ?? 0)", &[(Code::MayBeNone, (14, 14))]),
        (
            "let x = c.ys[0..1]\n    x.nope()",
            &[(Code::MayBeNone, (14, 13))],
        ),
    ]);
}

/// `==` gives the same answer under both remedies, so its operand's flag stays behind: the
/// `bool` that it gives is not optional.
#[test]
fn unwrapped_flag_should_not_spread_through_equality() {
    cell_rows(&[(
        "print((c.v == 1) ?? false)",
        &[
            (Code::CoalesceNotOptional, (14, 11)),
            (Code::MayBeNone, (14, 12)),
        ],
    )]);
}

/// A name that takes all of a value that E0062 typed binds `Error`: under the `?.` fix the
/// pattern can fail, so `if let` and `let ... else` do not report that it always matches.
#[test]
fn name_pattern_on_a_value_typed_by_e0062_should_not_always_match() {
    cell_rows(&[
        (
            "if let x = c.e { print(x) }",
            &[(Code::MayBeNone, (14, 16))],
        ),
        (
            "let x = c.e else { return }",
            &[(Code::MayBeNone, (14, 13))],
        ),
        (
            "if let _ = c.e { print(1) }",
            &[(Code::MayBeNone, (14, 16))],
        ),
    ]);
}

/// Under the `?.` fix the value is optional, so `Some` and `None` patterns on it are no mistake,
/// and a name that takes all of it has no type.
#[test]
fn some_and_none_patterns_on_a_value_typed_by_e0062_should_be_accepted() {
    cell_rows(&[
        (
            "match c.v { Some(x) => print(x); None => {} }",
            &[(Code::MayBeNone, (14, 11))],
        ),
        (
            "match c.v { None => {}; _ => {} }",
            &[(Code::MayBeNone, (14, 11))],
        ),
        (
            "match c.v { x => print(x ?? 0) }",
            &[(Code::MayBeNone, (14, 11))],
        ),
    ]);
}

/// A place through `?.` after a `.` on an optional goes on with the payload's type: the write is
/// checked, and the `?.` of the field makes the place optional.
#[test]
fn place_through_safe_access_after_e0062_should_go_on() {
    cell_rows(&[
        (
            "c.n?.ys.push(1)",
            &[
                (Code::MayBeNone, (14, 5)),
                (Code::MayBeNone, (14, 5)),
                (Code::AssignToImmutable, (14, 5)),
            ],
        ),
        (
            "c.n?.w = \"a\"",
            &[(Code::MayBeNone, (14, 5)), (Code::Mismatch, (14, 14))],
        ),
    ]);
}

/// A generic variant and a call through an interface bound keep the flag of an argument, as a
/// generic call does. A result that names a type parameter may take the type of the argument.
#[test]
fn unwrapped_flag_should_spread_through_a_generic_variant_and_an_interface_call() {
    cell_rows(&[(
        "match Bx2.Full(c.v) { Bx2.Full(x) => print(x ?? 0); Bx2.Empty => {} }",
        &[(Code::MayBeNone, (14, 20))],
    )]);
    codes(
        "interface Get {\n    fn get<T>(self, t: T) -> T\n}\nclass C {\n    v: i64\n}\nfn r<X: Get>(x: X, c: C?) {\n    print(x.get(c.v) ?? 0)\n}\nfn main() {}",
        &[(Code::MayBeNone, (8, 17))],
    );
}

/// A name inside a `Some` on a value that is not optional, and that E0062 typed, binds `Error`:
/// the `Some` is a mistake without the `?.` fix, and its names have no type.
#[test]
fn name_inside_a_some_on_a_value_typed_by_e0062_should_have_no_type() {
    cell_rows(&[
        (
            "match c.v { Some(x) => print(x ?? 0); None => {} }",
            &[(Code::MayBeNone, (14, 11))],
        ),
        (
            "match c.v { Some(x) => x.nope(); None => {} }",
            &[(Code::MayBeNone, (14, 11))],
        ),
        (
            "match Some(c.v) { Some(x) => print(x ?? 0); None => {} }",
            &[(Code::MayBeNone, (14, 16))],
        ),
    ]);
}

/// A member whose own type is optional has that type under both remedies, so a name bound to it
/// keeps its type and a mistake with it is reported.
#[test]
fn name_pattern_on_an_optional_typed_by_e0062_should_keep_its_type() {
    cell_rows(&[
        (
            "if let x = c.n { x.nope() }",
            &[(Code::MayBeNone, (14, 16)), (Code::NoMember, (14, 24))],
        ),
        (
            "if let x = c.n { print(x.v ?? 0) }",
            &[
                (Code::MayBeNone, (14, 16)),
                (Code::CoalesceNotOptional, (14, 28)),
            ],
        ),
    ]);
}

/// A generic method on an optional receiver keeps the flag of an argument: the result is built
/// from it, so it is not a member, and `T?` is `T??` under the `?.` fix.
#[test]
fn generic_method_on_an_optional_should_keep_a_built_flag() {
    cell_rows(&[
        (
            "if let x = c.opt(c.v) { print(x ?? 0) }",
            &[(Code::MayBeNone, (14, 16)), (Code::MayBeNone, (14, 22))],
        ),
        (
            "print((c.opt(c.v) ?? 0) ?? 0)",
            &[(Code::MayBeNone, (14, 12)), (Code::MayBeNone, (14, 18))],
        ),
    ]);
}

/// A built optional on the left of `??` keeps the flag: it is `T??` under the `?.` fix. A member
/// optional (`c.n?.v`) has the same type under both remedies, so its `??` reports.
#[test]
fn left_side_of_coalesce_should_spread_a_built_flag() {
    cell_rows(&[
        (
            "print((Some(c.v) ?? 0) ?? 0)",
            &[(Code::MayBeNone, (14, 17))],
        ),
        (
            "print((c.n?.v ?? 0) ?? 0)",
            &[
                (Code::CoalesceNotOptional, (14, 11)),
                (Code::MayBeNone, (14, 12)),
            ],
        ),
    ]);
}

/// A `?.` keeps the state of its scrutinee or of its member, so the value after it keeps or
/// loses the type that the remedy gives it.
#[test]
fn safe_access_after_a_flagged_optional_should_keep_its_state() {
    cell_rows(&[
        (
            "if let x = c.n?.v { x.nope() }",
            &[(Code::MayBeNone, (14, 16)), (Code::NoMember, (14, 27))],
        ),
        (
            "if let x = c?.opt(c.v) { print(x ?? 0) }",
            &[(Code::MayBeNone, (14, 23))],
        ),
    ]);
}

/// A `let` binds the type of a member that is optional itself, whichever way a join or a block
/// carries it: the type is the same under both remedies.
#[test]
fn let_of_a_flagged_optional_should_keep_its_type() {
    cell_rows(&[
        (
            "let y = c.n\n    y.nope()",
            &[(Code::MayBeNone, (14, 13)), (Code::NoMember, (15, 7))],
        ),
        (
            "let y = if k { c.n } else { None }\n    y.nope()",
            &[(Code::MayBeNone, (14, 20)), (Code::NoMember, (15, 7))],
        ),
    ]);
}

/// Under the `?.` fix the scrutinee is optional, and `match` takes an optional, so E0124 does
/// not report a value that E0062 typed.
#[test]
fn match_on_a_value_typed_by_e0062_should_not_report_e0124() {
    cell_rows(&[
        (
            "match c.ys { xs => print(xs) }",
            &[(Code::MayBeNone, (14, 11))],
        ),
        (
            "match id(c.ys) { xs => print(xs) }",
            &[(Code::MayBeNone, (14, 14))],
        ),
    ]);
}

/// An index of an optional array that is a member has the same element type under both remedies,
/// so a `??` on the element is reported when it is not optional.
#[test]
fn index_of_a_member_optional_array_should_keep_the_element_type() {
    rows(&[(
        "print(c.oys[0][0] ?? 0)",
        &[
            (Code::MayBeNone, (11, 11)),
            (Code::MayBeNone, (11, 11)),
            (Code::CoalesceNotOptional, (11, 11)),
        ],
    )]);
}
