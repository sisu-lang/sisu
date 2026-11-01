//! Interface values: `any I` is written after an interface, converts from an object, conforms to
//! its interface and the interfaces that it inherits, and is called through its interface.

use std::fmt::Write as _;

use super::*;

/// The interfaces and classes that the rows name. A row's program follows it, so a position in a
/// row is counted from the end of this text: `lines` gives the number of its lines.
const DECLS: &str = "interface Shape {\n    fn area(self) -> f64\n}\ninterface Named {\n    fn name(self) -> string\n}\ninterface Solid: Named {\n    fn volume(self) -> f64\n}\ninterface Convert<T> {\n    fn convert(self) -> T\n}\ninterface Observer: AnyObject {\n    fn notify(self, event: string)\n}\nclass Circle: Shape {\n    r: f64\n    fn area(self) -> f64 { self.r }\n}\nclass Rect: Shape {\n    w: f64\n    fn area(self) -> f64 { self.w }\n}\nclass Cube: Solid {\n    s: f64\n    fn name(self) -> string { \"cube\" }\n    fn volume(self) -> f64 { self.s }\n}\nclass Kelvin {\n    k: f64\n}\nclass Celsius: Convert<Kelvin> {\n    c: f64\n    fn convert(self) -> Kelvin { Kelvin(k: self.c) }\n}\nclass Button: Observer {\n    fn notify(self, event: string) {}\n}\n";

/// The number of lines of `DECLS`.
const LINES: usize = 38;

/// `codes` for a row after `DECLS`, whose lines start at line 1.
fn codes_after(row: &str, src: &str, expected: &[(Code, (usize, usize))]) {
    let shifted: Vec<_> = expected
        .iter()
        .map(|&(code, (line, col))| (code, (line + LINES, col)))
        .collect();
    codes_row(row, &format!("{DECLS}{src}"), &shifted);
}

/// A row of `codes_after`: its name, its program, and the codes with their positions.
type Row<'a> = (&'a str, &'a str, Vec<(Code, (usize, usize))>);

