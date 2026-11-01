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
        (
            "an enum, a number, a string, a char and an array",
            "enum Num {\n    Of(i64)\n}\nfn main() {\n    let a: any Display = Num.Of(1)\n    let b: any Display = 5\n    let c: any Display = \"hi\"\n    let d: any Display = 'c'\n    let e: any Display = [1, 2]\n}",
        ),
        (
            "an enum that conforms to the interface",
            "enum Slab: Shape {\n    Plain\n    fn area(self) -> f64 { 1.0 }\n}\nfn main() {\n    let s: any Shape = Slab.Plain\n}",
        ),
        (
            "a bounded parameter of any kind, at a class, an enum and an interface value",
            "enum Slab: Shape {\n    Plain\n    fn area(self) -> f64 { 1.0 }\n}\nfn wrap<T: Shape>(x: T) -> any Shape { x }\nfn main() {\n    let s: any Shape = Rect(w: 1.0)\n    let a = wrap(Circle(r: 1.0))\n    let b = wrap(Slab.Plain)\n    let c = wrap(s)\n}",
        ),
        (
            "a result of inference that is a number",
            "fn id<T>(x: T) -> T { x }\nfn main() {\n    let d: any Display = id(5)\n}",
        ),
        (
            "an interface value of an enum that converts to `any Display`",
            "enum Slab: Shape {\n    Plain\n    fn area(self) -> f64 { 1.0 }\n}\nfn main() {\n    let s: any Shape = Slab.Plain\n    let d: any Display = s\n}",
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

/// A value converts to an `any` type when it conforms, an object or not. Any other is reported.
#[test]
fn a_value_should_convert_to_any_only_where_it_conforms() {
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
            "an optional whose type does not conform, which does not convert",
            "fn main() {\n    let m: Circle? = None\n    let s: any Shape = m\n}",
            vec![(Code::Mismatch, (3, 24))],
        ),
        (
            "an optional of a number, where an optional interface value is expected",
            "fn main() {\n    let x: i64? = None\n    let d: (any Display)? = x\n}",
            vec![(Code::Mismatch, (3, 29))],
        ),
        (
            "an enum that does not conform",
            "enum Num {\n    One(i64)\n}\nfn main() {\n    let s: any Shape = Num.One(1)\n}",
            vec![(Code::NotConform, (5, 24))],
        ),
        (
            "a bounded parameter that does not conform",
            "fn wrap<T: Display>(x: T) -> any Shape { x }\nfn main() {}",
            vec![(Code::NotConform, (1, 42))],
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

/// A written `any` type needs the layout of its table, which only `mono` gives, so a program that
/// names one and converts nothing still goes through `mono`.
#[test]
fn a_written_any_type_should_make_the_program_go_through_mono() {
    for (row, src) in [
        ("a local", "fn main() {\n    let o: (any Shape)? = None\n}"),
        ("an array", "fn main() {\n    let xs: [any Shape] = []\n}"),
        ("a parameter", "fn f(s: any Shape) {}\nfn main() {}"),
        ("a field", "class H {\n    s: (any Shape)?\n}\nfn main() {}"),
        ("a payload", "enum E {\n    A(any Shape)\n}\nfn main() {}"),
        (
            "an alias",
            "type S = any Shape\nfn main() {\n    let o: S? = None\n}",
        ),
        (
            "a type argument",
            "fn main() {\n    let xs: [[any Shape]] = []\n}",
        ),
    ] {
        let (program, ds) = checked(&format!("{DECLS}{src}"));
        assert!(
            ds.iter().all(|d| d.code().severity() != Severity::Error),
            "{row}: {ds:?}"
        );
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
            "three classes in an `else if` chain, where the earlier branch is the hint",
            "fn main() {\n    let r = if true { Circle(r: 1.0) } else if false { Rect(w: 1.0) } else { Cube(s: 1.0) }\n}",
            vec![(Code::Mismatch, (2, 56))],
        ),
        (
            "a `Some` of a class after a `Some` of another class: the class is the mistake",
            "fn main() {\n    let r = if true { Some(Circle(r: 1.0)) } else { Some(Rect(w: 1.0)) }\n}",
            vec![(Code::Mismatch, (2, 58))],
        ),
        (
            "a `Some` of a class after a value that is no object: the `Some` is the mistake",
            "fn main() {\n    let r = if true { 1 } else { Some(Circle(r: 1.0)) }\n}",
            vec![(Code::Mismatch, (2, 34))],
        ),
        (
            "a `Some` of a class after another class: the class is the mistake",
            "fn main() {\n    let r = if true { Circle(r: 1.0) } else { Some(Rect(w: 1.0)) }\n}",
            vec![(Code::Mismatch, (2, 52))],
        ),
        (
            "a `Some` of a class after a `Some` of another class, in a loop: the class is the mistake",
            "fn main() {\n    let r = loop {\n        if true { break Some(Circle(r: 1.0)) }\n        break Some(Rect(w: 1.0))\n    }\n}",
            vec![(Code::Mismatch, (4, 20))],
        ),
        (
            "a `Some` of a `Some` of a class after one of another class: the class is the mistake",
            "fn main() {\n    let r = if true { Some(Some(Circle(r: 1.0))) } else { Some(Some(Rect(w: 1.0))) }\n}",
            vec![(Code::Mismatch, (2, 69))],
        ),
        (
            "a `Some` of a `Some` of a class after one of another class, in a loop",
            "fn main() {\n    let r = loop {\n        if true { break Some(Some(Circle(r: 1.0))) }\n        break Some(Some(Rect(w: 1.0)))\n    }\n}",
            vec![(Code::Mismatch, (4, 25))],
        ),
        (
            "a `Some` of a class after the class, in a loop: the optional is the mistake",
            "fn main() {\n    let r = loop {\n        if true { break Circle(r: 1.0) }\n        break Some(Circle(r: 1.0))\n    }\n}",
            vec![(Code::Mismatch, (4, 15))],
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
            "two `any` values of unrelated interfaces, in an `if`",
            "fn main() {\n    let s: any Shape = Circle(r: 1.0)\n    let k: any Named = Cube(s: 1.0)\n    let r = if true { s } else { k }\n}",
            vec![(Code::Mismatch, (4, 34))],
        ),
        (
            "two `any` values of unrelated interfaces, as `break` values",
            "fn main() {\n    let s: any Shape = Circle(r: 1.0)\n    let k: any Named = Cube(s: 1.0)\n    let r = loop {\n        if true { break s }\n        break k\n    }\n}",
            vec![(Code::Mismatch, (6, 15))],
        ),
        (
            "two `any` values of unrelated interfaces, in an array literal",
            "fn main() {\n    let s: any Shape = Circle(r: 1.0)\n    let k: any Named = Cube(s: 1.0)\n    let r = [s, k]\n}",
            vec![(Code::Mismatch, (4, 17))],
        ),
        (
            "an optional object and an `any` value",
            "fn main() {\n    let s: any Shape = Circle(r: 1.0)\n    let o: Circle? = None\n    let r = if true { o } else { s }\n}",
            vec![(Code::Mismatch, (4, 23))],
        ),
    ] {
        codes_after(row, src, &expected);
    }
}

/// A join with an `any` type in reach, its own or its siblings', reports the member that does not
/// convert to the widest `any` type, whatever the order of the members and at every site: a class
/// that does not conform gets `NotConform`, as a value does where the `any` type is expected.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one row for each site and form of a join that leaves its `any` type"
)]
fn a_join_should_report_the_member_that_leaves_its_any_type() {
    let sites = [
        (
            "a `match`",
            "match n {\n        0 => {1}\n        1 => {2}\n        _ => {3}\n    }",
        ),
        (
            "an `else if` chain",
            "if c { {1} } else if d { {2} } else { {3} }",
        ),
        (
            "a nested `if`",
            "if c { {1} } else { if d { {2} } else { {3} } }",
        ),
        ("an array literal", "[{1}, {2}, {3}]"),
        (
            "a `loop`",
            "loop {\n        if c { break {1} }\n        if d { break {2} }\n        break {3}\n    }",
        ),
        (
            "the default of `??`",
            "o ?? if c { {1} } else if d { {2} } else { {3} }",
        ),
    ];
    let members = ["s", "Circle(r: 1.0)", "Cube(s: 1.0)"];
    for (site, shape) in sites {
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
                "fn main() {{\n    let c = true\n    let d = false\n    let n = 1\n    let s: any Shape = Circle(r: 1.0)\n    let o: (any Shape)? = None\n    let r = {join}\n}}"
            );
            let cube = src
                .find("Cube")
                .expect("the program should hold the `Cube`");
            let line = src[..cube].matches('\n').count() + 1;
            let col = cube - src[..cube].rfind('\n').map_or(0, |i| i + 1) + 1;
            let row = format!("{site}: {x}, {y}, {z}");
            codes_after(&row, &src, &[(Code::NotConform, (line, col))]);
        }
    }
    rows_after(&[
        (
            "a number, after an `any` value",
            "fn main() {\n    let s: any Shape = Circle(r: 1.0)\n    let r = if true { s } else { 5 }\n}",
            vec![(Code::NotConform, (3, 34))],
        ),
        (
            "a number in an array literal, after an `any` value",
            "fn main() {\n    let s: any Shape = Circle(r: 1.0)\n    let r = [s, 5]\n}",
            vec![(Code::NotConform, (3, 17))],
        ),
        (
            "a type parameter that has no bound, after an `any` value",
            "fn f<T>(t: T, s: any Shape) {\n    let r = if true { s } else { t }\n}\nfn main() {}",
            vec![(Code::NotConform, (2, 34))],
        ),
        (
            "a `Some` of a class that does not conform, after an `any` value",
            "fn main() {\n    let x: any Solid = Cube(s: 1.0)\n    let r = if true { x } else { Some(Circle(r: 1.0)) }\n}",
            vec![(Code::NotConform, (3, 39))],
        ),
        (
            "a `Some` of a class that does not conform, after an `any` value, in a loop",
            "fn main() {\n    let x: any Solid = Cube(s: 1.0)\n    let r = loop {\n        if true { break x }\n        break Some(Circle(r: 1.0))\n    }\n}",
            vec![(Code::NotConform, (5, 20))],
        ),
        (
            "a `Some` of a class that conforms, after an `any` value, in a loop: the optional is the mistake",
            "fn main() {\n    let s: any Shape = Circle(r: 1.0)\n    let r = loop {\n        if true { break s }\n        break Some(Circle(r: 1.0))\n    }\n}",
            vec![(Code::Mismatch, (5, 15))],
        ),
        (
            "a `Some` of a value that is no object, after an `any` value: it converts as under an expected type",
            "fn main() {\n    let x: any Solid = Cube(s: 1.0)\n    let r = if true { x } else { Some(5) }\n}",
            vec![(Code::NotConform, (3, 39))],
        ),
        (
            "a branch with no value, after an `any` value and a class",
            "fn main() {\n    let s: any Shape = Circle(r: 1.0)\n    let r = if true { s } else if false { Rect(w: 1.0) } else { print(1) }\n}",
            vec![(Code::NoValue, (3, 65))],
        ),
        (
            "a branch with no value, after an `any` value and a class, as a block",
            "fn main() {\n    let c = true\n    let s: any Shape = Circle(r: 1.0)\n    let r = if c { s } else if c { Rect(w: 1.0) } else { let z = 1 }\n}",
            vec![(Code::NoValue, (4, 58))],
        ),
        (
            "a nested join that leaves its own `any` type, under an `any` hint it does not join",
            "fn main() {\n    let c = true\n    let d = false\n    let s: any Shape = Circle(r: 1.0)\n    let k: any Named = Cube(s: 1.0)\n    let r = if c { s } else { if d { k } else { Circle(r: 1.0) } }\n}",
            vec![(Code::NotConform, (6, 49))],
        ),
        (
            "a `Some` of a value that is no object, then an `any` value",
            "fn main() {\n    let c = true\n    let s: any Shape = Circle(r: 1.0)\n    let r = if c { Some(5) } else { s }\n}",
            vec![(Code::Mismatch, (4, 20))],
        ),
        (
            "an optional class that does not conform, then an optional `any` default",
            "fn main() {\n    let o: Circle? = None\n    let k: (any Named)? = None\n    let r = o ?? k\n}",
            vec![(Code::NotConform, (4, 13))],
        ),
        (
            "an optional value that is no object, then an `any` default",
            "fn main() {\n    let i: i64? = None\n    let s: any Shape = Circle(r: 1.0)\n    let r = i ?? s\n}",
            vec![(Code::Mismatch, (4, 18))],
        ),
        (
            "an optional class that does not conform, then an `any` default",
            "fn main() {\n    let s: any Shape = Circle(r: 1.0)\n    let o: Cube? = None\n    let r = o ?? s\n}",
            vec![(Code::NotConform, (4, 13))],
        ),
        (
            "an optional `any` value, then a default that does not conform",
            "fn main() {\n    let o: (any Shape)? = None\n    let r = o ?? Cube(s: 1.0)\n}",
            vec![(Code::NotConform, (3, 18))],
        ),
        (
            "an `any` value, then an object that does not conform to its interface",
            "fn main() {\n    let s: any Solid = Cube(s: 1.0)\n    let r = if true { s } else { Circle(r: 1.0) }\n}",
            vec![(Code::NotConform, (3, 34))],
        ),
        (
            "a `Some` of a `Some` of a class that does not conform, after one of an `any` value",
            "fn main() {\n    let s: any Shape = Circle(r: 1.0)\n    let r = if true { Some(Some(s)) } else { Some(Some(Kelvin(k: 1.0))) }\n}",
            vec![(Code::NotConform, (3, 56))],
        ),
        (
            "three layers of `Some` of a class that does not conform, after an `any` value",
            "fn main() {\n    let s: any Shape = Circle(r: 1.0)\n    let r = if true { Some(Some(Some(s))) } else { Some(Some(Some(Kelvin(k: 1.0)))) }\n}",
            vec![(Code::NotConform, (3, 67))],
        ),
    ]);
}

