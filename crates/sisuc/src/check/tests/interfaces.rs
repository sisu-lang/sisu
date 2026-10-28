//! Interface declarations: their requirements and inheritance, `Self`, the names that may stand
//! where an interface or a type belongs, and the closure of a bound's inherited interfaces.

use std::fmt::Write as _;

use super::*;
use crate::check::cycle;
use crate::check::interface::{Bound, nests_within};
use crate::conformance::CHOICE_DEPTH_LIMIT;

/// The interfaces and classes the rows name, and an empty `main`, after the line a row tests,
/// which is line 1 (a row of several lines moves them down).
const DECLS: &str = "interface Shape {\n    fn area(self) -> f64\n}\ninterface Named {\n    fn name(self) -> string\n}\ninterface Convert<T> {\n    fn convert(self) -> T\n}\ninterface Pair2<A, B> {\n    fn first(self) -> A\n}\ninterface Same {\n    fn same(self, other: Self) -> bool\n}\nclass Circle {\n    r: f64\n}\nclass Box<T> {\n    value: T\n}\nclass Stack<T> {\n    items: [T]\n}\nfn main() {}";

/// `line` on line 1, then `DECLS`.
fn with_decls(line: &str) -> String {
    format!("{line}\n{DECLS}")
}

#[test]
fn interface_declarations_should_check_clean() {
    for src in [
        "interface Ord2: Same {\n    fn less(self, other: Self) -> bool\n}",
        // The inherited interface may stand after the interface that inherits it.
        "interface Solid: Later {\n    fn area(self) -> f64\n}\ninterface Later {\n    fn volume(self) -> f64\n}",
        // A diamond: `C` reaches `D` twice.
        "interface C {\n    fn c(self) -> i64\n}\ninterface A: C {}\ninterface B: C {}\ninterface D: A, B {\n    fn d(self) -> i64\n}",
        "interface Solid: Named {\n    fn area(self) -> f64\n}",
        "interface Solid: Named, Same, Shape {}",
        // A parameter of an interface has a bound that writes the associated type of another: the
        // declarations of `Holds2` are read while the interfaces are not all resolved.
        "interface Holds2 {\n    type Item\n}\ninterface Wrap<S: Holds2, T: Convert<S.Item>> {\n    fn wrap(self, s: S, t: T)\n}",
        // A requirement may name a class, and `Self`, in any position of its signature.
        "interface Draw {\n    fn f(self, c: Circle) -> Self\n    fn g(self, xs: [Self], b: Box<Self>?) -> [Self?]\n}",
        "interface Both {\n    fn f(self) -> Circle\n    fn g<U>(self, u: U) -> U\n    fn h(self)\n}",
        // The arguments of an inherited interface are a parameter alone, or free of parameters.
        "interface Feed<T>: Convert<T> {}",
        "interface Fixed<T>: Convert<i64> {}",
        "interface Boxed<T>: Convert<Box<i64>> {}",
        "interface Swap<A, B>: Pair2<B, A> {}",
        "interface Me: Convert<Self> {}",
        "interface Both<T>: Convert<T>, Convert<i64> {}",
        // A member of an interface may end a line with `;`, and a body may be on one line.
        "interface Line { fn a(self) -> i64; fn b(self) -> i64 }",
        // `Self` in a class or an enum is the declared type, with its own arguments.
        "class Node {\n    next: Self?\n    fn same(self, other: Self) -> Self { other }\n}",
        "class Pair<T> {\n    first: T\n    fn swap(self, other: Self) -> Self { other }\n}",
        "enum Shade {\n    Dark\n    Light\n    fn flip(self) -> Self { Shade.Light }\n}",
        "enum Tree<T> {\n    Leaf\n    Node(Self, T)\n    fn me(self) -> Self { self }\n}",
        // A bound names an interface, with its arguments, and the parameters of the declaration
        // are in scope in them, wherever they are declared.
        "fn f<T: Shape>(x: T) {}",
        "fn f<T: Named & Same>(x: T) {}",
        "fn f<T: Shape & Named & Same>(x: T) {}",
        "fn f<T: Convert<U>, U>(x: T, y: U) {}",
        "fn f<T: Convert<[U]>, U: Same>(x: T, y: U) {}",
        "class Holder<T: Shape> {\n    value: T\n    fn get<U: Convert<T>>(self, u: U) -> T { self.value }\n}",
        "enum Opt<T: Same> {\n    No\n    Yes(T)\n}",
        "interface Wrap<T: Same> {\n    fn f<U: Convert<T>>(self, u: U) -> T\n}",
        // Two requirements may name a parameter alike, and one that repeats the interface's own
        // is reported at it and leaves the interface's in scope.
        "interface Twice {\n    fn a<U>(self, u: U)\n    fn b<U>(self, u: U)\n}",
    ] {
        clean(&with_decls(src));
    }
}

/// `Self` in a class or an enum is the declared type with its own parameters.
#[test]
fn self_should_lower_to_the_declared_type() {
    contains(
        "class Pair<T> {\n    first: T\n    fn swap(self, other: Self) -> Self { other }\n}\nenum Shade {\n    Dark\n    fn flip(self, to: Self) -> Self { to }\n}\nfn main() {}",
        &[
            "(fn Pair<T>.swap ((self#0 Pair<T>) (other#1 Pair<T>)) Pair<T> ",
            "(fn Shade.flip ((self#0 Shade) (to#1 Shade)) Shade ",
        ],
    );
}

/// A bare interface name where a type or a value belongs: E0147, and the interface form of
/// E0105.
#[test]
fn interface_names_should_be_reported_as_types_and_values() {
    for (src, expected) in [
        // A bare interface name where a type belongs.
        (
            with_decls("fn f(s: Shape) {}"),
            vec![(Code::InterfaceAsType, (1, 9))],
        ),
        (
            with_decls("fn f(s: [Shape]) {}"),
            vec![(Code::InterfaceAsType, (1, 10))],
        ),
        (
            with_decls("fn f(s: Shape?) {}"),
            vec![(Code::InterfaceAsType, (1, 9))],
        ),
        (
            with_decls("fn f(s: Stack<Shape>) {}"),
            vec![(Code::InterfaceAsType, (1, 15))],
        ),
        (
            with_decls("fn f() -> Shape {}"),
            vec![(Code::InterfaceAsType, (1, 11))],
        ),
        (
            with_decls("fn f(c: Convert<i64>) {}"),
            vec![(Code::InterfaceAsType, (1, 9))],
        ),
        (
            with_decls("fn g() {\n    let s: Shape = 1\n}"),
            vec![(Code::InterfaceAsType, (2, 12))],
        ),
        (
            with_decls("class C {\n    s: Shape\n}"),
            vec![(Code::InterfaceAsType, (2, 8))],
        ),
        (
            with_decls("enum E {\n    A(Shape)\n}"),
            vec![(Code::InterfaceAsType, (2, 7))],
        ),
        // A call on the value of such a type adds nothing.
        (
            with_decls("fn f(s: Shape) -> f64 { s.area() }"),
            vec![(Code::InterfaceAsType, (1, 9))],
        ),
        // An interface named as a value is the interface form of E0105.
        (
            with_decls("fn g() {\n    Shape(1)\n}"),
            vec![(Code::TypeAsValue, (2, 5))],
        ),
        (
            with_decls("fn g() -> i64 {\n    Shape.x\n}"),
            vec![(Code::TypeAsValue, (2, 5))],
        ),
        (
            with_decls("fn g() {\n    let s = Shape\n}"),
            vec![(Code::TypeAsValue, (2, 13))],
        ),
        // A variant of an enum with the name of an interface does not take the interface's place.
        (
            with_decls("enum E {\n    Shape\n    Other\n}\nfn g() {\n    let s = Shape\n}"),
            vec![(Code::TypeAsValue, (6, 13))],
        ),
    ] {
        codes(&src, &expected);
    }
}

/// The error for the arguments of a bound covers its name.
#[test]
fn a_bound_with_missing_arguments_should_be_reported_at_its_name() {
    diagnostic(
        &with_decls("fn f<T: Convert>() {}"),
        Code::TypeArguments,
        (1, 9, "Convert"),
        &[],
    );
}

/// A bound, a conformance or an inherited interface names an interface with fitting arguments.
#[test]
fn a_bound_should_name_an_interface() {
    for (src, expected) in [
        // A bound, an inherited interface that is no interface.
        (
            with_decls("fn f<T: Circle>() {}"),
            vec![(Code::NotAnInterface, (1, 9))],
        ),
        (
            with_decls("fn f<T: i64>() {}"),
            vec![(Code::NotAnInterface, (1, 9))],
        ),
        (
            with_decls("fn f<T: [i64]>() {}"),
            vec![(Code::NotAnInterface, (1, 9))],
        ),
        (
            with_decls("fn f<T, U: T>() {}"),
            vec![(Code::NotAnInterface, (1, 12))],
        ),
        // An optional or an array of a class or of an interface is a type, and no interface.
        (
            with_decls("fn f<T: Circle?>() {}"),
            vec![(Code::NotAnInterface, (1, 9))],
        ),
        (
            with_decls("fn f<T: [Circle]>() {}"),
            vec![(Code::NotAnInterface, (1, 9))],
        ),
        (
            with_decls("fn f<U: Shape?>() {}"),
            vec![(Code::NotAnInterface, (1, 9))],
        ),
        (
            with_decls("fn f<U: [Shape]>() {}"),
            vec![(Code::NotAnInterface, (1, 9))],
        ),
        (
            with_decls("fn f<T: [Shape?]>() {}"),
            vec![(Code::NotAnInterface, (1, 9))],
        ),
        (
            with_decls("fn f<T: Convert<i64>?>() {}"),
            vec![(Code::NotAnInterface, (1, 9))],
        ),
        (
            with_decls("fn f<T: Box<i64>>() {}"),
            vec![(Code::NotAnInterface, (1, 9))],
        ),
        (
            with_decls("enum Color {\n    Red\n}\nfn f<T: Color>() {}"),
            vec![(Code::NotAnInterface, (4, 9))],
        ),
        (
            with_decls("class C<T: Circle> {}"),
            vec![(Code::NotAnInterface, (1, 12))],
        ),
        (
            with_decls("class C: Circle {}"),
            vec![(Code::NotAnInterface, (1, 10))],
        ),
        (
            with_decls("interface I: Circle {}"),
            vec![(Code::NotAnInterface, (1, 14))],
        ),
        (
            with_decls("interface I {\n    fn f<U: Circle>(self, u: U)\n}"),
            vec![(Code::NotAnInterface, (2, 13))],
        ),
        // An interface's arguments fit its parameters, as a class's do.
        (
            with_decls("fn f<T: Convert<i64, i64>>() {}"),
            vec![(Code::TypeArguments, (1, 16))],
        ),
        (
            with_decls("fn f<T: Shape<i64>>() {}"),
            vec![(Code::TypeArguments, (1, 14))],
        ),
    ] {
        codes(&src, &expected);
    }
}

/// `Self` outside a class, an enum and an interface, in a type and in a bound.
#[test]
fn self_outside_a_class_an_enum_and_an_interface_should_be_reported() {
    for (src, expected) in [
        // `Self` outside a class, an enum and an interface.
        (
            with_decls("fn f() -> Self {}"),
            vec![(Code::SelfOutside, (1, 11))],
        ),
        (
            with_decls("fn f() {\n    let x: Self = 1\n}"),
            vec![(Code::SelfOutside, (2, 12))],
        ),
        (
            with_decls("fn f(x: [Self?]) {}"),
            vec![(Code::SelfOutside, (1, 10))],
        ),
        // `Self` does not outlast the class, the enum or the interface that has it.
        (
            with_decls("class K {}\nfn f() -> Self {}"),
            vec![(Code::SelfOutside, (2, 11))],
        ),
        (
            with_decls("enum Color {\n    Red\n}\nfn f() -> Self {}"),
            vec![(Code::SelfOutside, (4, 11))],
        ),
        (
            with_decls("interface I {\n    fn f(self) -> Self\n}\nfn f() -> Self {}"),
            vec![(Code::SelfOutside, (4, 11))],
        ),
        // `Self` is no bound, wherever it stands in one.
        (
            with_decls("fn f<T: Self?>() {}"),
            vec![(Code::SelfOutside, (1, 9))],
        ),
        (
            with_decls("fn f<T: [Self]>() {}"),
            vec![(Code::SelfOutside, (1, 10))],
        ),
    ] {
        codes(&src, &expected);
    }
}

/// A cycle of inheritance, and an inherited argument built from a type parameter.
#[test]
fn inheritance_errors_should_be_reported() {
    for (src, expected) in [
        // A cycle is reported once, on its first interface.
        (
            "interface A: B {}\ninterface B: A {}\nfn main() {}".to_string(),
            vec![(Code::InheritanceCycle, (1, 14))],
        ),
        (
            "interface A: A {}\nfn main() {}".to_string(),
            vec![(Code::InheritanceCycle, (1, 14))],
        ),
        (
            "interface A: B {}\ninterface B: C {}\ninterface C: A {}\nfn main() {}".to_string(),
            vec![(Code::InheritanceCycle, (1, 14))],
        ),
        (
            "interface A: B, C {}\ninterface B: A {}\ninterface C: A {}\nfn main() {}"
                .to_string(),
            vec![(Code::InheritanceCycle, (1, 14))],
        ),
        (
            "interface Ok1 {}\ninterface A: Ok1, B {}\ninterface B: A {}\nfn main() {}"
                .to_string(),
            vec![(Code::InheritanceCycle, (2, 14))],
        ),
        // Two separate cycles are two reports.
        (
            "interface A: B {}\ninterface B: A {}\ninterface C: D {}\ninterface D: C {}\nfn main() {}"
                .to_string(),
            vec![
                (Code::InheritanceCycle, (1, 14)),
                (Code::InheritanceCycle, (3, 14)),
            ],
        ),
        // An argument built from a type parameter.
        (
            with_decls("interface Feed<T>: Convert<[T]> {}"),
            vec![(Code::InheritedArgument, (1, 28))],
        ),
        (
            with_decls("interface Feed<T>: Convert<T?> {}"),
            vec![(Code::InheritedArgument, (1, 28))],
        ),
        (
            with_decls("interface Feed<T>: Convert<Box<T>> {}"),
            vec![(Code::InheritedArgument, (1, 28))],
        ),
        (
            with_decls("interface Feed: Convert<[Self]> {}"),
            vec![(Code::InheritedArgument, (1, 25))],
        ),
        (
            with_decls("interface Feed<T>: Pair2<[T], T?> {}"),
            vec![
                (Code::InheritedArgument, (1, 26)),
                (Code::InheritedArgument, (1, 31)),
            ],
        ),
        // A name that is no type or interface.
        (
            with_decls("interface Feed: Nothing {}"),
            vec![(Code::UnknownType, (1, 17))],
        ),
        (
            with_decls("fn f<T: Nothing>() {}"),
            vec![(Code::UnknownType, (1, 9))],
        ),
        (with_decls("interface i64 {}"), vec![(Code::BuiltInName, (1, 11))]),
    ] {
        codes(&src, &expected);
    }
}

/// A name, and a requirement, once.
#[test]
fn an_interface_name_and_its_requirements_should_be_defined_once() {
    for (src, expected) in [
        // A name, once.
        (
            "interface Dup {}\ninterface Dup {}\nfn main() {}".to_string(),
            vec![(Code::DefinedTwice, (2, 11))],
        ),
        (
            "interface Dup {}\nclass Dup {}\nfn main() {}".to_string(),
            vec![(Code::DefinedTwice, (2, 7))],
        ),
        (
            "interface Dup {}\nfn Dup() {}\nfn main() {}".to_string(),
            vec![(Code::DefinedTwice, (2, 4))],
        ),
        (
            with_decls("interface Dup {\n    fn a(self)\n    fn a(self)\n}"),
            vec![(Code::DefinedTwice, (3, 8))],
        ),
        // A type parameter of a requirement that repeats one of the interface, each time; the
        // interface's stays in scope.
        (
            "interface Dup<T> {\n    fn a<T>(self, x: T)\n    fn b<T>(self, y: T)\n    fn c(self, z: T)\n}\nfn main() {}"
                .to_string(),
            vec![
                (Code::DefinedTwice, (2, 10)),
                (Code::DefinedTwice, (3, 10)),
            ],
        ),
        // A requirement that an inherited interface has.
        (
            with_decls("interface Solid: Named {\n    fn name(self) -> string\n}"),
            vec![(Code::DefinedTwice, (2, 8))],
        ),
        (
            with_decls("interface Solid: Named {\n    fn area(self) -> f64\n    fn name(self) -> string\n}"),
            vec![(Code::DefinedTwice, (3, 8))],
        ),
        // A type parameter of a requirement is not one of the next.
        (
            with_decls("interface Gone {\n    fn a<U>(self, u: U)\n    fn b(self, u: U)\n}"),
            vec![(Code::UnknownType, (3, 19))],
        ),
        // A cycle is E0150 alone, whatever its members declare.
        (
            "interface A: B {\n    fn x(self)\n}\ninterface B: A {\n    fn x(self)\n}\nfn main() {}"
                .to_string(),
            vec![(Code::InheritanceCycle, (1, 14))],
        ),
        // Once for a diamond: the closure holds `C` once.
        (
            "interface C {\n    fn x(self)\n}\ninterface A: C {}\ninterface B: C {}\ninterface D: A, B {\n    fn x(self)\n}\nfn main() {}"
                .to_string(),
            vec![(Code::DefinedTwice, (7, 8))],
        ),
    ] {
        codes(&src, &expected);
    }
}

/// The parameters of a requirement meet the rules of the parameters of a function, and a
/// requirement has no body to leave them unused in.
#[test]
fn the_parameters_of_a_requirement_should_be_named_as_a_function_names_them() {
    for (src, expected) in [
        (
            "interface I {\n    fn f(self, x: i64, x: string)\n}\nfn main() {}",
            vec![(Code::AlreadyDeclared, (2, 24))],
        ),
        (
            "interface I {\n    fn f(self, Some: i64)\n}\nfn main() {}",
            vec![(Code::BuiltInName, (2, 16))],
        ),
        (
            "enum E {\n    A\n}\ninterface I {\n    fn f(self, E: i64)\n}\nfn main() {}",
            vec![(Code::EnumNamedLocal, (5, 16))],
        ),
    ] {
        codes(src, &expected);
    }
}

#[test]
fn an_unknown_type_should_offer_a_similar_interface() {
    let src = with_decls("fn g() {\n    let x: Shap = 1\n}");
    let d = diagnostic(&src, Code::UnknownType, (2, 12, "Shap"), &[]);
    assert_eq!(
        fixes(&src, &d),
        [(false, vec![(2, 12, "Shap", "Shape".to_string())])]
    );
}

/// The secondary label of a requirement that an inherited interface has points at that one, and
/// a class's name that an interface took points at the interface.
#[test]
fn a_name_taken_twice_should_point_at_the_first() {
    diagnostic(
        &with_decls("interface Solid: Named {\n    fn name(self) -> string\n}"),
        Code::DefinedTwice,
        (2, 8, "name"),
        &[(8, 8)],
    );
    diagnostic(
        "interface Dup {}\nclass Dup {}\nfn main() {}",
        Code::DefinedTwice,
        (2, 7, "Dup"),
        &[(1, 11)],
    );
    // The nearest interface that has the requirement is the one named.
    let src = "interface A {\n    fn x(self)\n}\ninterface B: A {\n    fn x(self)\n}\ninterface C: B {\n    fn x(self)\n}\nfn main() {}";
    assert_eq!(
        found(src),
        [
            (Code::DefinedTwice, (5, 8), vec![(2, 8)]),
            (Code::DefinedTwice, (8, 8), vec![(5, 8)]),
        ]
    );
}

/// An interface that inherits one twice, with a parameter and with a type, doubles the closure of
/// its arguments at each level. The check for a requirement declared again reads the interfaces
/// alone: it visits each inherited interface once per interface, `n (n - 1) / 2` in all.
#[test]
fn inherited_requirements_should_be_found_without_arguments() {
    let visits = |levels: usize| {
        let chain = (1..levels)
            .map(|k| {
                format!(
                    "interface L{k}<P0, P1>: L{p}<P0, P1>, L{p}<i64, P1> {{}}\n",
                    p = k - 1
                )
            })
            .collect::<Vec<String>>()
            .concat();
        let src = format!("interface L0<P0, P1> {{\n    fn r(self)\n}}\n{chain}fn main() {{}}");
        INHERIT_VISITS.set(0);
        clean(&src);
        INHERIT_VISITS.get()
    };
    assert_eq!(visits(12), 66);
    assert_eq!(visits(24), 276);
}

/// A requirement adds its own type parameters to the scope of its interface and drops them, so
/// the work grows with the parameters and the requirements, not with their product: one step to
/// index each parameter, and one for each type name that is looked up.
#[test]
fn the_scope_of_an_interface_should_be_set_up_once() {
    let steps = |n: usize| {
        let params: Vec<_> = (0..n).map(|i| format!("T{i}")).collect();
        let requirements = (0..n)
            .map(|i| format!("    fn r{i}<U{i}>(self, u: U{i}) -> T{i}\n"))
            .collect::<Vec<String>>()
            .concat();
        let src = format!(
            "interface Wide<{}> {{\n{requirements}}}\nfn main() {{}}",
            params.join(", ")
        );
        PARAM_STEPS.set(0);
        clean(&src);
        PARAM_STEPS.get()
    };
    // 1 + n to index `Self` and the parameters, then 3 for each requirement.
    assert_eq!(steps(32), 129);
    assert_eq!(steps(64), 257);
}

/// The interfaces a bound gives: itself, then what it inherits, each once, with the arguments
/// copied from the bound or from the receiver.
#[test]
fn a_bound_should_give_each_inherited_interface_once() {
    let src = "interface C<X> {}\ninterface A<X>: C<X> {}\ninterface B<X>: C<X> {}\ninterface D<X>: A<X>, B<X> {}\ninterface E<X, Y>: C<Y>, C<i64> {}\ninterface F: C<Self> {}\ninterface Loop<X>: Loop2<X> {}\ninterface Loop2<X>: Loop<X> {}\nfn main() {}";
    let t = |name| Type::intern(TypeKind::Param(Symbol::intern(name), 0));
    let (i64s, strs) = (Type::I64, Type::STRING);
    let receiver = t("R");
    // `D<string>` gives `D`, `A` and `B`, and `C<string>` once.
    assert_eq!(
        closure(src, &[("D", &[strs])], receiver),
        [
            ("D", vec![strs]),
            ("A", vec![strs]),
            ("B", vec![strs]),
            ("C", vec![strs])
        ]
    );
    // An argument is copied by place, and one without a parameter stays.
    assert_eq!(
        closure(src, &[("E", &[strs, i64s])], receiver),
        [("E", vec![strs, i64s]), ("C", vec![i64s])]
    );
    // `Self` is the receiver.
    assert_eq!(
        closure(src, &[("F", &[])], receiver),
        [("F", vec![]), ("C", vec![receiver])]
    );
    // Several bounds give the interfaces they share once, each bound with those it inherits.
    assert_eq!(
        closure(
            src,
            &[("A", &[strs]), ("B", &[strs]), ("A", &[strs])],
            receiver
        ),
        [("A", vec![strs]), ("C", vec![strs]), ("B", vec![strs])]
    );
    // A cycle ends.
    assert_eq!(
        closure(src, &[("Loop", &[i64s])], receiver),
        [("Loop", vec![i64s]), ("Loop2", vec![i64s])]
    );
}

/// The interfaces of `bounds`, each an interface and its arguments, as `interfaces_of` finds them
/// for `receiver`: each with its name and arguments.
fn closure(
    src: &str,
    bounds: &[(&str, &[Type])],
    receiver: Type,
) -> Vec<(&'static str, Vec<Type>)> {
    let program = parse_ok(src);
    let mut checker = Checker::new(&program, src);
    let items: Vec<&ast::Item> = program.items.iter().collect();
    checker.collect_names(&items, items.len(), &program.broken);
    checker.interface_signatures(&items);
    let bounds: Vec<Bound> = bounds
        .iter()
        .map(|&(name, args)| {
            let Some(&Declared::Known(interface)) =
                checker.interface_ids.get(&Symbol::intern(name))
            else {
                panic!("{name} should be an interface");
            };
            Bound {
                interface,
                args: TypeArgs::intern(args),
                at: Span::new(0, 0),
            }
        })
        .collect();
    let mut out = Vec::new();
    checker
        .interfaces_of(receiver, &bounds, &mut out, &mut FxHashSet::default())
        .expect("the closure rows should fit the limit");
    out.into_iter()
        .map(|(id, args)| {
            (
                checker.interfaces[id.0].name.name.as_str(),
                args.get().to_vec(),
            )
        })
        .collect()
}

/// The interfaces the conformance rows use. A row is a whole program, and these follow it, so
/// that a position in the row is its own line and column.
const SHAPES: &str = "interface Shape {\n    fn area(self) -> f64\n}\ninterface Named {\n    fn name(self) -> string\n}\ninterface Same {\n    fn same(self, other: Self) -> bool\n}\ninterface Solid: Named {\n    fn volume(self) -> f64\n}\ninterface Label {\n    fn name(self) -> string\n}\ninterface Code {\n    fn name(self) -> i64\n}\ninterface Keep {\n    fn keep<U>(self, x: U) -> U\n}\n";

/// `row`, a program, then the interfaces.
fn with_shapes(row: &str) -> String {
    format!("{row}\n{SHAPES}")
}

/// A class or an enum conforms in its declaration list or in an `extend` block, with methods
/// that have the requirement's signature; the type's own methods and the methods of other blocks
/// stand beside them.
#[test]
fn conformances_should_check_clean() {
    for src in [
        // A class and an enum, each conforming in its list or in a block, and a plain block.
        "class Circle: Shape {\n    r: f64\n    fn area(self) -> f64 { self.r }\n}\nclass Square {\n    side: f64\n}\nextend Square: Shape {\n    fn area(self) -> f64 { self.side }\n}\nenum Dir: Same {\n    North\n    South\n    fn same(self, other: Dir) -> bool { true }\n}\nextend Circle {\n    fn grow(self, by: f64) -> Circle { Circle(r: self.r + by) }\n}\nfn main() {\n    let g = Circle(r: 1.0).grow(1.0)\n    print(g.area() + Square(side: 1.0).area())\n    print(Dir.North.same(Dir.South))\n}",
        // Two interfaces that both require `name`, met by one method of the body.
        "interface A {\n    fn name(self) -> string\n}\ninterface B {\n    fn name(self) -> string\n}\nclass C: A, B {\n    fn name(self) -> string { \"c\" }\n}\nfn main() {\n    print(C().name())\n}",
        // `Solid` inherits `Named`: a block gives it, or the body holds its method, or both
        // interfaces are listed and `Named` counts once.
        "class Cube: Solid {\n    fn volume(self) -> f64 { 1.0 }\n}\nextend Cube: Named {\n    fn name(self) -> string { \"cube\" }\n}\nclass Ball: Solid {\n    fn volume(self) -> f64 { 1.0 }\n    fn name(self) -> string { \"ball\" }\n}\nclass Disc: Solid, Named {\n    fn volume(self) -> f64 { 1.0 }\n    fn name(self) -> string { \"disc\" }\n}\nfn main() {\n    print(Cube().volume() + Ball().volume() + Disc().volume())\n}",
        // A block for the inherited interface may come before the class that lists the other.
        "extend Cube: Named {\n    fn name(self) -> string { \"cube\" }\n}\nclass Cube: Solid {\n    fn volume(self) -> f64 { 1.0 }\n}\nfn main() {\n    print(Cube().volume())\n}",
        // The parameter of `same` is `Self`, which is the type itself.
        "class Pin: Same {\n    n: i64\n    fn same(self, other: Pin) -> bool { self.n == other.n }\n}\nfn main() {\n    print(Pin(n: 1).same(Pin(n: 1)))\n}",
        // The same interface through `Self`, in a class and in a block.
        "class Pin: Same {\n    n: i64\n    fn same(self, other: Self) -> bool { self.n == other.n }\n}\nclass Peg {\n    n: i64\n}\nextend Peg: Same {\n    fn same(self, other: Self) -> bool { self.n == other.n }\n}\nfn main() {\n    print(Pin(n: 1).same(Pin(n: 1)))\n    print(Peg(n: 1).same(Peg(n: 2)))\n}",
        // A method that no interface requires, beside the conformance.
        "class Circle: Shape {\n    r: f64\n    fn area(self) -> f64 { self.r }\n    fn grow(self) -> f64 { self.r }\n}\nfn main() {\n    print(Circle(r: 1.0).grow())\n}",
        // Two blocks of unrelated interfaces, each with a `name`; the context picks one.
        "class Circle {\n    r: f64\n}\nextend Circle: Label {\n    fn name(self) -> string { \"circle\" }\n}\nextend Circle: Code {\n    fn name(self) -> i64 { 7 }\n}\nfn show(s: string) -> string { s }\nfn code() -> i64 {\n    Circle(r: 1.0).name()\n}\nfn main() {\n    let c = Circle(r: 1.0)\n    let s: string = c.name()\n    let k: i64 = c.name()\n    let o: string? = c.name()\n    print(o ?? \"\")\n    print(show(c.name()))\n    print(s)\n    print(k)\n    print(code())\n}",
        // A method of the requirement's own type parameters, named alike or not.
        "class Box {\n    n: i64\n    fn keep<V>(self, x: V) -> V { x }\n}\nclass Crate: Keep {\n    n: i64\n    fn keep<V>(self, x: V) -> V { x }\n}\nfn main() {\n    print(Crate(n: 1).keep(2))\n}",
        // The set of bounds counts, not their order.
        "interface Both {\n    fn both<U: Named & Same>(self, x: U)\n}\nclass Crate: Both {\n    fn both<V: Same & Named>(self, x: V) {}\n}\nclass Pin: Named, Same {\n    fn name(self) -> string { \"pin\" }\n    fn same(self, other: Pin) -> bool { true }\n}\nfn main() {\n    Crate().both(Pin())\n}",
        // A body method beside a block's method of the same name: the call takes the body's.
        "class Circle {\n    r: f64\n    fn name(self) -> string { \"body\" }\n}\nextend Circle: Label {\n    fn name(self) -> string { \"label\" }\n}\nfn main() {\n    let c = Circle(r: 1.0)\n    let s = c.name()\n    print(s)\n    print(c.r)\n}",
        // The same, with the method in a block of a conformance.
        "interface Maker {\n    fn make<U>(self) -> [U]\n}\nclass C {\n    x: i64\n}\nextend C: Maker {\n    fn make<U>(self) -> [U] { [] }\n}\nfn take<T>(x: T, y: T) -> T { x }\nfn main() {\n    let c = C(x: 1)\n    print(take(c.make(), [1]).len())\n}",
        // `Self` in the interface of a block is the block's target.
        "interface Back<T> {\n    fn back(self) -> T\n}\nclass Loop {\n    x: i64\n}\nextend Loop: Back<Self> {\n    fn back(self) -> Self { self }\n}\nfn main() {\n    print(Loop(x: 1).back().x)\n}",
        // An argument of a generic call waits for the other, with the method in a block.
        "class C {\n    x: i64\n}\nextend C {\n    fn make<U>(self) -> [U] { [] }\n}\nfn take<T>(x: T, y: T) -> T { x }\nfn main() {\n    let c = C(x: 1)\n    print(take(c.make(), [1]).len())\n}",
        // The method as the help for a missing one writes it: its own parameter has a new name,
        // as the block's parameter `U` stands in the argument of `Mix`.
        "interface Convert<Y> {\n    fn convert(self) -> Y\n}\nclass Box<U> {\n    value: U\n}\ninterface Mix<T> {\n    fn mix<U: Convert<T>>(self, other: U) -> T\n}\nextend<U> Box<U>: Mix<U> {\n    fn mix<U2: Convert<U>>(self, other: U2) -> U { other.convert() }\n}\nfn main() {}",
        // `Self` in the bound of a block's parameter is the block's target.
        "interface Pairs<X> {\n    fn pair(self) -> i64\n}\nclass Leaf {\n    n: i64\n}\nclass Crate<T> {\n    inner: T\n}\nextend Leaf: Pairs<Crate<Leaf>> {\n    fn pair(self) -> i64 { self.n }\n}\nextend<T: Pairs<Self>> Crate<T> {\n    fn with_self(self) -> i64 { self.inner.pair() }\n}\nfn main() {\n    print(Crate(inner: Leaf(n: 1)).with_self())\n}",
    ] {
        clean(&with_shapes(src));
    }
}

