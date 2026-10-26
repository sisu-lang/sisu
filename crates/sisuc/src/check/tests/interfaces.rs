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
        "interface Both {\n    fn both<U: Named & Same>(self, x: U)\n}\nclass Crate: Both {\n    fn both<V: Same & Named>(self, x: V) {}\n}\nfn main() {\n    Crate().both(1)\n}",
        // A body method beside a block's method of the same name: the call takes the body's.
        "class Circle {\n    r: f64\n    fn name(self) -> string { \"body\" }\n}\nextend Circle: Label {\n    fn name(self) -> string { \"label\" }\n}\nfn main() {\n    let c = Circle(r: 1.0)\n    let s = c.name()\n    print(s)\n    print(c.r)\n}",
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
