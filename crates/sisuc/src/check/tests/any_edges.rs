//! Interface values at the edges that the final review of milestone 8 found: a crash, a false
//! acceptance or a report at the wrong place, each fixed, and the rules that only a mutant of the
//! code broke. Most programs share `PRE`.

use std::fmt::Write as _;

use super::*;
use Code::{
    CannotBeAny, CannotInfer, DefinedTwice, InstantiationCycle, Mismatch, NoMember, NotConform,
    PayloadTooLarge,
};

/// The interfaces and classes that most rows name. A position in a row counts its lines too.
const PRE: &str = "interface Shape {\n    fn area(self) -> f64\n}\n\nclass Circle: Shape {\n    r: f64\n    fn area(self) -> f64 { self.r }\n}\n\nclass Square: Shape {\n    s: f64\n    fn area(self) -> f64 { self.s }\n}\n\nclass Rock {\n    w: f64\n}\n\ninterface Sink<T> {\n    fn put(self, x: T) -> i64\n}\n\ninterface Tap<T> {\n    fn put(self, x: T) -> i64\n}\n\nclass Cup: Sink<i64>, Tap<i64> {\n    n: i64\n    fn put(self, x: i64) -> i64 { x + self.n }\n}\n\nclass W<T> {\n    v: T\n}\n\n";

/// A call whose parameter holds `any I<U>`, with an argument that has no type yet, reports E0050.
#[test]
fn an_any_argument_with_no_type_yet_should_report_cannot_infer() {
    codes_row(
        "an argument of a class of `any`",
        "interface I<T> {}\nclass C<T> { x: T }\nfn f<T>() -> C<T> { loop {} }\nfn g<T>(_x: C<any I<T>>) {}\nfn main() { g(f()) }\n",
        &[(CannotInfer, (5, 15))],
    );
    codes_row(
        "an array of `any`",
        "interface Source<T> {\n    fn get(self) -> T\n}\n\nclass B: Source<i64> {\n    fn get(self) -> i64 {\n        1\n    }\n}\n\nfn empty<T>() -> [T] {\n    []\n}\n\nfn read<U>(s: [any Source<U>]) -> i64 {\n    s.len()\n}\n\nfn main() {\n    print(read(empty()))\n}\n",
        &[(CannotInfer, (20, 16))],
    );
    codes_row(
        "an optional `any`",
        "interface Source<T> {\n    fn get(self) -> T\n}\n\nfn nothing<T>() -> T? {\n    None\n}\n\nfn take<U>(s: (any Source<U>)?) -> i64 {\n    0\n}\n\nfn main() {\n    print(take(nothing()))\n}\n",
        &[(CannotInfer, (14, 16))],
    );
}

/// A call whose result holds `any I<X>` waits for the later argument that binds `X`.
#[test]
fn a_call_should_wait_for_a_later_argument_that_binds_an_any_argument() {
    with_prelude();
    codes_row(
        "`Two<any Source<X>, bool>`",
        "interface Source<T> {}\nclass Two<A, B> {\n    a: A\n    b: B\n}\nfn mka<X>() -> Two<any Source<X>, bool> { panic(\"e\") }\nfn mkr<E>() -> Result<any Source<i64>, E> { panic(\"e\") }\nfn g<T>(_c: Two<T, bool>, _d: Result<T, string>) -> i64 { 0 }\nfn main() { print(g(mka(), mkr())) }\n",
        &[],
    );
}

/// An interface that inherits an interface with `Self` in its arguments cannot be `any`.
#[test]
fn an_interface_whose_inherited_arguments_name_self_should_not_be_any() {
    codes_row(
        "a `let`",
        "interface I<T> {}\ninterface J: I<Self> {}\nclass C: J {}\nfn main() { let _x: any J = C() }\n",
        &[(CannotBeAny, (4, 25))],
    );
    codes_row(
        "the return of a bounded generic function",
        "interface I<T> {}\ninterface J: I<Self> {}\nclass C: J {}\nfn wrap<T: J>(x: T) -> any J { x }\nfn main() { let _x = wrap(C()) }\n",
        &[(CannotBeAny, (4, 28))],
    );
}