/// A call of a method is a call of `Type.method`, whichever block declares it; the methods of a
/// conformance block carry their interface in the symbol, so two blocks may each hold a `name`.
#[test]
fn conformance_methods_should_lower_with_their_interface_in_the_symbol() {
    contains(
        &with_shapes(
            "class Circle {\n    r: f64\n    fn name(self) -> string { \"body\" }\n}\nextend Circle: Label {\n    fn name(self) -> string { \"label\" }\n}\nextend Circle: Code {\n    fn name(self) -> i64 { 7 }\n}\nfn main() {\n    let c = Circle(r: 1.0)\n    let s = c.name()\n}",
        ),
        &[
            "(fn Circle.name ((self#0 Circle)) string ",
            "(fn Circle.Label.name ((self#0 Circle)) string ",
            "(fn Circle.Code.name ((self#0 Circle)) i64 ",
            "(let s#",
            "(call Circle.name c#",
        ],
    );
    // A call of a block's method writes the interface as its definition does.
    contains(
        &with_shapes(
            "class Circle {\n    r: f64\n}\nextend Circle: Label {\n    fn name(self) -> string { \"label\" }\n}\nfn main() {\n    let s = Circle(r: 1.0).name()\n}",
        ),
        &["(fn Circle.Label.name (", "(call Circle.Label.name "],
    );
}

/// The interface of the method that the first `let` of `main` that calls one initializes from,
/// through the `Wrap` that an optional adds; `None` for a method that no block declares.
fn called_interface(src: &str) -> Option<String> {
    let (program, ds) = checked(src);
    assert!(ds.is_empty(), "{ds:?}");
    let program = program.expect("the program should check");
    let main = (program.functions.iter().find(|f| f.name == "main")).expect("a main");
    for stmt in &main.body.stmts {
        let tir::Stmt::Let { init, .. } = stmt else {
            continue;
        };
        let mut init = init;
        while let tir::ExprKind::Wrap(inner) = &init.kind {
            init = inner;
        }
        if let tir::ExprKind::Call { func, .. } = init.kind {
            let conformance = program.functions[func.0].conformance;
            return conformance.map(|(interface, _)| program.interfaces[interface.0].name.clone());
        }
    }
    panic!("main should call a method in a `let`");
}

/// The type that the call's context expects picks the block when more than one has the method:
/// the only one that returns it, or, under an optional, the only one that returns its payload.
#[test]
fn the_expected_type_should_pick_the_conformance_block() {
    let program = |binding: &str| {
        with_shapes(&format!(
            "class Circle {{\n    r: f64\n}}\nextend Circle: Label {{\n    fn name(self) -> string {{ \"label\" }}\n}}\nextend Circle: Code {{\n    fn name(self) -> i64 {{ 7 }}\n}}\nfn main() {{\n    let c = Circle(r: 1.0)\n    {binding}\n}}"
        ))
    };
    for (binding, expected) in [
        ("let s: string = c.name()", "Label"),
        ("let k: i64 = c.name()", "Code"),
        ("let o: string? = c.name()", "Label"),
        ("let o: i64? = c.name()", "Code"),
    ] {
        assert_eq!(
            called_interface(&program(binding)).as_deref(),
            Some(expected),
            "{binding}"
        );
    }
}

/// The errors of a conformance that a type does not meet: what lacks, what does not match, and
/// what a block holds that its interface does not require.
#[test]
fn a_conformance_that_a_type_does_not_meet_should_be_reported() {
    for (row, expected) in [
        // Two requirements missing: one diagnostic, at the interface in the list.
        (
            "interface Both {\n    fn area(self) -> f64\n    fn perimeter(self) -> f64\n}\nclass Circle: Both {\n    r: f64\n}\nfn main() {}",
            vec![(Code::ConformanceBody, (5, 15))],
        ),
        // A method that does not match: its return type, a parameter's type, the parameters'
        // number, no return type.
        (
            "class Circle: Shape {\n    fn area(self) -> i64 { 1 }\n}\nfn main() {}",
            vec![(Code::ConformanceBody, (2, 8))],
        ),
        (
            "class Pin: Same {\n    fn same(self, other: i64) -> bool { true }\n}\nfn main() {}",
            vec![(Code::ConformanceBody, (2, 8))],
        ),
        (
            "class Circle: Shape {\n    fn area(self, by: f64) -> f64 { by }\n}\nfn main() {}",
            vec![(Code::ConformanceBody, (2, 8))],
        ),
        (
            "class Circle: Shape {\n    fn area(self) {}\n}\nfn main() {}",
            vec![(Code::ConformanceBody, (2, 8))],
        ),
        // The method's own type parameters: a bound that the requirement lacks, one that it has
        // and the method lacks, another number of them.
        (
            "class Crate: Keep {\n    fn keep<V: Same>(self, x: V) -> V { x }\n}\nfn main() {}",
            vec![(Code::ConformanceBody, (2, 8))],
        ),
        (
            "interface Both {\n    fn both<U: Named & Same>(self, x: U)\n}\nclass Crate: Both {\n    fn both<V: Same>(self, x: V) {}\n}\nfn main() {}",
            vec![(Code::ConformanceBody, (5, 8))],
        ),
        (
            "class Crate: Keep {\n    fn keep<V, W>(self, x: V) -> V { x }\n}\nfn main() {}",
            vec![(Code::ConformanceBody, (2, 8))],
        ),
        // A block holds the interface's methods only.
        (
            "class Circle {\n    r: f64\n}\nextend Circle: Shape {\n    fn area(self) -> f64 { 1.0 }\n    fn perimeter(self) -> f64 { 1.0 }\n}\nfn main() {}",
            vec![(Code::ConformanceBody, (6, 8))],
        ),
        // A method of another block, or of the body, does not meet a block's requirement.
        (
            "class Circle {\n    r: f64\n}\nextend Circle: Label {\n    fn name(self) -> string { \"l\" }\n}\nextend Circle: Named {\n}\nfn main() {}",
            vec![(Code::ConformanceBody, (7, 16))],
        ),
        (
            "class Circle {\n    r: f64\n    fn name(self) -> string { \"b\" }\n}\nextend Circle: Named {\n}\nfn main() {}",
            vec![(Code::ConformanceBody, (5, 16))],
        ),
        // A body that lacks a method of an inherited interface: reported at the listed one.
        (
            "class Cube: Solid {\n    fn volume(self) -> f64 { 1.0 }\n}\nfn main() {}",
            vec![(Code::ConformanceBody, (1, 13))],
        ),
        // An enum conforms as a class does.
        (
            "enum Dir: Same {\n    North\n}\nfn main() {}",
            vec![(Code::ConformanceBody, (1, 11))],
        ),
        (
            "enum Dir {\n    North\n}\nextend Dir: Same {\n    fn same(self, other: Dir) -> bool { true }\n    fn flip(self) -> Dir { Dir.North }\n}\nfn main() {}",
            vec![(Code::ConformanceBody, (6, 8))],
        ),
    ] {
        codes(&with_shapes(row), &expected);
    }
}

/// A type conforms to an interface once. The second conformance in source order is reported,
/// whether a list or a block writes it, and its methods answer no call.
#[test]
fn a_second_conformance_to_an_interface_should_be_reported_once() {
    for (row, expected) in [
        (
            "class Circle {\n    r: f64\n}\nextend Circle: Shape {\n    fn area(self) -> f64 { 1.0 }\n}\nextend Circle: Shape {\n    fn area(self) -> f64 { 2.0 }\n}\nfn main() {\n    print(Circle(r: 1.0).area())\n}",
            vec![(Code::ConformsTwice, (7, 16))],
        ),
        (
            "class Circle: Shape {\n    r: f64\n    fn area(self) -> f64 { 1.0 }\n}\nextend Circle: Shape {\n    fn area(self) -> f64 { 2.0 }\n}\nfn main() {}",
            vec![(Code::ConformsTwice, (5, 16))],
        ),
        // The block comes first in the source, so the list is the second.
        (
            "extend Circle: Shape {\n    fn area(self) -> f64 { 2.0 }\n}\nclass Circle: Shape {\n    r: f64\n    fn area(self) -> f64 { 1.0 }\n}\nfn main() {}",
            vec![(Code::ConformsTwice, (4, 15))],
        ),
        (
            "class Circle: Shape, Shape {\n    fn area(self) -> f64 { 1.0 }\n}\nfn main() {}",
            vec![(Code::ConformsTwice, (1, 22))],
        ),
        // Two types conform once each, and one type conforms to two interfaces.
        (
            "class Circle: Shape {\n    fn area(self) -> f64 { 1.0 }\n}\nclass Square: Shape {\n    fn area(self) -> f64 { 1.0 }\n}\nfn main() {}",
            vec![],
        ),
    ] {
        codes(&with_shapes(row), &expected);
    }
}

/// An `extend` block's interface needs the interfaces it inherits: the type's list or a block of
/// its own gives them, and each one missing is reported at the block's interface.
#[test]
fn a_conformance_without_an_inherited_conformance_should_be_reported() {
    for (row, expected) in [
        (
            "class Square {\n    side: f64\n}\nextend Square: Solid {\n    fn volume(self) -> f64 { 1.0 }\n}\nfn main() {}",
            vec![(Code::NotConform, (4, 16))],
        ),
        // Two interfaces missing, one diagnostic for each.
        (
            "interface Top: Mid {\n    fn top(self) -> f64\n}\ninterface Mid: Named {\n    fn mid(self) -> f64\n}\nclass X {\n    n: i64\n}\nextend X: Top {\n    fn top(self) -> f64 { 1.0 }\n}\nfn main() {}",
            vec![(Code::NotConform, (10, 11)), (Code::NotConform, (10, 11))],
        ),
        // The list of the class gives the inherited interface, and so does a block of its own.
        (
            "class Square: Named {\n    fn name(self) -> string { \"s\" }\n}\nextend Square: Solid {\n    fn volume(self) -> f64 { 1.0 }\n}\nfn main() {}",
            vec![],
        ),
        (
            "class Square {\n    side: f64\n}\nextend Square: Named {\n    fn name(self) -> string { \"s\" }\n}\nextend Square: Solid {\n    fn volume(self) -> f64 { 1.0 }\n}\nfn main() {}",
            vec![],
        ),
        // An inherited interface that takes arguments has no help until Task 6: a block cannot
        // write a generic interface yet.
        (
            "interface Convert<T> {\n    fn convert(self) -> T\n}\ninterface Wide: Convert<i64> {\n    fn wide(self) -> f64\n}\nclass Circle {\n    r: f64\n}\nextend Circle: Wide {\n    fn wide(self) -> f64 { 1.0 }\n}\nfn main() {}",
            vec![(Code::NotConform, (10, 16))],
        ),
        // An enum is a type like a class.
        (
            "enum Dir {\n    North\n}\nextend Dir: Solid {\n    fn volume(self) -> f64 { 1.0 }\n}\nfn main() {}",
            vec![(Code::NotConform, (4, 13))],
        ),
    ] {
        codes(&with_shapes(row), &expected);
    }
}

/// A call that more than one block answers is ambiguous, unless the type's own method answers
/// or the expected type picks one result.
#[test]
fn a_call_that_two_conformances_answer_should_be_ambiguous() {
    let blocks = |second: &str, binding: &str| {
        with_shapes(&format!(
            "class Circle {{\n    r: f64\n}}\nextend Circle: Label {{\n    fn name(self) -> string {{ \"a\" }}\n}}\nextend Circle: {second}\nfn main() {{\n    let c = Circle(r: 1.0)\n    {binding}\n}}"
        ))
    };
    let code = "Code {\n    fn name(self) -> i64 { 7 }\n}";
    let named = "Named {\n    fn name(self) -> string { \"b\" }\n}";
    for (src, expected) in [
        // No expected type.
        (
            blocks(code, "let n = c.name()"),
            vec![(Code::Ambiguous, (12, 15))],
        ),
        // A call that no `let` initializes.
        (
            blocks(code, "print(c.name())"),
            vec![(Code::Ambiguous, (12, 13))],
        ),
        // The same result type in both: no annotation tells them apart.
        (
            blocks(named, "let n = c.name()"),
            vec![(Code::Ambiguous, (12, 15))],
        ),
        (
            blocks(named, "let s: string = c.name()"),
            vec![(Code::Ambiguous, (12, 23))],
        ),
        // An expected type that no answer returns is no choice, and no mismatch is added.
        (
            blocks(code, "let b: bool = c.name()"),
            vec![(Code::Ambiguous, (12, 21))],
        ),
        // The expected payload picks, whatever the optional's depth.
        (blocks(code, "let n: i64? = c.name()"), vec![]),
    ] {
        codes(&src, &expected);
    }
}

/// The method a call takes is the body's, or a plain block's, though blocks of conformances have
/// a method of the name.
#[test]
fn a_types_own_method_should_be_the_one_called_beside_conformance_blocks() {
    let blocks = "extend Circle: Label {\n    fn name(self) -> string { \"label\" }\n}\nextend Circle: Code {\n    fn name(self) -> i64 { 7 }\n}\n";
    let main = "fn main() {\n    let c = Circle(r: 1.0)\n    let s = c.name()\n}";
    for own in [
        "class Circle {\n    r: f64\n    fn name(self) -> string { \"body\" }\n}\n",
        "class Circle {\n    r: f64\n}\nextend Circle {\n    fn name(self) -> string { \"plain\" }\n}\n",
    ] {
        let src = with_shapes(&format!("{own}{blocks}{main}"));
        assert_eq!(called_interface(&src), None, "{own}");
    }
}

/// A body method, or a plain block's, is the type's own, and takes the call before the methods
/// of conformance blocks; the blocks of other types are not read.
#[test]
fn a_types_own_method_should_take_the_call_before_a_conformance_block() {
    for src in [
        "class Circle {\n    r: f64\n}\nextend Circle {\n    fn name(self) -> string { \"plain\" }\n}\nextend Circle: Label {\n    fn name(self) -> string { \"label\" }\n}\nextend Circle: Code {\n    fn name(self) -> i64 { 7 }\n}\nfn main() {\n    let c = Circle(r: 1.0)\n    let s = c.name()\n    print(s)\n}",
        // A block of another type has the name too.
        "class Circle {\n    r: f64\n}\nclass Square {\n    side: f64\n}\nextend Square: Label {\n    fn name(self) -> string { \"square\" }\n}\nextend Circle: Code {\n    fn name(self) -> i64 { 7 }\n}\nfn main() {\n    let c = Circle(r: 1.0)\n    let n = c.name()\n    print(n)\n    print(Square(side: 1.0).name())\n}",
    ] {
        clean(&with_shapes(src));
    }
}

/// A plain block's method whose bounds fail for the receiver is no candidate: the conformance
/// block that applies answers the call. Without one, the bound is reported; with one that the
/// bounds allow, the plain method still wins.
#[test]
fn a_plain_method_whose_bounds_fail_should_not_hide_an_applicable_conformance() {
    let program = |bound: &str, receiver: &str| {
        format!(
            "interface Flag {{}}\ninterface Get {{\n    fn get(self) -> i64\n}}\nclass Box<T> {{\n    value: T\n}}\nclass Node {{}}\nclass Mark: Flag {{}}\nextend<T: {bound}> Box<T> {{\n    fn get(self) -> i64 {{ 1 }}\n}}\nextend Box<Node>: Get {{\n    fn get(self) -> i64 {{ 2 }}\n}}\nfn main() {{\n    let n = Box(value: {receiver}).get()\n}}"
        )
    };
    let src = program("Flag", "Node()");
    assert_eq!(called_interface(&src).as_deref(), Some("Get"));
    // The receiver meets the bound: the plain method is the type's own.
    assert_eq!(called_interface(&program("Flag", "Mark()")), None);
    // No conformance covers the receiver: the bound is reported at the call.
    // The same call as the argument of a generic call: the plain method has no type parameter of
    // its own, so only the conformance's makes the call wait for the other argument.
    clean(
        "interface Flag {}\ninterface Get2 {\n    fn get<U>(self) -> [U]\n}\nclass Box<T> {\n    value: T\n}\nclass Node {}\nextend<T: Flag> Box<T> {\n    fn get(self) -> i64 { 1 }\n}\nextend Box<Node>: Get2 {\n    fn get<U>(self) -> [U] { [] }\n}\nfn take<T>(x: T, y: T) -> T {\n    x\n}\nfn main() {\n    let n = take(Box(value: Node()).get(), [1]).len()\n}",
    );
    let src = program("Flag", "Mark()").replace("Box<Node>: Get", "Box<Mark>: Get");
    assert_eq!(called_interface(&src), None);
    let src = src.replace("Mark()).get()", "Node()).get()");
    codes(&src, &[(Code::NotConform, (17, 13))]);
}

/// A call of a method through a bound waits for the other arguments of the call it is an argument
/// of, to find the type its own parameters stand for, as a call of a function does: when it takes
/// no argument (`make()`), when it takes one that binds nothing (`make(1)`), and when two bounds
/// have a method of the name and the others decide.
#[test]
fn a_call_through_a_bound_should_wait_for_the_arguments_that_bind_its_type() {
    let decls = "interface Make {\n    fn make<U>(self) -> [U]\n}\ninterface MakeArg {\n    fn make_arg<U>(self, n: i64) -> [U]\n}\ninterface Text {\n    fn make(self) -> string\n}\ninterface List {\n    fn make(self) -> [i64]\n}\nenum Res<T, E> {\n    Good(T)\n    Bad(E)\n}\ninterface Mk {\n    fn mk<E>(self) -> Res<i64, E>\n}\nfn g<T>(r: Res<T, string>) -> i64 {\n    0\n}\nfn take<T>(x: T, y: T) -> T {\n    x\n}\n";
    for body in [
        "fn f<B: Make>(b: B) -> i64 {\n    take(b.make(), [1]).len()\n}",
        "fn f<B: MakeArg>(b: B) -> i64 {\n    take(b.make_arg(1), [1]).len()\n}",
        "fn f<B: List & Text>(b: B) -> i64 {\n    take(b.make(), \"s\").len()\n}",
        // The parameter's type fixes the call's own `E`, while `T` is unbound.
        "fn f<B: Mk>(b: B) -> i64 {\n    g(b.mk())\n}",
        // Two bounds answer `make` with different results, and no context is set: the call waits
        // for the second argument, which binds `T`, and then the result `i64` picks one.
        "interface One {\n    fn make(self) -> i64\n}\ninterface Two {\n    fn make(self) -> string\n}\nfn mkr<E>() -> Res<i64, E> {\n    Res.Good(5)\n}\nfn pair<T>(a: T, r: Res<T, string>) -> T {\n    a\n}\nfn f<B: One & Two>(b: B) -> i64 {\n    let r = pair(b.make(), mkr())\n    r\n}",
    ] {
        clean(&format!("{decls}{body}\nfn main() {{}}"));
    }
}

/// `row`, and an empty `main` unless it has one.
fn with_main(row: &str) -> String {
    if row.contains("fn main") {
        row.to_string()
    } else {
        format!("{row}\nfn main() {{}}")
    }
}

/// The target of an `extend` block is a class or an enum. Any other target is reported at it, and
/// the methods of the block are not checked: its error says what is wrong.
#[test]
fn an_extend_target_that_is_no_class_or_enum_should_be_reported() {
    for (row, expected) in [
        (
            "extend i64 {\n    fn f(self) -> i64 { 1 }\n}",
            vec![(Code::CannotExtend, (1, 8))],
        ),
        ("extend [i64] {\n}", vec![(Code::CannotExtend, (1, 8))]),
        // A type parameter is reported once.
        (
            "extend<T> T {\n    fn f(self) -> bool { true }\n}",
            vec![(Code::CannotExtend, (1, 11))],
        ),
        // The body of a method in a block that failed is not checked.
        (
            "extend i64 {\n    fn f(self) -> i64 {\n        let b: bool = 1\n        1\n    }\n}",
            vec![(Code::CannotExtend, (1, 8))],
        ),
        // An interface, and a name that is nothing.
        ("extend Shape {\n}", vec![(Code::InterfaceAsType, (1, 8))]),
        ("extend Missing {\n}", vec![(Code::UnknownType, (1, 8))]),
        // A block of a failed target holds methods that a call may have wanted.
        (
            "class Circle {\n    r: f64\n}\nextend Circle: Missing {\n    fn grow(self) {}\n}\nfn main() {\n    Circle(r: 1.0).grow()\n}",
            vec![(Code::UnknownType, (4, 16))],
        ),
        // A class that is no interface.
        (
            "class Circle {\n    r: f64\n}\nextend Circle: Circle {\n}",
            vec![(Code::NotAnInterface, (4, 16))],
        ),
        // An interface with arguments is an interface all the same.
        (
            "interface Convert<T> {\n    fn convert(self) -> T\n}\nextend Convert<i64> {\n}",
            vec![(Code::InterfaceAsType, (4, 8))],
        ),
        // A choice of an associated type that the block's interface does not declare.
        (
            "interface Plain {}\nclass Circle {\n    r: f64\n}\nextend Circle: Plain {\n    type Item = i64\n}",
            vec![(Code::ConformanceBody, (6, 10))],
        ),
        // The methods of a block that failed for its interface or its unused parameter are not
        // checked.
        (
            "class Circle {\n    r: f64\n}\nextend Circle: Missing {\n    fn grow(self) {\n        let b: bool = 1\n    }\n}",
            vec![(Code::UnknownType, (4, 16))],
        ),
        (
            "class Pile<T> {\n    items: [T]\n}\nextend<U> Pile<i64> {\n    fn f(self) {\n        let b: bool = 1\n    }\n}",
            vec![(Code::CannotExtend, (4, 8))],
        ),
    ] {
        codes(&with_shapes(&with_main(row)), &expected);
    }
}

/// The methods of a type's body and of its plain `extend` blocks share a namespace with its
/// fields and variants, as do the methods of one block. The one that starts last is reported.
#[test]
fn a_method_defined_in_the_body_and_in_a_block_should_be_reported_twice() {
    for (row, expected) in [
        (
            "class Circle {\n    r: f64\n    fn grow(self) -> f64 { 1.0 }\n}\nextend Circle {\n    fn grow(self) -> f64 { 2.0 }\n}\nfn main() {}",
            vec![(Code::DefinedTwice, (6, 8))],
        ),
        // The block starts first, so the body's method is the second.
        (
            "extend Circle {\n    fn grow(self) -> f64 { 2.0 }\n}\nclass Circle {\n    r: f64\n    fn grow(self) -> f64 { 1.0 }\n}\nfn main() {}",
            vec![(Code::DefinedTwice, (6, 8))],
        ),
        (
            "class Circle {\n    r: f64\n}\nextend Circle {\n    fn grow(self) -> f64 { 1.0 }\n}\nextend Circle {\n    fn grow(self) -> f64 { 2.0 }\n}\nfn main() {}",
            vec![(Code::DefinedTwice, (8, 8))],
        ),
        (
            "class Circle {\n    r: f64\n}\nextend Circle {\n    fn grow(self) -> f64 { 1.0 }\n    fn grow(self) -> f64 { 2.0 }\n}\nfn main() {}",
            vec![(Code::DefinedTwice, (6, 8))],
        ),
        (
            "enum Dir {\n    North\n    fn grow(self) -> f64 { 1.0 }\n}\nextend Dir {\n    fn grow(self) -> f64 { 2.0 }\n}\nfn main() {}",
            vec![(Code::DefinedTwice, (6, 8))],
        ),
        // A field, and a variant, with the name of a method of a block.
        (
            "class Circle {\n    r: f64\n}\nextend Circle {\n    fn r(self) -> f64 { 1.0 }\n}\nfn main() {}",
            vec![(Code::DefinedTwice, (5, 8))],
        ),
        (
            "enum Dir {\n    North\n}\nextend Dir {\n    fn North(self) -> f64 { 1.0 }\n}\nfn main() {}",
            vec![(Code::DefinedTwice, (5, 8))],
        ),
        // A block of a conformance has a namespace of its own, and so does each of its methods
        // against a plain block's.
        (
            "class Circle {\n    r: f64\n}\nextend Circle {\n    fn name(self) -> string { \"p\" }\n}\nextend Circle: Label {\n    fn name(self) -> string { \"l\" }\n}\nfn main() {}",
            vec![],
        ),
    ] {
        codes(&with_shapes(row), &expected);
    }
}

/// `mut self` in an `extend` block is reported once the target is known, in the form of a class
/// or of an enum, as in a declaration's body.
#[test]
fn mut_self_in_an_extend_block_should_be_reported_as_in_a_declaration() {
    let class =
        "class Circle {\n    r: f64\n}\nextend Circle {\n    fn grow(mut self) {}\n}\nfn main() {}";
    let d = diagnostic(class, Code::MutSelf, (5, 13, "mut"), &[]);
    assert_eq!(
        fixes(class, &d),
        [(true, vec![(5, 13, "mut ", String::new())])]
    );
    fix_removes_its_code(class, &d);
    // The type annotation makes the fix unsafe, as it does in a class.
    let typed = "class Circle {\n    r: f64\n}\nextend Circle {\n    fn grow(mut self: Circle) {}\n}\nfn main() {}";
    let d = diagnostic(typed, Code::MutSelf, (5, 13, "mut"), &[]);
    assert!(!fixes(typed, &d)[0].0);
    let e = "enum Dir {\n    North\n}\nextend Dir {\n    fn flip(mut self) {}\n}\nfn main() {}";
    let d = diagnostic(e, Code::MutSelf, (5, 13, "mut"), &[]);
    assert!(!fixes(e, &d)[0].0);
    // The target of a block that failed is not known, so nothing is reported.
    codes(
        "extend Missing {\n    fn flip(mut self) {}\n}\nfn main() {}",
        &[(Code::UnknownType, (1, 8))],
    );
}

/// The conformances a program records: one for each type and interface, an inherited interface
/// that the list gives included, each with the methods in the order of the requirements.
#[test]
fn conformances_should_list_their_methods_in_the_order_of_the_requirements() {
    let src = with_shapes(
        "interface Both {\n    fn area(self) -> f64\n    fn perimeter(self) -> f64\n}\nclass Circle: Both {\n    fn perimeter(self) -> f64 { 1.0 }\n    fn area(self) -> f64 { 2.0 }\n}\nclass Ball: Solid, Named {\n    fn name(self) -> string { \"ball\" }\n    fn volume(self) -> f64 { 1.0 }\n}\nclass Cube: Solid {\n    fn volume(self) -> f64 { 1.0 }\n    fn name(self) -> string { \"cube\" }\n}\nclass Disc {\n    r: f64\n}\nextend Disc: Both {\n    fn perimeter(self) -> f64 { 1.0 }\n    fn area(self) -> f64 { 2.0 }\n}\nfn main() {}",
    );
    let (program, ds) = checked(&src);
    assert!(ds.is_empty(), "{ds:?}");
    let program = program.expect("the program should check");
    let mut found: Vec<(String, String, Vec<String>)> = program
        .conformances
        .iter()
        .map(|(interface, c)| {
            (
                program.interfaces[interface.0].name.clone(),
                program.type_name(c.target),
                c.methods
                    .iter()
                    .map(|f| program.functions[f.0].name.clone())
                    .collect(),
            )
        })
        .collect();
    found.sort();
    let want = |interface: &str, ty: &str, methods: &[&str]| {
        (
            interface.to_string(),
            ty.to_string(),
            methods.iter().map(ToString::to_string).collect::<Vec<_>>(),
        )
    };
    assert_eq!(
        found,
        [
            want("Both", "Circle", &["Circle.area", "Circle.perimeter"]),
            want("Both", "Disc", &["Disc.area", "Disc.perimeter"]),
            want("Named", "Ball", &["Ball.name"]),
            want("Named", "Cube", &["Cube.name"]),
            want("Solid", "Ball", &["Ball.volume"]),
            want("Solid", "Cube", &["Cube.volume"]),
        ]
    );
}

/// A call of a method that a syntax error took poisons its function, so the program has no `tir`
/// and the checker no error: the parser's error says why. This is checked without the prelude,
/// which declares an enum of its own.
#[test]
fn a_call_of_a_method_lost_to_a_syntax_error_should_withhold_the_program() {
    for src in [
        "class C {\n    fn f(self) {\n        let = 1\n    }\n}\nfn main() {\n    C().f()\n}",
        "enum E {\n    A\n    fn f(self) {\n        let = 1\n    }\n}\nfn main() {\n    E.A.f()\n}",
        "class C {}\nextend C {\n    fn f(self) {\n        let = 1\n    }\n}\nfn main() {\n    C().f()\n}",
    ] {
        let lexed = crate::lexer::lex(src).expect("the source should lex");
        let parsed = crate::parser::parse(lexed, false);
        let empty = ast::Program::default();
        let (program, ds) = check(&parsed.program, &empty, src, &parsed.diagnostics);
        assert!(program.is_none() && ds.is_empty(), "{src}: {ds:?}");
    }
}

/// Two blocks whose `make` has a type parameter of its own, one returning it and one an array of
/// it, and a `main` that calls it on line 16.
fn make_blocks(statement: &str) -> String {
    format!(
        "interface One {{\n    fn make<U>(self, x: U) -> U\n}}\ninterface Many {{\n    fn make<V>(self, x: V) -> [V]\n}}\nclass C {{}}\nextend C: One {{\n    fn make<U>(self, x: U) -> U {{ x }}\n}}\nextend C: Many {{\n    fn make<V>(self, x: V) -> [V] {{ [x] }}\n}}\nfn main() {{\n    let c = C()\n    {statement}\n}}"
    )
}

