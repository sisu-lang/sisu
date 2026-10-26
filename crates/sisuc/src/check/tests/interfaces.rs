//! Interface declarations: their requirements and inheritance, `Self`, the names that may stand
//! where an interface or a type belongs, and the closure of a bound's inherited interfaces.

use std::fmt::Write as _;

use super::*;
use crate::check::interface::Bound;

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
    // Several bounds give the interfaces they share once.
    assert_eq!(
        closure(
            src,
            &[("A", &[strs]), ("B", &[strs]), ("A", &[strs])],
            receiver
        ),
        [("A", vec![strs]), ("B", vec![strs]), ("C", vec![strs])]
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
        // An argument of a generic call waits for the other, with the method in a block.
        "class C {\n    x: i64\n}\nextend C {\n    fn make<U>(self) -> [U] { [] }\n}\nfn take<T>(x: T, y: T) -> T { x }\nfn main() {\n    let c = C(x: 1)\n    print(take(c.make(), [1]).len())\n}",
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
        // An associated type waits for a later task.
        (
            "interface Plain {}\nclass Circle {\n    r: f64\n}\nextend Circle: Plain {\n    type Item = i64\n}",
            vec![(Code::UnknownType, (6, 10))],
        ),
        // The methods of a block that failed for its interface, its generic target or its
        // generic interface are not checked.
        (
            "class Circle {\n    r: f64\n}\nextend Circle: Missing {\n    fn grow(self) {\n        let b: bool = 1\n    }\n}",
            vec![(Code::UnknownType, (4, 16))],
        ),
        (
            "class Pile<T> {\n    items: [T]\n}\nextend<T> Pile<T> {\n    fn f(self) {\n        let b: bool = 1\n    }\n}",
            vec![(Code::UnknownType, (4, 11))],
        ),
        (
            "interface Convert<T> {\n    fn convert(self) -> T\n}\nclass Circle {\n    r: f64\n}\nextend Circle: Convert<i64> {\n    fn convert(self) -> i64 {\n        let b: bool = 1\n        1\n    }\n}",
            vec![(Code::UnknownType, (7, 16))],
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

/// A type with parameters and an interface that takes some wait for a later task, and report the
/// name they cannot handle yet.
#[test]
fn generic_conformances_should_report_the_name_they_cannot_take_yet() {
    for (row, expected) in [
        (
            "class Pile<T> {\n    items: [T]\n}\nextend<T> Pile<T> {\n}",
            vec![(Code::UnknownType, (4, 11))],
        ),
        (
            "class Pile<T> {\n    items: [T]\n}\nextend Pile<i64> {\n}",
            vec![(Code::UnknownType, (4, 8))],
        ),
        (
            "enum Tree<T> {\n    Leaf\n}\nextend<T> Tree<T> {\n}",
            vec![(Code::UnknownType, (4, 11))],
        ),
        (
            "class Pile<T>: Same {\n    items: [T]\n}",
            vec![(Code::UnknownType, (1, 16))],
        ),
        (
            "enum Tree<T>: Same {\n    Leaf\n}",
            vec![(Code::UnknownType, (1, 15))],
        ),
        (
            "interface Convert<T> {\n    fn convert(self) -> T\n}\nclass Circle {\n    r: f64\n}\nextend Circle: Convert<i64> {\n    fn convert(self) -> i64 { 1 }\n}",
            vec![(Code::UnknownType, (7, 16))],
        ),
        (
            "interface Convert<T> {\n    fn convert(self) -> T\n}\nclass Circle: Convert<i64> {\n    fn convert(self) -> i64 { 1 }\n}",
            vec![(Code::UnknownType, (4, 15))],
        ),
    ] {
        codes(&with_shapes(&with_main(row)), &expected);
    }
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
        // A target with arguments waits for Task 5, and reports that alone.
        (
            "class Pile<V> {\n    items: [V]\n}\nextend<T> Pile<i64> {\n}",
            vec![(Code::UnknownType, (7, 11))],
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

/// An interface inherited with arguments waits for Task 6 as a listed one does, at the listed
/// interface, once.
#[test]
fn an_inherited_generic_interface_should_report_the_stopgap_at_the_listed_interface() {
    for (row, expected) in [
        (
            "interface G<T> {}\ninterface I: G<i64> {}\nclass C: I {}",
            vec![(Code::UnknownType, (3, 10))],
        ),
        // Two entries of one interface, one report.
        (
            "interface G<T> {}\ninterface I: G<i64>, G<f64> {}\nclass C: I {}",
            vec![(Code::UnknownType, (3, 10))],
        ),
        // Two listed interfaces that inherit it: the first listed one has it, in the one walk.
        (
            "interface G<T> {}\ninterface I: G<i64> {}\ninterface J: G<i64> {}\nclass C: I, J {}",
            vec![(Code::UnknownType, (4, 10))],
        ),
        // Another argument is another entry, reached through another listed interface.
        (
            "interface G<T> {}\ninterface I: G<i64> {}\ninterface J: G<f64> {}\nclass C: I, J {}",
            vec![(Code::UnknownType, (4, 10)), (Code::UnknownType, (4, 13))],
        ),
        // The block path reports the missing conformance, as it does.
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
    // The interface itself, then a step for each of its requirements.
    for distinct in [true, false] {
        assert_eq!(steps(8, distinct), 1 + 8);
        assert_eq!(steps(32, distinct), 1 + 32);
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
    src += "interface C1 {\n    fn deep(self) -> i64\n}\n";
    for k in 2..=n {
        writeln!(src, "interface C{k}: C{} {{}}", k - 1)
            .expect("writing to a `String` should succeed");
    }
    src + "fn main() {}"
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
    // Bounds that are many, with no inheritance, pass the limit too.
    let names: Vec<String> = (1..=257).map(|k| format!("C{k}")).collect();
    let mut src = format!(
        "fn _use<T: {}>(x: T) {{\n    need(x)\n}}\nfn need<T: C1>(_y: T) {{}}\n",
        names.join(" & ")
    );
    for name in &names {
        writeln!(src, "interface {name} {{}}").expect("writing to a `String` should succeed");
    }
    // The 257th bound is the one that passes the limit.
    let col = 1 + "fn _use<T: ".len() + names[..256].join(" & ").len() + " & ".len();
    codes(&(src + "fn main() {}"), &[(Code::BoundTooWide, (1, col))]);
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