/// With a written type, a join with an `any` member reports as a join with none does: the first
/// member, in source order, that does not fit the written type, against it; a branch with no
/// value first. So does a `??` default. The wording is in the fixtures.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one row for each join site and order under a written type"
)]
fn a_join_should_report_the_member_that_leaves_it_against_the_written_type() {
    for (row, written, join, (line, col, text), code) in [
        (
            "an `if`",
            ": (any Shape)?",
            "if true { o } else { s }",
            (5, 37, "o"),
            Code::Mismatch,
        ),
        (
            "a `match`",
            ": (any Shape)?",
            "match 1 {\n        0 => s\n        _ => o\n    }",
            (7, 14, "o"),
            Code::Mismatch,
        ),
        (
            "a class, then an optional class",
            ": (any Shape)?",
            "if true { Circle(r: 1.0) } else { o }",
            (5, 61, "o"),
            Code::Mismatch,
        ),
        (
            "an `any` value under a class it does not fit",
            ": Circle?",
            "if true { s } else { o }",
            (5, 32, "s"),
            Code::Mismatch,
        ),
        (
            "an `any` value under a class it does not fit, in a `match`",
            ": Circle?",
            "match 1 {\n        0 => o\n        _ => s\n    }",
            (7, 14, "s"),
            Code::Mismatch,
        ),
        (
            "an `any` value under a class it does not fit, second",
            ": Circle?",
            "if true { o } else { s }",
            (5, 43, "s"),
            Code::Mismatch,
        ),
        (
            "an `any` value beside a `Some` of the class",
            ": Circle?",
            "if true { s } else { Some(Circle(r: 1.0)) }",
            (5, 32, "s"),
            Code::Mismatch,
        ),
        (
            "an `any` value beside a `None`",
            ": Circle?",
            "if true { s } else { None }",
            (5, 32, "s"),
            Code::Mismatch,
        ),
        (
            "an `any` value under a type that is no object",
            ": i64",
            "if true { s } else { o }",
            (5, 28, "s"),
            Code::Mismatch,
        ),
        (
            "a branch with no value, after a branch that does not fit",
            ": (any Shape)?",
            "if true { o } else { print(1) }",
            (5, 48, "print(1)"),
            Code::NoValue,
        ),
        (
            "a `??` default of a `Some` of a class that does not conform",
            ": (any Shape)?",
            "os ?? Some(Kelvin(k: 1.0))",
            (5, 38, "Kelvin(k: 1.0)"),
            Code::NotConform,
        ),
        (
            "a `??` default of a `Some` of a class that does not conform, with no written type",
            "",
            "os ?? Some(Kelvin(k: 1.0))",
            (5, 24, "Kelvin(k: 1.0)"),
            Code::NotConform,
        ),
        (
            "a `??` default that is a join",
            ": (any Shape)?",
            "os ?? if true { o } else { s }",
            (5, 43, "o"),
            Code::Mismatch,
        ),
        (
            "a `??` default of an optional class",
            ": (any Shape)?",
            "os ?? o",
            (5, 33, "o"),
            Code::Mismatch,
        ),
        (
            "a `??` whose optional `any` value does not fit",
            ": Circle?",
            "os ?? o",
            (5, 22, "os"),
            Code::Mismatch,
        ),
    ] {
        let src = format!(
            "{DECLS}fn main() {{\n    let o: Circle? = None\n    let s: any Shape = Circle(r: 1.0)\n    let os: (any Shape)? = None\n    let r{written} = {join}\n}}"
        );
        // `NotConform` points at where `any Shape` is first written too.
        let secondary: &[_] = if code == Code::NotConform {
            &[(3 + LINES, 12)]
        } else {
            &[]
        };
        diagnostic_row(row, &src, code, (line + LINES, col, text), secondary);
    }
    for (row, join) in [
        ("an optional class, then an `any` default", "o ?? s"),
        ("an optional `any` value, then an `any` default", "os ?? s"),
        (
            "an optional `any` value, then a `Some` of a class",
            "os ?? Some(Circle(r: 1.0))",
        ),
        (
            "an optional `any` value, then a join",
            "os ?? if true { s } else { Circle(r: 1.0) }",
        ),
    ] {
        clean_row(
            row,
            &format!(
                "{DECLS}fn main() {{\n    let o: Circle? = None\n    let s: any Shape = Circle(r: 1.0)\n    let os: (any Shape)? = None\n    let r: (any Shape)? = {join}\n}}"
            ),
        );
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
/// and an `any` value, of a class, an `any` value and a `None`, of two `any` values and an
/// object, of two `any` values and a `Some` of an object, of an `any` value, a `Some` of an
/// object and a `None`, and of an `any` value, a `Some` of an `any` value that converts to it
/// and an object, joins at every site that joins. An `else if` chain nests its joins, and
/// the join is the `any` type, not its optional, where no member is optional. A chain of joins
/// that each hold an `any` member is checked once for each branch.
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
    let (array, loops) = (sites[2].0, sites[3].0);
    // A `None` joins without an expected type everywhere but in a loop, whose values do not. A
    // `Some` of an object joins an `any` type only as a branch: an array literal and a loop do
    // not join a `Some` of an object with an `any` value of another type. A set with no `None`
    // is read where its `any` type is expected. Each set lists the sites where it does not join.
    let sets: [([&str; 3], &str, &str, &[&str]); 7] = [
        (
            ["Circle(r: 1.0)", "Rect(w: 1.0)", "s"],
            "",
            "\n    let t: any Shape = r",
            &[],
        ),
        (["Circle(r: 1.0)", "s", "None"], ": (any Shape)?", "", &[]),
        (["Circle(r: 1.0)", "s", "None"], "", "", &[loops]),
        (
            ["x", "Cube(s: 1.0)", "n2"],
            "",
            "\n    let t: any Named = r",
            &[],
        ),
        (
            ["x", "Some(Tag())", "n2"],
            "",
            "\n    let t: (any Named)? = r",
            &[array, loops],
        ),
        (
            ["s", "Some(Circle(r: 1.0))", "None"],
            "",
            "\n    let t: (any Shape)? = r",
            &[array, loops],
        ),
        (
            ["n2", "Some(x)", "Cube(s: 1.0)"],
            "",
            "\n    let t: (any Named)? = r",
            &[array, loops],
        ),
    ];
    // `Tag` conforms to `Named` only, so it does not convert to `x`, an `any Solid`.
    let tag = "class Tag: Named {\n    fn name(self) -> string { \"tag\" }\n}\n";
    for (site, shape) in sites {
        for (members, ann, read, skip) in sets {
            if skip.contains(&site) {
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
                    "fn main() {{\n    let c = true\n    let d = false\n    let n = 1\n    let s: any Shape = Circle(r: 1.0)\n    let x: any Solid = Cube(s: 1.0)\n    let n2: any Named = x\n    let r{ann} = {join}{read}\n}}"
                );
                clean_row(
                    &format!("{site}: {x}, {y}, {z}"),
                    &format!("{DECLS}{tag}{src}"),
                );
            }
        }
    }
}