/// An alias of `any I<T>`, where `I` bounds `T`, checks the bound at each use with its argument.
#[test]
fn an_alias_of_a_bounded_any_type_should_check_the_bounds_at_each_use() {
    codes_row(
        "an argument that conforms",
        "interface I<T: Eq> {}\ntype A<T> = any I<T>\nfn main() { let _x: A<i64>? = None }\n",
        &[],
    );
    codes_row(
        "an argument that does not conform",
        "interface I<T: Eq> {}\nclass K {}\ntype A<T> = any I<T>\nfn main() {\n    let _x: A<i64>? = None\n    let _y: A<K>? = None\n}\n",
        &[(NotConform, (6, 13))],
    );
    codes_row(
        "through a second alias",
        "interface I<T: Eq> {}\nclass K {}\ntype A<T> = any I<T>\ntype B<U> = A<[U]>\nfn main() {\n    let _x: B<i64>? = None\n    let _y: B<K>? = None\n}\n",
        &[(NotConform, (7, 13))],
    );
}

/// A class or an enum that names itself with a larger argument inside `any` grows without end.
#[test]
fn a_type_that_grows_through_an_any_argument_should_report_the_cycle() {
    codes_row(
        "a field of a class",
        "interface I<T> {}\nclass C<T> {\n    x: (any I<C<[T]>>)?\n}\nfn main() {\n    let c = C<i64>(x: None)\n    print(c.x == None)\n}\n",
        &[(InstantiationCycle, (3, 15))],
    );
    codes_row(
        "a payload of an enum",
        "interface I<T> {}\nenum E<T> {\n    A(any I<E<[T]>>)\n    B\n}\nfn main() {\n    let e = E<i64>.B\n    print(1)\n}\n",
        &[(InstantiationCycle, (3, 13))],
    );
    codes_row(
        "a compared enum, with no second report",
        "interface I<T> {}\nenum E<T> {\n    A(any I<E<[T]>>)\n    B\n}\nfn main() {\n    print(E<i64>.B == E<i64>.B)\n}\n",
        &[(InstantiationCycle, (3, 13))],
    );
}

/// A requirement that returns `any Sink<Item>` gets the conformance's own `Item`.
#[test]
fn an_associated_type_inside_an_any_argument_should_normalize() {
    codes_row(
        "`any Sink<Item>`",
        &format!(
            "{PRE}{}",
            "interface Source {\n    type Item\n    fn sink(self) -> any Sink<Item>\n}\n\nclass Src: Source {\n    type Item = i64\n    c: Cup\n    fn sink(self) -> any Sink<i64> { self.c }\n}\n\nfn go<S: Source>(s: S) -> i64 { 0 }\n\nfn main() {\n    let s = Src(c: Cup(n: 1))\n    print(s.sink().put(2))\n}\n"
        ),
        &[],
    );
}

/// An `extend` block of `W<any I<T>>` covers `W` of `any I` alone, so blocks of two interfaces do
/// not overlap.
#[test]
fn an_extend_of_an_any_argument_should_cover_only_its_interface() {
    codes_row(
        "another interface has no method",
        &format!(
            "{PRE}{}",
            "extend<T> W<any Sink<T>> {\n    fn f(self) -> i64 { 1 }\n}\n\nfn main() {\n    let t: any Tap<i64> = Cup(n: 1)\n    let w = W(v: t)\n    print(w.f())\n}\n"
        ),
        &[(NoMember, (43, 13))],
    );
    codes_row(
        "two interfaces",
        &format!(
            "{PRE}{}",
            "extend<T> W<any Sink<T>> {\n    fn f(self) -> i64 { 1 }\n}\n\nextend<T> W<any Tap<T>> {\n    fn f(self) -> i64 { 2 }\n}\n\nfn main() {\n    let t: any Tap<i64> = Cup(n: 1)\n    print(W(v: t).f())\n}\n"
        ),
        &[],
    );
    codes_row(
        "a generic block and a plain block",
        &format!(
            "{PRE}{}",
            "extend<T> W<any Sink<T>> {\n    fn f(self) -> i64 { 1 }\n}\n\nextend W<any Sink<i64>> {\n    fn f(self) -> i64 { 2 }\n}\n\nfn main() {\n    let t: any Sink<i64> = Cup(n: 1)\n    print(W(v: t).f())\n}\n"
        ),
        &[(DefinedTwice, (41, 8))],
    );
    codes_row(
        "two conformances",
        "interface A<T> {}\ninterface B<T> {}\ninterface Mark {}\nclass Wrap<T> {\n    v: T\n}\nextend<T> Wrap<any A<T>>: Mark {}\nextend<T> Wrap<any B<T>>: Mark {}\nfn main() {}\n",
        &[],
    );
}

