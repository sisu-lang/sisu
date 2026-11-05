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