/// A branch gives its type to the inference of the branches after it, a class with type arguments
/// as much as any other type: a literal in a later branch takes the type that the earlier branch
/// fixed.
#[test]
fn a_branch_should_steer_the_inference_of_the_branches_after_it() {
    let decls = "class G<T> {\n    x: T\n}\nfn mk<T>(v: T) -> G<T> { G(x: v) }\n";
    for (row, join) in [
        ("an `if`", "if c { G(x: u) } else { G(x: 1) }"),
        (
            "a `match`",
            "match n {\n        0 => G(x: u)\n        _ => G(x: 1)\n    }",
        ),
        (
            "an `else if` chain",
            "if c { G(x: u) } else if d { G(x: 1) } else { G(x: 2) }",
        ),
        ("an `if let`", "if let v = o { G(x: v) } else { G(x: 1) }"),
        ("a generic function", "if c { mk(u) } else { mk(1) }"),
        (
            "a nested class",
            "if c { G(x: G(x: u)) } else { G(x: G(x: 1)) }",
        ),
        ("an array field", "if c { G(x: [u]) } else { G(x: [1]) }"),
        ("a `Some`", "if c { G(x: u) } else { Some(G(x: 1)) }"),
    ] {
        let src = format!(
            "{decls}fn main() {{\n    let u: u8 = 5\n    let o: u8? = 5\n    let c = true\n    let d = false\n    let n = 1\n    let b = {join}\n}}"
        );
        clean_row(row, &src);
    }
}