/// A type argument in parentheses is the type, and `any` is a name where no name follows it.
#[test]
fn a_grouped_type_argument_should_parse() {
    codes_row(
        "`(any Shape)`",
        &format!(
            "{PRE}{}",
            "fn main() {\n    let w = W<(any Shape)>(v: Circle(r: 1.0))\n    print(w.v.area())\n}\n"
        ),
        &[],
    );
    codes_row(
        "`(i64)?`",
        &format!(
            "{PRE}{}",
            "fn main() {\n    let w = W<(i64)?>(v: None)\n    print(w.v ?? 3)\n}\n"
        ),
        &[],
    );
    codes_row(
        "a class named `any`, optional",
        "class any {\n    x: i64\n}\n\nclass W<T> {\n    v: T\n}\n\nfn main() {\n    let w = W<(any)?>(v: None)\n    print(w.v?.x ?? 3)\n}\n",
        &[],
    );
    codes_row(
        "a class named `any`, a type argument",
        "class any {\n    n: i64\n}\nfn id<T>(x: T) -> T { x }\nfn main() {\n    print(id<any>(any(n: 3)).n)\n}\n",
        &[],
    );
}

/// A value that cannot convert to the `any` type of its siblings is the one that a report names.
#[test]
fn a_conversion_mistake_should_be_reported_at_the_value_that_cannot_convert() {
    codes_row(
        "an element of an array branch",
        &format!(
            "{PRE}{}",
            "fn main() {\n    let b = true\n    let xs: [any Shape] = [Circle(r: 1.0)]\n    let ys = if b { xs } else { [Circle(r: 2.0), None] }\n    print(ys.len())\n}\n"
        ),
        &[(Mismatch, (39, 50))],
    );
    codes_row(
        "an element beside an `any` element",
        "fn main() {\n    let d: any Display = 3\n    let xs = if true { [d] } else { [1, None] }\n    print(xs.len())\n}\n",
        &[(Mismatch, (3, 41))],
    );
    codes_row(
        "a `break` of two optionals",
        &format!(
            "{PRE}{}",
            "fn main() {\n    let b = true\n    let a: any Shape = Circle(r: 1.0)\n    let r = loop {\n        if b { break a }\n        break Some(Some(Rock(w: 1.0)))\n    }\n    print(0)\n}\n"
        ),
        &[(Mismatch, (41, 15))],
    );
    codes_row(
        "an `any` of another interface",
        &format!(
            "{PRE}{}",
            "fn f<T>(x: any Sink<T>) -> i64 { 1 }\n\nfn main() {\n    let t: any Tap<i64> = Cup(n: 1)\n    print(f(t))\n}\n"
        ),
        &[(Mismatch, (40, 13))],
    );
}

/// A `??` that does not fit is reported at the place that the comments of `coalesce` state.
#[test]
fn a_coalesce_should_be_reported_where_its_code_says() {
    codes_row(
        "a default of another class",
        &format!(
            "{PRE}{}",
            "fn main() {\n    let c: Circle? = None\n    let x: i64 = c ?? Rock(w: 1.0)\n    print(x)\n}\n"
        ),
        &[(Mismatch, (38, 23))],
    );
    codes_row(
        "`None` after an optional class",
        &format!(
            "{PRE}{}",
            "fn main() {\n    let c: Circle? = None\n    let r: any Shape = c ?? None\n    print(0)\n}\n"
        ),
        &[(Mismatch, (38, 24))],
    );
    codes_row(
        "`None` after an optional `any` of a child",
        &format!(
            "{PRE}{}",
            "interface Named {\n    fn name(self) -> string\n}\n\ninterface Both: Shape, Named {}\n\nfn main() {\n    let c: (any Both)? = None\n    let r: any Shape = c ?? None\n    print(0)\n}\n"
        ),
        &[(Mismatch, (44, 24))],
    );
    codes_row(
        "`None` after an optional `any`",
        &format!(
            "{PRE}{}",
            "fn main() {\n    let c: (any Shape)? = None\n    let r: any Shape = c ?? None\n    print(0)\n}\n"
        ),
        &[(Mismatch, (38, 29))],
    );
    codes_row(
        "a default of another conforming class",
        &format!(
            "{PRE}{}",
            "fn main() {\n    let c: Circle? = None\n    let r: any Shape = c ?? Square(s: 2.0)\n    print(r.area())\n}\n"
        ),
        &[(Mismatch, (38, 29))],
    );
    codes_row(
        "a default that does not conform",
        &format!(
            "{PRE}{}",
            "fn main() {\n    let c: Circle? = None\n    let r: any Shape = c ?? Rock(w: 1.0)\n    print(0)\n}\n"
        ),
        &[(NotConform, (38, 29))],
    );
}