/// The result of a candidate is compared after the call's type arguments replace the method's own
/// type parameters; a result that still holds a parameter cannot be told from the others.
#[test]
fn written_type_arguments_should_pick_the_conformance_block_by_its_result() {
    for (statement, expected) in [
        ("let n: i64 = c.make<i64>(1)", "One"),
        ("let a: [i64] = c.make<i64>(1)", "Many"),
        // The payload of an expected optional, when no result is the optional.
        ("let n: i64? = c.make<i64>(1)", "One"),
    ] {
        assert_eq!(
            called_interface(&make_blocks(statement)).as_deref(),
            Some(expected),
            "{statement}"
        );
    }
    for (statement, expected) in [
        // No type argument is written: the parameters are not bound, and the results are not known.
        ("let n: i64 = c.make(1)", vec![(Code::Ambiguous, (16, 20))]),
        // The results are known and the expected type is neither.
        (
            "let b: bool = c.make<i64>(1)",
            vec![(Code::Ambiguous, (16, 21))],
        ),
        // Written arguments of a wrong number leave the results unknown.
        (
            "let n: i64 = c.make<i64, i64>(1)",
            vec![(Code::Ambiguous, (16, 20))],
        ),
        // A written argument that does not resolve is reported, and the call is not also ambiguous.
        (
            "let n: i64 = c.make<Missing>(1)",
            vec![(Code::UnknownType, (16, 25))],
        ),
    ] {
        codes(&make_blocks(statement), &expected);
    }
}

/// A call with several candidates waits for its context in a generic call, as a call whose
/// arguments cannot bind its parameters does; the expected type of the outer call picks one.
#[test]
fn a_call_with_several_candidates_should_wait_for_the_context_of_the_outer_call() {
    let program = |statement: &str| {
        with_shapes(&format!(
            "class C {{}}\nextend C: Label {{\n    fn name(self) -> string {{ \"a\" }}\n}}\nextend C: Code {{\n    fn name(self) -> i64 {{ 7 }}\n}}\nfn id<T>(x: T) -> T {{ x }}\nfn main() {{\n    let c = C()\n    {statement}\n}}"
        ))
    };
    for (statement, part) in [
        ("let s: string = id(c.name())", "(call C.Label.name"),
        ("let k: i64 = id(c.name())", "(call C.Code.name"),
        ("let s: string = id(id(c.name()))", "(call C.Label.name"),
    ] {
        contains(&program(statement), &[part]);
    }
    for (statement, expected) in [
        // The outer call fixes `T`, and neither candidate returns it.
        (
            "let b: bool = id(c.name())",
            vec![(Code::Ambiguous, (11, 24))],
        ),
        // Nothing fixes `T`: the outer call is reported, and the inner one is not.
        ("let x = id(c.name())", vec![(Code::CannotInfer, (11, 13))]),
    ] {
        codes(&program(statement), &expected);
    }
}

/// Candidates that all return one known type leave nothing for the context to pick: the call is
/// reported at once, not first as a type the outer call cannot infer.
#[test]
fn a_call_whose_candidates_return_one_type_should_be_reported_at_once() {
    let blocks = "interface Tag1 {\n    fn name(self) -> string\n}\ninterface Tag2 {\n    fn name(self) -> string\n}\nclass C {}\nextend C: Tag1 {\n    fn name(self) -> string { \"a\" }\n}\nextend C: Tag2 {\n    fn name(self) -> string { \"b\" }\n}\nfn id<T>(x: T) -> T { x }\n";
    codes(
        &with_shapes(&format!(
            "{blocks}fn main() {{\n    let c = C()\n    let x = id(c.name())\n}}"
        )),
        &[(Code::Ambiguous, (17, 18))],
    );
}

/// A call waits for the context of an outer generic call only when several blocks answer it and its
/// arguments fit them; with one block, none, or arguments that fit none, it is checked at once.
#[test]
fn a_call_that_the_context_does_not_pick_should_not_wait_for_it() {
    let program = |blocks: &str, statement: &str| {
        with_shapes(&format!(
            "class C {{}}\n{blocks}fn id<T>(x: T) -> T {{ x }}\nfn main() {{\n    let c = C()\n    {statement}\n}}"
        ))
    };
    let label = "extend C: Label {\n    fn name(self) -> string { \"a\" }\n}\n";
    let two = format!("{label}extend C: Code {{\n    fn name(self) -> i64 {{ 7 }}\n}}\n");
    // One block answers: `T` is the type of the call, with no expected type.
    clean(&program(label, "let x = id(c.name())"));
    // No block answers: the call is unknown, and `T` is not reported besides.
    codes(
        &program(label, "let x = id(c.other())"),
        &[(Code::NoMember, (8, 18))],
    );
    // One block, whose generic result its argument binds: nothing is left to the context.
    let one = "interface Make {\n    fn make<U>(self, x: U) -> U\n}\nextend C: Make {\n    fn make<U>(self, x: U) -> U { x }\n}\n";
    clean(&program(one, "let x = id(c.make(1))"));
    // Arguments that fit no block: the call is checked at once, and is ambiguous.
    codes(
        &program(&two, "let x = id(c.name(1))"),
        &[(Code::Ambiguous, (11, 18))],
    );
    // Parameters that fit: the candidates wait for the context of the outer call.
    let tags = "interface Tag1 {\n    fn tag(self, n: i64) -> string\n}\ninterface Tag2 {\n    fn tag(self, n: i64) -> i64\n}\nextend C: Tag1 {\n    fn tag(self, n: i64) -> string { \"a\" }\n}\nextend C: Tag2 {\n    fn tag(self, n: i64) -> i64 { n }\n}\n";
    clean(&program(tags, "let s: string = id(c.tag(1))"));
    clean(&program(tags, "let k: i64 = id(c.tag(1))"));
}

/// An expected type that failed is reported already, and the call it reaches is poisoned with it.
#[test]
fn an_ambiguous_call_should_not_be_reported_when_its_context_failed() {
    let blocks = "class C {}\nextend C: Label {\n    fn name(self) -> string { \"a\" }\n}\nextend C: Code {\n    fn name(self) -> i64 { 7 }\n}\n";
    for (tail, expected) in [
        (
            "fn main() {\n    let c = C()\n    let x: Missing = c.name()\n}",
            vec![(Code::UnknownType, (10, 12))],
        ),
        (
            "fn take(a: Missing) {}\nfn main() {\n    let c = C()\n    take(c.name())\n}",
            vec![(Code::UnknownType, (8, 12))],
        ),
    ] {
        codes(&with_shapes(&format!("{blocks}{tail}")), &expected);
    }
}

/// A candidate whose result type failed to resolve has reported it, and the call is poisoned with
/// it.
#[test]
fn an_ambiguous_call_should_not_be_reported_when_a_candidate_failed() {
    codes(
        &with_shapes(
            "class C {}\nextend C: Label {\n    fn name(self) -> Missing { \"a\" }\n}\nextend C: Code {\n    fn name(self) -> i64 { 7 }\n}\nfn main() {\n    let c = C()\n    let n = c.name()\n}",
        ),
        &[(Code::UnknownType, (3, 22))],
    );
}

/// Type arguments on a method that takes none of its own are reported for every kind of method:
/// a body's, a plain block's and a conformance block's.
#[test]
fn type_arguments_on_a_method_of_a_block_should_be_reported() {
    let program = |statement: &str| {
        with_shapes(&format!(
            "class C {{\n    x: i64\n    fn body(self) {{}}\n}}\nextend C {{\n    fn grow(self) {{}}\n}}\nextend C: Label {{\n    fn name(self) -> string {{ \"a\" }}\n}}\nfn main() {{\n    let c = C(x: 1)\n    {statement}\n}}"
        ))
    };
    for (statement, col) in [
        ("c.body<i64>()", 11),
        ("c.grow<i64>()", 11),
        ("let _ = c.name<i64>()", 19),
    ] {
        codes(&program(statement), &[(Code::TypeArguments, (13, col))]);
    }
    // A method with type parameters of its own takes them.
    clean(&make_blocks("let n: i64 = c.make<i64>(1)"));
}

/// A parameter of an `extend` block that its target does not name is reported at the parameter,
/// and the block's methods give the target nothing.
#[test]
fn an_extend_parameter_that_its_target_does_not_name_should_be_reported() {
    let class = "class C {\n    x: i64\n}\n";
    for (row, expected) in [
        ("extend<T> C {\n}", vec![(Code::CannotExtend, (4, 8))]),
        // The methods of the block are not checked, and a call of one is not reported.
        (
            "extend<T> C {\n    fn keep(self, x: T) -> T { x }\n}\nfn main() {\n    C(x: 1).keep(1)\n}",
            vec![(Code::CannotExtend, (4, 8))],
        ),
        // Each parameter is reported; the target and the interface are found all the same.
        (
            "extend<T, U> C: Named {\n    fn name(self) -> string { \"c\" }\n}",
            vec![(Code::CannotExtend, (4, 8)), (Code::CannotExtend, (4, 11))],
        ),
        // A target with arguments names the parameters it holds, and no others.
        (
            "class Pile<V> {\n    items: [V]\n}\nextend<T> Pile<i64> {\n}",
            vec![(Code::CannotExtend, (7, 8))],
        ),
        (
            "class Pile<V> {\n    items: [V]\n}\nextend<T, U> Pile<U> {\n}",
            vec![(Code::CannotExtend, (7, 8))],
        ),
    ] {
        codes(
            &with_shapes(&with_main(&format!("{class}{row}"))),
            &expected,
        );
    }
}

/// The fix of a parameter that its target does not name takes the list out when it holds that
/// parameter alone, and the block names it nowhere else.
#[test]
fn the_fix_of_an_unused_extend_parameter_should_take_out_the_list_when_it_is_certain() {
    let program = |block: &str| with_main(&format!("class C {{\n    x: i64\n}}\n{block}"));
    let src = program("extend<T> C {\n}");
    let d = diagnostic(&src, Code::CannotExtend, (4, 8, "T"), &[]);
    assert_eq!(
        fixes(&src, &d),
        [(true, vec![(4, 7, "<T> ", " ".to_string())])]
    );
    fix_removes_its_code(&src, &d);
    // The list is the only text between `extend` and the target: one space stays.
    let src = program("extend<T>C {\n}");
    let d = diagnostic(&src, Code::CannotExtend, (4, 8, "T"), &[]);
    assert_eq!(applied(&src, &d), program("extend C {\n}"));
    fix_removes_its_code(&src, &d);
    // A target with arguments names its own parameters, and a parameter it does not name goes.
    let generic =
        |block: &str| with_main(&format!("class Stack<T> {{\n    items: [T]\n}}\n{block}"));
    let src = generic("extend<T> Stack<i64> {\n}");
    let d = diagnostic(&src, Code::CannotExtend, (4, 8, "T"), &[]);
    assert_eq!(applied(&src, &d), generic("extend Stack<i64> {\n}"));
    fix_removes_its_code(&src, &d);
    // Another use of the name, a second parameter and a comment each leave the list in place.
    for block in [
        "extend<T> C {\n    fn keep(self, x: T) -> T { x }\n}",
        "extend<T, U> C {\n}",
        "extend<T // why\n> C {\n}",
    ] {
        let src = program(block);
        let (_, ds) = checked(&src);
        assert!(ds.iter().all(|d| d.fixes().is_empty()), "{block}: {ds:?}");
    }
}

/// An interface inherited with arguments is one the type conforms to, as one without: the list of
/// a class gives it through the body, and a block of another interface needs a block of its own.
#[test]
fn an_inherited_generic_interface_should_be_conformed_to_as_a_plain_one() {
    for (row, expected) in [
        (
            "interface G<T> {}\ninterface I: G<i64> {}\nclass C: I {}",
            vec![],
        ),
        // Two entries of one interface.
        (
            "interface G<T> {}\ninterface I: G<i64>, G<f64> {}\nclass C: I {}",
            vec![],
        ),
        // Another argument is another entry, reached through another listed interface.
        (
            "interface G<T> {}\ninterface I: G<i64> {}\ninterface J: G<f64> {}\nclass C: I, J {}",
            vec![],
        ),
        // A block of the inherited interface, with its argument, gives the conformance.
        (
            "interface G<T> {}\ninterface I: G<i64> {}\nclass C {}\nextend C: G<i64> {}\nextend C: I {}",
            vec![],
        ),
        // A block of the same interface with another argument does not.
        (
            "interface G<T> {}\ninterface I: G<i64> {}\nclass C {}\nextend C: G<f64> {}\nextend C: I {}",
            vec![(Code::NotConform, (5, 11))],
        ),
        // No block at all.
        (
            "interface G<T> {}\ninterface I: G<i64> {}\nclass C {}\nextend C: I {}",
            vec![(Code::NotConform, (4, 11))],
        ),
    ] {
        codes(&with_main(row), &expected);
    }
}

/// An extension's method is checked against the fields of its class through one index, built once
/// for the class: the work follows the fields and not the product with the methods.
#[test]
fn extension_methods_should_read_the_fields_of_their_class_once() {
    let indexed = |n: usize| {
        let fields: Vec<String> = (0..n).map(|i| format!("    f{i}: i64\n")).collect();
        let methods: Vec<String> = (0..n)
            .map(|i| format!("    fn m{i}(self) {{}}\n"))
            .collect();
        let (fields, methods) = (fields.concat(), methods.concat());
        let src = format!("class C {{\n{fields}}}\nextend C {{\n{methods}}}\nfn main() {{}}");
        FIELDS_INDEXED.set(0);
        clean(&src);
        FIELDS_INDEXED.get()
    };
    assert_eq!(indexed(32), 32);
    assert_eq!(indexed(64), 64);
}

/// The interfaces that one declaration lists share one visited set: each listed interface costs a
/// step, and the steps do not grow with the interfaces listed before it.
#[test]
fn a_declaration_should_close_its_listed_interfaces_in_one_walk() {
    let steps = |n: usize| {
        let interfaces: Vec<String> = (0..n).map(|i| format!("interface I{i} {{}}\n")).collect();
        let listed: Vec<String> = (0..n).map(|i| format!("I{i}")).collect();
        let src = format!(
            "{}class C: {} {{}}\nfn main() {{}}",
            interfaces.concat(),
            listed.join(", ")
        );
        CLOSURE_STEPS.set(0);
        clean(&src);
        CLOSURE_STEPS.get()
    };
    assert_eq!(steps(32), 32);
    assert_eq!(steps(64), 64);
}

/// The check of choices reads each conformance and each choice a fixed number of times, whatever
/// else the program holds: `n` classes that choose one type each, `n` blocks that do, and one
/// class that chooses `n` types of one interface. The reads of `2n` are at most twice those of `n`.
#[test]
fn the_checks_of_choices_should_read_each_conformance_and_choice_a_fixed_number_of_times() {
    let steps = |src: String| {
        CHOICE_STEPS.set(0);
        clean(&src);
        CHOICE_STEPS.get()
    };
    let classes = |n: usize| {
        let each: Vec<String> = (0..n)
            .map(|i| format!("class C{i}: Source {{\n    type Item = i64\n}}\n"))
            .collect();
        format!(
            "interface Source {{\n    type Item\n}}\n{}fn main() {{}}",
            each.concat()
        )
    };
    let blocks = |n: usize| {
        let each: Vec<String> = (0..n)
            .map(|i| format!("class C{i} {{}}\nextend C{i}: Source {{\n    type Item = i64\n}}\n"))
            .collect();
        format!(
            "interface Source {{\n    type Item\n}}\n{}fn main() {{}}",
            each.concat()
        )
    };
    let many = |n: usize| {
        let declared: Vec<String> = (0..n).map(|i| format!("    type A{i}\n")).collect();
        let chosen: Vec<String> = (0..n).map(|i| format!("    type A{i} = i64\n")).collect();
        format!(
            "interface Source {{\n{}}}\nclass C: Source {{\n{}}}\nfn main() {{}}",
            declared.concat(),
            chosen.concat()
        )
    };
    let classes: &dyn Fn(usize) -> String = &classes;
    for shape in [classes, &blocks, &many] {
        let (small, large) = (steps(shape(32)), steps(shape(64)));
        assert!(small >= 32, "{small} reads for 32");
        assert!(large <= 2 * small, "{large} reads for 64, {small} for 32");
    }
}

/// A method sees the choices of its class by name, without a copy of them: a class with `n`
/// choices and `n` methods that each write one reads `n` choices to find the others, not `n` for
/// each method. The reads of 64 are at most twice those of 32.
#[test]
fn a_method_should_find_the_choices_of_its_class_without_copying_them() {
    let steps = |n: usize| {
        let declared: Vec<String> = (0..n).map(|i| format!("    type A{i}\n")).collect();
        let chosen: Vec<String> = (0..n).map(|i| format!("    type A{i} = i64\n")).collect();
        let methods: Vec<String> = (0..n)
            .map(|i| format!("    fn m{i}(self) -> A{i} {{ 1 }}\n"))
            .collect();
        let src = format!(
            "interface Source {{\n{}}}\nclass C: Source {{\n    n: i64\n{}{}}}\nfn main() {{}}",
            declared.concat(),
            chosen.concat(),
            methods.concat()
        );
        CHOICE_STEPS.set(0);
        clean(&src);
        CHOICE_STEPS.get()
    };
    let (small, large) = (steps(32), steps(64));
    assert!(small >= 32, "{small} reads for 32");
    assert!(large <= 2 * small, "{large} reads for 64, {small} for 32");
}

/// The bound and closure steps and the scopes kept for `list`, a list of type parameters
/// that is checked clean beside the interfaces it names.
fn bound_steps(list: &str) -> (usize, usize) {
    let src = format!(
        "interface Source {{\n    type Item\n}}\ninterface Holds<Y> {{\n    type Item\n}}\ninterface Convert<Y> {{}}\n{list}\nfn main() {{}}"
    );
    BOUND_STEPS.set(0);
    SCOPES_KEPT.set(0);
    CLOSURE_STEPS.set(0);
    let ds = diags(&src);
    assert!(ds.iter().all(|d| d.severity() != Severity::Error), "{ds:?}");
    (BOUND_STEPS.get() + CLOSURE_STEPS.get(), SCOPES_KEPT.get())
}

/// The bounds that write an associated type of another parameter are read a fixed number of
/// times each, and put in scope in place. The steps are the bounds resolved and the
/// entries of closures built. Those of 64 are at most twice those of 32
/// (and a few more), for five lists: a chain of bounds written in reverse (each in a wave of its
/// own); one parameter with `n` bounds that write `S.Item` (a closure that is built once, not
/// after each bound); a chain whose bounds write an associated type of a parameter with `n`
/// bounds (a closure that stays when a wave adds bounds to other parameters); and one parameter
/// whose `n` bounds each write the type that the one before declares (a closure that grows by the
/// closure of each bound); and one interface that declares `n` types, named by `n` bounds of
/// one parameter, which `n` other bounds write (the declarations are read once, not for each
/// bound); and one interface that declares `n` types, named by the bound of each of `n`
/// parameters that one bound writes a type of (a parameter reads the types that its bounds write,
/// not all that the interface declares). The scopes kept are as many for 64 as for 32.
#[test]
fn bounds_that_write_associated_types_should_be_read_a_fixed_number_of_times() {
    let chain = |n: usize| {
        let params: Vec<String> = (0..n)
            .rev()
            .map(|i| {
                if i == 0 {
                    "T0: Source".to_string()
                } else {
                    format!("T{i}: Holds<T{}.Item>", i - 1)
                }
            })
            .collect();
        format!("fn f<{}>() {{}}", params.join(", "))
    };
    let repeated = |n: usize| {
        let bounds: Vec<String> = (0..n).map(|_| "Convert<S.Item>".to_string()).collect();
        format!("fn f<S: Source & {}>(_s: S) {{}}", bounds.join(" & "))
    };
    let beside = |n: usize| {
        let params: Vec<String> = (0..n)
            .rev()
            .map(|i| {
                if i == 0 {
                    "T0: Source".to_string()
                } else {
                    format!("T{i}: Holds2<T{}.Item, Q.Item>", i - 1)
                }
            })
            .collect();
        let each: Vec<String> = (0..n).map(|i| format!("Z{i}")).collect();
        let zs: Vec<String> = (0..n).map(|i| format!("interface Z{i} {{}}\n")).collect();
        format!(
            "{}interface Holds2<Y, Z> {{\n    type Item\n}}\nfn f<{}, Q: Source & {}>() {{}}",
            zs.concat(),
            params.join(", "),
            each.join(" & ")
        )
    };
    let within = |n: usize| {
        let declared: Vec<String> = (0..n)
            .map(|i| format!("interface I{i}<X> {{\n    type N{i}\n}}\n"))
            .collect();
        let bounds: Vec<String> = (1..n)
            .rev()
            .map(|i| format!("I{i}<S.N{}>", i - 1))
            .collect();
        format!(
            "{}fn f<S: I0<i64> & {}>(_s: S) {{}}",
            declared.concat(),
            bounds.join(" & ")
        )
    };
    let many = |n: usize| {
        let declared: Vec<String> = (0..n).map(|i| format!("    type A{i}\n")).collect();
        let bounds: Vec<String> = (0..n).map(|_| "Big<R.Item>".to_string()).collect();
        let writers: Vec<String> = (0..n).map(|i| format!("U{i}: Convert<T.A{i}>")).collect();
        format!(
            "interface Big<X> {{\n{}}}\nfn f<R: Source, T: {}, {}>() {{}}",
            declared.concat(),
            bounds.join(" & "),
            writers.join(", ")
        )
    };
    let roots = |n: usize| {
        let declared: Vec<String> = (0..n).map(|i| format!("    type A{i}\n")).collect();
        let owners: Vec<String> = (0..n).map(|i| format!("S{i}: Big<R.Item>")).collect();
        let writers: Vec<String> = (0..n)
            .map(|i| format!("U{i}: Convert<S{i}.A{i}>"))
            .collect();
        format!(
            "interface Big<X> {{\n{}}}\nfn f<R: Source, {}, {}>() {{}}",
            declared.concat(),
            owners.join(", "),
            writers.join(", ")
        )
    };
    let chain: &dyn Fn(usize) -> String = &chain;
    for (name, shape) in [
        ("chain", chain),
        ("repeated", &repeated),
        ("beside", &beside),
        ("within", &within),
        ("many", &many),
        ("roots", &roots),
    ] {
        let ((small, kept_small), (large, kept_large)) =
            (bound_steps(&shape(32)), bound_steps(&shape(64)));
        assert!(small >= 32, "{name}: {small} steps for 32");
        // Twice the steps, and a few more for the work that no bound adds.
        assert!(
            large <= 2 * small + 8,
            "{name}: {large} steps for 64, {small} for 32"
        );
        assert_eq!(
            kept_large, kept_small,
            "{name}: {kept_large} scopes for 64, {kept_small} for 32"
        );
    }
}

/// What an interface declares is indexed by name once for the program, and a type is probed once
/// for a bound. The steps are the declarations built and the types `normalize` walks. Those of 64
/// are at most twice those of 32 (and a few more), for an interface that declares `n` types
/// which `n` functions each name through a bound (not one index for each function), for a type
/// that an interface with `n` types, inherited by the bound `Holder`, meets in `n` calls (not
/// every type for each call, and not every declaration read for each call), and for a type that
/// meets `n` interfaces which inherit each other, each with a type (not every type for each
/// interface).
#[test]
fn a_bound_should_build_what_its_interface_declares_once_and_probe_a_type_once() {
    let functions = |n: usize| {
        let declared: Vec<String> = (0..n).map(|i| format!("    type A{i}\n")).collect();
        let each: Vec<String> = (0..n)
            .map(|i| format!("fn f{i}<S: Big, U: Convert<S.A0>>(_s: S, _u: U) {{}}"))
            .collect();
        format!(
            "interface Big {{\n{}}}\n{}\nfn main() {{}}",
            declared.concat(),
            each.join("\n")
        )
    };
    let calls = |n: usize| {
        let declared: Vec<String> = (0..n).map(|i| format!("    type A{i}\n")).collect();
        let chosen: Vec<String> = (0..n).map(|i| format!("    type A{i} = i64\n")).collect();
        format!(
            "interface Big {{\n{}}}\ninterface Holder: Big {{}}\nclass Leaf {{}}\nextend Leaf: Big {{\n{}}}\nextend Leaf: Holder {{}}\nfn need<S: Holder>(_s: S) {{}}\nfn main() {{\n{}}}",
            declared.concat(),
            chosen.concat(),
            "    need(Leaf())\n".repeat(n)
        )
    };
    // `n` interfaces, each inheriting the one before and declaring a type, and a call for each.
    let chain = |n: usize| {
        let mut src = String::from("interface I0 {\n    type T0\n}\n");
        for i in 1..n {
            let _ = writeln!(src, "interface I{i}: I{} {{\n    type T{i}\n}}", i - 1);
        }
        src.push_str("class Leaf {}\n");
        for i in 0..n {
            let _ = writeln!(src, "extend Leaf: I{i} {{\n    type T{i} = i64\n}}");
            let _ = writeln!(src, "fn go{i}<S: I{i}>(_s: S) {{}}");
        }
        src.push_str("fn main() {\n");
        for i in 0..n {
            let _ = writeln!(src, "    go{i}(Leaf())");
        }
        src + "}"
    };
    let steps = |src: String| {
        DECLARED_BUILT.set(0);
        PROBE_STEPS.set(0);
        crate::conformance::NORMALIZED.set(0);
        let ds = diags(&with_convert_interface(&src));
        assert!(ds.iter().all(|d| d.severity() != Severity::Error), "{ds:?}");
        [
            DECLARED_BUILT.get(),
            crate::conformance::NORMALIZED.get(),
            PROBE_STEPS.get(),
        ]
    };
    // The counters each shape is measured by: the declarations built, the types walked, and the
    // declarations the probes read.
    let functions: &dyn Fn(usize) -> String = &functions;
    for (name, shape, counters) in [
        ("functions", functions, &[0][..]),
        ("calls", &calls, &[1, 2]),
        ("chain", &chain, &[1]),
    ] {
        let (small, large) = (steps(shape(32)), steps(shape(64)));
        for &counter in counters {
            let (small, large) = (small[counter], large[counter]);
            assert!(small >= 32, "{name}: {small} steps for 32");
            assert!(
                large <= 2 * small + 8,
                "{name}: {large} steps for 64, {small} for 32 (counter {counter})"
            );
        }
    }
}

/// `src` and the interface `Convert`, which no shape of a counting test declares.
fn with_convert_interface(src: &str) -> String {
    format!("interface Convert<Y> {{}}\n{src}")
}

/// A type written through a parameter, as `S.Name`, is found by its name in an index of the
/// parameter's closure, which grows with the closure: a lookup reads the entries of the
/// interfaces that declare the name, and no others. The steps are the entries indexed and the
/// entries read. Those of 64 are at most twice those of 32 (and a few more), for one interface
/// that declares `n` types, which a function names through one parameter; for `n` interfaces that
/// declare a type of one name, which a function names through a parameter of one of them; and for
/// one parameter whose `n` bounds each write the type that the one before declares.
#[test]
fn an_associated_type_should_be_found_by_its_name_without_reading_the_others() {
    let names = |n: usize| {
        let declared: Vec<String> = (0..n).map(|i| format!("    type A{i}\n")).collect();
        let params: Vec<String> = (0..n).map(|i| format!("x{i}: S.A{i}")).collect();
        format!(
            "interface Big {{\n{}}}\nfn f<S: Big>({}) {{}}\nfn main() {{}}",
            declared.concat(),
            params.join(", ")
        )
    };
    let shared = |n: usize| {
        let declared: Vec<String> = (0..n)
            .map(|i| format!("interface G{i} {{\n    type Unit\n}}\n"))
            .collect();
        let params: Vec<String> = (0..n).map(|i| format!("x{i}: T.Unit")).collect();
        format!(
            "{}fn f<T: G0>({}) {{}}\nfn main() {{}}",
            declared.concat(),
            params.join(", ")
        )
    };
    let within = |n: usize| {
        let declared: Vec<String> = (0..n)
            .map(|i| format!("interface I{i}<X> {{\n    type N{i}\n}}\n"))
            .collect();
        let bounds: Vec<String> = (1..n)
            .rev()
            .map(|i| format!("I{i}<S.N{}>", i - 1))
            .collect();
        format!(
            "{}fn f<S: I0<i64> & {}>(_s: S) {{}}\nfn main() {{}}",
            declared.concat(),
            bounds.join(" & ")
        )
    };
    let steps = |src: String| {
        LOOKUP_STEPS.set(0);
        let ds = diags(&src);
        assert!(ds.iter().all(|d| d.severity() != Severity::Error), "{ds:?}");
        LOOKUP_STEPS.get()
    };
    let names: &dyn Fn(usize) -> String = &names;
    for (name, shape) in [("names", names), ("shared", &shared), ("within", &within)] {
        let (small, large) = (steps(shape(32)), steps(shape(64)));
        assert!(small >= 32, "{name}: {small} steps for 32");
        assert!(
            large <= 2 * small + 8,
            "{name}: {large} steps for 64, {small} for 32"
        );
    }
}

/// A block that lacks every interface its own inherits reports each one by the place the closure
/// recorded for it, so no report reads an inheritance list again.
#[test]
fn each_missing_inherited_interface_should_be_traced_back_without_a_search() {
    let steps = |n: usize| {
        let interfaces: Vec<String> = (0..n).map(|i| format!("interface I{i} {{}}\n")).collect();
        let listed: Vec<String> = (0..n).map(|i| format!("I{i}")).collect();
        let src = format!(
            "{}interface Root: {} {{}}\nclass X {{}}\nextend X: Root {{}}\nfn main() {{}}",
            interfaces.concat(),
            listed.join(", ")
        );
        CLOSURE_STEPS.set(0);
        let ds = diags(&src);
        assert_eq!(ds.len(), n, "{ds:?}");
        CLOSURE_STEPS.get()
    };
    // The root, then one read of the inheritance of `Root` for each of the others.
    assert_eq!(steps(32), 1 + 32);
    assert_eq!(steps(64), 1 + 64);
}

/// A call that more than one block answers reads the results of its candidates a fixed number of
/// times: once to count them and once to find the first that one candidate has, however many
/// results are alike.
#[test]
fn an_ambiguous_call_should_read_each_candidate_a_fixed_number_of_times() {
    let steps = |n: usize| {
        // All but the last return a `string`, so a search for the one result that only one
        // candidate has would pass the others `n` times.
        let blocks: Vec<String> = (0..n)
            .map(|i| {
                let (ret, value) = if i + 1 == n { ("i64", "1") } else { ("string", "\"\"") };
                format!(
                    "interface L{i} {{\n    fn name(self) -> {ret}\n}}\nextend C: L{i} {{\n    fn name(self) -> {ret} {{ {value} }}\n}}\n"
                )
            }).collect();
        let src = format!(
            "class C {{}}\n{}fn main() {{\n    let n = C().name()\n}}",
            blocks.concat()
        );
        PICK_STEPS.set(0);
        codes(&src, &[(Code::Ambiguous, (6 * n + 3, 17))]);
        PICK_STEPS.get()
    };
    assert_eq!(steps(32), 2 * 32);
    assert_eq!(steps(64), 2 * 64);
}

