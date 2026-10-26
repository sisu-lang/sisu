//! Interface declarations: their requirements and inheritance, `Self`, the names that may stand
//! where an interface or a type belongs, and the closure of a bound's inherited interfaces.

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

/// `Self` outside a class, an enum and an interface, a cycle of inheritance, and an inherited
/// argument built from a type parameter.
#[test]
fn self_outside_and_inheritance_errors_should_be_reported() {
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
        // A requirement that an inherited interface has.
        (
            with_decls("interface Solid: Named {\n    fn name(self) -> string\n}"),
            vec![(Code::DefinedTwice, (2, 8))],
        ),
        (
            with_decls("interface Solid: Named {\n    fn area(self) -> f64\n    fn name(self) -> string\n}"),
            vec![(Code::DefinedTwice, (3, 8))],
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
            }
        })
        .collect();
    let mut out = Vec::new();
    checker.interfaces_of(receiver, &bounds, &mut out);
    out.into_iter()
        .map(|(id, args)| {
            (
                checker.interfaces[id.0].name.name.as_str(),
                args.get().to_vec(),
            )
        })
        .collect()
}