/// A `loop` whose type is written as `any Shape` converts each `break` value to it.
#[test]
fn a_written_any_type_should_convert_each_break_value() {
    codes_row(
        "two classes",
        &format!(
            "{PRE}{}",
            "fn main() {\n    let b = true\n    let r: any Shape = loop {\n        if b { break Circle(r: 1.0) }\n        break Square(s: 2.0)\n    }\n    print(r.area())\n}\n"
        ),
        &[],
    );
}

/// An array after an array branch of a plain type expects that type, so each element that is not of
/// it is reported.
#[test]
fn a_hinted_array_with_no_any_element_should_report_each_element() {
    codes_row(
        "two strings after an `i64` array",
        "fn main() {\n    let b = true\n    let ys = if b { [1] } else { [\"a\", \"b\"] }\n    print(ys)\n}\n",
        &[(Mismatch, (3, 35)), (Mismatch, (3, 40))],
    );
}

/// A generic call whose result is or holds an `any` type takes from its context the arguments of
/// that type alone.
#[test]
fn a_generic_call_in_an_any_context_should_bind_from_each_argument_of_the_any_type() {
    codes_row(
        "an optional `any` context binds the parameter",
        "interface Src<T> {}\nfn mk<T>() -> any Src<T> { panic(\"e\") }\nfn main() {\n    let _x: (any Src<i64>)? = mk()\n}\n",
        &[],
    );
    codes_row(
        "a fixed argument meets a context part that is open",
        "interface P<A, B> {}\nclass W<T> {\n    v: T\n}\nfn fix<A>(x: A) -> W<any P<i64, A>> { panic(\"e\") }\nfn h<U, V>(c: V, b: W<any P<U, V>>) {}\nfn main() {\n    h(true, fix(\"s\"))\n}\n",
        &[(Mismatch, (8, 17))],
    );
    codes_row(
        "an open context part gives a later argument context",
        "interface P<A, B> {}\nclass W<T> {\n    v: T\n}\nclass Box<A, B> {\n    a: A\n    b: B\n}\nfn mk<A, B>(a: A, b: B) -> W<any P<A, B>> { panic(\"e\") }\nfn h<U, V>(a: U, b: W<any P<U, Box<V, i64>>>) {}\nfn main() {\n    h(true, mk(\"s\", Box(a: 1, b: \"x\")))\n}\n",
        &[(Mismatch, (12, 16)), (Mismatch, (12, 34))],
    );
    codes_row(
        "an `any` of another interface gives no context",
        "interface P<A, B> {}\ninterface Q<A, B> {}\nclass W<T> {\n    v: T\n}\nclass Box<A, B> {\n    a: A\n    b: B\n}\nfn mk<A, B>(a: A, b: B) -> W<any P<A, B>> { panic(\"e\") }\nfn h<U, V>(a: U, b: W<any Q<U, Box<V, i64>>>) {}\nfn main() {\n    h(true, mk(\"s\", Box(a: 1, b: \"x\")))\n}\n",
        &[(Mismatch, (13, 13))],
    );
}

/// The text of a program with enums `E0` to `E{levels}`, each with two of the next one, and the
/// last with an `any Shape`: the payload of `E0` is `2^levels` times two words.
fn doubling_any(levels: usize) -> String {
    let mut src = String::from("interface Shape {\n    fn area(self) -> f64\n}\n");
    for k in 0..levels {
        let next = k + 1;
        writeln!(src, "enum E{k} {{\n    End\n    A(E{next}, E{next})\n}}")
            .expect("writing to a `String` should succeed");
    }
    writeln!(src, "enum E{levels} {{\n    End\n    A(any Shape)\n}}")
        .expect("writing to a `String` should succeed");
    src + "fn main() {\n    let _e = E0.End\n}"
}

/// An `any` value is two words, so 28 levels of doubling reach the limit of 4 GiB and 27 do not.
#[test]
fn an_enum_that_holds_an_any_value_should_count_two_words() {
    codes_row("27 levels", &doubling_any(27), &[]);
    codes_row("28 levels", &doubling_any(28), &[(PayloadTooLarge, (4, 6))]);
}