/// The interfaces, classes and conformances the bound rows use, and an empty `main`. A row is a
/// whole program that these follow, so that a position in the row is its own line and column.
const BOUNDS: &str = "interface Named {\n    fn name(self) -> string\n}\ninterface Shape: Named {\n    fn area(self) -> f64\n}\ninterface Same {\n    fn same(self, other: Self) -> bool\n}\ninterface Ord2: Same {\n    fn less(self, other: Self) -> bool\n}\ninterface Keep {\n    fn keep<U>(self, x: U) -> U\n}\ninterface Make {\n    fn make<U>(self) -> [U]\n}\nclass Circle {\n    r: f64\n}\nextend Circle: Named {\n    fn name(self) -> string { \"circle\" }\n}\nextend Circle: Shape {\n    fn area(self) -> f64 { self.r }\n}\nextend Circle: Same {\n    fn same(self, other: Circle) -> bool { self.r == other.r }\n}\nextend Circle: Ord2 {\n    fn less(self, other: Circle) -> bool { self.r < other.r }\n}\nextend Circle: Keep {\n    fn keep<V>(self, x: V) -> V { x }\n}\nextend Circle: Make {\n    fn make<V>(self) -> [V] { [] }\n}\nclass Node {\n    id: i64\n}\nclass Sorted<T: Same> {\n    items: [T]\n    fn me(self) -> Sorted<T> { self }\n}\nenum Pick<T: Same> {\n    One(T)\n}\nfn main() {}";

/// `row`, a program, then the interfaces and classes of `BOUNDS`.
fn with_bounds(row: &str) -> String {
    format!("{row}\n{BOUNDS}")
}

/// A generic body uses what its bounds give: the requirements of the interfaces a bound names and
/// inherits, each once, through a bound, an optional with `?.`, another generic call, and a type
/// that is bounded.
#[test]
fn bounds_should_check_clean() {
    for row in [
        // A requirement of a bound, in a loop.
        "fn total<T: Shape>(xs: [T]) -> f64 {\n    let mut sum = 0.0\n    for x in xs {\n        sum += x.area()\n    }\n    sum\n}",
        // Two bounds, and a requirement with `Self` as a parameter.
        "fn both<T: Shape & Same>(a: T, b: T) -> bool {\n    a.same(b) && a.area() > 0.0\n}",
        // An inherited bound.
        "fn named<T: Shape>(x: T) -> string {\n    x.name()\n}",
        // A diamond: `C` reaches `D` twice, and its method is found once.
        "interface C {\n    fn c(self) -> i64\n}\ninterface A: C {}\ninterface B: C {}\ninterface D: A, B {}\nfn d<T: D>(x: T) -> i64 {\n    x.c()\n}",
        // A generic function passes its parameter on, to a bound it has and to one it inherits.
        "fn f<T: Same>(x: T) -> bool {\n    x.same(x)\n}\nfn g<T: Same>(x: T) -> bool {\n    f(x)\n}",
        "fn f<T: Same>(x: T) -> bool {\n    x.same(x)\n}\nfn g<T: Ord2>(x: T) -> bool {\n    f(x)\n}",
        // `?.` calls the requirement on the payload.
        "fn opt<T: Shape>(x: T?) -> f64? {\n    x?.area()\n}",
        // A bounded class and enum, built, named and returned.
        "fn make() -> Sorted<Circle> {\n    Sorted(items: [Circle(r: 1.0)])\n}\nfn pick(c: Circle) -> Pick<Circle> {\n    Pick.One(c)\n}\nfn local() {\n    let s: Sorted<Circle> = Sorted(items: [])\n}",
        // The checks of a signature, a field and a payload run with the declaration's bounds.
        "fn f<T: Same>(s: Sorted<T>) -> i64 {\n    1\n}\nclass Holder<T: Same> {\n    s: Sorted<T>\n}\nenum Hold<T: Same> {\n    H(Pick<T>)\n}\ninterface I: Same {\n    fn f(self) -> Sorted<Self>\n}\nfn g<T: Same>(s: Sorted<T>) -> Sorted<T> {\n    s.me()\n}",
        // A method of a bounded class or enum, and a method of a plain block, call through the
        // bounds they have.
        "class Holder<T: Shape> {\n    v: T\n    fn area(self) -> f64 {\n        self.v.area()\n    }\n}\nenum Wrap<T: Shape> {\n    W(T)\n    fn name(self) -> string {\n        match self {\n            Wrap.W(x) => x.name()\n        }\n    }\n}\nextend Circle {\n    fn fit<T: Shape>(self, x: T) -> f64 {\n        x.area() + self.r\n    }\n}\nfn h(c: Circle) -> f64 {\n    Holder(v: c).area() + c.fit(c)\n}",
        // A requirement with a type parameter of its own, called at two types.
        "fn k<T: Keep>(x: T) -> string {\n    let a: i64 = x.keep(1)\n    x.keep(\"a\")\n}",
        // A call through a bound that is an argument of a generic call: it waits only when its
        // result is a parameter that its arguments do not give.
        "interface Me {\n    fn me(self) -> Self\n}\nfn id<T>(a: T) -> T {\n    a\n}\nfn f<T: Me & Keep>(x: T) {\n    let n = id(x.me())\n    let m = id(x.keep(1))\n}",
        // The argument that waits for the other: `make` is generic in its own parameter.
        "fn take<T>(x: T, y: T) -> T {\n    x\n}\nfn use_make<S: Make>(s: S) -> i64 {\n    take(s.make(), [1]).len()\n}",
        // The call on a type parameter that has one bound of two that require a name.
        "interface A {\n    fn name(self) -> string\n}\ninterface B {\n    fn name(self) -> i64\n}\nfn pick<T: A & B>(x: T, s: string) -> i64 {\n    let n: i64 = x.name()\n    take(x.name(), s)\n    n\n}\nfn take<T>(a: T, b: T) {}",
    ] {
        clean(&with_bounds(row));
    }
}

/// A call through a bound is `Interface.method(receiver)` before `mono`.
#[test]
fn a_call_through_a_bound_should_lower_to_an_interface_call() {
    contains(
        &with_bounds("fn total<T: Shape>(x: T) -> f64 {\n    x.area()\n}"),
        &["(fn total<T> ((x#0 T)) f64 ", "(call Shape.area x#0)"],
    );
    // The call's own type arguments, and an inherited requirement.
    contains(
        &with_bounds(
            "fn k<T: Keep>(x: T) -> string {\n    x.keep(\"a\")\n}\nfn n<T: Shape>(x: T) -> string {\n    x.name()\n}",
        ),
        &[
            "(call Keep.keep<string> x#0 \"a\")",
            "(call Named.name x#0)",
        ],
    );
}

/// The errors of a bound that a type does not meet, at the call, the type or the argument that
/// meets it, each once.
#[test]
fn bound_violations_should_be_reported_once_at_the_call() {
    for (row, expected) in [
        // The argument that binds the parameter.
        (
            "fn total<T: Shape>(xs: [T]) -> f64 {\n    0.0\n}\nfn go(n: Node) -> f64 {\n    total([n])\n}",
            vec![(Code::NotConform, (5, 11))],
        ),
        // A parameter without the bound: one report, and the body is not checked again.
        (
            "fn f<T: Same>(x: T) -> bool {\n    true\n}\nfn g<T>(x: T) -> bool {\n    f(x)\n}",
            vec![(Code::NotConform, (5, 7))],
        ),
        // A bounded class named by a local, a parameter, a field and a payload.
        (
            "fn h(s: Sorted<Node>) {}",
            vec![(Code::NotConform, (1, 16))],
        ),
        (
            "fn h() {\n    let s: Sorted<Node> = Sorted(items: [])\n}",
            vec![(Code::NotConform, (2, 19))],
        ),
        (
            "class Bad {\n    s: Sorted<Node>\n}",
            vec![(Code::NotConform, (2, 15))],
        ),
        (
            "enum Bad {\n    B(Sorted<Node>)\n}",
            vec![(Code::NotConform, (2, 14))],
        ),
        // A class and a variant built with arguments that do not meet the bound.
        (
            "fn mk(n: Node) {\n    let s = Sorted(items: [n])\n}",
            vec![(Code::NotConform, (2, 27))],
        ),
        (
            "fn mk(n: Node) {\n    let p = Pick.One(n)\n}",
            vec![(Code::NotConform, (2, 22))],
        ),
        // A type argument written for the call.
        (
            "fn f<T: Same>(x: T) {}\nfn g(n: Node) {\n    f<Node>(n)\n}",
            vec![(Code::NotConform, (3, 7))],
        ),
        // The second of two written type arguments binds the parameter with the bound.
        (
            "fn f<A, B: Same>(a: A, b: B) {}\nfn g(n: Node) {\n    f<i64, Node>(1, n)\n}",
            vec![(Code::NotConform, (3, 12))],
        ),
        // The method of a generic class has an owner argument before its own parameters, and a
        // bound that fails poisons the call: its tail adds no mismatch.
        (
            "class Hold<T> {\n    v: T\n    fn put<U: Same>(self, u: U) -> i64 { 0 }\n}\nfn go(h: Hold<Circle>, n: Node) -> string {\n    h.put(n)\n}",
            vec![(Code::NotConform, (6, 11))],
        ),
        // A built-in type conforms to nothing yet.
        (
            "fn f<T: Same>(x: T) {}\nfn g() {\n    f(1)\n}",
            vec![(Code::NotConform, (3, 7))],
        ),
        // An interface with arguments: the report names them, and the parameter gets no help.
        (
            "interface Convert<T> {\n    fn convert(self) -> T\n}\nfn f<T: Convert<i64>>(x: T) {}\nfn g(n: Node) {\n    f(n)\n}",
            vec![(Code::NotConform, (6, 7))],
        ),
        // A parameter bound for the type it is a parameter of, with no bound of its own.
        (
            "fn f<T>(s: Sorted<T>) {}",
            vec![(Code::NotConform, (1, 19))],
        ),
        // A bound on the requirement's own parameter.
        (
            "interface Bind {\n    fn bind<U: Same>(self, x: U)\n}\nclass B: Bind {\n    fn bind<V: Same>(self, x: V) {}\n}\nfn go(b: B, n: Node) {\n    b.bind(n)\n}\nfn through<T: Bind>(t: T, n: Node) {\n    t.bind(n)\n}",
            vec![(Code::NotConform, (8, 12)), (Code::NotConform, (11, 12))],
        ),
    ] {
        codes(&with_bounds(row), &expected);
    }
}

/// A body that uses what its bounds do not give is a missing method on the parameter.
#[test]
fn a_method_that_no_bound_gives_should_be_reported() {
    for (row, expected) in [
        (
            "fn f<T>(x: T) -> f64 {\n    x.area()\n}",
            vec![(Code::NoMember, (2, 7))],
        ),
        (
            "fn f<T: Named>(x: T) -> f64 {\n    x.area()\n}",
            vec![(Code::NoMember, (2, 7))],
        ),
        // Two interfaces declare it, and one that takes a parameter does: no bound to add.
        (
            "interface Area2 {\n    fn area(self) -> f64\n}\nfn f<T>(x: T) -> f64 {\n    x.area()\n}",
            vec![(Code::NoMember, (5, 7))],
        ),
        (
            "interface Convert<T> {\n    fn convert(self) -> T\n}\nfn f<T>(x: T) -> f64 {\n    x.convert()\n}",
            vec![(Code::NoMember, (5, 7))],
        ),
        // The arguments of the call are checked all the same.
        (
            "fn f<T>(x: T) -> f64 {\n    x.area(1 + \"a\")\n}",
            vec![(Code::NoMember, (2, 7)), (Code::Mismatch, (2, 12))],
        ),
    ] {
        codes(&with_bounds(row), &expected);
    }
}

/// A variant that no argument can give its enum's parameter waits for the context of the call it
/// is an argument of, which here is the other argument.
#[test]
fn a_variant_that_leaves_a_parameter_open_should_wait_for_context() {
    clean(
        "enum Tagged<T> {\n    Tag(i64)\n}\nfn take<T>(a: T, b: T) {}\nfn f(t: Tagged<string>) {\n    take(Tagged.Tag(1), t)\n}\nfn main() {}",
    );
}

/// A call through a bound has the arguments of its requirement: a wrong count is reported at the
/// call, unless a comparison misread as a call took the commas of the list.
#[test]
fn a_call_through_a_bound_should_check_the_count_of_its_arguments() {
    for (row, expected) in [
        (
            "fn f<T: Shape>(x: T) -> f64 {\n    x.area(1)\n}",
            vec![(Code::ArgumentCount, (2, 5))],
        ),
        (
            "interface Two {\n    fn two(self, a: bool, b: bool)\n}\nfn f<T: Two>(x: T, a: i64, b: i64, c: i64, d: i64) {\n    x.two(a < b, c > (d))\n}",
            vec![(Code::TypeArguments, (5, 11))],
        ),
        // The arguments do not fit the requirement: the call does not wait for the one around it.
        (
            "fn id<T>(a: T) -> T {\n    a\n}\nfn f<S: Make>(s: S) {\n    let n = id(s.make(1))\n}",
            vec![(Code::ArgumentCount, (5, 16))],
        ),
        // An argument of the wrong type, and a label.
        (
            "fn f<T: Same>(x: T) -> bool {\n    x.same(1)\n}",
            vec![(Code::Mismatch, (2, 12))],
        ),
        // The call is `Error` when its argument fails, so the tail adds no mismatch of its own.
        (
            "fn f<T: Same>(x: T) -> i64 {\n    x.same(1)\n}",
            vec![(Code::Mismatch, (2, 12))],
        ),
        (
            "fn f<T: Keep>(x: T) -> i64 {\n    x.keep(v: 1)\n}",
            vec![(Code::LabeledArgument, (2, 12))],
        ),
        // A call whose argument has a label is `Error`: its tail adds no mismatch.
        (
            "fn f<T: Same>(x: T) -> string {\n    x.same(v: x)\n}",
            vec![(Code::LabeledArgument, (2, 12))],
        ),
    ] {
        codes(&with_bounds(row), &expected);
    }
}

/// An own type parameter of the requirement that the instantiation cycle passes through: the
/// argument grows each time `go` calls `wrap` and `wrap` calls `go`.
#[test]
fn a_cycle_through_a_requirements_own_parameter_should_be_reported() {
    let row = "interface Wrap {\n    fn wrap<U>(self, x: U)\n}\nclass Ring {\n    r: f64\n}\nextend Ring: Wrap {\n    fn wrap<U>(self, x: U) {\n        go(self, [x])\n    }\n}\nfn go<T: Wrap, U>(t: T, x: U) {\n    t.wrap(x)\n}\nfn main() {}";
    codes(row, &[(Code::InstantiationCycle, (9, 9))]);
    // The call through the requirement grows its argument: the cycle shows the class.
    let grows = "interface Wrap {\n    fn wrap<U>(self, x: U)\n}\nclass Ring {\n    r: f64\n}\nclass Box<T> {\n    v: T\n}\nextend Ring: Wrap {\n    fn wrap<U>(self, x: U) {\n        go(self, x)\n    }\n}\nfn go<T: Wrap, U>(t: T, x: U) {\n    t.wrap(Box(v: x))\n}\nfn main() {}";
    codes(grows, &[(Code::InstantiationCycle, (16, 5))]);
    // The second own parameter closes the cycle; an earlier requirement has its own hubs.
    let second = "interface Wrap {\n    fn first<W>(self, w: W)\n    fn wrap<U, V>(self, x: U, y: V)\n}\nclass Ring {\n    r: f64\n}\nextend Ring: Wrap {\n    fn first<W>(self, w: W) {}\n    fn wrap<U, V>(self, x: U, y: V) {\n        go(self, x, y)\n    }\n}\nfn go<T: Wrap, U, V>(t: T, x: U, y: V) {\n    t.wrap(x, [y])\n}\nfn main() {}";
    codes(second, &[(Code::InstantiationCycle, (15, 5))]);
    // The parameters of one requirement are not one hub: `x` grows and flows nowhere back.
    let apart = "interface Wrap {\n    fn wrap<U, V>(self, x: U, y: V)\n}\nclass Ring {\n    r: f64\n}\nextend Ring: Wrap {\n    fn wrap<U, V>(self, x: U, y: V) {\n        go(self, y, 1)\n    }\n}\nfn go<T: Wrap, U, V>(t: T, x: U, y: V) {\n    t.wrap([x], y)\n}\nfn main() {}";
    clean(apart);
    // The argument shrinks back: not a cycle.
    let steady = "interface Wrap {\n    fn wrap<U>(self, x: U)\n}\nclass Ring {\n    r: f64\n}\nextend Ring: Wrap {\n    fn wrap<U>(self, x: U) {\n        go(self, x)\n    }\n}\nfn go<T: Wrap, U>(t: T, x: U) {\n    t.wrap(x)\n}\nfn main() {}";
    clean(steady);
}

/// A call through two bounds that both require the method is ambiguous, unless the call's
/// context picks one; the bounds are all there is to tell the candidates apart.
#[test]
fn a_method_that_two_bounds_require_should_be_ambiguous_without_a_context() {
    let both = "interface A {\n    fn name(self) -> string\n}\ninterface B {\n    fn name(self) -> string\n}\n";
    let other = "interface A {\n    fn name(self) -> string\n}\ninterface B {\n    fn name(self) -> i64\n}\n";
    for (src, expected) in [
        // One return type: nothing tells them apart.
        (
            format!("{both}fn f<T: A & B>(x: T) -> string {{\n    x.name()\n}}\nfn main() {{}}"),
            vec![(Code::Ambiguous, (8, 7))],
        ),
        // Two, and a `let` with no type.
        (
            format!("{other}fn f<T: A & B>(x: T) {{\n    let n = x.name()\n}}\nfn main() {{}}"),
            vec![(Code::Ambiguous, (8, 15))],
        ),
        // The context picks one.
        (
            format!(
                "{other}fn f<T: A & B>(x: T) -> i64 {{\n    let n: i64 = x.name()\n    n\n}}\nfn main() {{}}"
            ),
            vec![],
        ),
        // The other argument of a generic call picks one; it is not the call's context.
        (
            format!(
                "{other}fn take<T>(a: T, b: T) {{}}\nfn f<T: A & B>(x: T, s: string) {{\n    take(x.name(), s)\n}}\nfn main() {{}}"
            ),
            vec![],
        ),
        // Both return one type, so the call does not wait for the generic call around it, which
        // could not infer its parameter.
        (
            format!("{both}fn id<T>(a: T) -> T {{\n    a\n}}\nfn f<T: A & B>(x: T) {{\n    let n = id(x.name())\n}}\nfn main() {{}}"),
            vec![(Code::Ambiguous, (11, 18))],
        ),
        // A context that failed is the report, and the call adds none.
        (
            format!("{other}fn f<T: A & B>(x: T) {{\n    let n: Missing = x.name()\n}}\nfn main() {{}}"),
            vec![(Code::UnknownType, (8, 12))],
        ),
        // A result that failed, and a type argument that does not resolve, are reported once.
        (
            "interface A {\n    fn name(self) -> string\n}\ninterface B {\n    fn name(self) -> Missing\n}\nfn f<T: A & B>(x: T) {\n    let n = x.name()\n}\nfn main() {}".to_string(),
            vec![(Code::UnknownType, (5, 22))],
        ),
        // The same in a call that is the argument of another: it is checked at once.
        (
            "interface A {\n    fn name(self) -> string\n}\ninterface B {\n    fn name(self) -> Missing\n}\nfn id<T>(a: T) -> T {\n    a\n}\nfn f<T: A & B>(x: T) {\n    let n = id(x.name())\n}\nfn main() {}".to_string(),
            vec![(Code::UnknownType, (5, 22))],
        ),
        (
            "interface A {\n    fn make<U>(self, x: U) -> U\n}\ninterface B {\n    fn make<V>(self, x: V) -> [V]\n}\nfn f<T: A & B>(x: T) {\n    let n = x.make<Missing>(1)\n}\nfn main() {}".to_string(),
            vec![(Code::UnknownType, (8, 20))],
        ),
        // Both return one type: the call does not wait for an outer call, and is reported.
        (
            format!("{both}fn take<T>(a: T, b: T) {{}}\nfn f<T: A & B>(x: T) {{\n    take(x.name(), \"s\")\n}}\nfn main() {{}}"),
            vec![(Code::Ambiguous, (9, 12))],
        ),
        // A parameter that no bound gives the method: the call does not wait.
        (
            format!("{other}fn take<T>(a: T, b: T) {{}}\nfn f<T: A>(x: T) {{\n    take(x.nothing(), 1)\n}}\nfn main() {{}}"),
            vec![(Code::NoMember, (9, 12))],
        ),
        // The interface's arguments name each candidate.
        (
            "interface Convert<T> {\n    fn convert(self) -> T\n}\nfn f<T: Convert<i64> & Convert<f64>>(x: T) {\n    let n = x.convert()\n}\nfn main() {}".to_string(),
            vec![(Code::Ambiguous, (5, 15))],
        ),
        // The candidates are only told apart by an argument, which a call must not wait for.
        (
            format!(
                "{other}fn take<T>(a: T, b: T) {{}}\nfn f<T: A & B>(x: T) {{\n    take(x.name(), 1)\n}}\nfn main() {{}}"
            ),
            vec![],
        ),
    ] {
        codes(&src, &expected);
    }
}

/// A call whose type arguments are written waits too, when several candidates answer it: its
/// context picks, here the other argument of `take`.
#[test]
fn a_call_with_written_type_arguments_and_several_candidates_should_wait_for_context() {
    let src = "interface A {\n    fn make<U>(self, x: U) -> U\n}\ninterface B {\n    fn make<V>(self, x: V) -> [V]\n}\nfn take<T>(a: T, b: T) {}\nfn f<T: A & B>(x: T, s: [i64]) {\n    take(x.make<i64>(1), s)\n}\nfn main() {}";
    clean(src);
}

/// A body costs the closure of each bound once, and the index of its requirements by name once:
/// the calls in it read what the first found, whether they repeat a name or not.
#[test]
fn a_body_should_close_and_index_the_bounds_of_a_parameter_once() {
    let steps = |n: usize, distinct: bool| {
        let methods = (0..n).fold(String::new(), |text, i| {
            text + &format!("    fn m{i}(self) -> i64\n")
        });
        let calls = (0..n).fold(String::new(), |text, i| {
            text + &format!("    let _ = x.m{}()\n", if distinct { i } else { 0 })
        });
        let src = format!(
            "interface Wide {{\n{methods}}}\nfn f<T: Wide>(x: T) {{\n{calls}}}\nfn main() {{}}"
        );
        CLOSURE_STEPS.set(0);
        clean(&src);
        CLOSURE_STEPS.get()
    };
    // The bound is read where it is declared and in the body, then a step for each requirement.
    for distinct in [true, false] {
        assert_eq!(steps(8, distinct), 2 + 8);
        assert_eq!(steps(32, distinct), 2 + 32);
    }
    // A bound that each call of another generic function checks is read from the closure.
    let checks = |n: usize| {
        let calls = "    g(x)\n".repeat(n);
        let src = format!(
            "interface Same {{\n    fn same(self) -> bool\n}}\nfn g<T: Same>(x: T) {{}}\nfn f<T: Same>(x: T) {{\n{calls}}}\nfn main() {{}}"
        );
        CLOSURE_STEPS.set(0);
        clean(&src);
        CLOSURE_STEPS.get()
    };
    assert_eq!(checks(32), checks(1));
}

/// A chain of `n` interfaces, each inheriting the one before it, and a function whose bound is the
/// last: the bound gives `n` interfaces. The function comes first, so that its bound is on line 1.
fn chain(n: usize, call: &str) -> String {
    let mut src = format!("fn _use<T: C{n}>(x: T) {{\n{call}}}\n");
    src += "fn need<T: C1>(_y: T) {}\n";
    src + &ladder("C", n) + "fn main() {}"
}

/// The interfaces `<prefix>1` through `<prefix>n`, each inheriting the one before it; the first
/// declares `deep`.
fn ladder(prefix: &str, n: usize) -> String {
    let mut src = format!("interface {prefix}1 {{\n    fn deep(self) -> i64\n}}\n");
    for k in 2..=n {
        writeln!(src, "interface {prefix}{k}: {prefix}{} {{}}", k - 1)
            .expect("writing to a `String` should succeed");
    }
    src
}

/// The most interfaces that a bound gives is 256, with those it inherits: 256 is fine, and one
/// more is E0157 at the bound, once. The parameter is then unknown in the body, so a method that
/// the walk did not reach draws no report, and the bound of a call that the parameter meets holds.
#[test]
fn a_bound_should_give_at_most_the_limit_of_interfaces() {
    // The call reads the closure of the bound, which is the whole chain.
    clean(&chain(256, "    need(x)\n"));
    codes(
        &chain(257, "    need(x)\n"),
        &[(Code::BoundTooWide, (1, 12))],
    );
    // `deep` is declared by the last interface of a chain that the walk does not finish: it is
    // not found, and that is no report. Nor is a call of a function with a bound that the
    // parameter does not give.
    let src = chain(300, "    print(x.deep())\n    need(x)\n");
    codes(&src, &[(Code::BoundTooWide, (1, 12))]);
    // The same bound read by two bodies is reported once.
    let mut src = String::from(
        "class K<T: C300> {\n    v: T\n    fn a(self) {\n        need(self.v)\n    }\n    fn b(self) {\n        need(self.v)\n    }\n}\nfn need<T: C1>(_y: T) {}\ninterface C1 {}\n",
    );
    for k in 2..=300 {
        writeln!(src, "interface C{k}: C{} {{}}", k - 1)
            .expect("writing to a `String` should succeed");
    }
    codes(&(src + "fn main() {}"), &[(Code::BoundTooWide, (1, 12))]);
    // The limit holds for each listed interface and not for all of them together: 257 bounds with
    // no inheritance are fine, and so are two chains of 128 and 129 interfaces that share none.
    let names: Vec<String> = (1..=257).map(|k| format!("C{k}")).collect();
    let mut src = format!(
        "fn _use<T: {}>(x: T) {{\n    need(x)\n}}\nfn need<T: C1>(_y: T) {{}}\n",
        names.join(" & ")
    );
    for name in &names {
        writeln!(src, "interface {name} {{}}").expect("writing to a `String` should succeed");
    }
    clean(&(src + "fn main() {}"));
    let (c, d) = (ladder("C", 128), ladder("D", 129));
    let call = "fn _use<T: C128 & D129>(x: T) {\n    need(x)\n    need2(x)\n}\nfn need<T: C1>(_y: T) {}\nfn need2<T: D1>(_y: T) {}\n";
    clean(&format!("{call}{c}{d}fn main() {{}}"));
    // The merged closure is over the limit, and no bound's own is.
    let (c, d) = (ladder("C", 200), ladder("D", 200));
    clean(&format!(
        "fn _use<T: C200 & D200>(x: T) {{}}\n{c}{d}fn main() {{}}"
    ));
    // The bound that passes the limit on its own is the one reported, wherever it stands.
    let wide = ladder("C", 257);
    codes(
        &format!("fn _use<T: D1 & C257>(x: T) {{}}\n{wide}interface D1 {{}}\nfn main() {{}}"),
        &[(Code::BoundTooWide, (1, 17))],
    );
    // The same for the list of a class: each listed interface on its own.
    clean(&format!(
        "class K: C200, D200 {{\n    fn deep(self) -> i64 {{ 1 }}\n}}\n{c}{d}fn main() {{}}"
    ));
}

/// A method that more than one interface of a closure that passed the limit gives is no error
/// either: the parameter is unknown, so the call adds nothing to the report of the bound.
#[test]
fn a_method_of_a_bound_that_is_too_wide_should_add_no_report() {
    let mut src = String::from("fn _use<T: Last>(x: T) {\n    print(x.same())\n}\n");
    src += "interface Base<A> {\n    fn same(self) -> i64\n}\n";
    // A chain of 300 interfaces; `Last` and the chain give `same` twice, with two arguments.
    src += "interface I0 : Base<i64> {}\n";
    for k in 1..300 {
        writeln!(src, "interface I{k}: I{}, Base<[i64]> {{}}", k - 1)
            .expect("writing to a `String` should succeed");
    }
    src += "interface Last: I299, Base<i64> {}\nfn main() {}";
    let ds = diags(&src);
    let found: Vec<_> = ds.iter().map(|d| (d.code(), at(&src, d.span()))).collect();
    assert_eq!(found, [(Code::BoundTooWide, (1, 12))]);
}

/// The text of the program with `levels` interfaces that each inherit the one before with each
/// of five arguments replaced by `i64` or `f64`, one at a time, besides the unchanged one: the
/// closure of a bound at the last doubles at each level, until it passes the limit.
fn doubling(levels: usize) -> String {
    let params = ["A", "B", "C", "D", "E"];
    let mut src = String::from("interface L0<A, B, C, D, E> {}\n");
    for k in 1..=levels {
        let mut list = vec![format!("L{}<A, B, C, D, E>", k - 1)];
        for at in 0..5 {
            for ty in ["i64", "f64"] {
                let mut args = params.map(String::from);
                args[at] = ty.to_string();
                list.push(format!("L{}<{}>", k - 1, args.join(", ")));
            }
        }
        writeln!(
            src,
            "interface L{k}<A, B, C, D, E>: {} {{}}",
            list.join(", ")
        )
        .expect("writing to a `String` should succeed");
    }
    src + &format!(
        "fn _use<T: L{levels}<string, string, string, string, string>>(x: T) {{\n    need(x)\n}}\nfn need<T: L0<string, string, string, string, string>>(_y: T) {{}}\nfn main() {{}}"
    )
}

/// A closure that would grow without end stops at the limit: the steps that walk it stay a small
/// multiple of 256 however many levels the program has, and one report is made.
#[test]
fn a_closure_that_doubles_should_stop_at_the_limit() {
    let steps = |levels: usize| {
        CLOSURE_STEPS.set(0);
        let src = doubling(levels);
        let ds = diags(&src);
        let found: Vec<_> = ds.iter().map(Diagnostic::code).collect();
        assert_eq!(found, [Code::BoundTooWide], "{levels} levels");
        CLOSURE_STEPS.get()
    };
    // Each entry that the walk reads gives at most 11 inherited interfaces to read.
    assert!(steps(20) <= 11 * 257 + 11, "{}", steps(20));
    assert!(steps(40) <= 11 * 257 + 11, "{}", steps(40));
    // Four levels give less than the limit.
    clean(&doubling(2));
}