/// A type that siblings give a value is a hint, not an expected type: the value does not convert
/// to it. The join converts its members once it knows its type, so a nested join, a loop, a `??`
/// default or a `Some` in a branch gives the type of its own members: never the optional that the
/// hint is, nor a narrower `any` type than a member has.
#[test]
fn a_hint_should_not_convert_a_value_to_its_any_type() {
    for (row, src) in [
        (
            "an `else if` chain of `any` values",
            "let r = if c { s } else if d { s } else { s }\n    let t: any Shape = r",
        ),
        (
            "a nested `if` of two classes",
            "let r = if c { s } else { if d { Circle(r: 1.0) } else { Rect(w: 1.0) } }\n    let t: any Shape = r",
        ),
        (
            "a nested `match` of two classes",
            "let r = if c { s } else {\n        match n {\n            0 => Circle(r: 1.0)\n            _ => Rect(w: 1.0)\n        }\n    }\n    let t: any Shape = r",
        ),
        (
            "a member wider than the hint",
            "let r = if c { x } else if d { Cube(s: 1.0) } else { n2 }\n    let t: any Named = r",
        ),
        (
            "a `None` after a member wider than the hint",
            "let r = if c { x } else if d { n2 } else { None }\n    let t: (any Named)? = r",
        ),
        (
            "a loop in a branch",
            "let r = if c { s } else { loop { break Circle(r: 1.0) } }\n    let t: any Shape = r",
        ),
        (
            "a loop of two classes in a branch",
            "let r = if c { s } else {\n        loop {\n            if d { break Circle(r: 1.0) }\n            break Rect(w: 1.0)\n        }\n    }\n    let t: any Shape = r",
        ),
        (
            "a loop value wider than the hint",
            "let r = if c { x } else { loop { break n2 } }\n    let t: any Named = r",
        ),
        (
            "a later `break` value that holds a join",
            "let r = loop {\n        if d { break x }\n        break if c { n2 } else { Cube(s: 1.0) }\n    }\n    let t: any Named = r",
        ),
        (
            "a `??` default that holds a join",
            "let r = jo ?? if d { n2 } else { Cube(s: 1.0) }\n    let t: any Named = r",
        ),
        (
            "a `??` default that is a block",
            "let r = jo ?? { if d { n2 } else { Cube(s: 1.0) } }\n    let t: any Named = r",
        ),
        (
            "a `Some` of a value wider than the hint",
            "let r = if c { x } else { Some(n2) }\n    let t: (any Named)? = r",
        ),
        (
            "a `Some` of a join wider than the hint",
            "let r = if c { x } else { Some(if d { n2 } else { Cube(s: 1.0) }) }\n    let t: (any Named)? = r",
        ),
    ] {
        let src = format!(
            "{DECLS}fn main() {{\n    let c = true\n    let d = false\n    let n = 1\n    let s: any Shape = Circle(r: 1.0)\n    let x: any Solid = Cube(s: 1.0)\n    let n2: any Named = x\n    let jo: (any Solid)? = x\n    {src}\n}}"
        );
        clean_row(row, &src);
    }
}