/// `codes_after` for each row.
fn rows_after(rows: &[Row<'_>]) {
    for (row, src, expected) in rows {
        codes_after(row, src, expected);
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one row for each place a value converts"
)]
fn any_values_should_check_clean_where_a_value_converts() {
    for (row, src) in [
        (
            "a let with a type",
            "fn main() {\n    let s: any Shape = Circle(r: 1.0)\n}",
        ),
        (
            "an array literal of two classes",
            "fn main() {\n    let xs: [any Shape] = [Circle(r: 1.0), Rect(w: 2.0)]\n}",
        ),
        (
            "an argument",
            "fn draw(s: any Shape) {}\nfn main() {\n    draw(Circle(r: 1.0))\n}",
        ),
        (
            "a return",
            "fn make() -> any Shape {\n    return Circle(r: 1.0)\n}\nfn main() {}",
        ),
        (
            "a tail",
            "fn make() -> any Shape {\n    Circle(r: 1.0)\n}\nfn main() {}",
        ),
        (
            "a field initializer",
            "class Holder {\n    s: any Shape\n}\nfn main() {\n    let h = Holder(s: Circle(r: 1.0))\n}",
        ),
        (
            "an assignment",
            "fn main() {\n    let mut s: any Shape = Circle(r: 1.0)\n    s = Rect(w: 1.0)\n}",
        ),
        (
            "a class where an optional interface value is expected",
            "fn main() {\n    let s: (any Shape)? = Circle(r: 1.0)\n    let n: (any Shape)? = None\n}",
        ),
        (
            "an interface value of an inheriting interface",
            "fn name_of(n: any Named) {}\nfn main() {\n    let s: any Solid = Cube(s: 1.0)\n    name_of(s)\n}",
        ),
        (
            "an interface value where an `any Display` is expected",
            "fn show(d: any Display) {}\nfn main() {\n    let s: any Shape = Circle(r: 1.0)\n    show(s)\n    show(Circle(r: 1.0))\n}",
        ),
        (
            "an array of interface values for a bounded parameter",
            "fn total<T: Shape>(xs: [T]) -> f64 { 0.0 }\nfn main() {\n    let xs: [any Shape] = [Circle(r: 1.0)]\n    let t = total(xs)\n}",
        ),
        (
            "two interface values of a class-only interface for `AnyObject`",
            "fn same<T: AnyObject>(a: T, b: T) -> bool { a is b }\nfn main() {\n    let a: any Observer = Button()\n    let b: any Observer = Button()\n    let t = same(a, b)\n}",
        ),
        (
            "a call and an interpolation on an interface value",
            "fn main() {\n    let s: any Shape = Circle(r: 1.0)\n    let a = s.area()\n    let t = \"{s}\"\n}",
        ),
        (
            "a call of a generic interface's requirement",
            "fn main() {\n    let c: any Convert<Kelvin> = Celsius(c: 1.0)\n    let k = c.convert()\n}",
        ),
        (
            "`is` on two interface values, and on an interface value and an object",
            "fn main() {\n    let a: any Observer = Button()\n    let b: any Observer = Button()\n    let t = a is b\n    let button = Button()\n    let u = a is button\n    let v = button is a\n}",
        ),
        (
            "an interface value in a generic class",
            "class Stack<T> {\n    items: [T]\n}\nfn main() {\n    let s: Stack<any Shape> = Stack(items: [])\n}",
        ),
        (
            "an alias of an array of interface values",
            "type Shapes = [any Shape]\nfn main() {\n    let xs: Shapes = [Circle(r: 1.0)]\n}",
        ),
        (
            "a result of inference that converts",
            "fn id<T>(x: T) -> T { x }\nfn main() {\n    let s: any Shape = id(Circle(r: 1.0))\n}",
        ),
        (
            "a result of a bounded parameter that the interface value lacks",
            "class Point: Eq, Shape {\n    x: f64\n    fn eq(self, other: Self) -> bool { self.x == other.x }\n    fn area(self) -> f64 { self.x }\n}\nfn keep<T: Eq>(x: T) -> T { x }\nfn main() {\n    let p = Point(x: 1.0)\n    let s: any Shape = keep(p)\n}",
        ),
        (
            "a result of inference that converts to an optional interface value",
            "fn id<T>(x: T) -> T { x }\nfn main() {\n    let s: (any Shape)? = id(Circle(r: 1.0))\n}",
        ),
        (
            "a result of an element type",
            "fn head<T>(xs: [T]) -> T { xs[0] }\nfn main() {\n    let cs = [Circle(r: 1.0)]\n    let s: any Shape = head(cs)\n}",
        ),
        (
            "a result of an argument and an optional argument",
            "fn pick<T>(x: T?, d: T) -> T { d }\nfn main() {\n    let c = Circle(r: 1.0)\n    let s: any Shape = pick(Some(c), c)\n}",
        ),
        (
            "an explicit type argument that is an interface value",
            "fn maybe<T>(x: T?) -> T? { x }\nfn main() {\n    let c = Circle(r: 1.0)\n    let _ = maybe<any Shape>(c)\n}",
        ),
        (
            "the arms of an `if`",
            "fn pick(f: bool) -> any Shape {\n    if f { Circle(r: 1.0) } else { Rect(w: 1.0) }\n}\nfn main() {}",
        ),
        (
            "the value of a `break`",
            "fn first() -> any Shape {\n    loop {\n        break Circle(r: 1.0)\n    }\n}\nfn main() {}",
        ),
        (
            "the default of `??`",
            "fn pick(o: (any Shape)?) -> any Shape {\n    o ?? Circle(r: 1.0)\n}\nfn main() {}",
        ),
        (
            "the payload of a variant",
            "enum Slot {\n    Full(any Shape)\n    Empty\n}\nfn main() {\n    let s = Slot.Full(Circle(r: 1.0))\n}",
        ),
        (
            "the argument of `Some`",
            "fn main() {\n    let s: (any Shape)? = Some(Circle(r: 1.0))\n}",
        ),
    ] {
        clean_row(row, &format!("{DECLS}{src}"));
    }
}

/// An interface that has a generic requirement, an associated type, or a requirement that needs
/// `Self` beside the receiver cannot be `any`: one report where `any` is written, at the interface.
#[test]
fn interfaces_should_not_be_any_when_a_requirement_needs_the_type() {
    let items = "interface Mapper {\n    fn map<U>(self, u: U) -> U\n}\ninterface Holds {\n    type Item\n}\ninterface Dup {\n    fn dup(self) -> Self\n}\ninterface Dups {\n    fn dups(self) -> [Self]\n}\ninterface Sorted: Ord {}\ninterface Deep: Sorted {}\ninterface Opt {\n    fn opt(self) -> Self?\n}\ninterface Many {\n    fn many(self, xs: [Self])\n}\nclass Cell<T> {\n    v: T\n}\ninterface Wrap {\n    fn wrap(self) -> Cell<Self>\n}\nenum Pick<T> {\n    One(T)\n    Nil\n}\ninterface Choose {\n    fn choose(self) -> Pick<Self>\n}\ninterface Back {\n    fn back(self) -> any Convert<Self>\n}\n";
    // `let` is the second line of `main`, after `items`.
    let line = items.lines().count() + 2;
    for (row, ty) in [
        ("`Eq` takes another `Self`", "Eq"),
        ("`Ord` takes another `Self`", "Ord"),
        ("a generic requirement", "Mapper"),
        ("an associated type", "Holds"),
        ("a requirement that returns `Self`", "Dup"),
        ("a requirement that returns `[Self]`", "Dups"),
        ("an inherited requirement that takes `Self`", "Sorted"),
        ("an interface that inherits the inheriting one", "Deep"),
        ("a requirement that returns `Self?`", "Opt"),
        ("a requirement that takes `[Self]`", "Many"),
        ("a requirement that returns a class of `Self`", "Wrap"),
        ("a requirement that returns an enum of `Self`", "Choose"),
        (
            "a requirement that returns an `any` value of `Self`",
            "Back",
        ),
    ] {
        let src = format!("{items}fn main() {{\n    let x: any {ty} = 5\n}}");
        codes_after(row, &src, &[(Code::CannotBeAny, (line, 16))]);
    }
    for (row, src) in [
        (
            "`Display`",
            "fn main() {\n    let x: any Display = Circle(r: 1.0)\n}",
        ),
        (
            "`AnyObject`",
            "fn main() {\n    let x: any AnyObject = Button()\n}",
        ),
        (
            "a generic interface with its arguments",
            "fn main() {\n    let x: any Convert<Kelvin> = Celsius(c: 1.0)\n}",
        ),
        (
            "a class-only interface",
            "fn main() {\n    let x: any Observer = Button()\n}",
        ),
    ] {
        clean_row(row, &format!("{DECLS}{src}"));
    }
}

/// An interface written where a type belongs is `InterfaceAsType`, a language interface too.
#[test]
fn an_interface_should_not_be_a_type() {
    rows_after(&[
        (
            "an interface where a type belongs",
            "fn main() {\n    let s: Shape = Circle(r: 1.0)\n}",
            vec![(Code::InterfaceAsType, (2, 12))],
        ),
        (
            "a language interface where a type belongs",
            "fn main() {\n    let e: Eq = 5\n}",
            vec![(Code::InterfaceAsType, (2, 12))],
        ),
        (
            "an interface with one `?`",
            "fn f(_x: Shape?) {}\nfn main() {}",
            vec![(Code::InterfaceAsType, (1, 10))],
        ),
        (
            "an interface with two `?`",
            "fn f(_x: Shape??) {}\nfn main() {}",
            vec![(Code::InterfaceAsType, (1, 10))],
        ),
    ]);
}

/// `any` takes the name of an interface, with the arguments of a generic one.
#[test]
fn any_should_take_the_name_of_an_interface_and_its_arguments() {
    rows_after(&[
        (
            "a class after `any`",
            "fn main() {\n    let s: any Circle = Circle(r: 1.0)\n}",
            vec![(Code::NotAnInterface, (2, 16))],
        ),
        (
            "a built-in type after `any`",
            "fn main() {\n    let s: any i64 = 5\n}",
            vec![(Code::NotAnInterface, (2, 16))],
        ),
        (
            "a type parameter after `any`",
            "fn f<T>(x: T) {\n    let s: any T = x\n}\nfn main() {}",
            vec![(Code::NotAnInterface, (2, 16))],
        ),
        (
            "a generic interface without its arguments",
            "fn main() {\n    let s: any Convert = Celsius(c: 1.0)\n}",
            vec![(Code::TypeArguments, (2, 16))],
        ),
        (
            "an unknown name after `any`",
            "fn main() {\n    let s: any Bogus = 5\n}",
            vec![(Code::UnknownType, (2, 16))],
        ),
    ]);
}

/// A value converts to an `any` type when it conforms and needs no box. Any other is reported.
#[test]
fn a_value_should_convert_to_any_only_where_it_conforms_and_needs_no_box() {
    rows_after(&[
        (
            "a value that does not conform",
            "fn main() {\n    let s: any Shape = 5\n}",
            vec![(Code::NotConform, (2, 24))],
        ),
        (
            "an object that does not conform",
            "fn main() {\n    let s: any Shape = Kelvin(k: 1.0)\n}",
            vec![(Code::NotConform, (2, 24))],
        ),
        (
            "an optional, which does not convert",
            "fn main() {\n    let m: Circle? = None\n    let s: any Shape = m\n}",
            vec![(Code::Mismatch, (3, 24))],
        ),
        (
            "an enum, which waits for the box",
            "enum Num: Shape {\n    One(i64)\n    fn area(self) -> f64 { 1.0 }\n}\nfn main() {\n    let s: any Shape = Num.One(1)\n}",
            vec![(Code::Mismatch, (6, 24))],
        ),
        (
            "a bounded parameter that is not an object, which waits for the box",
            "fn wrap<T: Shape>(x: T) -> any Shape { x }\nfn main() {}",
            vec![(Code::Mismatch, (1, 40))],
        ),
        (
            "an optional of the class where an optional interface value is expected",
            "fn some<T>(x: T) -> T? { Some(x) }\nfn main() {\n    let c = Circle(r: 1.0)\n    let s: (any Shape)? = some(c)\n}",
            vec![(Code::Mismatch, (4, 27))],
        ),
        (
            "a class that does not conform, as the value of a body",
            "fn make() -> any Shape {\n    Kelvin(k: 1.0)\n}\nfn main() {}",
            vec![(Code::NotConform, (2, 5))],
        ),
        (
            "an object that is in error, as the value of a body",
            "fn make() -> any Shape {\n    Circle(r: true)\n}\nfn main() {}",
            vec![(Code::Mismatch, (2, 15))],
        ),
        (
            "an object that is in error, as the value of a `let`",
            "fn main() {\n    let s: any Shape = Circle(r: true)\n}",
            vec![(Code::Mismatch, (2, 34))],
        ),
        (
            "a default of `??` that does not conform",
            "fn pick(o: (any Shape)?) -> any Shape {\n    o ?? Kelvin(k: 1.0)\n}\nfn main() {}",
            vec![(Code::NotConform, (2, 10))],
        ),
    ]);
}

/// `[v; n]` shares one object, so a fill of an object or an interface value is `SameObject`.
#[test]
fn an_array_fill_of_an_object_should_be_reported_for_a_class_or_an_interface_value() {
    rows_after(&[
        (
            "an array fill of an object, which stays the same object",
            "fn main() {\n    let c = Circle(r: 1.0)\n    let xs: [any Shape] = [c; 3]\n}",
            vec![(Code::SameObject, (3, 28))],
        ),
        (
            "an array fill of an interface value that may hold an object",
            "fn f(s: any Shape) {\n    let xs = [s; 3]\n}\nfn main() {}",
            vec![(Code::SameObject, (2, 15))],
        ),
        (
            "an array fill of an interface value that holds an object",
            "fn f(s: any Observer) {\n    let xs = [s; 3]\n}\nfn main() {}",
            vec![(Code::SameObject, (2, 15))],
        ),
    ]);
}

/// An interface value has no `==` and no `<`.
#[test]
fn an_interface_value_should_have_no_equality_or_order() {
    rows_after(&[
        (
            "`==` on an interface value",
            "fn f(a: any Shape, b: any Shape) {\n    let x = a == b\n}\nfn main() {}",
            vec![(Code::NoOperator, (2, 13))],
        ),
        (
            "`==` on an interface value of a class-only interface",
            "fn f(a: any Observer, b: any Observer) {\n    let x = a == b\n}\nfn main() {}",
            vec![(Code::NoOperator, (2, 13))],
        ),
        (
            "`<` on an interface value",
            "fn f(a: any Shape, b: any Shape) {\n    let x = a < b\n}\nfn main() {}",
            vec![(Code::NoOperator, (2, 13))],
        ),
    ]);
}

/// `is` takes objects, or interface values whose interface has `AnyObject`.
#[test]
fn is_should_need_interface_values_that_hold_objects() {
    rows_after(&[
        (
            "`is` on an interface value that may hold anything",
            "fn f(a: any Shape, b: any Shape) {\n    let x = a is b\n}\nfn main() {}",
            vec![(Code::IsNeedsObjects, (2, 13))],
        ),
        (
            "`is` on an interface value of the prelude",
            "fn f(a: any Display, b: any Display) {\n    let x = a is b\n}\nfn main() {}",
            vec![(Code::IsNeedsObjects, (2, 13))],
        ),
    ]);
}

/// An `extend` block cannot extend an interface value.
#[test]
fn an_interface_value_should_not_be_extended() {
    rows_after(&[(
        "`extend` of an interface value",
        "extend any Shape {}\nfn main() {}",
        vec![(Code::CannotExtend, (1, 8))],
    )]);
}

/// A method that the interface lacks is reported on an interface value.
#[test]
fn an_interface_value_should_have_only_the_methods_of_its_interface() {
    rows_after(&[(
        "a method that the interface lacks",
        "fn f(s: any Shape) {\n    s.perimeter()\n}\nfn main() {}",
        vec![(Code::NoMember, (2, 7))],
    )]);
}

/// An argument binds a type parameter before the context does, and the result converts after
/// that: the context never binds a parameter to an `any` type that an argument fixes.
#[test]
fn inference_should_bind_arguments_before_an_any_context() {
    for (row, src, expected) in [
        (
            "two arguments of different classes",
            "fn pair<T>(a: T, b: T) -> T { a }\nfn main() {\n    let c = Circle(r: 1.0)\n    let r = Rect(w: 1.0)\n    let s: any Shape = pair(c, r)\n}",
            vec![(Code::Mismatch, (5, 32))],
        ),
        (
            "a number, which waits for the box",
            "fn id<T>(x: T) -> T { x }\nfn main() {\n    let d: any Display = id(5)\n}",
            vec![(Code::Mismatch, (3, 26))],
        ),
        (
            "an array of the argument's type, which no conversion makes an array of `any`",
            "fn single<T>(x: T) -> [T] { [x] }\nfn main() {\n    let c = Circle(r: 1.0)\n    let xs: [any Shape] = single(c)\n}",
            vec![(Code::Mismatch, (4, 27))],
        ),
        (
            "an argument that the other argument rejects",
            "fn pair<T>(a: T, b: T) -> T { a }\nfn main() {\n    let d: any Display = pair(1, \"x\")\n}",
            vec![(Code::Mismatch, (3, 31))],
        ),
    ] {
        codes_after(row, src, &expected);
    }
    // A parameter that no argument binds takes the context.
    clean(&format!(
        "{DECLS}fn make<T>(x: T?) -> T? {{ x }}\nfn main() {{\n    let s: (any Shape)? = make(None)\n}}"
    ));
    // `keep` converts nothing for an interface value: its `T` is the interface value already.
    clean(&format!(
        "{DECLS}fn keep<T: Observer>(x: T) -> any Observer {{ x }}\nfn main() {{\n    let a: any Observer = Button()\n    let b = keep(a)\n}}"
    ));
}

/// A conversion to `any I` reaches the methods of the table of the converted type, so a cycle of
/// instantiations through one is found where the conversion is.
#[test]
fn a_conversion_should_close_an_instantiation_cycle() {
    let run = "interface Run {\n    fn run(self)\n}\nclass Box<T> {\n    value: T\n}\n";
    codes_after(
        "a conversion of a deeper box inside the method",
        &format!(
            "{run}extend<T> Box<T>: Run {{\n    fn run(self) {{\n        let x: any Run = Box(value: self)\n    }}\n}}\nfn main() {{\n    let x: any Run = Box(value: 0)\n}}"
        ),
        &[(Code::InstantiationCycle, (9, 26))],
    );
    clean(&format!(
        "{DECLS}{run}extend<T> Box<T>: Run {{\n    fn run(self) {{\n        let x: any Run = Box(value: 0)\n    }}\n}}\nfn main() {{}}"
    ));
}

/// `interface J: Take<Self>` and `interface K: Make<Self>` inherit a requirement that needs `Self`
/// once `Self` takes the place of the argument.
#[test]
fn an_inherited_requirement_should_be_checked_after_its_arguments_are_in_place() {
    let src = "interface Take<T> {\n    fn take(self, other: T)\n}\ninterface J: Take<Self> {}\ninterface Make<T> {\n    fn make(self) -> T\n}\ninterface K: Make<Self> {}\nfn main() {\n    let a: any J = 5\n    let b: any K = 5\n}";
    codes_after(
        "the arguments of an inherited interface",
        src,
        &[(Code::CannotBeAny, (10, 16)), (Code::CannotBeAny, (11, 16))],
    );
}

/// A chain where each interface inherits the two before it has `2^n` paths and `n` interfaces:
/// the walk reads each interface once, however many times `any` names the last one.
#[test]
fn can_be_any_should_read_each_interface_once() {
    let mut src = String::from(
        "interface I1 {\n    fn f1(self)\n}\ninterface I2: I1 {\n    fn f2(self)\n}\n",
    );
    for k in 3..=30 {
        writeln!(
            src,
            "interface I{k}: I{}, I{} {{\n    fn f{k}(self)\n}}",
            k - 1,
            k - 2
        )
        .expect("writing to a string should not fail");
    }
    src.push_str("fn main() {\n");
    for _ in 0..10 {
        src.push_str("    let x: any I30 = 5\n");
    }
    src.push_str("}\n");
    crate::check::any::ANY_VISITS.set(0);
    let ds = diags(&src);
    // `5` conforms to none of them, so each annotation reports that, and no other.
    assert_eq!(ds.len(), 10, "{ds:?}");
    assert!(ds.iter().all(|d| d.code() == Code::NotConform));
    // Each interface joins the closure of `any I30` once and is read for its reason once.
    assert_eq!(
        crate::check::any::ANY_VISITS.get(),
        60,
        "each of the thirty interfaces should be read twice, however many annotations name `I30`"
    );
}

/// A conversion makes a table that only `mono` reaches, so a program that converts and calls
/// nothing still goes through `mono`, as one that calls a method on an `any` value does.
#[test]
fn a_conversion_should_make_the_program_go_through_mono() {
    for (row, src) in [
        (
            "a conversion alone",
            "fn main() {\n    let s: any Shape = Circle(r: 1.0)\n}",
        ),
        (
            "a call on an `any` value",
            "fn main() {\n    let s: any Shape = Circle(r: 1.0)\n    let a = s.area()\n}",
        ),
    ] {
        let (program, ds) = checked(&format!("{DECLS}{src}"));
        assert!(ds.is_empty(), "{row}: {ds:?}");
        let program = program.expect("a program that checks has a typed tree");
        assert!(program.instantiates, "{row}");
    }
}

/// An object and an `any` type that it converts to join to the `any` type, at every site that
/// joins: the arms of an `if` and a `match`, the `break` values of a loop, an array literal. The
/// values that came before convert too.
#[test]
fn a_join_should_make_the_any_type_where_one_side_converts_to_it() {
    let sites = [
        ("an `if`", "if c { {x} } else { {y} }"),
        (
            "a `match`",
            "match n {\n        0 => {x}\n        _ => {y}\n    }",
        ),
        (
            "a `loop`",
            "loop {\n        if c { break {x} }\n        break {y}\n    }",
        ),
        ("an array literal", "[{x}, {y}][0]"),
    ];
    let pairs = [
        ("an `any` value, then an object", "s", "Rect(w: 1.0)"),
        ("an object, then an `any` value", "Rect(w: 1.0)", "s"),
        ("an `any` value, then an optional one", "s", "o"),
        (
            "an optional `any` value, then an object",
            "o",
            "Circle(r: 1.0)",
        ),
        (
            "an object, then an optional `any` value",
            "Circle(r: 1.0)",
            "o",
        ),
        (
            "an `any` that inherits, then the one it inherits",
            "x",
            "n2",
        ),
        (
            "an `any` that is inherited, then the one that inherits",
            "n2",
            "x",
        ),
        (
            "an object of the inheriting interface, then an `any`",
            "Cube(s: 1.0)",
            "n2",
        ),
    ];
    let locals = [
        ("s", "let s: any Shape = Circle(r: 1.0)\n    "),
        ("o", "let o: (any Shape)? = None\n    "),
        ("x", "let x: any Solid = Cube(s: 1.0)\n    "),
        ("n2", "let n2: any Named = x\n    "),
    ];
    for (site, shape) in sites {
        let mut header = String::new();
        for (name, text) in [("c", "let c = true\n    "), ("n", "let n = 1\n    ")] {
            if shape.contains(&format!(" {name} ")) {
                header.push_str(text);
            }
        }
        for (pair, x, y) in pairs {
            let mut body = header.clone();
            for (name, text) in locals {
                if [x, y].contains(&name) || (name == "x" && [x, y].contains(&"n2")) {
                    body.push_str(text);
                }
            }
            let join = shape.replace("{x}", x).replace("{y}", y);
            let src = format!("fn main() {{\n    {body}let r = {join}\n}}");
            clean_row(&format!("{site}: {pair}"), &format!("{DECLS}{src}"));
        }
    }
}

/// A join never makes an `any` type that neither side has, and an optional object does not
/// convert: the mismatch keeps its wording.
#[test]
fn a_join_should_stay_a_mismatch_where_neither_side_converts_to_the_other() {
    for (row, src, expected) in [
        (
            "two classes that both conform, in an `if`",
            "fn main() {\n    let r = if true { Circle(r: 1.0) } else { Rect(w: 1.0) }\n}",
            vec![(Code::Mismatch, (2, 47))],
        ),
        (
            "two classes that both conform, as `break` values",
            "fn main() {\n    let r = loop {\n        if true { break Circle(r: 1.0) }\n        break Rect(w: 1.0)\n    }\n}",
            vec![(Code::Mismatch, (4, 15))],
        ),
        (
            "two classes that both conform, in an array literal",
            "fn main() {\n    let r = [Circle(r: 1.0), Rect(w: 1.0)]\n}",
            vec![(Code::Mismatch, (2, 30))],
        ),
        (
            "an optional object and an `any` value",
            "fn main() {\n    let s: any Shape = Circle(r: 1.0)\n    let o: Circle? = None\n    let r = if true { o } else { s }\n}",
            vec![(Code::Mismatch, (4, 34))],
        ),
        (
            "an `any` value and an object that does not conform to its interface",
            "fn main() {\n    let s: any Solid = Cube(s: 1.0)\n    let r = if true { s } else { Circle(r: 1.0) }\n}",
            vec![(Code::Mismatch, (3, 34))],
        ),
    ] {
        codes_after(row, src, &expected);
    }
}

/// An `any` value written where an interface belongs is reported as what it is, with the
/// parentheses that a `?` needs and none otherwise. The wording is in the fixtures.
#[test]
fn an_interface_value_should_be_named_as_one_where_an_interface_belongs() {
    for (row, src, expected) in [
        (
            "a bound",
            "fn f<T: any Shape>(_x: T) {}\nfn main() {}",
            vec![(Code::NotAnInterface, (1, 9))],
        ),
        (
            "an interface that a class meets",
            "class Sq: any Shape {\n    s: f64\n    fn area(self) -> f64 { self.s }\n}\nfn main() {}",
            vec![(Code::NotAnInterface, (1, 11))],
        ),
        (
            "an interface that an interface inherits",
            "interface Deeper: any Shape {}\nfn main() {}",
            vec![(Code::NotAnInterface, (1, 19))],
        ),
        (
            "an optional interface value",
            "fn f<T: (any Shape)?>(_x: T) {}\nfn main() {}",
            vec![(Code::NotAnInterface, (1, 9))],
        ),
    ] {
        codes_after(row, src, &expected);
    }
}

/// The join does not depend on the order of its members: every order of a class, a second class
/// and an `any` value, and of a class, an `any` value and a `None`, joins at every site that joins.
/// An `else if` chain nests its joins, and a chain of joins that each hold an `any` member is
/// checked once for each branch.
#[test]
fn a_join_should_not_depend_on_the_order_of_its_members() {
    let sites = [
        (
            "a `match`",
            "match n {\n        0 => {1}\n        1 => {2}\n        _ => {3}\n    }",
        ),
        (
            "an `else if` chain",
            "if c { {1} } else if d { {2} } else { {3} }",
        ),
        ("an array literal", "[{1}, {2}, {3}][0]"),
        (
            "a `loop`",
            "loop {\n        if c { break {1} }\n        if d { break {2} }\n        break {3}\n    }",
        ),
    ];
    // A `None` joins without an expected type everywhere but in a loop, whose values do not.
    let sets: [([&str; 3], &str, bool); 3] = [
        (["Circle(r: 1.0)", "Rect(w: 1.0)", "s"], "", true),
        (["Circle(r: 1.0)", "s", "None"], ": (any Shape)?", true),
        (["Circle(r: 1.0)", "s", "None"], "", false),
    ];
    for (site, shape) in sites {
        for (members, ann, in_loop) in sets {
            if !in_loop && site == "a `loop`" {
                continue;
            }
            for order in [
                [0, 1, 2],
                [0, 2, 1],
                [1, 0, 2],
                [1, 2, 0],
                [2, 0, 1],
                [2, 1, 0],
            ] {
                let [x, y, z] = order.map(|i| members[i]);
                let join = shape.replace("{1}", x).replace("{2}", y).replace("{3}", z);
                let src = format!(
                    "fn main() {{\n    let c = true\n    let d = false\n    let n = 1\n    let s: any Shape = Circle(r: 1.0)\n    let r{ann} = {join}\n}}"
                );
                clean_row(&format!("{site}: {x}, {y}, {z}"), &format!("{DECLS}{src}"));
            }
        }
    }
}

/// Each branch of a join is checked once, however deep the joins nest: the expressions checked
/// grow in step with the source. A branch checked again when an `any` member widens the join would
/// double them at each level.
#[test]
fn nested_joins_with_an_any_member_should_check_each_branch_once() {
    let checks = |depth: usize| {
        let mut join = "Circle(r: 1.0)".to_owned();
        for _ in 0..depth {
            join = format!("if c {{ Circle(r: 1.0) }} else if d {{ s }} else {{ {join} }}");
        }
        let src = format!(
            "{DECLS}fn main() {{\n    let c = true\n    let d = false\n    let s: any Shape = Circle(r: 1.0)\n    let r = {join}\n}}"
        );
        EXPR_CHECKS.set(0);
        assert!(checked(&src).0.is_some(), "{src}");
        EXPR_CHECKS.get()
    };
    assert_near_linear(checks(4), checks(8), checks(12));
}

/// `??` joins its payload and its default as the arms of an `if` do: a class and an `any` value
/// make the `any` value, on either side, with an optional on either side.
#[test]
fn coalesce_should_join_its_payload_and_its_default_to_the_any_type() {
    for (row, src) in [
        (
            "a class payload and an `any` default",
            "fn main() {\n    let c: Circle? = None\n    let s: any Shape = Rect(w: 1.0)\n    let r = c ?? s\n}",
        ),
        (
            "with an expected `any` type",
            "fn main() {\n    let c: Circle? = None\n    let s: any Shape = Rect(w: 1.0)\n    let r: any Shape = c ?? s\n}",
        ),
        (
            "with an expected optional `any` type",
            "fn main() {\n    let c: Circle? = None\n    let s: any Shape = Rect(w: 1.0)\n    let r: (any Shape)? = c ?? s\n}",
        ),
        (
            "an `any` payload and a class default",
            "fn main() {\n    let o: (any Shape)? = None\n    let r = o ?? Circle(r: 1.0)\n}",
        ),
        (
            "a class payload and an optional `any` default",
            "fn main() {\n    let c: Circle? = None\n    let o: (any Shape)? = None\n    let r = c ?? o\n}",
        ),
        (
            "a chain, in either grouping",
            "fn main() {\n    let c: Circle? = None\n    let d: Rect? = None\n    let s: any Shape = Rect(w: 1.0)\n    let r = c ?? d ?? s\n    let t = c ?? (d ?? s)\n}",
        ),
    ] {
        clean_row(row, &format!("{DECLS}{src}"));
    }
    codes_after(
        "two classes that both conform",
        "fn main() {\n    let c: Circle? = None\n    let r = c ?? Rect(w: 1.0)\n}",
        &[(Code::Mismatch, (3, 18))],
    );
}