/// A list of interfaces on a class, and an `extend` block, report E0157 as a bound does: at the
/// interface, once, when it gives more than 256 interfaces with those it inherits.
#[test]
fn a_conformance_should_give_at_most_the_limit_of_interfaces() {
    let mut chain = String::from("interface D1 {}\n");
    for k in 2..=257 {
        writeln!(chain, "interface D{k}: D{} {{}}", k - 1)
            .expect("writing to a `String` should succeed");
    }
    chain += "fn main() {}";
    // In the list of a class.
    let src = format!("class K: D257 {{}}\n{chain}");
    codes(&src, &[(Code::BoundTooWide, (1, 10))]);
    // The conformances of the list are unknown: the interface that an earlier root inherits, and
    // that the class does not meet, draws no report.
    let src = format!(
        "interface Base {{\n    fn base(self) -> i64\n}}\ninterface Extra: Base {{}}\nclass K: Extra, D257 {{}}\n{chain}"
    );
    codes(&src, &[(Code::BoundTooWide, (5, 17))]);
    // The roots after the one that passed the limit are not walked.
    let src = format!("class K: D257, Extra {{}}\ninterface Extra {{}}\n{chain}");
    codes(&src, &[(Code::BoundTooWide, (1, 10))]);
    // In a block: the conformance is unknown after it, so what the walk reached before it passed
    // the limit is not asked for.
    let src = format!("class K {{}}\nextend K: D257 {{}}\n{chain}");
    codes(&src, &[(Code::BoundTooWide, (2, 11))]);
    // A use of the class asks for an interface of the walk's end: the class conforms.
    let src = format!(
        "class K: D257 {{}}\nfn go() {{\n    need(K())\n}}\nfn need<T: D1>(_y: T) {{}}\n{chain}"
    );
    codes(&src, &[(Code::BoundTooWide, (1, 10))]);
    let src = format!(
        "class K {{}}\nextend K: D257 {{}}\nfn go() {{\n    need(K())\n}}\nfn need<T: D1>(_y: T) {{}}\n{chain}"
    );
    codes(&src, &[(Code::BoundTooWide, (2, 11))]);
}

/// A call that writes type arguments waits only when several methods answer it, whether the
/// receiver is a class with conformance blocks, an optional of one, or a class with a method of its
/// own that wins; other receivers never wait.
#[test]
fn a_call_with_written_type_arguments_should_wait_only_for_several_methods() {
    let decls = "interface One {\n    fn make<U>(self, x: U) -> U\n}\ninterface Many {\n    fn make<V>(self, x: V) -> [V]\n}\nclass C {\n    x: i64\n}\nextend C: One {\n    fn make<U>(self, x: U) -> U { x }\n}\nextend C: Many {\n    fn make<V>(self, x: V) -> [V] { [x] }\n}\nclass D {\n    x: i64\n    fn make<U>(self, x: U) -> U { x }\n}\nfn take<T>(a: T, b: T) {}\nfn main() {}";
    clean(&format!(
        "fn f(c: C, o: C?, s: [i64], t: [i64]?) {{\n    take(c.make<i64>(1), s)\n    take(o?.make<i64>(1), t)\n}}\n{decls}"
    ));
    clean(&format!(
        "fn f(d: D) {{\n    take(d.make<i64>(1), 2)\n}}\n{decls}"
    ));
    // The method of the class itself wins over two blocks, and one block is not several: the
    // call is checked at once, where it would wait if it were several, for its result names a
    // parameter that no argument gives.
    let src = "interface Tag {\n    fn tag<U>(self) -> [U]\n}\ninterface Tag2 {\n    fn tag<V>(self) -> V\n}\nfn id<T>(a: T) -> T {\n    a\n}\nfn f(e: E, c: C1) {\n    let n = id(e.tag<i64>())\n    let m = id(c.tag<i64>())\n}\nfn g<T: Tag>(x: T) {\n    let k = id(x.tag<i64>())\n}\nclass E {\n    x: i64\n    fn tag<U>(self) -> [U] { [] }\n}\nextend E: Tag {\n    fn tag<U>(self) -> [U] { [] }\n}\nextend E: Tag2 {\n    fn tag<V>(self) -> V { panic(\"x\") }\n}\nclass C1 {\n    x: i64\n}\nextend C1: Tag {\n    fn tag<U>(self) -> [U] { [] }\n}\nfn main() {}";
    clean(src);
    // A built-in receiver draws its own report and no wait.
    codes(
        &format!("fn f() {{\n    take(5.max<i64>(2), 3)\n}}\n{decls}"),
        &[(Code::TypeArguments, (2, 15))],
    );
}

/// A class and an enum with a type parameter and no bound, whose methods have a bound of their
/// own, and a `main` that calls both.
const TRAY: &str = "interface Shape {\n    fn area(self) -> f64\n}\nclass Circle {\n    r: f64\n}\nextend Circle: Shape {\n    fn area(self) -> f64 { self.r }\n}\nclass Tray<T> {\n    item: T\n    fn fit<U: Shape>(self, x: U) -> f64 {\n        x.area()\n    }\n}\nenum Case<T> {\n    Holds(T)\n    fn fit<U: Shape>(self, x: U) -> f64 {\n        x.area()\n    }\n}\nfn main() {\n    let c = Circle(r: 1.0)\n    let t = Tray(item: 1)\n    print(t.fit(c))\n    print(t.item)\n    print(Case.Holds(1).fit(c))\n}";

/// The bounds of a method's own parameters stand at the places of those parameters, after the
/// parameters of its class or enum, which have none here: the bound is `U`'s, not `T`'s.
#[test]
fn a_methods_own_bound_should_stay_at_its_own_parameter() {
    // The item has the type `T`, which has no bound: it has no `area`.
    codes(
        &TRAY.replacen("x.area()", "self.item.area()", 1),
        &[(Code::NoMember, (13, 19))],
    );
    // The enum has the same rule: the payload has the type `T`.
    codes(
        &with_bounds(
            "enum Case<T> {\n    Holds(T)\n    fn peek<U: Shape>(self, x: U) -> f64 {\n        match self {\n            Case.Holds(v) => v.area()\n        }\n    }\n}",
        ),
        &[(Code::NoMember, (5, 32))],
    );
    // The call through `U`'s bound is clean, and the bound uses the interface: no warning.
    judge_unused();
    clean(TRAY);
}

/// The bounds of one declaration all stand when the check of an applied type that one of them
/// names runs: `C<T>` meets its bound through the `T: I` that is written beside it.
#[test]
fn an_applied_type_in_a_bound_should_see_the_other_bounds_of_its_declaration() {
    let decls = "interface I {}\ninterface J<X> {}\nclass C<T: I> {\n    v: T\n}\nfn main() {}";
    for row in [
        "fn f<T: I, U: J<C<T>>>(x: U) {}",
        "class K<T: I, U: J<C<T>>> {\n    a: T\n    b: U\n}",
        "enum En<T: I, U: J<C<T>>> {\n    A(T, U)\n}",
        "interface W<T: I, U: J<C<T>>> {}",
        "interface R {\n    fn m<T: I, U: J<C<T>>>(self, x: U)\n}",
        "class P {\n    fn m<T: I, U: J<C<T>>>(self, x: U) {}\n}",
    ] {
        clean(&format!("{row}\n{decls}"));
    }
    // The parameters of an `extend` block that its target does not name are reported, and the
    // bounds among them meet each other all the same.
    codes(
        &format!("class Circle {{}}\nextend<T: I, U: J<C<T>>> Circle {{}}\n{decls}"),
        &[(Code::CannotExtend, (2, 8)), (Code::CannotExtend, (2, 14))],
    );
    // A bound that is missing is still reported, once, at the argument.
    codes(
        &format!("fn f<T, U: J<C<T>>>(x: U) {{}}\n{decls}"),
        &[(Code::NotConform, (1, 16))],
    );
}

/// Every bound is read where it is declared, whether a body asks for its interfaces or not.
#[test]
fn a_bound_that_passes_the_limit_should_be_reported_where_it_is_declared() {
    let wide = ladder("C", 257) + "fn main() {}";
    for (row, at) in [
        ("fn _f<T: C257>(_x: T) {}", (1, 10)),
        ("class H<T: C257> {\n    v: T\n}", (1, 12)),
        ("enum E<T: C257> {\n    V(T)\n}", (1, 11)),
        ("interface R {\n    fn m<T: C257>(self, x: T)\n}", (2, 13)),
        (
            "class K {}\nextend K {\n    fn m<T: C257>(self, x: T) {}\n}",
            (3, 13),
        ),
        ("interface J<X: C257> {}", (1, 16)),
    ] {
        codes(&format!("{row}\n{wide}"), &[(Code::BoundTooWide, at)]);
    }
    // A body that reads the same bound adds no second report.
    codes(
        &format!("fn _f<T: C257>(x: T) {{\n    need(x)\n}}\nfn need<T: C1>(_y: T) {{}}\n{wide}"),
        &[(Code::BoundTooWide, (1, 10))],
    );
}

/// The parameter whose bound passes the limit is unknown: no requirement is looked up for it, so a
/// call that does not fit the one requirement the walk reached draws no report.
#[test]
fn a_bound_that_passes_the_limit_should_give_no_requirement() {
    for call in [
        // The count of the arguments.
        "    print(x.deep(1))\n",
        // The type of the result.
        "    let s: string = x.deep()\n    print(s)\n",
    ] {
        codes(&chain(257, call), &[(Code::BoundTooWide, (1, 12))]);
    }
    // A bound before the one that passes the limit gives `deep` once, and it is not used either.
    let wide = ladder("C", 257);
    codes(
        &format!(
            "fn _use<T: D1 & C257>(x: T) {{\n    print(x.deep(1))\n}}\ninterface D1 {{\n    fn deep(self) -> i64\n}}\n{wide}fn main() {{}}"
        ),
        &[(Code::BoundTooWide, (1, 17))],
    );
}

/// A check of an applied type that fails leaves no type at its place: a value of it is unknown,
/// so it draws no second report where it meets another bound.
#[test]
fn a_type_that_fails_its_bound_should_be_unknown_at_its_place() {
    let decls = "interface I {}\nclass Node {}\nclass C<T: I> {\n    v: T\n}\nfn g<T: I>(_y: T) {}\nfn main() {}";
    for (row, at) in [
        // A parameter, a field, a payload, a result, and an element of an array.
        ("fn f(x: C<Node>) {\n    g(x)\n}", (1, 11)),
        (
            "class H {\n    c: C<Node>\n}\nfn f(h: H) {\n    g(h.c)\n}",
            (2, 10),
        ),
        (
            "enum E {\n    A(C<Node>)\n}\nfn f(e: E) {\n    match e {\n        E.A(c) => g(c)\n    }\n}",
            (2, 9),
        ),
        (
            "fn mk() -> C<Node> {\n    panic(\"x\")\n}\nfn f() {\n    g(mk())\n}",
            (1, 14),
        ),
        (
            "fn f(xs: [C<Node>]) {\n    for x in xs {\n        g(x)\n    }\n}",
            (1, 13),
        ),
        // The parameter and the result of a requirement: a method that has another type there
        // is no mismatch.
        (
            "interface R {\n    fn m(self, c: C<Node>)\n}\nclass X {}\nextend X: R {\n    fn m(self, c: i64) {}\n}",
            (2, 21),
        ),
        (
            "interface R {\n    fn m(self) -> C<Node>\n}\nclass X {}\nextend X: R {\n    fn m(self) -> i64 { 1 }\n}",
            (2, 21),
        ),
    ] {
        codes(&format!("{row}\n{decls}"), &[(Code::NotConform, at)]);
    }
}

/// Two bounds that require `same(self) -> Self` return one type for a receiver `T`, which is not
/// an open parameter of the call: the call is ambiguous at once, as for a result like `string`.
#[test]
fn equal_results_that_name_self_should_make_the_call_ambiguous_at_once() {
    let src = "interface A {\n    fn same(self) -> Self\n}\ninterface B {\n    fn same(self) -> Self\n}\nfn id<T>(a: T) -> T {\n    a\n}\nfn f<T: A & B>(x: T) {\n    let n = id(x.same())\n}\nfn main() {}";
    codes(src, &[(Code::Ambiguous, (11, 18))]);
}

/// The checks that wait for every conformance run with their scope set up once: `n` bounded
/// parameters with an applied type each, in a signature and in the fields of a class, cost the
/// parameters and the types, and not their product.
#[test]
fn bound_checks_that_wait_should_set_up_their_scope_once() {
    let steps = |n: usize, class: bool| {
        let params: Vec<String> = (0..n).map(|i| format!("T{i}: I")).collect();
        let types: Vec<String> = (0..n).map(|i| format!("x{i}: C<T{i}>")).collect();
        let fields: Vec<String> = (0..n).map(|i| format!("    f{i}: C<T{i}>\n")).collect();
        let user = if class {
            format!("class K<{}> {{\n{}}}", params.join(", "), fields.concat())
        } else {
            format!("fn f<{}>({}) {{}}", params.join(", "), types.join(", "))
        };
        let src =
            format!("{user}\ninterface I {{}}\nclass C<T: I> {{\n    v: T\n}}\nfn main() {{}}");
        PARAM_STEPS.set(0);
        clean(&src);
        PARAM_STEPS.get()
    };
    // A constant number of steps for each parameter, and 6 besides; a replay of the scope for
    // each check would add a step for each pair.
    for class in [false, true] {
        assert_eq!(steps(16, class), 7 * 16 + 6);
        assert_eq!(steps(32, class), 7 * 32 + 6);
    }
}

/// The payloads of an enum's variants are resolved with the enum's scope set up once: `n`
/// bounded parameters and `n` variants, each with an applied type in its payload, cost the
/// parameters and the variants, and not their product.
#[test]
fn the_variants_of_an_enum_should_share_the_scope_of_its_parameters() {
    let steps = |n: usize| {
        let params: Vec<String> = (0..n).map(|i| format!("T{i}: I")).collect();
        let variants: Vec<String> = (0..n).map(|i| format!("    V{i}(C<T{i}>)\n")).collect();
        let src = format!(
            "enum E<{}> {{\n{}}}\ninterface I {{}}\nclass C<T: I> {{\n    v: T\n}}\nfn main() {{}}",
            params.join(", "),
            variants.concat()
        );
        PARAM_STEPS.set(0);
        clean(&src);
        PARAM_STEPS.get()
    };
    let (small, large) = (steps(16), steps(32));
    assert!(small >= 16, "{small} steps for 16");
    assert!(large <= 2 * small, "{large} steps for 32, {small} for 16");
}

/// The classes the generic extension rows share, and an empty `main` for a row that has none.
const GENERIC_DECLS: &str = "class Stack<T>: Same {\n    items: [T]\n    fn same(self, other: Self) -> bool { self.items.len() == other.items.len() }\n}\nclass Pair<T> {\n    first: T\n    second: T\n}\nclass Circle: Same {\n    r: f64\n    fn same(self, other: Circle) -> bool { self.r == other.r }\n}\nclass Node {\n    id: i64\n}\n";

/// `row`, after the classes of `GENERIC_DECLS`, then the interfaces. A row's positions are its own
/// lines, counted from the line after the classes: `GENERIC_LINES` lines.
fn with_generic(row: &str) -> String {
    with_shapes(&with_main(&format!("{GENERIC_DECLS}{row}")))
}

/// The lines of `GENERIC_DECLS`.
const GENERIC_LINES: usize = 15;

/// A block that extends every instantiation of a class, one instantiation, and an instantiation
/// that holds a type parameter; a conformance that holds when the arguments conform; the blocks
/// meet at a call on any instantiation they cover.
#[test]
fn generic_extension_should_check_clean() {
    for row in [
        // `Stack<T>` conforms in its body; a block for every `T` is called on two instantiations.
        "extend<T> Stack<T> {\n    fn top(self) -> T? {\n        if self.items.len() == 0 { None } else { self.items[0] }\n    }\n}\nfn main() {\n    print(Stack(items: [1]).top() ?? 0)\n    print(Stack(items: [\"a\"]).top() ?? \"\")\n}",
        // A block for one instantiation.
        "extend Stack<i64> {\n    fn total(self) -> i64 { self.items.sum() }\n}\nfn main() {\n    print(Stack(items: [1, 2]).total())\n}",
        // A conformance that holds when `T` conforms, used through a bound and two levels deep.
        "extend<T: Same> Pair<T>: Same {\n    fn same(self, other: Self) -> bool {\n        self.first.same(other.first) && self.second.same(other.second)\n    }\n}\nfn agree<T: Same>(a: T, b: T) -> bool { a.same(b) }\nfn main() {\n    let c = Circle(r: 1.0)\n    let p = Pair(first: c, second: c)\n    print(agree(p, p))\n    print(agree(Pair(first: p, second: p), Pair(first: p, second: p)))\n    print(p.same(p))\n}",
        // A block whose target holds the parameter inside an array.
        "extend<T> Stack<[T]> {\n    fn flat(self) -> [T] { self.items[0] }\n}\nfn main() {\n    print(Stack(items: [[1], [2]]).flat().len())\n}",
        // Two instantiations, each with a method of one name.
        "extend Stack<i64> {\n    fn sum2(self) -> i64 { self.items.sum() * 2 }\n}\nextend Stack<f64> {\n    fn sum2(self) -> f64 { self.items.sum() * 2.0 }\n}\nfn main() {\n    print(Stack(items: [1]).sum2())\n    print(Stack(items: [1.5]).sum2())\n}",
        // The argument of a call waits for the result of a block's method that has a type parameter
        // of its own, though `Stack` has one too.
        "extend Stack<i64> {\n    fn make<U>(self) -> [U] { [] }\n}\nfn take<T>(x: T, y: T) -> T { [x, y][0] }\nfn main() {\n    print(take(Stack(items: [1]).make(), [1]).len())\n}",
        // The parameter of a conformance may be a method's own too: a block's methods see it.
        "extend<T: Same> Stack<T> {\n    fn twice(self, other: Self) -> bool {\n        self.items[0].same(other.items[0])\n    }\n}\nfn main() {\n    let s = Stack(items: [Circle(r: 1.0)])\n    print(s.twice(s))\n}",
        // Two conformance blocks of other instantiations hold a `name` too: the call takes the
        // block that covers the receiver.
        "extend Stack<i64>: Label {\n    fn name(self) -> string { \"ints\" }\n}\nextend Stack<f64>: Code {\n    fn name(self) -> i64 { 1 }\n}\nfn main() {\n    print(Stack(items: [1]).name())\n    print(Stack(items: [1.5]).name())\n}",
        // Two blocks over every `T` hold a `dup`, and the expected type picks by the result with
        // the receiver's arguments in it.
        "interface DupA {\n    fn dup(self) -> Self\n}\ninterface DupB {\n    fn dup(self) -> [Self]\n}\nextend<T> Stack<T>: DupA {\n    fn dup(self) -> Self { self }\n}\nextend<T> Stack<T>: DupB {\n    fn dup(self) -> [Self] { [self] }\n}\nfn main() {\n    let d: Stack<i64> = Stack(items: [1]).dup()\n    print(d.items.len())\n}",
        // Two blocks with parameters, each with a method of one name, that no type fits together.
        "class Two<A, B> {\n    a: A\n    b: B\n}\nextend<T> Two<T, [T]> {\n    fn m(self) -> i64 { 1 }\n}\nextend<U> Two<[U], U> {\n    fn m(self) -> i64 { 2 }\n}",
        // The depth of a parameter in a target counts arrays and optionals, and a leaf with none.
        "interface Run {\n    fn run(self)\n}\nclass Box<T> {\n    value: T\n}\nclass Two<A, B> {\n    a: A\n    b: B\n}\nfn dispatch<T: Run>(x: T) {\n    x.run()\n}\nextend<T> Box<[T]>: Run {\n    fn run(self) {\n        dispatch(Box(value: self.value))\n    }\n}\nextend<T> Box<T?>: Run {\n    fn run(self) {\n        dispatch(Box(value: self.value))\n    }\n}\nextend<T> Two<[i64], T>: Run {\n    fn run(self) {}\n}",
        // Different parameters in different blocks do not overlap: the occurs check.
        "class Two<A, B> {\n    a: A\n    b: B\n}\nextend<T> Two<T, [T]>: Same {\n    fn same(self, other: Self) -> bool { true }\n}\nextend<U> Two<[U], U>: Same {\n    fn same(self, other: Self) -> bool { false }\n}",
        // A conformance whose method calls through its own bound, with the zero cycle of
        // `total([self])`: the receiver nests the parameter once, and the conformance takes it out.
        "interface Area {\n    fn area(self) -> f64\n}\nclass Unit {\n    r: f64\n}\nextend Unit: Area {\n    fn area(self) -> f64 { self.r }\n}\nextend<T: Area> Pair<T>: Area {\n    fn area(self) -> f64 { total([self]) }\n}\nfn total<T: Area>(xs: [T]) -> f64 {\n    let mut sum = 0.0\n    for x in xs {\n        sum += x.area()\n    }\n    sum\n}\nfn main() {\n    print(total([Pair(first: Unit(r: 1.0), second: Unit(r: 2.0))]))\n}",
    ] {
        clean(&with_generic(row));
    }
}

/// What generic extension reports: methods of one name that cover an instantiation twice, two
/// conformances that overlap, a method or a conformance that a type does not meet, and a cycle
/// through a conformance. A row's positions are its own lines (`l`) after `GENERIC_DECLS`.
#[test]
fn generic_extension_should_report_what_it_cannot_take() {
    let l = |n: usize| GENERIC_LINES + n;
    for (row, expected) in [
        // Two blocks that cover `Stack<i64>`, labels on both.
        (
            "extend<T> Stack<T> {\n    fn top(self) -> i64 { 1 }\n}\nextend Stack<i64> {\n    fn top(self) -> i64 { 2 }\n}",
            vec![(Code::DefinedTwice, (l(5), 8), vec![(l(2), 8)])],
        ),
        // Two conformances to one interface that overlap: at the second, the first labeled.
        (
            "extend<T> Stack<T>: Named {\n    fn name(self) -> string { \"a\" }\n}\nextend Stack<i64>: Named {\n    fn name(self) -> string { \"b\" }\n}",
            vec![(Code::ConformsTwice, (l(4), 20), vec![(l(1), 21)])],
        ),
        // `Pair<T, i64>` and `Pair<i64, U>` meet at `Pair<i64, i64>`.
        (
            "class P2<A, B> {\n    a: A\n    b: B\n}\nextend<T> P2<T, i64>: Named {\n    fn name(self) -> string { \"a\" }\n}\nextend<U> P2<i64, U>: Named {\n    fn name(self) -> string { \"b\" }\n}",
            vec![(Code::ConformsTwice, (l(8), 23), vec![(l(5), 23)])],
        ),
        // The call is checked at the argument: `Pair<Node>` needs `Node: Same`.
        (
            "extend<T: Same> Pair<T>: Same {\n    fn same(self, other: Self) -> bool { true }\n}\nfn agree<T: Same>(a: T, b: T) -> bool { a.same(b) }\nfn main() {\n    let p = Pair(first: Node(id: 1), second: Node(id: 2))\n    print(agree(p, p))\n}",
            vec![(Code::NotConform, (l(7), 17), vec![(l(4), 13), (l(1), 11)])],
        ),
        // A call of a method of the conformance reports at the call.
        (
            "extend<T: Same> Pair<T>: Same {\n    fn same(self, other: Self) -> bool { true }\n}\nfn main() {\n    let p = Pair(first: Node(id: 1), second: Node(id: 2))\n    print(p.same(p))\n}",
            vec![(Code::NotConform, (l(6), 11), vec![(l(1), 11)])],
        ),
        // A block for another instantiation has no method here.
        (
            "extend Stack<i64> {\n    fn total(self) -> i64 { 1 }\n}\nfn main() {\n    print(Stack(items: [1.5]).total())\n}",
            vec![(Code::NoMember, (l(5), 31), vec![])],
        ),
        // The block of one instantiation comes first, and the block for every `T` second.
        (
            "extend Stack<i64> {\n    fn top(self) -> i64 { 1 }\n}\nextend<T> Stack<T> {\n    fn top(self) -> i64 { 2 }\n}",
            vec![(Code::DefinedTwice, (l(5), 8), vec![(l(2), 8)])],
        ),
        // The class holds the method, and the block for every `T` has one too.
        (
            "class Pile<V> {\n    items: [V]\n    fn top(self) -> i64 { 1 }\n}\nextend<T> Pile<T> {\n    fn top(self) -> i64 { 2 }\n}",
            vec![(Code::DefinedTwice, (l(6), 8), vec![(l(3), 8)])],
        ),
        // A block without a conformance has its bounds checked at the call, as a conformance
        // block does.
        (
            "extend<T: Same> Stack<T> {\n    fn twice(self) -> bool { true }\n}\nfn main() {\n    print(Stack(items: [Node(id: 1)]).twice())\n}",
            vec![(Code::NotConform, (l(5), 11), vec![(l(1), 11)])],
        ),
        // `T?` in the target matches an optional, and not a plain `i64`.
        (
            "class Cell<T> {\n    value: T\n}\nextend<T> Cell<T?> {\n    fn get(self) -> T { panic(\"x\") }\n}\nfn main() {\n    print(Cell(value: 1).get())\n}",
            vec![(Code::NoMember, (l(8), 26), vec![])],
        ),
    ] {
        let src = with_generic(row);
        assert_eq!(found(&src), expected, "{row}");
    }
}

/// The interfaces the inheritance rows use, and the class they extend, after the row's line 1.
const ORD2: &str = "interface Ord2: Same {\n    fn less(self, other: Self) -> bool\n}\nclass Heap<T> {\n    items: [T]\n}\n";

/// A conformance that inherits another needs the other too, under the conditions of its own
/// block: the type has none, or has one whose bounds the block does not give.
#[test]
fn an_inherited_conformance_should_hold_under_the_conditions_of_the_block() {
    let row = |block: &str| with_shapes(&with_main(&format!("{ORD2}{block}")));
    // No conformance of `Heap` to `Same`: the help writes a block with the same conditions.
    let src =
        row("extend<T: Same> Heap<T>: Ord2 {\n    fn less(self, other: Self) -> bool { true }\n}");
    assert_eq!(
        found(&src),
        [(Code::NotConform, (7, 26), vec![(1, 17)])],
        "{src}"
    );
    // A conformance whose condition the block does not give: no help, and a label on the bound.
    let other = "extend<T: Ord2> Heap<T>: Same {\n    fn same(self, other: Self) -> bool { true }\n}\nextend<T: Same> Heap<T>: Ord2 {\n    fn less(self, other: Self) -> bool { true }\n}";
    let src = row(other);
    assert_eq!(
        found(&src),
        [(Code::NotConform, (10, 26), vec![(1, 17), (7, 11)])],
        "{src}"
    );
    // A block with a parameter that has no bound: the help writes it as it is.
    let src = row("extend<T> Heap<T>: Ord2 {\n    fn less(self, other: Self) -> bool { true }\n}");
    assert_eq!(
        found(&src),
        [(Code::NotConform, (7, 20), vec![(1, 17)])],
        "{src}"
    );
    // The conformance holds when the block gives its condition, or more.
    clean(&row(
        "extend<T: Same> Heap<T>: Same {\n    fn same(self, other: Self) -> bool { true }\n}\nextend<T: Same> Heap<T>: Ord2 {\n    fn less(self, other: Self) -> bool { true }\n}",
    ));
    clean(&row(
        "extend<T: Same> Heap<T>: Same {\n    fn same(self, other: Self) -> bool { true }\n}\nextend<T: Same & Named> Heap<T>: Ord2 {\n    fn less(self, other: Self) -> bool { true }\n}",
    ));
}

/// A class that lists an interface gets the interfaces it inherits from a block of its own, if
/// the block's bounds hold for the class's parameters: the list meets them as a conformance in a
/// block does.
#[test]
fn a_listed_conformance_should_hold_the_bounds_of_a_block_that_gives_an_inherited_one() {
    let row = |class: &str, block: &str| {
        with_shapes(&with_main(&format!(
            "{ORD2}{class}\n{block}\nclass Node {{\n    id: i64\n}}"
        )))
    };
    let body = "{\n    items: [T]\n    fn less(self, other: Self) -> bool { true }\n}";
    let block = |bound: &str| {
        format!(
            "extend<T: {bound}> Stack<T>: Same {{\n    fn same(self, other: Self) -> bool {{ true }}\n}}"
        )
    };
    // The block needs `T: Ord2` and the class gives none: reported at the listed interface, with
    // a label on the bound the block needs and none on the help.
    let src = row(&format!("class Stack<T>: Ord2 {body}"), &block("Ord2"));
    assert_eq!(
        found(&src),
        [(Code::NotConform, (7, 17), vec![(1, 17), (11, 11)])],
        "{src}"
    );
    // The same with a block that needs `T: Same`: the class lists no bound either.
    let src = row(&format!("class Stack<T>: Ord2 {body}"), &block("Same"));
    assert_eq!(
        found(&src),
        [(Code::NotConform, (7, 17), vec![(1, 17), (11, 11)])],
        "{src}"
    );
    // The class gives what the block needs: clean.
    clean(&row(
        &format!("class Stack<T: Ord2>: Ord2 {body}"),
        &block("Ord2"),
    ));
    // A listed interface that an earlier one inherits adds nothing, and a later one still finds
    // the block that gives its inherited interface: `Same` is listed after `Ord2` inherits it.
    let src = with_shapes(&with_main(&format!(
        "{ORD2}class Stack<T>: Ord2, Same, Solid {{\n    items: [T]\n    fn less(self, other: Self) -> bool {{ true }}\n    fn same(self, other: Self) -> bool {{ true }}\n    fn volume(self) -> f64 {{ 1.0 }}\n    fn name(self) -> string {{ \"s\" }}\n}}\nextend<T: Same> Stack<T>: Named {{\n    fn name(self) -> string {{ \"s\" }}\n}}"
    )));
    assert_eq!(
        found(&src),
        [(Code::NotConform, (7, 29), vec![(27, 18), (14, 11)])],
        "{src}"
    );
}

/// The classes and functions the cycle rows share, after the row's line 1.
const CYCLE_DECLS: &str = "interface First {\n    fn first(self)\n}\ninterface Run {\n    fn run(self)\n}\nclass Box<T> {\n    value: T\n}\nclass Done<T> {\n    value: T\n}\nextend<T> Done<T>: Run {\n    fn run(self) {}\n}\nfn dispatch<T: Run>(x: T) {\n    x.run()\n}\n";

/// The lines of `CYCLE_DECLS`.
const CYCLE_LINES: usize = 18;

/// A cycle through a conformance: the argument of a call grows, and the receiver of a call through
/// a bound is the conformance's type, which takes one level off; the hub counts both.
#[test]
fn a_cycle_through_a_conformance_should_be_reported_at_the_call_that_grows() {
    let l = |n: usize| CYCLE_LINES + n;
    let program = |row: &str| with_main(&format!("{CYCLE_DECLS}{row}"));
    for (row, expected) in [
        // `Box(value: self)` is a `Box<Box<T>>`: two levels in, one out.
        (
            "extend<T> Box<T>: Run {\n    fn run(self) {\n        dispatch(Box(value: self))\n    }\n}",
            vec![(
                Code::InstantiationCycle,
                (l(3), 9),
                vec![(CYCLE_LINES - 1, 5)],
            )],
        ),
        // The conservative case: `Done<T>.run` calls nothing, and the hub cannot tell.
        (
            "extend<T> Box<T>: Run {\n    fn run(self) {\n        dispatch(Done(value: [self.value]))\n    }\n}",
            vec![(
                Code::InstantiationCycle,
                (l(3), 9),
                vec![(CYCLE_LINES - 1, 5)],
            )],
        ),
        // One level in, one level out is no growth.
        (
            "extend<T> Box<T>: Run {\n    fn run(self) {\n        dispatch(self)\n    }\n}",
            vec![],
        ),
    ] {
        let src = program(row);
        assert_eq!(found(&src), expected, "{row}");
    }
}