/// Each branch of a join is checked once, however deep the joins nest: the expressions checked
/// grow in step with the source. A branch checked again when an `any` member widens the join would
/// double them at each level.
#[test]
fn nested_joins_with_an_any_member_should_check_each_branch_once() {
    let checks = |depth: usize| {
        let mut join = "Circle(r: 1.0)".to_owned();
        for level in 0..depth {
            // The `any` member comes first on every other level, so it is the hint below it.
            join = if level % 2 == 0 {
                format!("if c {{ Circle(r: 1.0) }} else if d {{ s }} else {{ {join} }}")
            } else {
                format!("if c {{ s }} else if d {{ Circle(r: 1.0) }} else {{ {join} }}")
            };
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
            "a narrower `any` payload and a wider `any` default",
            "fn main() {\n    let x: any Solid = Cube(s: 1.0)\n    let jo: (any Solid)? = x\n    let none: (any Solid)? = None\n    let n2: any Named = x\n    let r: any Named = jo ?? n2\n    let t: any Named = none ?? n2\n}",
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

/// An `any` value beside `None`, `Some` or an optional `any` value joins to an optional `any`
/// value: the optional side does not go into a bare `any I`. Only an expected `any I` boxes it.
#[test]
fn a_join_of_an_any_value_and_an_optional_should_stay_optional() {
    for (row, src) in [
        (
            "`None` beside an `any` value, in an `if`",
            "fn main() {\n    let d: any Display = 1\n    let r = if true { d } else { None }\n    print(r ?? 9)\n}",
        ),
        (
            "`??` of two optional `any` values",
            "fn main() {\n    let a: (any Display)? = None\n    let b: (any Display)? = None\n    let c = a ?? b\n    print(c ?? 9)\n}",
        ),
        (
            "`Some` beside an `any` value, in an `if`",
            "fn main() {\n    let d: any Display = 1\n    let r = if false { d } else { Some(2) }\n    print(r ?? 9)\n}",
        ),
    ] {
        clean_row(row, src);
    }
    // The join is optional, so printing it warns, as it does for any optional.
    for (row, src) in [
        (
            "`None` beside an `any` value",
            "fn main() {\n    let d: any Display = 1\n    let r = if true { d } else { None }\n    print(r)\n}",
        ),
        (
            "`Some` beside an `any` value",
            "fn main() {\n    let d: any Display = 1\n    let r = if true { d } else { Some(2) }\n    print(r)\n}",
        ),
        (
            "`??` of an optional `any` value and `None`",
            "fn main() {\n    let o: (any Display)? = None\n    let k = o ?? None\n    print(k)\n}",
        ),
    ] {
        codes_row(row, src, &[(Code::PrintsOptional, (4, 11))]);
    }
}

/// An array literal joins a `Some` with an `any` element as the branches of an `if` do: after an
/// `any` element, the payload of a `Some` converts and the elements are optional; a `Some` of an
/// object before an `any` element joins as the object. A `Some` of another value before it, and
/// an optional local, are the mistake, in the `if` too.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one row for each way an array literal joins a `Some` with an `any` element"
)]
fn an_array_literal_should_join_a_some_with_an_any_element_as_an_if_does() {
    for (row, src) in [
        (
            "`Some` of a number after an `any` value",
            "fn main() {\n    let d: any Display = 1\n    let r = [d, Some(2)]\n}",
        ),
        (
            "`Some` of a string after an `any` value",
            "fn main() {\n    let d: any Display = 1\n    let r = [d, Some(\"a\")]\n}",
        ),
        (
            "`Some` of a class after an `any` value",
            "fn main() {\n    let s: any Shape = Circle(r: 1.0)\n    let r = [s, Some(Circle(r: 2.0))]\n}",
        ),
        (
            "`Some` of a class before an `any` value",
            "fn main() {\n    let s: any Shape = Circle(r: 1.0)\n    let r = [Some(Circle(r: 2.0)), s]\n}",
        ),
        (
            "an `any` value that joins an `any` value of another interface, then a `Some`",
            "fn main() {\n    let s: any Shape = Circle(r: 1.0)\n    let d: any Display = 1\n    let r = [s, d, Some(2)]\n}",
        ),
        (
            "a `Some` of an `any` value before an `any` value",
            "fn main() {\n    let s: any Shape = Circle(r: 1.0)\n    let d: any Display = 1\n    let r = [Some(s), d]\n}",
        ),
        (
            "an array of an `any` value and a number beside an array of an `any` value",
            "fn main() {\n    let d: any Display = 1\n    let c = true\n    let r = if c { [d] } else { [d, 2] }\n}",
        ),
        (
            "an array of an `any` value beside an array of an optional `any` value",
            "fn main() {\n    let d: any Display = 1\n    let o: (any Display)? = None\n    let c = true\n    let r = if c { [o] } else { [d] }\n}",
        ),
        (
            "an array of a number beside an array of an optional `any` value",
            "fn main() {\n    let o: (any Display)? = None\n    let c = true\n    let r = if c { [o] } else { [2] }\n}",
        ),
        (
            "an empty array in an array beside an array of an array of an `any` value",
            "fn main() {\n    let d: any Display = 1\n    let c = true\n    let r = if c { [[d]] } else { [[]] }\n}",
        ),
        (
            "an empty array in an array beside an array that holds one",
            "fn main() {\n    let ys: [[any Display]] = [[]]\n    let c = true\n    let r = if c { ys } else { [[]] }\n}",
        ),
        (
            "an empty array beside an array of an `any` value",
            "fn main() {\n    let d: any Display = 1\n    let c = true\n    let r = if c { [d] } else { [] }\n}",
        ),
        (
            "an array of `Some` beside an array that holds a `Some` and an `any` value",
            "fn main() {\n    let d: any Display = 1\n    let c = true\n    let r = if c { [d, Some(2)] } else { [Some(2)] }\n}",
        ),
    ] {
        clean_row(row, &format!("{DECLS}{src}"));
    }
    rows_after(&[
        (
            "`Some` of a number before an `any` value, as in an `if`",
            "fn main() {\n    let d: any Display = 1\n    let r = [Some(2), d]\n}",
            vec![(Code::Mismatch, (3, 14))],
        ),
        (
            "an optional number after an `any` value",
            "fn main() {\n    let d: any Display = 1\n    let x: i64? = Some(5)\n    let r = [d, x]\n}",
            vec![(Code::Mismatch, (4, 17))],
        ),
        (
            "an array of an optional number beside an array of an `any` value, in an `if`",
            "fn main() {\n    let d: any Display = 1\n    let x: i64? = Some(5)\n    let c = true\n    let r = if c { [d] } else { [x] }\n}",
            vec![(Code::Mismatch, (5, 34))],
        ),
        (
            "an array of an `any` value of another interface beside an array of an `any` value",
            "fn main() {\n    let s: any Shape = Circle(r: 1.0)\n    let n: any Named = Cube(s: 1.0)\n    let c = true\n    let r = if c { [s] } else { [n] }\n}",
            vec![(Code::Mismatch, (5, 34))],
        ),
        (
            "an `any` value of another interface after a `Some` of a class and an `any` value",
            "fn main() {\n    let s: any Shape = Circle(r: 1.0)\n    let n: any Named = Cube(s: 1.0)\n    let r = [Some(Circle(r: 2.0)), s, n]\n}",
            vec![(Code::Mismatch, (4, 39))],
        ),
        (
            "an array of an optional number beside an array of an optional `any` value",
            "fn main() {\n    let o: (any Display)? = None\n    let x: i64? = Some(5)\n    let c = true\n    let r = if c { [o] } else { [x] }\n}",
            vec![(Code::Mismatch, (5, 34))],
        ),
        (
            "a `None` in an array in an array beside an array of an array of an `any` value",
            "fn main() {\n    let d: any Display = 1\n    let c = true\n    let r = if c { [[d]] } else { [[None]] }\n}",
            vec![(Code::Mismatch, (4, 37))],
        ),
        (
            "an array of a `Some` beside an array of an `any` value, in an `if`",
            "fn main() {\n    let d: any Display = 1\n    let c = true\n    let r = if c { [d] } else { [Some(2)] }\n}",
            vec![(Code::Mismatch, (4, 34))],
        ),
        (
            "an optional number after a `Some` of a class and an `any` value",
            "fn main() {\n    let s: any Shape = Circle(r: 1.0)\n    let x: i64? = Some(5)\n    let r = [Some(Circle(r: 2.0)), s, x]\n}",
            vec![(Code::Mismatch, (4, 39))],
        ),
        (
            "a `Some` of a class that does not conform, after a `Some` of one that does",
            "fn main() {\n    let s: any Shape = Circle(r: 1.0)\n    let r = [Some(Circle(r: 2.0)), Some(Kelvin(k: 1.0)), s]\n}",
            vec![(Code::NotConform, (3, 41))],
        ),
        (
            "a generic call that makes an optional number after an `any` value, as in an `if`",
            "fn opt<T>(x: T) -> T? { Some(x) }\nfn main() {\n    let d: any Display = 1\n    let r = [d, opt(2)]\n}",
            vec![(Code::Mismatch, (4, 17))],
        ),
        (
            "a `Some` of a class that does not conform before an `any` value",
            "fn main() {\n    let s: any Shape = Circle(r: 1.0)\n    let r = [Some(Kelvin(k: 1.0)), s]\n}",
            vec![(Code::NotConform, (3, 19))],
        ),
    ]);
}

/// An optional beside an `any` value does not go into a bare `any I` where no type is written: a
/// join and a comparison report it, and `??` keeps its result optional. A written `any I` does
/// not take the optional either.
#[test]
fn an_optional_beside_an_any_value_should_stay_a_mistake_where_no_type_is_written() {
    for (row, src, expected) in [
        (
            "`break` of an optional after an `any` value",
            "fn main() {\n    let d: any Display = 1\n    let x: i64? = Some(5)\n    let c = true\n    let r = loop {\n        if c { break d }\n        break x\n    }\n    print(r)\n}",
            vec![(Code::Mismatch, (7, 15))],
        ),
        (
            "`??` of an optional `any` value and an optional number",
            "fn main() {\n    let x: i64? = Some(5)\n    let o: (any Display)? = None\n    let r = o ?? x\n    print(r)\n}",
            vec![(Code::Mismatch, (4, 18))],
        ),
        (
            "`==` of an `any` value and an optional number",
            "fn main() {\n    let d: any Display = 1\n    let x: i64? = Some(5)\n    let r = d == x\n    print(r)\n}",
            vec![(Code::CannotCompare, (4, 13))],
        ),
        (
            "`??` of an optional default, under a written `any` type",
            "fn main() {\n    let x: i64? = Some(5)\n    let o: (any Display)? = None\n    let r: any Display = o ?? x\n    print(r)\n}",
            vec![(Code::Mismatch, (4, 31))],
        ),
    ] {
        codes_row(row, src, &expected);
    }
}

/// The type `E` of the earlier branch in the parity cases, each with the value that a local of
/// that type starts from.
const PARITY_TYPES: [(&str, &str); 8] = [
    ("any Display", "1"),
    ("(any Display)?", "Some(7)"),
    ("any Shape", "Circle(r: 1.0)"),
    ("[any Display]", "[d]"),
    ("[any Display]?", "[d]"),
    ("i64", "1"),
    ("i64?", "Some(5)"),
    ("string", "\"z\""),
];

/// The values of the later branch in the parity cases. The last one is two values, `[]` beside
/// `2`, which the later array holds as `[[], 2]`.
const PARITY_VALUES: [&[&str]; 15] = [
    &["d"],
    &["o"],
    &["x"],
    &["2"],
    &["\"a\""],
    &["Some(2)"],
    &["Some(d)"],
    &["None"],
    &["[]"],
    &["s"],
    &["Circle(r: 2.0)"],
    &["Some(Circle(r: 2.0))"],
    &["[d]"],
    &["Some([d])"],
    &["[]", "2"],
];

/// The reason of the parity exceptions where the scalar join blames the earlier branch.
const EARLIER_LEAVES: &str = "the scalar join takes the `any` member first, so `e` leaves its type; the array blames the later element (deferred)";

/// The reason of the parity exceptions of a `Some` in a `loop`.
const LOOP_SOME: &str = "the scalar `loop` reports the `Some` as a mismatch, where the scalar `if` and `match` and the array report its argument as not conforming";

/// The parity cases, each `E`, the later values and the forms, where the array errors differ
/// from the scalar join's, with the reason. In these, each error of the array must be at an
/// element of the later array, of any code.
const PARITY_EXCEPTIONS: [(&str, &str, &[JoinForm], &str); 14] = [
    (
        "[any Display]",
        "s",
        &[JoinForm::If, JoinForm::Match],
        EARLIER_LEAVES,
    ),
    (
        "[any Display]?",
        "d",
        &[JoinForm::If, JoinForm::Match],
        EARLIER_LEAVES,
    ),
    (
        "[any Display]?",
        "o",
        &[JoinForm::If, JoinForm::Match],
        EARLIER_LEAVES,
    ),
    (
        "[any Display]?",
        "Some(d)",
        &[JoinForm::If, JoinForm::Match],
        EARLIER_LEAVES,
    ),
    (
        "[any Display]?",
        "s",
        &[JoinForm::If, JoinForm::Match],
        EARLIER_LEAVES,
    ),
    ("i64", "s", &[JoinForm::If, JoinForm::Match], EARLIER_LEAVES),
    (
        "i64?",
        "d",
        &[JoinForm::If, JoinForm::Match],
        EARLIER_LEAVES,
    ),
    (
        "i64?",
        "o",
        &[JoinForm::If, JoinForm::Match],
        EARLIER_LEAVES,
    ),
    (
        "i64?",
        "Some(d)",
        &[JoinForm::If, JoinForm::Match],
        EARLIER_LEAVES,
    ),
    (
        "i64?",
        "s",
        &[JoinForm::If, JoinForm::Match],
        EARLIER_LEAVES,
    ),
    (
        "string",
        "s",
        &[JoinForm::If, JoinForm::Match],
        EARLIER_LEAVES,
    ),
    ("any Shape", "Some(2)", &[JoinForm::Loop], LOOP_SOME),
    ("any Shape", "Some([d])", &[JoinForm::Loop], LOOP_SOME),
    (
        "string",
        "[], 2",
        &[JoinForm::If],
        "a hint with no `any` type is expected as before, so the array reports each element that does not fit; the scalar `if` stops at the first",
    ),
];

/// The joins of the parity cases: an `if`, a `match` and a `loop` of `break`s.
#[derive(Clone, Copy, Debug, PartialEq)]
enum JoinForm {
    If,
    Match,
    Loop,
}

/// The join `form` of the `members`, each a value in source order, and where each member is in
/// it, as byte ranges.
fn join_of(form: JoinForm, members: &[String]) -> (String, Vec<Range<usize>>) {
    let last = members.len() - 1;
    let mut text = String::new();
    let mut ranges = Vec::new();
    for (i, member) in members.iter().enumerate() {
        let before = match (form, i) {
            (JoinForm::If, 0) => "if c { ",
            (JoinForm::If, i) if i == last => " } else { ",
            (JoinForm::If, _) => " } else if c { ",
            (JoinForm::Match, 0) => "match n { 0 => ",
            (JoinForm::Match, i) if i == last => "; _ => ",
            (JoinForm::Match, _) => "; 1 => ",
            (JoinForm::Loop, 0) => "loop {\n        if c { break ",
            (JoinForm::Loop, i) if i == last => " }\n        break ",
            (JoinForm::Loop, _) => " }\n        if c { break ",
        };
        text.push_str(before);
        ranges.push(text.len()..text.len() + member.len());
        text.push_str(member);
    }
    text.push_str(match form {
        JoinForm::If | JoinForm::Match => " }",
        JoinForm::Loop => "\n    }",
    });
    (text, ranges)
}

/// What checking a parity program found: the start and code of each error, and the types of the
/// locals `r`, the join, and `w`, of the written type the join is compared with, when it checked.
struct ParityRun {
    errors: Vec<(usize, Code)>,
    types: Option<(Type, Type)>,
}

/// Checks `let r = <join>` after locals of each value's type, with `e` of the type `ty`, made
/// from `init`, and `w` of the type `written`. The ranges of the members are counted in `src`.
fn parity_run(
    (ty, init): (&str, &str),
    written: &str,
    join: &(String, Vec<Range<usize>>),
) -> (String, Vec<Range<usize>>, ParityRun) {
    let head = format!(
        "{DECLS}fn main() {{\n    let c = true\n    let n = 1\n    let d: any Display = 1\n    let o: (any Display)? = Some(7)\n    let x: i64? = Some(5)\n    let s: any Shape = Circle(r: 1.0)\n    let e: {ty} = {init}\n    let w: {written} = []\n    let r = "
    );
    let src = format!("{head}{}\n}}", join.0);
    let ranges = (join.1.iter())
        .map(|r| r.start + head.len()..r.end + head.len())
        .collect();
    let (program, ds) = checked(&src);
    let errors = (ds.iter())
        .filter(|d| d.severity() == Severity::Error)
        .map(|d| (d.span().start(), d.code()))
        .collect();
    let types = program.and_then(|p| {
        let main = p.functions.iter().find(|f| f.name == "main")?;
        let local = |name: &str| {
            (main.locals.iter())
                .find(|l| l.name.as_deref() == Some(name))
                .map(|l| l.ty)
        };
        Some((local("r")?, local("w")?))
    });
    (src, ranges, ParityRun { errors, types })
}

/// A branch array in a join with no written type joins as its elements would beside the elements
/// of the earlier branch array, of type `E`: `if c { [e] } else { [v] }` checks clean exactly
/// when `if c { e } else { v }` does and has the type `E`, and then has the type `[E]`; an array
/// does not widen. Else an error is in the later array, or at the `e` that the scalar join
/// blames, and nowhere else. The same holds in a `match` and a `loop`, and one level deeper.
#[test]
fn a_branch_array_should_join_as_its_elements_would_in_a_scalar_join() {
    let mut failures = Vec::new();
    let mut cases = 0;
    let mut used = [false; PARITY_EXCEPTIONS.len()];
    for (ty, init) in PARITY_TYPES {
        for values in PARITY_VALUES {
            for form in [JoinForm::If, JoinForm::Match, JoinForm::Loop] {
                let scalar_members: Vec<String> = std::iter::once("e".to_string())
                    .chain(values.iter().map(ToString::to_string))
                    .collect();
                let (_, ranges, scalar) = parity_run(
                    (ty, init),
                    &format!("[{ty}]"),
                    &join_of(form, &scalar_members),
                );
                // The member that each error of the scalar join is at, and its code.
                let blamed: Vec<(Option<usize>, Code)> = (scalar.errors.iter())
                    .map(|&(at, code)| (ranges.iter().position(|r| r.contains(&at)), code))
                    .collect();
                let builds_as_e = scalar.errors.is_empty()
                    && scalar.types.is_some_and(|(r, w)| Type::array(r) == w);
                for depth in [1, 2] {
                    cases += 1;
                    let (open, close) = ("[".repeat(depth), "]".repeat(depth));
                    let members = [
                        format!("{open}e{close}"),
                        format!("{open}{}{close}", values.join(", ")),
                    ];
                    let written = format!("{open}{ty}{close}");
                    let (src, ranges, array) =
                        parity_run((ty, init), &written, &join_of(form, &members));
                    // Where the element of member `k` of the scalar join is in the array program.
                    let element = |k: usize| {
                        let at = if k == 0 {
                            ranges[0].start + depth
                        } else {
                            let before: usize = values[..k - 1].iter().map(|v| v.len() + 2).sum();
                            ranges[1].start + depth + before
                        };
                        at..at + scalar_members[k].len()
                    };
                    let exception = PARITY_EXCEPTIONS.iter().position(|&(t, v, forms, _)| {
                        t == ty && v == values.join(", ") && forms.contains(&form)
                    });
                    let fine = if builds_as_e {
                        array.errors.is_empty() && array.types.is_some_and(|(r, w)| r == w)
                    } else if let Some(i) = exception {
                        used[i] = true;
                        !array.errors.is_empty()
                            && (array.errors.iter()).all(|&(at, _)| {
                                (1..scalar_members.len()).any(|k| element(k).contains(&at))
                            })
                    } else if blamed.is_empty() {
                        // The scalar join checks clean, as a type that is not `E`: the array
                        // does not widen, so an element of the later array is the mistake.
                        !array.errors.is_empty()
                            && array.errors.iter().all(|&(at, code)| {
                                code == Code::Mismatch
                                    && (1..scalar_members.len()).any(|k| element(k).contains(&at))
                            })
                    } else {
                        !array.errors.is_empty()
                            && array.errors.iter().all(|&(at, code)| {
                                blamed.iter().any(|&(k, c)| {
                                    c == code && k.is_some_and(|k| element(k).contains(&at))
                                })
                            })
                    };
                    if !fine {
                        let found = |run: &ParityRun, src: &str| -> Vec<_> {
                            (run.errors.iter())
                                .map(|&(at, code)| (code, line_col(src, at)))
                                .collect()
                        };
                        failures.push(format!(
                            "E `{ty}`, values `{}`, {form:?}, depth {depth}: the scalar join blames {blamed:?}; the array has errors {:?} and type {:?}",
                            values.join(", "),
                            found(&array, &src),
                            array.types.map(|(r, _)| r),
                        ));
                    }
                }
            }
        }
    }
    for (&(ty, values, _, _), used) in PARITY_EXCEPTIONS.iter().zip(used) {
        if !used {
            failures.push(format!(
                "the exception for E `{ty}`, values `{values}` is not used"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {cases} cases fail:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The locals of the branch array rows, before the row's own lines, which start at line 7.
const BRANCH_ARRAY_LOCALS: &str = "fn main() {\n    let c = true\n    let d: any Display = 1\n    let o: (any Display)? = Some(7)\n    let x: i64? = Some(5)\n    let s: any Shape = Circle(r: 1.0)\n";

/// The branch arrays that reviews found, each with its exact codes and positions: an element of a
/// branch array in a join with no written type joins as the value would beside the elements of
/// the earlier branch array, and a written type outside the join converts each value but an
/// optional.
#[test]
fn a_branch_array_should_report_the_element_that_leaves_the_earlier_elements() {
    let rows: [Row<'_>; 19] = [
        (
            "an `any` value beside optional `any` values",
            "    let r = if c { [o] } else { [d] }",
            vec![],
        ),
        (
            "a number beside optional `any` values",
            "    let r = if c { [o] } else { [2] }",
            vec![],
        ),
        (
            "an optional number beside optional `any` values",
            "    let r = if c { [o] } else { [x] }",
            vec![(Code::Mismatch, (7, 34))],
        ),
        (
            "an empty array beside arrays of `any` values",
            "    let r = if c { [[d]] } else { [[]] }",
            vec![],
        ),
        (
            "an empty array beside a local of arrays of `any` values",
            "    let ys: [[any Display]] = [[d]]\n    let r = if c { ys } else { [[]] }",
            vec![],
        ),
        (
            "a `None` beside arrays of `any` values",
            "    let r = if c { [[d]] } else { [[None]] }",
            vec![(Code::Mismatch, (7, 37))],
        ),
        (
            "an array of `any` values beside optional arrays of them",
            "    let r = if c { [Some([d])] } else { [[d]] }",
            vec![],
        ),
        (
            "an array of `any` values beside a local optional array of them",
            "    let ys: [any Display]? = Some([d])\n    let r = if c { [ys] } else { [[d]] }",
            vec![],
        ),
        (
            "a number after an empty array beside arrays of `any` values",
            "    let r = if c { [[d]] } else { [[], 2] }",
            vec![(Code::Mismatch, (7, 40))],
        ),
        (
            "a number after two empty arrays beside arrays of `any` values",
            "    let r = if c { [[d]] } else { [[], [], 2] }",
            vec![(Code::Mismatch, (7, 44))],
        ),
        (
            "an optional number beside `any` values",
            "    let r = if c { [d] } else { [x] }",
            vec![(Code::Mismatch, (7, 34))],
        ),
        (
            "a `Some` of a number beside `any` values",
            "    let r = if c { [d] } else { [Some(2)] }",
            vec![(Code::Mismatch, (7, 34))],
        ),
        (
            "an optional number beside `any` values, under a written type",
            "    let r: [any Display] = if c { [d] } else { [x] }",
            vec![(Code::Mismatch, (7, 49))],
        ),
        (
            "a `Some` of a number beside `any` values, under a written type",
            "    let r: [any Display] = if c { [d] } else { [Some(2)] }",
            vec![(Code::Mismatch, (7, 49))],
        ),
        (
            "a `None` beside `any` values, under a written optional type",
            "    let r: [(any Display)?] = if c { [d] } else { [None] }",
            vec![],
        ),
        (
            "an object beside `any` values, under a written type",
            "    let r: [any Shape] = if c { [s] } else { [Circle(r: 2.0)] }",
            vec![],
        ),
        (
            "a `None` beside `any` values",
            "    let r = if c { [d] } else { [None] }",
            vec![(Code::Mismatch, (7, 34))],
        ),
        (
            "a `Some` of an object beside `any` values",
            "    let r = if c { [s] } else { [Some(Circle(r: 2.0))] }",
            vec![(Code::Mismatch, (7, 34))],
        ),
        (
            "a number beside `any` values of an interface it does not conform to",
            "    let r = if c { [s] } else { [2] }",
            vec![(Code::NotConform, (7, 34))],
        ),
    ];
    for (row, lines, expected) in rows {
        codes_after(row, &format!("{BRANCH_ARRAY_LOCALS}{lines}\n}}"), &expected);
    }
}

/// The locals of the hinted `Some` rows, before the row's own lines, which start at line 10.
const HINTED_SOME_LOCALS: &str = "fn main() {\n    let c = true\n    let d: any Display = 1\n    let o: (any Display)? = Some(7)\n    let x: i64? = Some(5)\n    let y: i64? = None\n    let s: any Shape = Circle(r: 1.0)\n    let so: (any Shape)? = Some(Circle(r: 1.0))\n    let cx: Circle? = Some(Circle(r: 3.0))\n";

/// A `Some` beside a value of an optional `any` type, with no type written, keeps an optional
/// argument optional: the `Some` is a `T??`, which the join reports. It never boxes the optional
/// into a bare `any` type, as the join of `[o, Some(y)]` does not.
#[test]
fn a_hinted_some_should_not_box_an_optional_argument_into_a_bare_any() {
    let rows: [Row<'_>; 12] = [
        (
            "an array of a `Some` of an optional beside optional `any` values",
            "    let r = if c { [o] } else { [Some(x)] }",
            vec![(Code::Mismatch, (10, 34))],
        ),
        (
            "the same one level deeper",
            "    let r = if c { [[o]] } else { [[Some(x)]] }",
            vec![(Code::Mismatch, (10, 37))],
        ),
        (
            "an array of a `Some` of an empty optional",
            "    let r = if c { [o] } else { [Some(y)] }",
            vec![(Code::Mismatch, (10, 34))],
        ),
        (
            "an `if`",
            "    let r = if c { o } else { Some(y) }",
            vec![(Code::Mismatch, (10, 31))],
        ),
        (
            "a `match`",
            "    let r = match c { true => o; false => Some(y) }",
            vec![(Code::Mismatch, (10, 43))],
        ),
        (
            "the default of `??`",
            "    let r = o ?? Some(y)",
            vec![(Code::Mismatch, (10, 18))],
        ),
        (
            "an array of a `Some` of a `Some`",
            "    let r = if c { [o] } else { [Some(Some(2))] }",
            vec![(Code::Mismatch, (10, 34))],
        ),
        (
            "a written optional `any` type, which the optional does not convert to",
            "    let r: (any Display)? = if c { o } else { Some(y) }",
            vec![(Code::Mismatch, (10, 52))],
        ),
        (
            "a written array of an optional `any` type, which the optional does not convert to",
            "    let r: [(any Display)?] = [Some(y)]",
            vec![(Code::Mismatch, (10, 37))],
        ),
        (
            "a `Some` of an optional beside a bare `any` value",
            "    let r = if c { d } else { Some(x) }",
            vec![(Code::Mismatch, (10, 31))],
        ),
        (
            "the same in an array with no sibling",
            "    let r = [d, Some(x)]",
            vec![(Code::Mismatch, (10, 17))],
        ),
        (
            "a `Some` of a number beside a bare `any` value, which converts",
            "    let r = [d, Some(2)]",
            vec![],
        ),
    ];
    for (row, lines, expected) in rows {
        codes_after(row, &format!("{HINTED_SOME_LOCALS}{lines}\n}}"), &expected);
    }
}

/// Beside `h`, of an optional `any` type, a `Some` checks clean when its argument converts to
/// the `any` type, and is a mismatch at the `Some` when its argument is an optional, in an `if`,
/// a `match`, the default of `??`, a `loop` of `break`s and an array in an `if`.
#[test]
fn a_hinted_some_should_build_only_when_its_argument_is_not_optional() {
    let builds: [(&str, &[&str]); 2] = [
        ("o", &["Some(d)", "Some(2)"]),
        ("so", &["Some(s)", "Some(Circle(r: 2.0))"]),
    ];
    let rejected = [
        "Some(x)",
        "Some(y)",
        "Some(Some(2))",
        "Some(o)",
        "Some(cx)",
        "Some(Some(Circle(r: 2.0)))",
        "Some(so)",
    ];
    let mut failures = Vec::new();
    let mut cases = 0;
    for (h, good) in builds {
        let values = (good.iter()).map(|&v| (v, true));
        for (value, clean) in values.chain(rejected.iter().map(|&v| (v, false))) {
            for form in [
                format!("if c {{ {h} }} else {{ {value} }}"),
                format!("match c {{ true => {h}; false => {value} }}"),
                format!("{h} ?? {value}"),
                format!("loop {{ if c {{ break {h} }}; break {value} }}"),
                format!("if c {{ [{h}] }} else {{ [{value}] }}"),
            ] {
                cases += 1;
                let src = format!("{DECLS}{HINTED_SOME_LOCALS}    let r = {form}\n}}");
                let start = src.rfind(value).expect("the program should hold the value");
                let (_, ds) = checked(&src);
                let errors: Vec<_> = (ds.iter())
                    .filter(|d| d.severity() == Severity::Error)
                    .map(|d| (d.code(), d.span().start()))
                    .collect();
                let fine = if clean {
                    errors.is_empty()
                } else {
                    !errors.is_empty()
                        && (errors.iter()).all(|&(code, at)| {
                            code == Code::Mismatch && (start..start + value.len()).contains(&at)
                        })
                };
                if !fine {
                    let found: Vec<_> = (errors.iter())
                        .map(|&(code, at)| (code, line_col(&src, at)))
                        .collect();
                    failures.push(format!("`{form}`: errors {found:?}"));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {cases} cases fail:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The locals of the rows of an optional and a bare `any` type, before the row's own lines,
/// which start at line 8.
const OPTIONAL_LOCALS: &str = "fn main() {\n    let c = true\n    let d: any Display = 1\n    let o: (any Display)? = Some(d)\n    let x: i64? = Some(5)\n    let y: i64? = None\n    let cx: Circle? = Some(Circle(r: 1.0))\n";

/// The declarations after `main` in the rows of an optional and a bare `any` type.
const OPTIONAL_DECLS: &str = "fn pick<T>(a: T, b: T) -> T { b }\nclass P<T> {\n    a: T\n    b: T\n}\nenum Cs<T> {\n    Two(T, T)\n}\nfn wrap<T: Display>(v: T) -> any Display { v }\nfn show(v: any Display) {}\nclass B {\n    v: any Display\n}\n";

/// An optional never converts to a bare `any I`, whether a type is written or a generic call
/// infers it from an earlier argument: each place reports it at the optional. A `Some` of a
/// value that converts, a `None`, and a bound type parameter whose type is an optional still
/// build.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one row for each place an optional meets a bare `any` type"
)]
fn an_optional_should_not_convert_to_a_bare_any() {
    let rows: [Row<'_>; 21] = [
        (
            "a written type",
            "    let e: any Display = x",
            vec![(Code::Mismatch, (8, 26))],
        ),
        (
            "a written type, from an optional `any` value",
            "    let e: any Display = o",
            vec![(Code::Mismatch, (8, 26))],
        ),
        (
            "a parameter",
            "    show(x)",
            vec![(Code::Mismatch, (8, 10))],
        ),
        (
            "a field",
            "    let b = B(v: x)",
            vec![(Code::Mismatch, (8, 18))],
        ),
        (
            "an array of a written type",
            "    let xs: [any Display] = [x]",
            vec![(Code::Mismatch, (8, 30))],
        ),
        (
            "an optional class, where a class `any` type is written",
            "    let k: any Shape = cx",
            vec![(Code::Mismatch, (8, 24))],
        ),
        (
            "a `Some` of an optional after an optional `any` value, in a generic call",
            "    let r = pick(o, Some(y))",
            vec![(Code::Mismatch, (8, 26))],
        ),
        (
            "the same in a generic class",
            "    let r = P(a: o, b: Some(y))",
            vec![(Code::Mismatch, (8, 29))],
        ),
        (
            "the same in a generic enum case",
            "    let r = Cs.Two(o, Some(y))",
            vec![(Code::Mismatch, (8, 28))],
        ),
        (
            "an optional after an `any` value, in a generic call",
            "    let r = pick(d, x)",
            vec![(Code::Mismatch, (8, 21))],
        ),
        (
            "the same in a generic class",
            "    let r = P(a: d, b: x)",
            vec![(Code::Mismatch, (8, 24))],
        ),
        (
            "a `Some` of a number after an `any` value, in a generic call",
            "    let r = pick(d, Some(2))",
            vec![(Code::Mismatch, (8, 21))],
        ),
        (
            "a hint with no `any` type and an optional argument of another type",
            "    let k: i64?? = None\n    let z: f64? = 1.0\n    let r = if c { k } else { Some(z) }",
            vec![(Code::Mismatch, (10, 36))],
        ),
        (
            "a `Some` of an `any` value",
            "    let w: (any Display)? = Some(d)",
            vec![],
        ),
        ("a `None`", "    let w: (any Display)? = None", vec![]),
        (
            "a `Some` of a number beside an `any` value",
            "    let r = [d, Some(2)]",
            vec![],
        ),
        (
            "the same in an `if`",
            "    let r = if c { d } else { Some(2) }",
            vec![],
        ),
        (
            "a bound type parameter at an optional",
            "    let r = wrap(x)",
            vec![],
        ),
        (
            "a `Some` of an `any` value after an optional `any` value, in a generic call",
            "    let r = pick(o, Some(d))",
            vec![],
        ),
        (
            "a `None` after an optional `any` value, in a generic call",
            "    let r = pick(o, None)",
            vec![],
        ),
        (
            "an unwrapped optional",
            "    let e: any Display = x ?? 0",
            vec![],
        ),
    ];
    for (row, lines, expected) in rows {
        codes_after(
            row,
            &format!("{OPTIONAL_LOCALS}{lines}\n}}\n{OPTIONAL_DECLS}"),
            &expected,
        );
    }
    codes_after(
        "a return",
        "fn give(v: i64?) -> any Display { v }\nfn main() {}",
        &[(Code::Mismatch, (1, 35))],
    );
}
