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

/// Each rule of `any`: the names it takes, the conversions it makes and the ones it does not, and
/// what it has no operator for. The wording is in the fixtures of each code.
#[test]
#[expect(clippy::too_many_lines, reason = "one row for each rule of `any`")]
fn any_types_should_report_each_rule_where_it_is_broken() {
    for (row, src, expected) in [
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
        (
            "`extend` of an interface value",
            "extend any Shape {}\nfn main() {}",
            vec![(Code::CannotExtend, (1, 8))],
        ),
        (
            "a method that the interface lacks",
            "fn f(s: any Shape) {\n    s.perimeter()\n}\nfn main() {}",
            vec![(Code::NoMember, (2, 7))],
        ),
    ] {
        codes_after(row, src, &expected);
    }
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