/// A component that holds a cycle of zero weight beside one of positive weight reports the
/// positive one, at the call that grows: the label is on the cycle the search found, and the
/// zero cycle through `area` has no note.
#[test]
fn a_component_with_a_zero_cycle_should_report_the_cycle_that_grows() {
    let src = "interface Shape {\n    fn area(self) -> f64\n}\nclass Unit {\n    r: f64\n}\nextend Unit: Shape {\n    fn area(self) -> f64 { self.r }\n}\nclass Pair<T> {\n    first: T\n}\nclass Box<T> {\n    value: T\n}\nextend<T: Shape> Pair<T>: Shape {\n    fn area(self) -> f64 {\n        total([self])\n    }\n}\nextend<T: Shape> Box<T>: Shape {\n    fn area(self) -> f64 {\n        dispatch(Box(value: self))\n    }\n}\nfn total<T: Shape>(xs: [T]) -> f64 {\n    xs[0].area()\n}\nfn dispatch<T: Shape>(x: T) -> f64 {\n    x.area()\n}\nfn main() {\n    print(total([Unit(r: 1.0)]))\n}";
    // The label is the call that wraps `self`; the only note is the call through the hub in
    // `dispatch`, and the call of `total` in the zero cycle has none.
    assert_eq!(
        found(src),
        [(Code::InstantiationCycle, (23, 9), vec![(30, 5)])]
    );
}

/// An argument that nests the parameter at two depths grows by the greatest: `Two<T, [[T]]>`
/// nests `T` at 1 and 3, and the conformance of `Box<Box<T>>` takes 2 out.
#[test]
fn an_argument_should_grow_by_the_greatest_depth_of_its_parameter() {
    let decls = "interface Run {\n    fn run(self)\n}\nclass Box<T> {\n    value: T\n}\nclass Two<A, B> {\n    a: A\n    b: B\n}\nextend<A, B> Two<A, B>: Run {\n    fn run(self) {}\n}\nfn dispatch<T: Run>(x: T) {\n    x.run()\n}\n";
    let program = |body: &str| {
        with_main(&format!(
            "{decls}extend<T> Box<Box<T>>: Run {{\n    fn run(self) {{\n        let v = self.value.value\n        dispatch({body})\n    }}\n}}"
        ))
    };
    // Depth 3 less 2 is 1; the smallest depth, 1, would give -1.
    assert_eq!(
        found(&program("Two(a: v, b: [[v]])")),
        [(Code::InstantiationCycle, (20, 9), vec![(15, 5)])]
    );
    // The same parameter at depth 1 only: 1 less 2.
    clean(&program("Two(a: v, b: v)"));
}

/// A bound on the parameter of an `extend` block is read where the block declares it, as a
/// function's is: E0157 once, whatever the block holds.
#[test]
fn a_bound_of_an_extend_block_should_be_checked_for_the_limit() {
    let mut chain = String::from("interface D1 {}\n");
    for k in 2..=257 {
        writeln!(chain, "interface D{k}: D{} {{}}", k - 1)
            .expect("writing to a `String` should succeed");
    }
    let src = format!(
        "class Stack<T> {{\n    items: [T]\n}}\nextend<T: D257> Stack<T> {{\n    fn top(self) -> i64 {{ 1 }}\n}}\nfn main() {{}}\n{chain}"
    );
    codes(&src, &[(Code::BoundTooWide, (4, 11))]);
}

/// A bound that two conformances of a type share costs one answer for each type: with
/// `extend<T: A & B> Box<T>: A` and `: B`, asking `Box^n<Leaf>: A` steps a number linear in
/// `n`, where an uncached walk would double the steps at each level.
#[test]
fn repeated_conformance_queries_should_cost_linear_steps() {
    let steps = |n: usize| {
        let mut ty = String::from("Leaf");
        for _ in 0..n {
            ty = format!("Box<{ty}>");
        }
        let src = format!(
            "interface A {{\n    fn a(self) -> i64\n}}\ninterface B {{\n    fn b(self) -> i64\n}}\nclass Leaf: A, B {{\n    fn a(self) -> i64 {{ 1 }}\n    fn b(self) -> i64 {{ 2 }}\n}}\nclass Box<T> {{\n    value: T\n}}\nextend<T: A & B> Box<T>: A {{\n    fn a(self) -> i64 {{ 1 }}\n}}\nextend<T: A & B> Box<T>: B {{\n    fn b(self) -> i64 {{ 2 }}\n}}\nfn need<T: A>(x: T) -> i64 {{ x.a() }}\nfn make(l: Leaf) -> i64 {{\n    need(l)\n}}\nfn go(x: {ty}) -> i64 {{\n    need(x)\n}}\nfn main() {{}}"
        );
        CONFORMS_STEPS.set(0);
        clean(&src);
        CONFORMS_STEPS.get()
    };
    // Two answers for each level, `A` and `B`, and a constant number besides.
    let (small, large) = (steps(8), steps(16));
    assert_eq!(large - small, 2 * 8, "{small} then {large}");
    assert_eq!(steps(32) - large, 2 * 16);
}

/// The number of edges of the instantiation graph of `src`, which checks clean, and how many
/// edges Bellman–Ford relaxed.
fn graph_cost(src: &str) -> (usize, usize) {
    cycle::EDGES.set(0);
    cycle::BELLMAN_FORD_STEPS.set(0);
    clean(src);
    (cycle::EDGES.get(), cycle::BELLMAN_FORD_STEPS.get())
}

/// `n` generic classes that conform to `Run`, and `n` generic functions that each call `x.run()`
/// on a bound: the hub of `Run` gives one edge for each call and one for each conformance
/// parameter, besides what the calls of the functions give. An edge for each pair of a call and a
/// conformance would give `n * n`.
#[test]
fn interface_edges_should_stay_linear() {
    const N: usize = 200;
    let mut src = String::from(
        "interface Run {\n    fn run(self)\n}\nclass Leaf: Run {\n    fn run(self) {}\n}\n",
    );
    for i in 0..N {
        write!(
            src,
            "class C{i}<T> {{\n    v: T\n}}\nextend<T: Run> C{i}<T>: Run {{\n    fn run(self) {{ go{i}(self.v) }}\n}}\nfn go{i}<U: Run>(x: U) {{\n    x.run()\n}}\n"
        )
        .expect("writing to a `String` should succeed");
    }
    src += "fn main() {\n    go0(Leaf())\n}";
    let (edges, relaxed) = graph_cost(&src);
    assert!(edges <= 4 * N, "{edges} edges");
    // One component of all the functions and the hub, with no weight above 0: the first pass
    // changes nothing, so Bellman–Ford reads each edge once.
    assert_eq!(relaxed, edges, "{relaxed} steps");
    // A requirement with a type parameter of its own: the calls pass their own argument, and the
    // conformances take it, through a hub for the parameter that costs an edge for each.
    let mut src = String::from("interface Keep {\n    fn keep<U>(self, x: U) -> U\n}\n");
    for i in 0..N {
        write!(
            src,
            "class C{i}<T> {{\n    v: T\n}}\nextend<T> C{i}<T>: Keep {{\n    fn keep<U>(self, x: U) -> U {{ x }}\n}}\nfn call{i}<T, V: Keep>(v: V, t: T) -> T {{\n    v.keep(t)\n}}\n"
        )
        .expect("writing to a `String` should succeed");
    }
    src += "fn main() {\n    print(call0(C0(v: 1), 2))\n}";
    let (edges, _) = graph_cost(&src);
    assert!(edges <= 4 * N, "{edges} edges");
}

/// A method of an `extend` block prints the type the block extends, with the block's parameters
/// in it, and not its class's: `Stack<[T]>.flat`, with its own parameters after the method.
#[test]
fn a_method_of_an_extend_block_should_print_the_type_it_extends() {
    contains(
        &with_generic(
            "extend<T> Stack<[T]> {\n    fn flat(self) -> [T] { self.items[0] }\n}\nextend Stack<i64> {\n    fn make<U>(self) -> [U] { [] }\n}\nextend<T: Same> Pair<T>: Same {\n    fn same(self, other: Self) -> bool { true }\n}",
        ),
        &[
            "(fn Stack<[T]>.flat ((self#0 Stack<[T]>)) [T] ",
            "(fn Stack<i64>.make<U> ((self#0 Stack<i64>)) [U] ",
            "(fn Pair<T>.Same.same (",
        ],
    );
}

/// The worst input of the search for a cycle: one component that holds every function and the hub,
/// with a cycle of positive weight, so that Bellman–Ford makes a pass for each node and one more,
/// and each pass reads each edge of the component. `n` classes that conform to `Run` and `n`
/// functions that call `x.run()`: `2n + 3` nodes (the hub, a parameter of each, and two types) and
/// `3n + 2` edges. The cost is the nodes plus one, times the edges, which a pass that changes
/// nothing would cut to the edges (`interface_edges_should_stay_linear`).
#[test]
fn a_cycle_in_a_large_component_should_cost_the_nodes_times_the_edges() {
    const N: usize = 200;
    let mut src = String::from(
        "interface Run {\n    fn run(self)\n}\nclass Leaf: Run {\n    fn run(self) {}\n}\nclass Wrap<T> {\n    v: T\n}\nextend<T: Run> Wrap<T>: Run {\n    fn run(self) {}\n}\n",
    );
    for i in 0..N {
        // The first method gives the next call a type that nests the parameter twice, and the
        // conformance takes one level off.
        let call = if i == 0 {
            format!("go{i}(Wrap(v: Wrap(v: self.v)))")
        } else {
            format!("go{i}(self.v)")
        };
        write!(
            src,
            "class C{i}<T> {{\n    v: T\n}}\nextend<T: Run> C{i}<T>: Run {{\n    fn run(self) {{ {call} }}\n}}\nfn go{i}<U: Run>(x: U) {{\n    x.run()\n}}\n"
        )
        .expect("writing to a `String` should succeed");
    }
    src += "fn main() {\n    go0(Leaf())\n}";
    cycle::EDGES.set(0);
    cycle::BELLMAN_FORD_STEPS.set(0);
    let found: Vec<_> = checked(&src).1.iter().map(Diagnostic::code).collect();
    assert_eq!(found, [Code::InstantiationCycle]);
    assert_eq!(cycle::EDGES.get(), 3 * N + 3);
    // The contract of the search: at most one pass for each node and one more, of the edges inside
    // the component. The search ran, and the size is the component's, not the graph's.
    let (nodes, edges) = cycle::SEARCHED.get();
    let steps = cycle::BELLMAN_FORD_STEPS.get();
    assert!(nodes > N && edges > N, "{nodes} nodes, {edges} edges");
    assert!(steps > 0, "the search should run");
    assert!(
        steps <= (nodes + 1) * edges,
        "{steps} steps for {nodes} nodes and {edges} edges"
    );
}

/// `n` interfaces that each have one conformance and one function, so that the graph holds `n`
/// components of their own, each with a negative weight, searched on its own. Finding the nodes of
/// a component reads the nodes once for all the components: an interface more gives the same few
/// nodes more, not a pass over all of them.
#[test]
fn the_nodes_of_the_components_should_be_gathered_in_one_pass() {
    let gathered = |n: usize| {
        let mut src = String::new();
        for i in 0..n {
            write!(
                src,
                "interface R{i} {{\n    fn run(self)\n}}\nclass C{i}<T> {{\n    v: T\n}}\nextend<T: R{i}> C{i}<T>: R{i} {{\n    fn run(self) {{ go{i}(self.v) }}\n}}\nfn go{i}<U: R{i}>(x: U) {{\n    x.run()\n}}\n"
            )
            .expect("writing to a `String` should succeed");
        }
        src += "fn main() {}";
        cycle::GATHER_STEPS.set(0);
        cycle::BELLMAN_FORD_STEPS.set(0);
        clean(&src);
        // Each component is searched: it reads at least one edge.
        assert!(cycle::BELLMAN_FORD_STEPS.get() >= n);
        cycle::GATHER_STEPS.get()
    };
    let (small, large) = (gathered(100), gathered(200));
    assert!(small > 0, "the nodes should be gathered");
    assert!(
        large <= 2 * small,
        "{small} nodes read for 100 interfaces, {large} for 200"
    );
}

/// A cycle with two steps that grow is labeled at the first in source order, whichever of them is
/// first on the cycle: the call of `f` in `run` and the call of `g` in `f` close it together.
#[test]
fn a_cycle_should_be_labeled_at_its_first_growing_step_in_source_order() {
    let helpers =
        "fn f<T: Run>(x: T) {\n    g(Box(value: x))\n}\nfn g<T: Run>(y: T) {\n    y.run()\n}\n";
    let conformance =
        "extend<T> Box<T>: Run {\n    fn run(self) {\n        f(Box(value: self))\n    }\n}\n";
    let decls = "interface Run {\n    fn run(self)\n}\nclass Box<T> {\n    value: T\n}\n";
    // The helpers come first: the call of `g` is the first in the source.
    let src = with_main(&format!("{decls}{helpers}{conformance}"));
    assert_eq!(
        found(&src),
        [(Code::InstantiationCycle, (8, 5), vec![(11, 5), (15, 9)])],
        "{src}"
    );
    // The conformance comes first: the call of `f` is.
    let src = with_main(&format!("{decls}{conformance}{helpers}"));
    assert_eq!(
        found(&src),
        [(Code::InstantiationCycle, (9, 9), vec![(13, 5), (16, 5)])],
        "{src}"
    );
}

/// A bound that a conditional conformance asks for fails once, at the use: at the call of a
/// method of the block, or at the argument that the conformance's bound is asked of, whether the
/// type argument is a class or a type parameter that lacks the bound. The body of the function
/// adds no report of its own.
#[test]
fn conditional_bound_violations_should_be_reported_once_at_the_use() {
    let l = |n: usize| GENERIC_LINES + n;
    let block = "extend<T: Same> Pair<T>: Same {\n    fn same(self, other: Self) -> bool { true }\n}\nfn agree<T: Same>(a: T, b: T) -> bool { a.same(b) }\n";
    for (row, expected) in [
        // The call of the method, on a type parameter that has no bound.
        (
            "fn h<T>(p: Pair<T>) -> bool {\n    p.same(p)\n}",
            vec![(Code::NotConform, (l(6), 5), vec![(l(1), 11)])],
        ),
        // The call of a function, whose bound the conformance's bound decides: one report for
        // the two arguments.
        (
            "fn g<T>(p: Pair<T>) -> bool {\n    agree(p, p)\n}",
            vec![(Code::NotConform, (l(6), 11), vec![(l(4), 13), (l(1), 11)])],
        ),
        // The result of the failed call is unknown, and the uses of it add none.
        (
            "fn k<T>(p: Pair<T>) -> i64 {\n    let b = p.same(p)\n    if b { 1 } else { 0 }\n}",
            vec![(Code::NotConform, (l(6), 13), vec![(l(1), 11)])],
        ),
    ] {
        let src = with_generic(&format!("{block}{row}"));
        assert_eq!(found(&src), expected, "{row}");
    }
}

/// A missing inherited interface has help that writes the block to add, with the interface's
/// arguments; the text is pinned by `E0154-inherited-arguments`.
#[test]
fn an_inherited_interface_with_arguments_should_have_help() {
    let (_, ds) = checked(&with_shapes(&with_main(
        "interface G<T> {}\ninterface I: G<i64> {}\nclass C {}\nextend C: I {}",
    )));
    assert_eq!(ds.len(), 1, "{ds:?}");
    assert!(ds[0].help().is_some(), "{ds:?}");
}

/// A component with no weight below zero is not searched: the first growing edge is the label,
/// and the cost stays that of the walk that builds the graph.
#[test]
fn a_component_without_a_negative_weight_should_not_be_searched() {
    for src in [
        "fn nest<T>(x: T, n: i64) {\n    if n > 0 {\n        nest([x], n - 1)\n    }\n}\nfn main() {\n    nest(1, 3)\n}",
        "fn f<T>(x: T) {\n    g([x])\n}\nfn g<T>(x: T) {\n    f(x)\n}\nfn main() {}",
        "class Cell<T> {\n    next: Cell<[T]>?\n}\nfn main() {}",
    ] {
        cycle::BELLMAN_FORD_STEPS.set(0);
        let found: Vec<_> = checked(src).1.iter().map(Diagnostic::code).collect();
        assert_eq!(found, [Code::InstantiationCycle], "{src}");
        assert_eq!(cycle::BELLMAN_FORD_STEPS.get(), 0, "{src}");
    }
}

/// The generic interface `Convert`, three classes of degrees, two conformances of `Celsius` to it
/// and the items the generic-interface rows use. A row is a whole program, and these follow it, so
/// that a position in the row is its own line and column.
const CONVERT: &str = "interface Convert<T> {\n    fn convert(self) -> T\n}\nclass Fahrenheit {\n    d: f64\n}\nclass Kelvin {\n    d: f64\n}\nclass Celsius {\n    d: f64\n}\nextend Celsius: Convert<Fahrenheit> {\n    fn convert(self) -> Fahrenheit { Fahrenheit(d: self.d) }\n}\nextend Celsius: Convert<Kelvin> {\n    fn convert(self) -> Kelvin { Kelvin(d: self.d) }\n}\nclass Holder {\n    k: Kelvin\n}\nclass Box<T> {\n    value: T\n}\nfn take_kelvin(k: Kelvin) -> f64 { k.d }\nfn take<T>(a: T, b: T) -> T { a }\nfn unbox<T>(b: Box<T>, x: T) -> T { x }\n";

/// `row`, a program, then `CONVERT`.
fn with_convert(row: &str) -> String {
    format!("{row}\n{CONVERT}")
}

/// A `main` that holds `statements` after a `Celsius` and a `Kelvin`.
fn convert_main(statements: &str) -> String {
    format!(
        "fn main() {{\n    let c = Celsius(d: 1.0)\n    let k = Kelvin(d: 1.0)\n    {statements}\n}}"
    )
}

/// A type that conforms to a generic interface more than once picks the conformance from what the
/// call's context expects: a `let`'s type, a parameter, a result, a field, a bound, the payload of
/// an optional, a sibling argument that fixes the type, an ordinary argument and written type
/// arguments that leave the wait alone. The inherited interface keeps its argument, and a block
/// over a generic target gives the interface its arguments.
#[test]
fn generic_interfaces_should_check_clean() {
    let two = |decls: &str, statements: &str| format!("{decls}{}", convert_main(statements));
    for src in [
        two("", "let f: Fahrenheit = c.convert()"),
        two("", "let _ = take_kelvin(c.convert())"),
        two(
            "fn kelvin_of(c: Celsius) -> Kelvin {\n    c.convert()\n}\n",
            "let _ = kelvin_of(c)",
        ),
        two("", "let _ = Holder(k: c.convert())"),
        two(
            "fn to<T: Convert<Kelvin>>(x: T) -> Kelvin {\n    x.convert()\n}\n",
            "let _ = to(c)",
        ),
        // Two bounds of one interface: the annotation picks the bound.
        two(
            "fn both<T: Convert<Fahrenheit> & Convert<Kelvin>>(x: T) -> f64 {\n    let f: Fahrenheit = x.convert()\n    let k: Kelvin = x.convert()\n    f.d + k.d\n}\n",
            "let _ = both(c)",
        ),
        // The payload of the expected optional picks one.
        two("", "let f: Fahrenheit? = c.convert()"),
        // The call waits for `k`, whose type picks `Convert<Kelvin>`.
        two("", "let _ = take(c.convert(), k)"),
        // A waiting call inside a construction that waits for the type its sibling fixes.
        two("", "let _ = unbox(Box(value: c.convert()), k)"),
        // An ordinary argument does not stop the wait.
        two(
            "interface Convert1<T> {\n    fn convert(self, by: i64) -> T\n}\nclass Meter {\n    d: f64\n}\nextend Meter: Convert1<Fahrenheit> {\n    fn convert(self, by: i64) -> Fahrenheit { Fahrenheit(d: self.d) }\n}\nextend Meter: Convert1<Kelvin> {\n    fn convert(self, by: i64) -> Kelvin { Kelvin(d: self.d) }\n}\n",
            "let _ = take(Meter(d: 1.0).convert(1), k)",
        ),
        // Type arguments written for the method's own parameter do not stop it either.
        two(
            "interface Pick<T> {\n    fn pick<U>(self, x: U) -> T\n}\nclass Meter {\n    d: f64\n}\nextend Meter: Pick<Fahrenheit> {\n    fn pick<U>(self, x: U) -> Fahrenheit { Fahrenheit(d: self.d) }\n}\nextend Meter: Pick<Kelvin> {\n    fn pick<U>(self, x: U) -> Kelvin { Kelvin(d: self.d) }\n}\n",
            "let _ = take(Meter(d: 1.0).pick<i64>(1), k)",
        ),
        // `Feed<Kelvin>` inherits `Convert<Kelvin>` with its argument.
        two(
            "interface Feed<T>: Convert<T> {}\nclass Probe: Feed<Kelvin> {\n    d: f64\n    fn convert(self) -> Kelvin { Kelvin(d: self.d) }\n}\nfn fed<T: Feed<Kelvin>>(x: T) -> Kelvin {\n    x.convert()\n}\n",
            "let _ = fed(Probe(d: 1.0))",
        ),
        // A block over a generic target gives the argument of the interface.
        two(
            "extend<T> Box<T>: Convert<T> {\n    fn convert(self) -> T { self.value }\n}\n",
            "let n: i64 = Box(value: 1).convert()",
        ),
        // Another block for one instantiation, and one over the array of the parameter.
        two(
            "extend<T> Box<T>: Convert<T> {\n    fn convert(self) -> T { self.value }\n}\nextend Box<i64>: Convert<string> {\n    fn convert(self) -> string { \"i\" }\n}\n",
            "let s: string = Box(value: 1).convert()\n    let n: i64 = Box(value: 1).convert()",
        ),
        two(
            "extend<T> Box<T>: Convert<T> {\n    fn convert(self) -> T { self.value }\n}\nextend<U> Box<U>: Convert<[U]> {\n    fn convert(self) -> [U] { [self.value] }\n}\n",
            "let a: [i64] = Box(value: 1).convert()\n    let n: i64 = Box(value: 1).convert()",
        ),
    ] {
        clean(&with_convert(&src));
    }
}

/// A call that two conformances of a generic interface answer, and the context does not pick, is
/// ambiguous; so is a conformance that comes twice, a bound that the type meets with other
/// arguments, and a block that overlaps another by its interface arguments.
#[test]
fn generic_interfaces_should_report_what_the_context_cannot_pick() {
    let m = convert_main;
    for (src, expected) in [
        // No expected type, and an expected type that no candidate returns.
        (m("let x = c.convert()"), vec![(Code::Ambiguous, (4, 15))]),
        (
            m("let x: f64 = c.convert()"),
            vec![(Code::Ambiguous, (4, 20))],
        ),
        // A third conformance.
        (
            format!(
                "class Rankine {{\n    d: f64\n}}\nextend Celsius: Convert<Rankine> {{\n    fn convert(self) -> Rankine {{ Rankine(d: self.d) }}\n}}\n{}",
                m("let x = c.convert()")
            ),
            vec![(Code::Ambiguous, (10, 15))],
        ),
        // The same conformance twice, and a block over every `T` beside one for `i64`.
        (
            format!(
                "class Dup {{}}\nextend Dup: Convert<Kelvin> {{\n    fn convert(self) -> Kelvin {{ Kelvin(d: 0.0) }}\n}}\nextend Dup: Convert<Kelvin> {{\n    fn convert(self) -> Kelvin {{ Kelvin(d: 1.0) }}\n}}\n{}",
                m("")
            ),
            vec![(Code::ConformsTwice, (5, 13))],
        ),
        (
            format!(
                "extend<T> Box<T>: Convert<T> {{\n    fn convert(self) -> T {{ self.value }}\n}}\nextend<U> Box<U>: Convert<i64> {{\n    fn convert(self) -> i64 {{ 1 }}\n}}\n{}",
                m("")
            ),
            vec![(Code::ConformsTwice, (4, 19))],
        ),
        // The occurs check keeps `Convert<[U]>` and `Convert<T>` apart, and so do other targets.
        (
            format!(
                "extend<T> Box<T>: Convert<T> {{\n    fn convert(self) -> T {{ self.value }}\n}}\nextend<U> Box<U>: Convert<[U]> {{\n    fn convert(self) -> [U] {{ [self.value] }}\n}}\nextend<V> Box<[V]>: Convert<V> {{\n    fn convert(self) -> V {{ self.value[0] }}\n}}\n{}",
                m("")
            ),
            vec![],
        ),
        // A bound that the type meets with other arguments.
        (
            "class Plain {\n    d: f64\n}\nextend Plain: Convert<Fahrenheit> {\n    fn convert(self) -> Fahrenheit { Fahrenheit(d: self.d) }\n}\nfn to<T: Convert<Kelvin>>(x: T) -> Kelvin {\n    x.convert()\n}\nfn main() {\n    to(Plain(d: 1.0))\n}".to_string(),
            vec![(Code::NotConform, (11, 8))],
        ),
    ] {
        codes(&with_convert(&src), &expected);
    }
    // Two arguments, and a result that appears in two candidates: the help is the first result
    // that exactly one candidate has; its text is pinned by `E0156-pairs`.
    let pairs = "interface I<T, U> {\n    fn m(self) -> T\n}\nclass C {}\nextend C: I<i64, bool> {\n    fn m(self) -> i64 { 1 }\n}\nextend C: I<i64, char> {\n    fn m(self) -> i64 { 2 }\n}\nextend C: I<string, bool> {\n    fn m(self) -> string { \"s\" }\n}\nfn main() {\n    let c = C()\n    let x = c.m()\n}";
    let (_, ds) = checked(pairs);
    assert_eq!(
        ds.iter().map(Diagnostic::code).collect::<Vec<_>>(),
        [Code::Ambiguous]
    );
    assert!(ds[0].help().is_some(), "{ds:?}");
}

/// The interface `Feed<T: Same>` and the classes the rows of the next two tests use.
const FEED: &str = "interface Same {\n    fn same(self, other: Self) -> bool\n}\ninterface Feed<T: Same> {\n    fn feed(self) -> T\n}\nclass Node {\n    id: i64\n}\nclass Feeder {\n    n: i64\n}\n";

/// An argument of an interface meets the bounds of its parameter, in every place that writes an
/// interface with arguments: a conformance list, an `extend` block, a bound, an inherited
/// interface. The wording is pinned by the `E0154-interface-*` fixtures.
#[test]
fn interface_arguments_should_meet_the_bounds_of_their_parameters() {
    for (row, expected) in [
        (
            "class Wire: Feed<Node> {\n    fn feed(self) -> Node { Node(id: 1) }\n}\nfn main() {}",
            (13, 18),
        ),
        (
            "extend Feeder: Feed<Node> {\n    fn feed(self) -> Node { Node(id: 1) }\n}\nfn main() {}",
            (13, 21),
        ),
        (
            "fn first<F: Feed<Node>>(feeder: F) -> i64 { 0 }\nfn main() {}",
            (13, 18),
        ),
        ("interface Relay<T>: Feed<T> {}\nfn main() {}", (13, 26)),
    ] {
        codes(&format!("{FEED}{row}"), &[(Code::NotConform, expected)]);
    }
}

/// An argument meets the bound when its type conforms, whichever item gives the conformance and
/// wherever it stands in the source, and when it is a parameter that has the bound.
#[test]
fn interface_arguments_that_meet_the_bounds_should_check_clean() {
    let later =
        "extend Node: Same {\n    fn same(self, other: Node) -> bool { self.id == other.id }\n}\n";
    for row in [
        // The conformance comes after the item that needs it.
        "class Wire: Feed<Node> {\n    fn feed(self) -> Node { Node(id: 1) }\n}",
        "extend Feeder: Feed<Node> {\n    fn feed(self) -> Node { Node(id: 1) }\n}",
        "fn first<F: Feed<Node>>(feeder: F) -> i64 { 0 }",
        // The parameter has the bound that the interface asks for.
        "interface Relay<T: Same>: Feed<T> {}",
        "fn second<T: Same, F: Feed<T>>(feeder: F) -> i64 { 0 }",
        "class Crate<T> {\n    value: T\n}\nextend<T: Same> Crate<T>: Feed<T> {\n    fn feed(self) -> T { self.value }\n}",
    ] {
        clean(&format!("{FEED}{row}\n{later}fn main() {{}}"));
    }
}

/// A bound on the parameter of an interface may name `Self`: it stands for the type that has the
/// interface. With `Link<T: Back<Self>>`, the bound `B: Link<Wrap<B>>` holds when `Wrap<B>`
/// conforms to `Back<B>`, whatever place `B` has among the parameters in scope.
#[test]
fn an_interface_argument_should_meet_a_bound_that_names_self() {
    let decls = "interface Back<S> {\n    fn back(self) -> S\n}\ninterface Link<T: Back<Self>> {\n    fn link(self) -> T\n}\nclass Wrap<S> {\n    value: S\n}\nextend<S> Wrap<S>: Back<S> {\n    fn back(self) -> S { self.value }\n}\n";
    for row in [
        // The place of `B` among a function's parameters, and among a method's.
        "fn f<A, B: Link<Wrap<B>>>(x: B) {}",
        "class K<Q> {\n    n: Q\n    fn m<B: Link<Wrap<B>>>(self, x: B) {}\n}",
        // The place of `T` among an interface's parameters.
        "interface Outer<T: Link<Wrap<T>>> {}",
    ] {
        clean(&format!("{decls}{row}\nfn main() {{}}"));
    }
}

/// The interface of the first method that `main` calls in a `let`, and its arguments.
fn called_conformance(src: &str) -> Option<String> {
    let (program, ds) = checked(src);
    assert!(ds.is_empty(), "{ds:?}");
    let program = program.expect("the program should check");
    let main = (program.functions.iter().find(|f| f.name == "main")).expect("a main");
    for stmt in &main.body.stmts {
        let tir::Stmt::Let { init, .. } = stmt else {
            continue;
        };
        let mut init = init;
        while let tir::ExprKind::Wrap(inner) = &init.kind {
            init = inner;
        }
        if let tir::ExprKind::Call { func, .. } = init.kind {
            return program.functions[func.0].conformance.map(|(_, args)| {
                let names: Vec<_> = (args.get().iter())
                    .map(|ty| match ty.kind() {
                        TypeKind::Class(id, _) => program.classes[id.0].name.clone(),
                        _ => ty.to_string(),
                    })
                    .collect();
                names.join(", ")
            });
        }
    }
    panic!("main should call a method in a `let`");
}

/// The expected type picks the conformance by the arguments of its interface: a `let`'s type, and
/// the payload of an optional.
#[test]
fn expected_types_should_pick_one_conformance_of_a_generic_interface() {
    for (statement, expected) in [
        ("let f: Fahrenheit = c.convert()", "Fahrenheit"),
        ("let k: Kelvin = c.convert()", "Kelvin"),
        ("let k: Kelvin? = c.convert()", "Kelvin"),
    ] {
        let src = with_convert(&convert_main(statement));
        assert_eq!(
            called_conformance(&src).as_deref(),
            Some(expected),
            "{statement}"
        );
    }
}

/// A call of a method of a generic conformance is a call of `Type.Interface<Arguments>.method`,
/// and the interface of a block over a generic target is written with the block's arguments for
/// the receiver, not with its parameters.
#[test]
fn generic_conformance_methods_should_lower_with_their_interface_arguments() {
    contains(
        &with_convert(&convert_main(
            "let f: Fahrenheit = c.convert()\n    let o: Kelvin = c.convert()",
        )),
        &[
            "(fn Celsius.Convert<Fahrenheit>.convert ((self#0 Celsius)) Fahrenheit ",
            "(fn Celsius.Convert<Kelvin>.convert ((self#0 Celsius)) Kelvin ",
            "(call Celsius.Convert<Fahrenheit>.convert c#",
            "(call Celsius.Convert<Kelvin>.convert c#",
        ],
    );
    contains(
        &with_convert(
            "extend<T> Box<T>: Convert<T> {\n    fn convert(self) -> T { self.value }\n}\nfn main() {\n    let n: i64 = Box(value: 1).convert()\n}",
        ),
        &["(fn Box<T>.Convert<T>.convert ((self#0 Box<T>)) T "],
    );
}

/// The interfaces and classes the associated type rows use. A row is a whole program, and these
/// follow it, so that a position in the row is its own line and column.
const SOURCES: &str = "interface Same {\n    fn same(self, other: Self) -> bool\n}\ninterface Named {\n    fn name(self) -> string\n}\ninterface Marker {\n    fn mark(self) -> i64\n}\ninterface Source {\n    type Item: Same\n    fn next(self) -> Item?\n}\ninterface Convert<T> {\n    fn convert(self) -> T\n}\nclass Node {\n    id: i64\n}\nclass Tag: Same {\n    id: i64\n    fn same(self, other: Tag) -> bool { self.id == other.id }\n}\nclass Countdown: Source {\n    type Item = Tag\n    mut n: i64\n    fn next(self) -> Item? { None }\n}\nclass Wrap<T> {\n    value: T\n}\nclass Peek<S> {\n    inner: S\n}\nfn pass<S: Source>(s: S, x: S.Item) -> S.Item { s.next() ?? x }\nfn pass2<S: Source>(x: S.Item, s: S) -> S.Item { s.next() ?? x }\nfn main() {}\n";

/// `row`, a program, then the interfaces and classes.
fn with_sources(row: &str) -> String {
    format!("{row}\n{SOURCES}")
}

/// An associated type of a type parameter or of another associated type is the type that the
/// conformance of the argument chooses, in a result, a parameter, a local, a field and a payload,
/// through `?` and `[ ]`, and in a call that passes its own parameter on.
#[test]
fn associated_types_should_normalize_through_generic_code() {
    for row in [
        // A result, and a call that passes the parameter on.
        "fn first_of<S: Source>(s: S) -> S.Item? { s.next() }\nfn g<S: Source>(s: S) -> S.Item? { first_of(s) }\nfn t() {\n    let a: Tag? = g(Countdown(n: 1))\n    let b: Tag? = first_of(Countdown(n: 1))\n}",
        // A local, an array and an optional of them.
        "fn drain<S: Source>(s: S) -> [S.Item] {\n    let mut last: [S.Item] = []\n    while let x = s.next() {\n        let y: S.Item = x\n        last = [y]\n    }\n    last\n}\nfn firsts<S: Source>(s: S) -> [S.Item?] { [s.next()] }\nfn t() {\n    let xs: [Tag] = drain(Countdown(n: 1))\n    let ys: [Tag?] = firsts(Countdown(n: 1))\n}",
        // An argument for `S.Item` is checked once `S` is bound, before or after it.
        "fn t() {\n    let c = Countdown(n: 1)\n    let a: Tag = pass(c, Tag(id: 5))\n    let b: Tag = pass2(Tag(id: 6), c)\n}",
        // The bounds of an associated type give its methods, and a nested one has its own.
        "interface Keyed {\n    type Key: Same\n    fn key(self) -> Key\n}\ninterface Bag {\n    type Elem: Keyed\n    fn pick(self) -> Elem\n}\nfn both<S: Source>(a: S.Item, b: S.Item) -> bool { a.same(b) }\nfn key_of<B: Bag>(b: B) -> B.Elem.Key { b.pick().key() }\nclass Row: Keyed {\n    type Key = Tag\n    fn key(self) -> Key { Tag(id: 1) }\n}\nclass Rows: Bag {\n    type Elem = Row\n    fn pick(self) -> Elem { Row() }\n}\nfn t() {\n    let r = both<Countdown>(Tag(id: 1), Tag(id: 2))\n    let k: Tag = key_of(Rows())\n}",
        // A conformance in an `extend` block chooses a type, also over a type parameter and over
        // the associated type of another conformance.
        "class Words {\n    n: i64\n}\nextend Words: Source {\n    type Item = Tag\n    fn next(self) -> Item? { None }\n}\nextend<T: Same> Wrap<T>: Source {\n    type Item = T\n    fn next(self) -> Item? { self.value }\n}\nclass Relay<S: Source> {\n    inner: S\n}\nextend<S: Source> Relay<S>: Source {\n    type Item = S.Item\n    fn next(self) -> Item? { self.inner.next() }\n}\nfn t() {\n    let a: Tag = pass(Words(n: 1), Tag(id: 1))\n    let b: Tag = pass(Wrap(value: Tag(id: 1)), Tag(id: 2))\n    let c: Tag = pass(Relay(inner: Countdown(n: 1)), Tag(id: 3))\n}",
        // A field, a payload and a method of a generic type.
        "class Holder<S: Source> {\n    item: S.Item\n    fn get(self) -> S.Item { self.item }\n}\nenum Got<S: Source> {\n    Got(S.Item)\n    Nothing\n}\nfn t() {\n    let h: Holder<Countdown> = Holder(item: Tag(id: 4))\n    let x: Tag = h.item\n    let y: Tag = h.get()\n    let e: Got<Countdown> = Got.Got(Tag(id: 1))\n    match e {\n        Got.Got(v) => print(v.id)\n        Got.Nothing => print(0)\n    }\n}",
        // A choice that names no parameter adds no edge: the children of a `Dir` are `Dir`s.
        "interface Tree {\n    type Child: Tree\n    fn kids(self) -> [Child]\n}\nfn size<T: Tree>(t: T) -> i64 {\n    let mut n = 1\n    for k in t.kids() {\n        n += size(k)\n    }\n    n\n}\nclass Dir: Tree {\n    type Child = Dir\n    name: string\n    fn kids(self) -> [Child] { [] }\n}\nfn t() {\n    print(size(Dir(name: \"a\")))\n}",
        // A local of the associated type, written `Item` in a body of the conformance.
        "class Local: Source {\n    type Item = Tag\n    fn next(self) -> Item? {\n        let t: Item = Tag(id: 1)\n        t\n    }\n}\nextend Wrap<Tag>: Source {\n    type Item = Tag\n    fn next(self) -> Item? {\n        let t: [Item] = []\n        if t.len() == 0 { self.value } else { None }\n    }\n}",
        // A requirement whose own parameter has a bound that names the associated type: the
        // method of the conformance meets it with the type chosen.
        "interface Mapper {\n    type Item\n    fn put<U: Convert<Item>>(self, u: U) -> i64\n}\nclass Sink: Mapper {\n    type Item = Tag\n    n: i64\n    fn put<U: Convert<Item>>(self, u: U) -> i64 { self.n }\n}",
        // A bound of the parameter that an associated type is written through waits for a
        // parameter that is not bound yet, and a parameter beside it is no concern of the check.
        "extend Countdown: Convert<i64> {\n    fn convert(self) -> i64 { 1 }\n}\nfn conv<U, S: Source & Convert<U>>(s: S, x: S.Item, u: U) -> U { s.convert() }\nfn two<S: Source, T>(s: S, x: S.Item, t: T) -> T { t }\nfn t() {\n    let a: i64 = conv(Countdown(n: 1), Tag(id: 1), 5)\n    let b: i64 = two(Countdown(n: 1), Tag(id: 1), 5)\n}",
        // A choice that names an associated type of a parameter whose bound has an argument.
        "interface Conv2<A> {\n    type Out\n}\ninterface Pick {\n    type Item\n}\nclass Duo2<T, U> {\n    a: T\n    b: U\n}\nextend<T: Conv2<U>, U> Duo2<T, U>: Pick {\n    type Item = T.Out\n}",
        // A choice that names the associated type of an associated type of its parameter, and one
        // that names an array, an optional and an enum around the parameter.
        "interface Deep {\n    type Item: Deep\n}\nclass Chain<T> {\n    value: T\n}\nextend<T: Deep> Chain<T>: Deep {\n    type Item = Chain<T.Item.Item>\n}\nclass Rows<T> {\n    value: T\n}\nextend<T> Rows<T>: Deep {\n    type Item = Rows<[T?]>\n}",
        // A bound whose argument is an associated type of a sibling parameter, before and after
        // the bound that gives the parameter its type, and a call of each.
        "extend Tag: Convert<Tag> {\n    fn convert(self) -> Tag { self }\n}\nfn conv<S: Source, T: Convert<S.Item>>(s: S, t: T) -> S.Item { t.convert() }\nfn conv2<T: Convert<S.Item>, S: Source>(t: T, s: S) -> S.Item? { s.next() }\nfn t() {\n    let a: Tag = conv(Countdown(n: 1), Tag(id: 2))\n    let b: Tag? = conv2(Tag(id: 2), Countdown(n: 1))\n}\nfn arr<T: Convert<[S.Item]>, S: Source>(_t: T, _s: S) {}\nfn nest<T: Convert<Wrap<S.Item>>, S: Source>(_t: T, _s: S) {}",
        // A chain through two associated types, and an argument beside one that is a type.
        "interface Keyed {\n    type Key: Same\n    fn key(self) -> Key\n}\ninterface Bag {\n    type Elem: Keyed\n    fn pick(self) -> Elem\n}\ninterface Two<A, B> {}\nfn deep<T: Convert<B.Elem.Key>, B: Bag>(_t: T, _b: B) {}\nfn mixed<T: Two<i64, S.Item>, S: Source>(_t: T, _s: S) {}",
        // A chain: the bound of `U` needs the bound of `S`, which needs the bound of `T`.
        "interface Holds<Y> {\n    type Item\n}\nfn chain<S: Holds<T.Item>, U: Convert<S.Item>, T: Source>(_s: S, _t: T, u: U) -> S.Item { u.convert() }",
        // A bound that writes an associated type that another bound of its own parameter declares,
        // and two such bounds on one parameter.
        "interface Has {\n    type Out\n}\nfn own<S: Source & Convert<S.Item>>(s: S) -> S.Item { s.convert() }\nfn has<T: Has & Convert<T.Out>>(_t: T) {}\nfn both<S: Source & Convert<S.Item> & Wrap2<S.Item>>(_s: S) {}\ninterface Wrap2<X> {}\nfn later<S: Holds2<T.Item> & Convert<S.Item>, T: Source>(_s: S, _t: T) {}\ninterface Holds2<Y> {\n    type Item\n}",
        // A bound that waits for the bound of a parameter after it, and three bounds of one
        // parameter in a method of a generic class.
        "interface Holds2<Y> {\n    type Item\n}\nfn waits<U: Convert<S.Item>, S: Holds2<T.Item>, T: Source>(u: U, _s: S, _t: T) -> S.Item { u.convert() }\ninterface Wrap2<X> {}\ninterface Wrap3<X> {}\nclass H<O> {\n    fn f<S: Source & Convert<S.Item> & Wrap2<S.Item> & Wrap3<S.Item>>(self, s: S) -> S.Item { s.convert() }\n}",
        // A loop's neighbour: `T` writes through `S`, and `S` writes through `R`, which has an
        // ordinary bound. The index of a later parameter is no concern of the order.
        "interface A<X, Y> {\n    type P\n}\ninterface B<X> {\n    type Q\n}\nfn later<T: A<S.Q, R.Item>, S: B<R.Item>, R: Source>(_t: T, _s: S, _r: R) {}",
        // A bound that a parameter's bounds wait for in turn: the bound of `U` declares `Out`, and
        // waits for the one of `S` that declares `Item`, which is not the last of its parameter.
        "interface Holds2<Y> {\n    type Item\n}\ninterface HasOut<X> {\n    type Out\n}\nfn retry<U: HasOut<S.Item>, S: Holds2<T.Item> & Convert<U.Out> & Convert<U.Out>, T: Source>(_u: U, _s: S, _t: T) {}",
        // A bound waits for two parameters, and its parameter's bound inherits one interface by two
        // ways. A closure found before the bound is added grows by the interfaces it inherits.
        "interface Holds2<Y> {\n    type Item\n}\ninterface Two<A, B> {}\nfn two<U: Two<S.Item, T.Item>, S: Holds2<R.Item>, T: Holds2<R.Item>, R: Source>(_u: U, _s: S, _t: T, _r: R) {}",
        "interface Base2 {\n    type Key\n}\ninterface L2: Base2 {}\ninterface R2: Base2 {}\ninterface BothG<X>: L2, R2 {}\nfn diamond<S: Source & BothG<T.Item>, V: Convert<S.Item>, U: Convert<S.Key>, T: Source>(_s: S, _v: V, _u: U, _t: T) {}",
        // Two bounds that inherit the one `Item` that `Source` declares, beside the bound of
        // `Source` itself: they are no declarations to wait for.
        "interface First<X>: Source {}\ninterface Second<X>: Source {}\nfn same<S: Source & First<S.Item> & Second<S.Item>>(_s: S) {}",
        // Two types of one name through a parameter with two bounds, which a closure indexed once
        // gives again at the second.
        "interface A0 {\n    type N0\n}\ninterface A1 {\n    type N1\n}\nfn two<S: A0 & A1>(_x: S.N1, _y: S.N1, _z: S.N0) {}",
        // Three bounds of one parameter that give the one `Item` that `Base3<R.Item>` declares,
        // two of them by the interfaces that inherit it with the same argument.
        "interface Base3<Y> {\n    type Item\n}\ninterface First3<X, Y>: Base3<Y> {}\ninterface Second3<X, Y>: Base3<Y> {}\nfn diamond3<S: Base3<R.Item> & First3<S.Item, R.Item> & Second3<S.Item, R.Item>, R: Source>(_s: S, _r: R) {}",
        // A bound of the parameter is in scope and gives a declaration of its own, and a bound of
        // another parameter writes the type; so does a group of two bounds that give a shared one.
        "interface Holds2<Y> {\n    type Item\n}\nfn scoped<S: Holds2<i64>, U: Convert<S.Item>>(u: U) -> S.Item { u.convert() }",
        "interface First<X>: Source {}\nfn group<S: First<R.Item> & First<R.Item>, U: Convert<S.Item>, R: Source>(u: U) -> S.Item { u.convert() }",
        // An associated type with two bounds, and an interface that inherits one that has them.
        "interface Rich {\n    type Item: Same & Named\n    fn get(self) -> Item\n}\ninterface Richer: Rich {}\nfn show<R: Richer>(r: R) -> string { r.get().name() }",
    ] {
        clean(&with_sources(row));
    }
    // The printed form of an associated type is `S.Item`, through a bound and inside `?`.
    contains(
        &with_sources(
            "fn first_of<S: Source>(s: S) -> S.Item? { s.next() }\nfn t() {\n    let a: Tag? = first_of(Countdown(n: 1))\n}",
        ),
        &[
            "(fn first_of<S> ((s#0 S)) S.Item? ",
            "(fn t () unit (block (let a#0 (call first_of<Countdown> (new Countdown 1)))))",
        ],
    );
}

/// What an associated type cannot be: a conformance without it or with a type that misses a
/// bound, a type written through no bound, through a class, or by a choice that names an
/// associated type of its own conformance; a bound that no argument meets; an argument that the
/// chosen type rejects; two interfaces that both declare it; a choice no interface declares; and
/// a block that names it in its target.
#[test]
fn associated_types_should_report_what_they_cannot_take() {
    let feed = "interface Feed {\n    type Item\n}\n";
    for (row, expected) in [
        // A conformance without `type Item`, and with a type that misses the bound.
        (
            "class Bare: Source {\n    n: i64\n    fn next(self) -> Tag? { None }\n}".to_string(),
            vec![(Code::ConformanceBody, (1, 13))],
        ),
        (
            "class Bad: Source {\n    type Item = Node\n    n: i64\n    fn next(self) -> Node? { None }\n}".to_string(),
            vec![(Code::NotConform, (2, 17))],
        ),
        // Written through a parameter with no bound, through a class, and through `Self` in a
        // conformance.
        (
            "fn f<S>(x: S.Item) {}".to_string(),
            vec![(Code::UnknownType, (1, 14))],
        ),
        (
            "fn f(x: Countdown.Item) {}".to_string(),
            vec![(Code::UnknownType, (1, 9))],
        ),
        (
            "class Bad2: Source {\n    type Item = Tag\n    n: i64\n    fn next(self) -> Self.Item? { None }\n}".to_string(),
            vec![(Code::UnknownType, (4, 22))],
        ),
        // An inherited interface cannot take `Item`, which names `Self`.
        (
            "interface Feed2: Convert<Item> {\n    type Item\n}".to_string(),
            vec![(Code::InheritedArgument, (1, 26))],
        ),
        // A call whose argument misses the bound that gives the associated type its meaning: one
        // report, and no mismatch for the argument that waited.
        (
            "fn t() {\n    let node = Node(id: 1)\n    let a = pass(node, 0)\n}".to_string(),
            vec![(Code::NotConform, (3, 18))],
        ),
        (
            "extend<T: Marker> Wrap<T>: Source {\n    type Item = Tag\n    fn next(self) -> Item? { None }\n}\nfn t() {\n    let node = Node(id: 1)\n    let a = pass(Wrap(value: node), \"x\")\n}".to_string(),
            vec![(Code::NotConform, (7, 18))],
        ),
        // An argument of the wrong type for the associated type, before or after its base.
        (
            "fn t() {\n    let c = Countdown(n: 1)\n    let a = pass(c, \"a\")\n    let b = pass2(\"a\", c)\n}".to_string(),
            vec![(Code::Mismatch, (3, 21)), (Code::Mismatch, (4, 19))],
        ),
        // Two interfaces that declare `Item`.
        (
            format!("{feed}fn f<S: Source & Feed>(x: S.Item) {{}}"),
            vec![(Code::Ambiguous, (4, 29))],
        ),
        // An associated type conforms to what its declaration bounds it by, and to no more.
        (
            "fn need<T: Marker>(t: T) {}\nfn f<S: Source>(x: S.Item) {\n    need(x)\n}".to_string(),
            vec![(Code::NotConform, (3, 10))],
        ),
        // A choice repeated, one whose type is unknown (no report beside the unknown name), and
        // one that no interface of the list declares.
        (
            format!("{feed}class Dup: Feed {{\n    type Item = i64\n    type Item = i64\n}}"),
            vec![(Code::DefinedTwice, (6, 10))],
        ),
        (
            "class Gone: Source {\n    type Item = Missing\n    n: i64\n    fn next(self) -> Item? { None }\n}".to_string(),
            vec![(Code::UnknownType, (2, 17))],
        ),
        (
            "class Odd: Source {\n    type Item = Tag\n    type Other = i64\n    n: i64\n    fn next(self) -> Item? { None }\n}".to_string(),
            vec![(Code::ConformanceBody, (3, 10))],
        ),
        // The base of an associated type is unknown, and two interfaces declare `Item` (no bound
        // to add).
        (
            "fn f(x: Missing.Item) {}".to_string(),
            vec![(Code::UnknownType, (1, 9))],
        ),
        (
            format!("{feed}fn f<S>(x: S.Item) {{}}"),
            vec![(Code::UnknownType, (4, 14))],
        ),
        // A choice no interface declares.
        (
            "extend Node: Marker {\n    type Item = i64\n    fn mark(self) -> i64 { 1 }\n}".to_string(),
            vec![(Code::ConformanceBody, (2, 10))],
        ),
        // A choice that names an associated type of its own conformance.
        (
            format!("{feed}class Own: Feed {{\n    type Item = [Item]\n}}"),
            vec![(Code::UnknownType, (5, 18))],
        ),
        (
            "interface Two {\n    type A\n    type B\n}\nclass Own: Two {\n    type A = B\n    type B = A\n}".to_string(),
            vec![(Code::UnknownType, (6, 14)), (Code::UnknownType, (7, 14))],
        ),
        // A block names an associated type in its target or in the argument of its interface.
        (
            "extend<S: Source> Wrap<S.Item> {\n    fn a(self) -> i64 { 1 }\n}".to_string(),
            vec![(Code::CannotExtend, (1, 19))],
        ),
        (
            "extend<S: Source> Peek<S>: Convert<S.Item> {\n    fn convert(self) -> S.Item { self.inner.next() ?? self.inner.next() }\n}".to_string(),
            vec![(Code::CannotExtend, (1, 28))],
        ),
        // An associated type declared again, and one an inherited interface declares.
        (
            "interface Again {\n    type Item\n    type Item\n}".to_string(),
            vec![(Code::DefinedTwice, (3, 10))],
        ),
        (
            format!("{feed}interface Sub: Feed {{\n    type Item\n}}"),
            vec![(Code::DefinedTwice, (5, 10))],
        ),
    ] {
        codes(&with_sources(&row), &expected);
    }
}

/// A conformance that lacks a requirement whose signature holds a type that does not exist is
/// reported once for the type and once for the conformance, which lists the requirement.
#[test]
fn a_requirement_with_an_unknown_type_should_be_listed_for_a_conformance_that_lacks_it() {
    codes(
        "interface Q {\n    fn f(self, x: Missing)\n}\nclass C: Q {}\nfn main() {}",
        &[
            (Code::UnknownType, (2, 19)),
            (Code::ConformanceBody, (4, 10)),
        ],
    );
}

/// A conformance that lacks its choice, or whose choice failed, is reported once. A call or a
/// constructor that takes the associated type of such a conformance fails after that report: no
/// mismatch, no crash. The argument that names the associated type comes after, before and apart
/// from its base.
#[test]
fn a_call_through_a_conformance_without_its_choice_should_fail_after_the_one_report() {
    let holder = "class Holder<S: Source> {\n    item: S.Item\n}\n";
    for (conformance, report) in [
        // No `type Item`: E0152 at the interface of the conformance.
        (
            "class Bad: Source {\n    n: i64\n    fn next(self) -> Tag? { None }\n}\n",
            (Code::ConformanceBody, (4, 12)),
        ),
        // A choice whose type is unknown: E0030 at that type.
        (
            "class Bad: Source {\n    type Item = Missing\n    n: i64\n    fn next(self) -> Tag? { None }\n}\n",
            (Code::UnknownType, (5, 17)),
        ),
    ] {
        for local in [
            "let a = pass(Bad(n: 1), 5)",
            "let a = pass2(5, Bad(n: 1))",
            "let a: Holder<Bad> = Holder(item: 3)",
        ] {
            let row = format!("{holder}{conformance}fn t() {{\n    {local}\n}}");
            codes(&with_sources(&row), &[report]);
        }
    }
}

/// An associated type whose choice leads back to the associated type itself has no type. It is
/// reported once, at the choice that closes the loop, however many calls use it, and the calls
/// that use it fail without a second report. The same choices with a chain that ends are clean.
#[test]
fn an_associated_type_that_needs_itself_should_be_reported_once() {
    let program = |leaf: &str, uses: &str| {
        format!(
            "interface I {{\n    type A: I\n    fn a(self) -> A\n}}\nclass Leaf: I {{\n    type A = {leaf}\n    fn a(self) -> A {{ panic(\"leaf\") }}\n}}\nclass W<T> {{\n    value: T\n}}\nextend<T: I> W<T>: I {{\n    type A = T.A.A\n    fn a(self) -> A {{ panic(\"wrapped\") }}\n}}\nfn get<S: I>(s: S) -> S.A {{ s.a() }}\nfn bare<S: I>(_s: S) {{}}\nfn keep<T>(t: T) {{}}\nfn main() {{\n{uses}}}\n"
        )
    };
    let uses = "    keep(get(W(value: Leaf())))\n    keep(get(W(value: Leaf())))\n";
    // `W<Leaf>.A` is `Leaf.A.A`, which is `W<Leaf>.A`.
    codes(&program("W<Leaf>", uses), &[(Code::UnknownType, (13, 14))]);
    // A call that writes no associated type finds it too, where the argument meets the bound.
    codes(
        &program("W<Leaf>", "    bare(W(value: Leaf()))\n"),
        &[(Code::UnknownType, (13, 14))],
    );
    // `Leaf.A` ends the chain.
    codes(&program("Leaf", uses), &[]);
    codes(&program("Leaf", "    bare(W(value: Leaf()))\n"), &[]);
    // The bound that the type meets inherits the interface that declares the associated type,
    // and no signature writes it: `mono` would meet the loop in the body of `get`.
    let inherited = "interface Base {\n    type B: Holder\n}\ninterface Holder: Base {}\nclass Leaf {\n    n: i64\n}\nextend Leaf: Base {\n    type B = W<Leaf>\n}\nextend Leaf: Holder {}\nclass W<T> {\n    value: T\n}\nextend<T: Holder> W<T>: Base {\n    type B = T.B.B\n}\nextend<T: Holder> W<T>: Holder {}\nfn get<S: Holder>(_s: S) {\n    let _x: [S.B] = []\n}\nfn main() {\n    get(W(value: Leaf(n: 1)))\n}";
    codes(inherited, &[(Code::UnknownType, (16, 14))]);
    // The inherited interface takes `Self` as an argument: the type that has the bound is that
    // argument, not the unit type.
    let with_self = "interface Base<X> {\n    type B: Holder\n}\ninterface Holder: Base<Self> {}\nclass Leaf {\n    n: i64\n}\nextend Leaf: Base<Leaf> {\n    type B = W<Leaf>\n}\nextend Leaf: Holder {}\nclass W<T> {\n    value: T\n}\nextend<T: Holder> W<T>: Base<W<T>> {\n    type B = T.B.B\n}\nextend<T: Holder> W<T>: Holder {}\nfn get<S: Holder>(_s: S) {\n    let _x: [S.B] = []\n}\nfn main() {\n    get(W(value: Leaf(n: 1)))\n}";
    codes(with_self, &[(Code::UnknownType, (16, 14))]);
    // The loop closes at a type that only a generic choice gives: `Outer<Leaf>.B` is `W<Leaf>`,
    // and no block names `W<Leaf>`. The type that a choice gives is probed for the bound of its
    // associated type, so the loop of `W<Leaf>.B` is found where `Leaf` meets `Holder`.
    let nested = "interface Base {\n    type B: Holder\n}\ninterface Holder: Base {}\nclass Leaf {}\nextend Leaf: Base {\n    type B = Outer<Leaf>\n}\nextend Leaf: Holder {}\nclass Outer<T> {}\nextend<T: Holder> Outer<T>: Base {\n    type B = W<T>\n}\nextend<T: Holder> Outer<T>: Holder {}\nclass W<T> {}\nextend<T: Holder> W<T>: Base {\n    type B = T.B.B.B\n}\nextend<T: Holder> W<T>: Holder {}\nfn get<S: Holder>(_s: S) {\n    let _x: [S.B.B.B] = []\n}\nfn main() {\n    get(Leaf())\n}";
    codes(nested, &[(Code::UnknownType, (17, 14))]);
    // The same chain ends at `Leaf`: clean.
    codes(&nested.replace("T.B.B.B", "T"), &[]);
    // A choice that gives a larger type at each step has no loop and no end: the probe stops. The
    // type grows by a level of arguments, of an array, and of an optional.
    for grown in ["W<W<T>>", "W<[T]>", "W<W<T>?>"] {
        let growing = format!(
            "interface Base {{\n    type B: Holder\n}}\ninterface Holder: Base {{}}\nclass Leaf {{}}\nextend Leaf: Base {{\n    type B = W<Leaf>\n}}\nextend Leaf: Holder {{}}\nclass W<T> {{}}\nextend<T> W<T>: Base {{\n    type B = {grown}\n}}\nextend<T> W<T>: Holder {{}}\nfn get<S: Holder>(_s: S) {{\n    let _x: [S.B] = []\n}}\nfn main() {{\n    get(Leaf())\n}}"
        );
        codes(&growing, &[]);
    }
}

/// The program of issue 226 with `levels` levels in the type that `Leaf` chooses: `Leaf.B` is
/// `Outer<..Outer<Leaf>..>`, `Outer<T>.B` is `W<T>`, and `W<T>.B` is `T.B.B.B`, which closes a
/// loop that is `levels` choices long. The choice of `W` is on line 17, column 14. `uses` is the
/// rest of the program after `get`, which writes `S.B.B.B`.
fn long_loop(levels: usize, uses: &str) -> String {
    let nested = format!("{}Leaf{}", "Outer<".repeat(levels), ">".repeat(levels));
    format!(
        "interface Base {{\n    type B: Holder\n}}\ninterface Holder: Base {{}}\nclass Leaf {{}}\nextend Leaf: Base {{\n    type B = {nested}\n}}\nextend Leaf: Holder {{}}\nclass Outer<T> {{}}\nextend<T: Holder> Outer<T>: Base {{\n    type B = W<T>\n}}\nextend<T: Holder> Outer<T>: Holder {{}}\nclass W<T> {{}}\nextend<T: Holder> W<T>: Base {{\n    type B = T.B.B.B\n}}\nextend<T: Holder> W<T>: Holder {{}}\nfn get<S: Holder>(_s: S) {{\n    let _x: [S.B.B.B] = []\n}}\n{uses}"
    )
}

/// A loop that closes after more choices than the probe once followed (16) is reported where the
/// type meets its bound, once, at the choice that closes it: `mono` never meets it. The probe
/// follows as many as the walk does (`CHOICE_DEPTH_LIMIT`), so a loop of 17 and one of 40
/// choices are found as the one of 15 is, however the call that meets the bound is written.
#[test]
fn a_loop_that_closes_after_many_choices_should_be_reported_at_the_bound() {
    let direct = "fn main() {\n    get(Leaf())\n}";
    let through = "fn pass<S: Holder>(s: S) {\n    get(s)\n}\nfn main() {\n    pass(Leaf())\n}";
    let report = [(Code::UnknownType, (17, 14))];
    for levels in [15, 17, 40, 100] {
        for (how, uses) in [("direct", direct), ("through a generic function", through)] {
            codes_row(
                &format!("{levels} levels, {how}"),
                &long_loop(levels, uses),
                &report,
            );
        }
    }
}

/// A loop that the probe does not follow, because the type that closes it nests deeper than the
/// probe goes, is not found by `check`. `mono` meets it and reports it (see `front_end`).
#[test]
fn a_type_nested_past_the_probe_should_pass_the_check() {
    let past = CHOICE_DEPTH_LIMIT + 1;
    codes(&long_loop(past, "fn main() {\n    get(Leaf())\n}"), &[]);
}

/// A choice that gives a larger type at each step, and names the associated type of that type,
/// never meets the same associated type, so only the limit ends it. It is reported once, at the
/// choice, with no crash, whatever type of the chain the call names.
#[test]
fn a_chain_of_growing_types_should_be_reported_at_the_choice_once_the_limit_ends_it() {
    let growing = |uses: &str| {
        format!(
            "interface Base {{\n    type B: Holder\n}}\ninterface Holder: Base {{}}\nclass Leaf {{}}\nextend Leaf: Base {{\n    type B = Leaf\n}}\nextend Leaf: Holder {{}}\nclass Outer<T> {{}}\nextend<T: Holder> Outer<T>: Base {{\n    type B = W<Outer<Outer<T>>>\n}}\nextend<T: Holder> Outer<T>: Holder {{}}\nclass W<T> {{}}\nextend<T: Holder> W<T>: Base {{\n    type B = T.B.B\n}}\nextend<T: Holder> W<T>: Holder {{}}\nfn get<S: Holder>(_s: S) {{\n    let _x: [S.B.B] = []\n}}\n{uses}"
        )
    };
    let report = [(Code::UnknownType, (17, 14))];
    codes(&growing("fn main() {\n    get(Outer<Leaf>())\n}"), &report);
    codes(
        &growing("fn main() {\n    get(Outer<Outer<Leaf>>())\n}"),
        &report,
    );
    // The same types with a choice that names no associated type are clean: a loop or a growth
    // of the walk is found at the bound, wherever the type is used.
    let ends = growing("fn main() {\n    get(Outer<Leaf>())\n}");
    codes(&ends.replace("type B = T.B.B", "type B = T"), &[]);
}

/// A chain of choices that ends, nested as deep as the limit allows, is clean: `W<T>` chooses the
/// associated type of `T`, so the associated type of `W<..W<Done>..>` with `levels` levels needs
/// `levels` choices, one after the other. `Start` chooses that type, and `get` writes its
/// associated type.
#[test]
fn a_chain_of_choices_that_ends_within_the_limit_should_pass_the_check() {
    for levels in [100, CHOICE_DEPTH_LIMIT] {
        let nested = format!("{}Done{}", "W<".repeat(levels), ">".repeat(levels));
        let src = format!(
            "interface Base {{\n    type B: Holder\n}}\ninterface Holder: Base {{}}\nclass Done {{}}\nextend Done: Base {{\n    type B = Done\n}}\nextend Done: Holder {{}}\nclass Start {{}}\nextend Start: Base {{\n    type B = {nested}\n}}\nextend Start: Holder {{}}\nclass W<T> {{}}\nextend<T: Holder> W<T>: Base {{\n    type B = T.B\n}}\nextend<T: Holder> W<T>: Holder {{}}\nfn get<S: Holder>(_s: S) {{\n    let _x: [S.B.B] = []\n}}\nfn main() {{\n    get(Start())\n}}"
        );
        codes_row(&format!("{levels} levels"), &src, &[]);
    }
}

/// Two associated types that each grow the type in their own way make `2^n` types in `n` steps. A
/// probe stops after `PROBE_BUDGET` declarations for each bound it meets, so the check does a
/// bounded amount of work and finds no error: the program is valid, and `mono` meets only the
/// types that its calls name.
#[test]
fn a_probe_of_branching_growth_should_stop_at_its_budget() {
    let src = "interface Base {\n    type A: Holder\n    type B: Holder\n}\ninterface Holder: Base {}\nclass Leaf {}\nextend Leaf: Base {\n    type A = W<Leaf>\n    type B = W<Leaf>\n}\nextend Leaf: Holder {}\nclass W<T> {}\nextend<T> W<T>: Base {\n    type A = W<W<T>>\n    type B = W<[T]>\n}\nextend<T> W<T>: Holder {}\nfn get<S: Holder>(_s: S) {\n    let _x: [S.A] = []\n}\nfn main() {\n    get(Leaf())\n}";
    PROBE_STEPS.set(0);
    codes(src, &[]);
    let steps = PROBE_STEPS.get();
    // The one probe that the call starts uses the whole budget (8 associated types for each
    // level of the limit); the other bounds of the program take a few steps.
    let budget = 8 * CHOICE_DEPTH_LIMIT;
    assert!((budget..budget + 64).contains(&steps), "{steps} steps");
}

/// A class, an enum, an array and an optional each add a level to the nesting of a type, and a
/// type is within a limit when its deepest part is. The type below shares `X` between the places
/// at two depths: its depth is measured once, and the limit left at the second place is what
/// counts, not the one that `X` was measured with. A type with `2^n` paths and `n` distinct types
/// is measured in `n` steps, so the check ends.
#[test]
fn nests_within_should_count_each_level_and_measure_a_shared_type_once() {
    use crate::tir::{ClassId, EnumId};
    type Wrap = fn(Type) -> Type;
    let wraps: [(&str, Wrap); 4] = [
        ("class", |t| {
            Type::intern(TypeKind::Class(ClassId(0), TypeArgs::intern(&[t])))
        }),
        ("enum", |t| {
            Type::intern(TypeKind::Enum(EnumId(0), TypeArgs::intern(&[t])))
        }),
        ("array", Type::array),
        ("optional", Type::optional),
    ];
    let within = |ty, limit| nests_within(ty, limit, &mut FxHashMap::default());
    for (kind, wrap) in wraps {
        let ty = (0..4).fold(Type::I64, |t, _| wrap(t));
        assert!(within(ty, 4), "{kind} of depth 4 within 4");
        assert!(!within(ty, 3), "{kind} of depth 4 within 3");
        assert!(within(Type::I64, 0), "{kind}: a leaf within 0");
    }
    let class = wraps[0].1;
    let shared = (0..3).fold(Type::I64, |t, _| class(t));
    let deep = (0..3).fold(shared, |t, _| class(t));
    let pair = Type::intern(TypeKind::Class(
        ClassId(1),
        TypeArgs::intern(&[class(shared), deep]),
    ));
    // `shared` is 3 deep, `deep` is 6, and the pair is 7.
    assert!(within(pair, 7), "the pair within 7");
    assert!(!within(pair, 6), "the pair within 6");
    let doubled = (0..100).fold(Type::I64, |t, _| {
        Type::intern(TypeKind::Class(ClassId(2), TypeArgs::intern(&[t, t])))
    });
    assert!(
        within(doubled, 100) && !within(doubled, 99),
        "100 shared levels"
    );
}

/// What the checker's probe does not follow, `mono` meets and reports: no panic, no program, and
/// the E0030 of the choice, at the type it writes, once, however the call that needs it is
/// written. The type that closes the loop nests deeper than the probe goes (a loop of 200
/// choices), or the chain that ends needs more steps than the limit (`W<Leaf>.B` is the 129th).
#[test]
fn mono_should_report_a_chain_that_the_probe_did_not_follow() {
    let direct = "fn main() {\n    get(Leaf())\n}";
    let through = "fn pass<S: Holder>(s: S) {\n    get(s)\n}\nfn main() {\n    pass(Leaf())\n}";
    // `plain<i64>` waits on the worklist when `get<Leaf>` fails, and is not walked.
    let queued =
        "fn plain<T>(t: T) -> T {\n    t\n}\nfn main() {\n    get(Leaf())\n    print(plain(1))\n}";
    for levels in [CHOICE_DEPTH_LIMIT + 1, 200] {
        for (how, uses) in [
            ("direct", direct),
            ("through a generic function", through),
            ("with more work queued", queued),
        ] {
            let src = long_loop(levels, uses);
            let (program, diagnostics) = crate::front_end::front_end(&src);
            let found: Vec<_> = (diagnostics.iter())
                .filter(|d| d.severity() == Severity::Error)
                .map(|d| (d.code(), at(&src, d.span())))
                .collect();
            let row = format!("{levels} levels, {how}");
            assert_eq!(found, [(Code::UnknownType, (17, 14))], "{row}");
            assert!(program.is_none(), "{row}: no program after an error");
        }
    }
}

/// A bound whose argument names an associated type of a sibling parameter finds that parameter's
/// bounds, so the only way to write the constraint works; a sibling that has no bound that
/// declares the type is still reported. A bound that names a parameter that a later argument
/// binds is checked once that argument binds it, before the argument that waits for `S.Item`:
/// one report, and none for the argument.
#[test]
fn a_bound_should_see_the_bounds_of_its_siblings_and_the_arguments_it_waits_for() {
    for (row, expected) in [
        (
            "fn f<S, T: Convert<S.Item>>(s: S, t: T) {}".to_string(),
            vec![(Code::UnknownType, (1, 22))],
        ),
        // A bound that writes an associated type that only itself declares: not found, as for any
        // parameter without a bound that declares it.
        (
            "interface Holds<Y> {\n    type Item\n}\nfn f<S: Holds<S.Item>>(_s: S) {}".to_string(),
            vec![(Code::UnknownType, (4, 17))],
        ),
        // Two bounds that wait for each other: each is reported where it writes the type, and not
        // as a type the base lacks. A bound that waits for them has no report of its own.
        (
            "interface A<X> {\n    type P\n}\ninterface B<X> {\n    type Q\n}\nfn f<T: A<S.Q>, S: B<T.P>, U: Convert<S.Q>>(t: T, s: S, u: U) {}".to_string(),
            vec![(Code::UnknownType, (7, 13)), (Code::UnknownType, (7, 24))],
        ),
        // A bound that waited for a parameter whose last bound declares nothing is reported as a
        // type the parameter lacks.
        (
            "interface Wrap2<X> {}\nfn f<U: Convert<S.Item>, S: Wrap2<T.Item>, T: Source>(u: U, s: S, t: T) {}".to_string(),
            vec![(Code::UnknownType, (2, 19))],
        ),
        // A resolved parameter in a loop bound is no part of the loop; a bound that waits for the
        // loop, and the bound that waits for that one, have no report.
        (
            "interface A<X, Y> {\n    type P\n}\ninterface B<X> {\n    type Q\n}\nfn f<T: A<S.Q, R.Item>, S: B<T.P>, R: Source, V: A<S.Q, R.Item>, W: Convert<V.P>>(t: T, s: S, r: R, v: V, w: W) {}".to_string(),
            vec![(Code::UnknownType, (7, 13)), (Code::UnknownType, (7, 32))],
        ),
        // A parameter of the class is in scope, and its associated type is no part of the loop.
        (
            "interface A<X> {\n    type P\n}\ninterface B<X, Y> {\n    type Q\n}\nclass H<O: Source> {\n    fn f<T: A<S.Q>, S: B<T.P, O.Item>>(self, t: T, s: S) {}\n}".to_string(),
            vec![(Code::UnknownType, (8, 17)), (Code::UnknownType, (8, 28))],
        ),
        // A loop that has a parameter with an ordinary bound after it: the parameter is no part
        // of the loop, and nothing is out of range.
        (
            "interface A<X, Y> {\n    type P\n}\ninterface B<X> {\n    type Q\n}\nfn f<T: A<S.Q, R.Item>, S: B<T.P>, R: Source>(t: T, s: S, r: R) {}".to_string(),
            vec![(Code::UnknownType, (7, 13)), (Code::UnknownType, (7, 32))],
        ),
        // Two bounds that inherit the same `Item`, and no bound of `Source` beside them: each gives
        // `Item` only through the other, so they wait for each other.
        (
            "interface First<X>: Source {}\ninterface Second<X>: Source {}\nfn f<S: First<S.Item> & Second<S.Item>>(_s: S) {}".to_string(),
            vec![(Code::UnknownType, (3, 17)), (Code::UnknownType, (3, 34))],
        ),
        // A bound waits for the bound of its own interface in no way, and for the other bounds of
        // the parameter that declare the name: `T: Add2<T.Item>` goes on once `Src2` is in scope,
        // and the bound of `U` waits for both, so it meets the two declarations.
        (
            "interface Add2<X> {\n    type Item\n}\ninterface Src2<X> {\n    type Item\n}\nfn f<T: Add2<T.Item> & Src2<R.Item>, U: Convert<T.Item>, R: Source>(_t: T, _u: U, _r: R) {}".to_string(),
            vec![(Code::Ambiguous, (7, 51))],
        ),
        // The other bounds of the parameter that name the interface are other instantiations of
        // it: the type is ambiguous when one is not the bound that writes it, whatever the order.
        (
            "interface Holds2<Y> {\n    type Item\n}\nfn f<S: Holds2<i64> & Holds2<S.Item> & Holds2<T.Item>, T: Source>(_s: S, _t: T) {}".to_string(),
            vec![(Code::Ambiguous, (4, 32))],
        ),
        (
            "interface Holds2<Y> {\n    type Item\n}\nfn f<S: Holds2<T.Item> & Holds2<S.Item> & Holds2<i64>, T: Source>(_s: S, _t: T) {}".to_string(),
            vec![(Code::Ambiguous, (4, 35))],
        ),
        // A bound that writes the wrong number of arguments is reported once, at its name.
        (
            "interface Holds2<Y> {\n    type Item\n}\nfn f<S: Holds2<S.Item, i64>>(_s: S) {}".to_string(),
            vec![(Code::TypeArguments, (4, 15))],
        ),
        // Two bounds that write the type, and so wait for each other.
        (
            "interface Holds2<Y> {\n    type Item\n}\nfn f<S: Holds2<S.Item> & Holds2<S.Item> & Holds2<i64>>(_s: S) {}".to_string(),
            vec![(Code::UnknownType, (4, 18)), (Code::UnknownType, (4, 35))],
        ),
        // A bound that names no interface declares nothing: a class, a type parameter and an
        // optional. The parameter it bounds has no associated type, and that is all.
        (
            "fn f<S: Source, T: Wrap<S.Item>, U: Convert<T.Out>>(_s: S, _t: T, _u: U) {}".to_string(),
            vec![(Code::NotAnInterface, (1, 20)), (Code::UnknownType, (1, 47))],
        ),
        (
            "fn f<S: Source, T, U: T<S.Item>, V: Convert<U.Out>>(_s: S, _t: T, _u: U, _v: V) {}".to_string(),
            vec![
                (Code::NotAnInterface, (1, 23)),
                (Code::TypeArguments, (1, 24)),
                (Code::UnknownType, (1, 47)),
            ],
        ),
        (
            "fn f<S: Source, U: [Convert<S.Item>], V: Convert<U.Out>>(_s: S, _u: U, _v: V) {}".to_string(),
            vec![(Code::NotAnInterface, (1, 20)), (Code::UnknownType, (1, 52))],
        ),
        // Two bounds of one parameter declare the name that another bound writes.
        (
            "interface Holds2<Y> {\n    type Item\n}\ninterface Holds3<Y> {\n    type Item\n}\nfn f<U: Convert<S.Item>, S: Holds2<R.Item> & Holds3<R.Item>, R: Source>(_u: U, _s: S, _r: R) {}".to_string(),
            vec![(Code::Ambiguous, (7, 19))],
        ),
        (
            "fn f<S: Source, U: Convert<S.Item>?, V: Convert<U.Out>>(_s: S, _u: U, _v: V) {}".to_string(),
            vec![(Code::NotAnInterface, (1, 20)), (Code::UnknownType, (1, 51))],
        ),
        (
            "interface GSource<T> {\n    type Item\n}\nextend<U: Marker> Wrap<U>: GSource<i64> {\n    type Item = i64\n}\nfn gpass<S: GSource<T>, T>(s: S, x: S.Item, y: T) {}\nfn t() {\n    let node = Node(id: 1)\n    gpass(Wrap(value: node), \"x\", 5)\n}".to_string(),
            vec![(Code::NotConform, (10, 11))],
        ),
    ] {
        codes(&with_sources(&row), &expected);
    }
}

/// Which bounds of a parameter declare a name does not depend on the order of the parameters: a
/// bound that writes `S.Item` waits for each bound of `S` that declares `Item`, those it names
/// before it and after it. `Source` and `Add<T.Item>` both declare `Item`, so the bound that
/// writes `S.Item` meets two declarations whichever parameter comes first (E0156), and a
/// parameter between them gives the same answer.
#[test]
fn a_bound_should_meet_every_declaration_of_a_name_whatever_the_order_of_the_parameters() {
    let add = "interface Add<X> {\n    type Item\n}\ninterface Use<X> {}\n";
    for (params, at) in [
        (
            "U: Use<S.Item>, S: Source & Add<T.Item>, T: Source",
            (5, 15),
        ),
        (
            "S: Source & Add<T.Item>, U: Use<S.Item>, T: Source",
            (5, 40),
        ),
        (
            "T: Source, S: Source & Add<T.Item>, U: Use<S.Item>",
            (5, 51),
        ),
        (
            "U: Use<S.Item>, T: Source, S: Add<T.Item> & Source",
            (5, 15),
        ),
    ] {
        let row = format!("{add}fn f<{params}>(_u: U, _s: S, _t: T) {{}}");
        codes(&with_sources(&row), &[(Code::Ambiguous, at)]);
    }
}

/// A bound that writes an associated type and gives more than 256 interfaces is reported at the
/// bound, once, whether another bound of the parameter has read the closure before it or not.
#[test]
fn a_bound_that_writes_an_associated_type_should_give_at_most_the_limit_of_interfaces() {
    let big = "interface Big<X>: C257 {}\n".to_string() + &ladder("C", 257);
    for (row, at) in [
        // The bound of `U` reads the closure of `S` in the wave that holds `Big`.
        (
            "fn f<U: Convert<S.Item>, S: Source & Big<T.Item>, T: Source>(_u: U, _s: S, _t: T) {}",
            (1, 38),
        ),
        // Nothing reads the closure before `Big` is added.
        (
            "fn f<S: Source & Big<T.Item>, T: Source>(_s: S, _t: T) {}",
            (1, 18),
        ),
    ] {
        let src = format!("{row}\n{big}");
        codes(&with_sources(&src), &[(Code::BoundTooWide, at)]);
    }
}

/// A parameter whose closure went past the limit when a bound was added is unknown for the bounds
/// that resolve after it: a bound of a later wave that writes a type of it is not reported again.
/// The type `Key` is declared at the end of the chain that the limit cuts. A chain of 256 holds
/// `Key` at its 256th interface; one entry more is past the limit. The bound of `W` waits for
/// `Big` in both, so it meets a parameter that is unknown, and is not reported.
#[test]
fn a_parameter_that_went_past_the_limit_should_stay_unknown_for_later_bounds() {
    for key_at in [3, 2] {
        let mut chain = String::new();
        for k in 1..=257 {
            let body = if k == key_at { "\n    type Key\n" } else { "" };
            let from = if k == 1 {
                String::new()
            } else {
                format!(": C{}", k - 1)
            };
            writeln!(chain, "interface C{k}{from} {{{body}}}")
                .expect("writing to a `String` should succeed");
        }
        let row = format!(
            "interface Big<X>: C257 {{}}\n{chain}fn f<V: Convert<S.Item>, S: Source & Big<R.Item>, W: Convert<S.Key>, R: Source>(_v: V, _s: S, _w: W, _r: R) {{}}"
        );
        codes(&with_sources(&row), &[(Code::BoundTooWide, (261, 38))]);
    }
}

/// A bound that writes an associated type of its own parameter finds the closure of the parameter
/// before it joins it, so the closure grows by the bound's, and passes the limit there. It is
/// reported once, at the bound, with the bound before the others, after them, and in a method.
#[test]
fn a_bound_that_writes_its_own_parameters_type_should_be_reported_past_the_limit() {
    let mut chain = String::new();
    for k in 1..=257 {
        let from = if k == 1 {
            String::new()
        } else {
            format!(": C{}", k - 1)
        };
        writeln!(chain, "interface C{k}{from} {{}}").expect("writing to a `String` should succeed");
    }
    for (function, at) in [
        ("fn f<S: Source & Big<S.Item>>(_s: S) {}", (259, 18)),
        ("fn f<S: Big<S.Item> & Source>(_s: S) {}", (259, 9)),
        (
            "class H {\n    fn f<S: Source & Big<S.Item>>(self, _s: S) {}\n}",
            (260, 22),
        ),
    ] {
        let row = format!("interface Big<X>: C257 {{}}\n{chain}{function}");
        codes(&with_sources(&row), &[(Code::BoundTooWide, at)]);
    }
}

/// A cycle through an associated type is a cycle of the type it stands for: its edge weighs how
/// much more deeply the choices of its conformances nest the types that stand for their
/// parameters than their targets do. A choice that grows by 1 closes a cycle with a call that
/// passes the associated type on; one that grows by 0 or less does not.
#[test]
fn an_associated_type_should_grow_a_cycle_by_what_its_choices_add() {
    let deep = "interface Deep {\n    type Item: Deep\n    fn next(self) -> Item?\n}\nclass Chain<T> {\n    value: T\n}\nfn drain_all<S: Deep>(s: S) {\n    if let x = s.next() {\n        drain_all(x)\n    }\n}\n";
    for (choice, expected) in [
        // One more level: a cycle, labeled at the call that passes the item on.
        ("Chain<Chain<T>>", vec![(Code::InstantiationCycle, (10, 9))]),
        ("Chain<T>", vec![]),
        ("T", vec![]),
        // Two more levels, through the same.
        (
            "Chain<Chain<Chain<T>>>",
            vec![(Code::InstantiationCycle, (10, 9))],
        ),
    ] {
        let row = format!(
            "{deep}extend<T: Deep> Chain<T>: Deep {{\n    type Item = {choice}\n    fn next(self) -> Item? {{ None }}\n}}"
        );
        codes(&with_sources(&row), &expected);
    }
}

/// A block over a generic enum counts the nesting of its choices as a block over a class does.
#[test]
fn an_enum_target_should_grow_a_cycle_by_what_its_choices_add() {
    let deep = "interface Deep {\n    type Item: Deep\n    fn next(self) -> Item?\n}\nenum Chain<T> {\n    One(T)\n}\nfn drain_all<S: Deep>(s: S) {\n    if let x = s.next() {\n        drain_all(x)\n    }\n}\n";
    for (choice, expected) in [
        ("Chain<Chain<T>>", vec![(Code::InstantiationCycle, (10, 9))]),
        ("Chain<T>", vec![]),
    ] {
        let row = format!(
            "{deep}extend<T: Deep> Chain<T>: Deep {{\n    type Item = {choice}\n    fn next(self) -> Item? {{ None }}\n}}"
        );
        codes(&with_sources(&row), &expected);
    }
}

/// A growth that goes around a cycle of positive weight is unbounded, and so is the growth of
/// each associated type that reaches it: a call that passes either on closes a cycle.
#[test]
fn an_unbounded_growth_should_close_a_cycle_through_each_type_that_reaches_it() {
    let ring = "interface Ring {\n    type A: Ring\n    type B: Ring\n    type C: Ring\n    fn a(self) -> A?\n    fn b(self) -> B?\n    fn c(self) -> C?\n}\nclass Duo<X, Y> {\n    x: X\n    y: Y\n}\nextend<T: Ring, U: Ring> Duo<T, U>: Ring {\n    type A = Duo<Duo<T.B, U>, U>\n    type B = Duo<T.A, U>\n    type C = Duo<T.B, U>\n    fn a(self) -> A? { None }\n    fn b(self) -> B? { None }\n    fn c(self) -> C? { None }\n}\n";
    for method in ["a", "b", "c"] {
        let row = format!(
            "{ring}fn go<R: Ring>(r: R) {{\n    if let x = r.{method}() {{\n        go(x)\n    }}\n}}"
        );
        codes(&with_sources(&row), &[(Code::InstantiationCycle, (23, 9))]);
    }
}

/// An associated type whose bound gives more than 256 interfaces is reported where the bound is
/// declared, once, and is unknown: it conforms to everything and no method of it is missing.
#[test]
fn an_associated_type_that_passes_the_limit_should_be_reported_once_and_unknown() {
    let wide = ladder("C", 257) + "fn main() {}";
    for (row, expected) in [
        // Declared and never used.
        ("interface W {\n    type Item: C257\n}", vec![(2, 16)]),
        // Used as a bound, as the base of a method call, and as a nested base.
        (
            "interface W {\n    type Item: C257\n}\nfn f<S: W>(x: S.Item) {\n    need(x)\n    x.nothing()\n    let y: S.Item.Item = x\n}\nfn need<T: C1>(_y: T) {}",
            vec![(2, 16)],
        ),
    ] {
        let expected: Vec<_> = expected
            .into_iter()
            .map(|at| (Code::BoundTooWide, at))
            .collect();
        codes(&format!("{row}\n{wide}"), &expected);
    }
}

/// The growth belongs to the associated type that the choice is for, not to the first of its
/// interface: of two types, the one that grows closes a cycle and the other does not.
#[test]
fn each_associated_type_of_an_interface_should_have_its_own_growth() {
    let pair = "interface Two {\n    type Grows: Two\n    type Fixed: Two\n    fn grows(self) -> Grows?\n    fn fixed(self) -> Fixed?\n}\nclass Chain<T> {\n    value: T\n}\nclass Stop {}\nextend Stop: Two {\n    type Grows = Stop\n    type Fixed = Stop\n    fn grows(self) -> Grows? { None }\n    fn fixed(self) -> Fixed? { None }\n}\nextend<T: Two> Chain<T>: Two {\n    type Grows = Chain<Chain<T>>\n    type Fixed = Stop\n    fn grows(self) -> Grows? { None }\n    fn fixed(self) -> Fixed? { None }\n}\n";
    for (method, expected) in [
        ("grows", vec![(Code::InstantiationCycle, (25, 9))]),
        ("fixed", vec![]),
    ] {
        let row = format!(
            "{pair}fn go<R: Two>(r: R) {{\n    if let x = r.{method}() {{\n        go(x)\n    }}\n}}"
        );
        codes(&with_sources(&row), &expected);
    }
}

/// The growth of an associated type is the growth of every associated type that its choice names
/// in a chain, `T.A.A`, added up; and a cycle of positive weight among the names is unbounded
/// though no direct parameter starts it, as in `W<W<T.A>>`. Each closes a cycle with a call that
/// passes the associated type on. A chain that adds up to 0 or less, and a cycle of weight 0, do
/// not.
#[test]
fn a_growth_should_add_up_a_chain_of_names_and_be_unbounded_on_a_cycle_that_nothing_starts() {
    for (call, a, b, cycle) in [
        // `A` grows by 1, so `T.A.A` adds 2 and the target takes 1 back.
        ("b", "W<W<T>>", "T.A.A", true),
        // The same for a chain of three names.
        ("b", "W<W<T>>", "T.A.A.A", true),
        // `A` grows by 0, and by 1 with a chain of one name: 1 - 1 + 0 and 1 - 1 + 1 - 1.
        ("b", "W<T>", "T.A.A", false),
        ("b", "W<W<T>>", "T.A", false),
        // A name of itself under a choice that nests it deeper: no end, though nothing starts it.
        ("a", "W<W<T.A>>", "T", true),
        // The same name at the same depth as the target: a cycle of weight 0.
        ("a", "W<T.A>", "T", false),
    ] {
        let row = format!(
            "interface I {{\n    type A: I\n    type B: I\n    fn a(self) -> A\n    fn b(self) -> B\n}}\nclass Leaf: I {{\n    type A = Leaf\n    type B = Leaf\n    fn a(self) -> A {{ self }}\n    fn b(self) -> B {{ self }}\n}}\nclass W<T> {{\n    value: T\n}}\nextend<T: I> W<T>: I {{\n    type A = {a}\n    type B = {b}\n    fn a(self) -> A {{ panic(\"a\") }}\n    fn b(self) -> B {{ panic(\"b\") }}\n}}\nfn drain<S: I>(s: S, run: bool) {{\n    if run {{\n        drain(s.{call}(), run)\n    }}\n}}\nfn main() {{\n    drain(W(value: W(value: Leaf())), false)\n}}\n"
        );
        let expected = if cycle {
            vec![(Code::InstantiationCycle, (24, 9))]
        } else {
            vec![]
        };
        codes(&row, &expected);
    }
}

/// An instantiation in the arguments of an associated type's interface is an instantiation of the
/// type that holds it: `Nest<[T], X>` inside `X.Out` closes a cycle through `Nest`. The bound of
/// `X` cannot hold, which is reported too.
#[test]
fn a_class_in_the_arguments_of_an_associated_type_should_close_a_cycle() {
    let row = "interface Conv<A> {\n    type Out\n    fn conv(self) -> Out\n}\nclass Nest<T, X: Conv<Nest<[T], X>>> {\n    f: X.Out\n}";
    codes(
        &with_sources(row),
        &[
            (Code::NotConform, (5, 33)),
            (Code::InstantiationCycle, (6, 8)),
        ],
    );
}

/// What a choice that names another associated type adds is that type's growth plus the depth at
/// which the choice names the parameter, less the depth of the parameter in the target. The call
/// that passes the item on shrinks by one through the conformance, so a growth of 1 closes no
/// cycle and one of 2 does.
#[test]
fn a_choice_should_add_the_growth_of_the_associated_type_it_names() {
    let two = "interface Two {\n    type A: Two\n    type B: Two\n    fn a(self) -> A?\n    fn poke(self)\n}\nclass Chain<T> {\n    value: T\n}\nfn go<S: Two>(s: S) {\n    if let x = s.a() {\n        x.poke()\n    }\n}\n";
    // `A` names `B` at depth `edge` + 1 under the target's 1, and `B` grows by `direct`.
    for (a, b, cycle) in [
        // An edge of 0 and a growth of 1, an edge of 1 and a growth of 0: 1 in all.
        ("Chain<T.B>", "Chain<Chain<T>>", false),
        ("Chain<Chain<T.B>>", "Chain<T>", false),
        // An edge of 0 and a growth of 2, an edge of 2 and a growth of 0: 2 in all.
        ("Chain<T.B>", "Chain<Chain<Chain<T>>>", true),
        ("Chain<Chain<Chain<T.B>>>", "Chain<T>", true),
    ] {
        let row = format!(
            "{two}extend<T: Two> Chain<T>: Two {{\n    type A = {a}\n    type B = {b}\n    fn a(self) -> A? {{ None }}\n    fn poke(self) {{\n        go(self.value)\n    }}\n}}"
        );
        let expected = if cycle {
            vec![(Code::InstantiationCycle, (12, 9))]
        } else {
            vec![]
        };
        codes(&with_sources(&row), &expected);
    }
}

/// A method of a generic class has the class's parameters bound by its receiver, and checks the
/// bound of its own parameter, which an associated type is written through, as soon as it is
/// bound: one report, and none for the argument that waits. A parameter that no associated type
/// is written through is checked with the others, and the argument that waits is still checked.
#[test]
fn a_method_should_check_the_bound_of_an_associated_types_parameter_first() {
    let row = "extend<T: Marker> Wrap<T>: Source {\n    type Item = Tag\n    fn next(self) -> Item? { None }\n}\nclass Holder<S: Source> {\n    held: S\n    fn put<U: Source>(self, u: U, x: U.Item) -> i64 { 1 }\n    fn both<U: Source, V: Marker>(self, u: U, x: U.Item, v: V) -> i64 { 1 }\n    fn later<A, U: Source>(self, a: A, u: U, x: U.Item) -> i64 { 1 }\n}\nfn t() {\n    let node = Node(id: 1)\n    let h = Holder(held: Countdown(n: 1))\n    let a = h.put(Wrap(value: node), \"x\")\n    let b = h.both(Countdown(n: 1), \"y\", node)\n    let c = h.later(1, Wrap(value: node), \"z\")\n}";
    codes(
        &with_sources(row),
        &[
            (Code::NotConform, (14, 19)),
            (Code::Mismatch, (15, 37)),
            (Code::NotConform, (15, 42)),
            (Code::NotConform, (16, 24)),
        ],
    );
}
