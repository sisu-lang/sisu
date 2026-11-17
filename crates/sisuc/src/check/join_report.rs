#[cfg(test)]
mod tests {
    use super::super::tests::{at, eye, if_else, int, local, orders};
    use super::super::*;
    use crate::check::expectation::Origin;
    use crate::check::infer::Kind;
    use crate::check::operator::Operator;
    use crate::check::tests::joins::with_params;
    use crate::diagnostic::{Code, Diagnostic};
    use crate::tir::FuncId;

    /// `Seen`, with the structs `Eye` and `Lid`, which conform to it.
    const EYE_LID: &str = "interface Seen {\n    fn seen(self) -> i64\n}\nstruct Eye: Seen {\n    v: i64\n    fn seen(self) -> i64 { self.v }\n}\nstruct Lid: Seen {\n    v: i64\n    fn seen(self) -> i64 { self.v }\n}\n";

    /// `Seen` and `Heard`, which both `Eye` and `Lid` conform to.
    const EYE_LID_HEARD: &str = "interface Seen {\n    fn seen(self) -> i64\n}\ninterface Heard {\n    fn heard(self) -> i64\n}\nstruct Eye: Seen, Heard {\n    v: i64\n    fn seen(self) -> i64 { self.v }\n    fn heard(self) -> i64 { self.v }\n}\nstruct Lid: Seen, Heard {\n    v: i64\n    fn seen(self) -> i64 { self.v }\n    fn heard(self) -> i64 { self.v }\n}\n";

    /// The functions `u` and `g`, which give `unit`.
    const UNITS: &str = "fn u() {}\nfn g() {}\n";

    fn head() -> Span {
        Span::new(10, 12)
    }

    /// Each of `values` as a whole member.
    fn whole(values: &mut [Checked]) -> Vec<JoinMember<'_>> {
        (values.iter_mut())
            .map(|value| {
                let label = match value {
                    Ok(e) => e.span,
                    Err(_) => at(0),
                };
                JoinMember {
                    value,
                    at: Place::Whole,
                    label,
                }
            })
            .collect()
    }

    /// The join of `values`, each a whole member, with the head `head()`.
    fn join_at(
        c: &mut Checker<'_>,
        form: JoinForm,
        values: &mut [Checked],
        expected: Expectation,
        binding: Option<Span>,
    ) -> Result<Type, Poisoned> {
        let mut members = whole(values);
        c.join_form(form, head(), &mut members, expected, binding)
    }

    fn required(ty: Type) -> Expectation {
        Expectation::Required(ty, Origin::None)
    }

    /// The diagnostics so far.
    fn diagnostics(c: &Checker<'_>) -> Vec<Diagnostic> {
        c.diagnostics.iter().cloned().collect()
    }

    /// The code and the primary span of each diagnostic so far.
    fn found(c: &Checker<'_>) -> Vec<(Code, Span)> {
        (diagnostics(c).iter())
            .map(|d| (d.code(), d.span()))
            .collect()
    }

    /// The span of each secondary label of the only diagnostic so far.
    fn labels(c: &Checker<'_>) -> Vec<Span> {
        let [d] = diagnostics(c).try_into().unwrap_or_else(|all: Vec<_>| {
            panic!("one diagnostic should be reported, not {}", all.len())
        });
        d.secondary().iter().map(|&(span, _)| span).collect()
    }

    /// The call `f()` of the function `name`, which gives `unit`.
    fn unit_call(c: &Checker<'_>, name: &str, n: usize) -> tir::Expr {
        let func = (c.signatures.iter())
            .position(|signature| signature.name == name)
            .expect("the program should declare the function");
        let kind = ExprKind::Call {
            func: FuncId(func),
            types: TypeArgs::NONE,
            args: Vec::new(),
        };
        tir::Expr::new(kind, Type::UNIT, at(n))
    }

    fn string(n: usize) -> tir::Expr {
        tir::Expr::new(ExprKind::Str("s".to_owned()), Type::STRING, at(n))
    }

    #[test]
    fn conflict_should_put_the_primary_label_at_the_head() {
        with_params("", "a: u8, s: string", |c, types| {
            // GIVEN the members `a: u8` and `s: string`
            let mut values = vec![Ok(local(types[0], 1)), Ok(local(types[1], 2))];
            // WHEN they join, with no required type
            let joined = join_at(c, JoinForm::If, &mut values, Expectation::None, None);
            // THEN one E0032 at the head, and the join is untyped poison
            assert_eq!(found(c), vec![(Code::Mismatch, head())]);
            assert!(joined.err().is_some_and(|poisoned| poisoned.ty.is_none()));
        });
    }

    #[test]
    fn conflict_should_label_each_group_in_the_same_order_for_each_member_order() {
        let run = |order: &[usize]| {
            with_params("", "a: u8, s: string, b: bool", |c, types| {
                let mut made: Vec<Option<Checked>> =
                    (0..3).map(|i| Some(Ok(local(types[i], i + 1)))).collect();
                let mut values: Vec<Checked> = (order.iter())
                    .map(|&i| made[i].take().expect("each index should come once"))
                    .collect();
                let _ = join_at(c, JoinForm::If, &mut values, Expectation::None, None);
                labels(c)
            })
        };
        // GIVEN three members of three types, WHEN they join in each order
        // THEN the labels are the same, one at each member, in the order of the source
        for order in orders(3) {
            assert_eq!(run(&order), vec![at(1), at(2), at(3)], "order {order:?}");
        }
    }

    #[test]
    fn conflict_should_name_each_member_at_each_depth() {
        with_params(EYE_LID, "e: string, x: Eye, y: Lid", |c, types| {
            // GIVEN `if c { e } else if d { Eye(v: 1) } else { Lid(v: 2) }`, with `e: string`
            let inner_ty = c.table.fresh(Kind::Any, at(4));
            let inner = if_else(eye(types[1], 2), eye(types[2], 3), inner_ty, 4);
            let outer_ty = c.table.fresh(Kind::Any, at(5));
            let outer = if_else(local(types[0], 1), inner, outer_ty, 5);
            let mut values = vec![Ok(outer)];
            // WHEN it joins
            let _ = join_at(c, JoinForm::If, &mut values, Expectation::None, None);
            // THEN one E0032 at the head, with a label at `e`, at `Eye(v: 1)` and at `Lid(v: 2)`
            assert_eq!(found(c), vec![(Code::Mismatch, head())]);
            assert_eq!(labels(c), vec![at(1), at(2), at(3)]);
        });
    }

    #[test]
    fn part_member_should_get_the_argument_label() {
        with_params("", "names: [string], n: i64", |c, types| {
            // GIVEN `names` at a parameter `[T]` and `n` at a parameter `T`
            let hole = Symbol::intern("T");
            let place = Type::array(Type::intern(TypeKind::Param(hole, 0)));
            let (mut first, mut second) = (Ok(local(types[0], 2)), Ok(local(types[1], 3)));
            let mut members = [
                JoinMember {
                    value: &mut first,
                    at: Place::Part {
                        place,
                        argument: at(2),
                    },
                    label: at(2),
                },
                JoinMember {
                    value: &mut second,
                    at: Place::Whole,
                    label: at(3),
                },
            ];
            // WHEN they join at `T`
            let form = JoinForm::Call { param: hole };
            let joined = c.join_form(form, head(), &mut members, Expectation::None, None);
            // THEN one E0032 at the head, with one label at `names` and one at `n`
            assert!(joined.is_err());
            assert_eq!(found(c), vec![(Code::Mismatch, head())]);
            assert_eq!(labels(c), vec![at(2), at(3)]);
        });
    }

    /// The source text of `if c { a } else { b }`, with `a` at 7 and `b` at 18.
    const IF_TEXT: &str = "if c { a } else { b }";

    /// The text of `IF_TEXT` after the fix of its join of `a` and `b`, in each order of the two.
    fn fixed_text(params: &str) -> Option<String> {
        let mut texts = Vec::new();
        for order in orders(2) {
            let text = with_params("", params, |c, types| {
                let spans = [Span::new(7, 8), Span::new(18, 19)];
                let mut values: Vec<Checked> = (order.iter())
                    .map(|&i| {
                        let mut value = local(types[i], 0);
                        value.span = spans[i];
                        Ok(value)
                    })
                    .collect();
                let _ = join_at(c, JoinForm::If, &mut values, Expectation::None, None);
                let [d] = diagnostics(c).try_into().unwrap_or_else(|all: Vec<_>| {
                    panic!("one diagnostic should be reported, not {}", all.len())
                });
                match d.fixes() {
                    [] => None,
                    [fix] => {
                        assert!(!fix.safe(), "a conversion is no safe fix");
                        Some(fix.apply(IF_TEXT))
                    }
                    more => panic!("{} fixes", more.len()),
                }
            });
            texts.push(text);
        }
        assert_eq!(texts[0], texts[1], "the fix should not depend on the order");
        texts.swap_remove(0)
    }

    #[test]
    fn conversion_fix_should_widen_u8_to_i64() {
        // GIVEN `if c { a } else { b }` with `a: u8` and `b: i64`
        // WHEN the branches do not join, in each order
        // THEN the fix converts `a`, and `u8` to `f64` goes through `i64`
        assert_eq!(
            fixed_text("a: u8, b: i64").as_deref(),
            Some("if c { i64(a) } else { b }")
        );
        assert_eq!(
            fixed_text("a: u8, b: f64").as_deref(),
            Some("if c { f64(i64(a)) } else { b }")
        );
    }

    #[test]
    fn conversion_fix_should_not_exist_for_i64_and_f64() {
        // GIVEN branches of `i64` and `f64`, or of `i64` and `string`, THEN there is no fix
        assert_eq!(fixed_text("a: i64, b: f64"), None);
        assert_eq!(fixed_text("a: i64, b: string"), None);
    }

    #[test]
    fn conversion_fix_should_skip_the_assigned_side() {
        let compound = |params: &str| {
            with_params("", params, |c, types| {
                // GIVEN `s += a`, where the member `s` is the place it writes to
                let (mut s, mut a) = (local(types[0], 0), local(types[1], 0));
                (s.span, a.span) = (Span::new(0, 1), Span::new(5, 6));
                let mut values = vec![Ok(s), Ok(a)];
                let op = Operator::Plus { compound: true };
                let form = JoinForm::Compound { op, assigned: 0 };
                let mut members = whole(&mut values);
                // WHEN they join
                let joined =
                    c.join_form(form, Span::new(2, 4), &mut members, Expectation::None, None);
                assert!(joined.is_err());
                let [d] = diagnostics(c).try_into().unwrap_or_else(|all: Vec<_>| {
                    panic!("one diagnostic should be reported, not {}", all.len())
                });
                d.fixes().first().map(|fix| fix.apply("s += a"))
            })
        };
        // THEN with `s: i64` and `a: u8` the fix converts `a`
        assert_eq!(compound("s: i64, a: u8").as_deref(), Some("s += i64(a)"));
        // AND with `s: u8` and `b: i64` there is no fix: it would convert `s`
        assert_eq!(compound("s: u8, a: i64"), None);
    }

    /// The only diagnostic of the join of `eye` and `lid` (`Eye(v: 1)` and `Lid(v: 2)`), with
    /// `binding` as the name of a `let` without a type.
    fn eye_lid(src: &str, form: JoinForm, binding: Option<Span>) -> Diagnostic {
        with_params(src, "x: Eye, y: Lid", |c, types| {
            let mut values = vec![Ok(eye(types[0], 1)), Ok(eye(types[1], 2))];
            let joined = join_at(c, form, &mut values, Expectation::None, binding);
            assert!(joined.is_err());
            let [d] = diagnostics(c).try_into().unwrap_or_else(|all: Vec<_>| {
                panic!("one diagnostic should be reported, not {}", all.len())
            });
            d
        })
    }

    /// `let x = ...`, with the name at 4.
    const LET_TEXT: &str = "let x = ...";

    #[test]
    fn one_declared_interface_should_give_an_unsafe_fix_at_a_let() {
        // GIVEN `let x = if c { Eye(v: 1) } else { Lid(v: 2) }`: only `Seen` fits both, and the
        // language interface `Display` fits too
        let binding = Some(Span::new(4, 5));
        let d = eye_lid(EYE_LID, JoinForm::If, binding);
        // THEN the fix writes `: any Seen` after `x`, and it is unsafe
        let [fix] = d.fixes() else {
            panic!("one fix should be offered, not {}", d.fixes().len());
        };
        assert!(!fix.safe());
        assert_eq!(fix.apply(LET_TEXT), "let x: any Seen = ...");
        // AND an array of them writes `[any Seen]`
        let d = eye_lid(EYE_LID, JoinForm::Array, binding);
        let [fix] = d.fixes() else {
            panic!("one fix should be offered, not {}", d.fixes().len());
        };
        assert_eq!(fix.apply(LET_TEXT), "let x: [any Seen] = ...");
        // AND with no `let` it is a help with no fix
        let d = eye_lid(EYE_LID, JoinForm::If, None);
        assert_eq!(d.fixes().len(), 0);
        assert!(d.help().is_some());
    }

    #[test]
    fn two_declared_interfaces_should_give_no_fix() {
        // GIVEN `let x = if c { Eye(v: 1) } else { Lid(v: 2) }`, where `Seen` and `Heard` fit both
        let d = eye_lid(EYE_LID_HEARD, JoinForm::If, Some(Span::new(4, 5)));
        // THEN the help names them, and there is no fix
        assert_eq!(d.fixes().len(), 0);
        assert!(d.help().is_some());
    }

    #[test]
    fn unit_member_should_give_e0043_no_value() {
        with_params(UNITS, "", |c, _| {
            // GIVEN `[u(), 1]`, WHEN it joins with no required type
            let one = int(c, 1, 2);
            let mut values = vec![Ok(unit_call(c, "u", 1)), Ok(one)];
            let _ = join_at(c, JoinForm::Array, &mut values, Expectation::None, None);
            // THEN E0043 at `u()`, in place of E0032
            assert_eq!(found(c), vec![(Code::NoValue, at(1))]);
        });
        with_params(UNITS, "", |c, _| {
            // GIVEN `if c { u() } else { 1 }`, THEN the same
            let one = int(c, 1, 2);
            let mut values = vec![Ok(unit_call(c, "u", 1)), Ok(one)];
            let _ = join_at(c, JoinForm::If, &mut values, Expectation::None, None);
            assert_eq!(found(c), vec![(Code::NoValue, at(1))]);
        });
        with_params(UNITS, "", |c, _| {
            // GIVEN `let b: u8 = id(u())`: one member at a `T`
            let mut values = vec![Ok(unit_call(c, "u", 1))];
            let form = JoinForm::Call {
                param: Symbol::intern("T"),
            };
            let _ = join_at(c, form, &mut values, required(Type::U8), None);
            // THEN E0043 at `u()`
            assert_eq!(found(c), vec![(Code::NoValue, at(1))]);
        });
        with_params(UNITS, "", |c, _| {
            // GIVEN `let x: u8 = if c { u() } else { u() }`, and the same `match`
            let mut values = vec![Ok(unit_call(c, "u", 1)), Ok(unit_call(c, "u", 2))];
            let _ = join_at(c, JoinForm::If, &mut values, required(Type::U8), None);
            // THEN one E0043 at the whole `if`
            assert_eq!(found(c), vec![(Code::NoValue, head())]);
        });
        with_params(UNITS, "", |c, _| {
            let mut values = vec![Ok(unit_call(c, "u", 1)), Ok(unit_call(c, "u", 2))];
            let _ = join_at(c, JoinForm::Match, &mut values, required(Type::U8), None);
            assert_eq!(found(c), vec![(Code::NoValue, head())]);
        });
        with_params(UNITS, "", |c, _| {
            // GIVEN `let xs: [u8] = [u(), u()]`, THEN E0043 at each `u()`
            let mut values = vec![Ok(unit_call(c, "u", 1)), Ok(unit_call(c, "u", 2))];
            let _ = join_at(c, JoinForm::Array, &mut values, required(Type::U8), None);
            assert_eq!(
                found(c),
                vec![(Code::NoValue, at(1)), (Code::NoValue, at(2))]
            );
        });
        with_params(UNITS, "", |c, _| {
            // GIVEN `let x: u8 = pick(u(), u())`, THEN E0043 at each `u()`
            let mut values = vec![Ok(unit_call(c, "u", 1)), Ok(unit_call(c, "u", 2))];
            let form = JoinForm::Call {
                param: Symbol::intern("T"),
            };
            let _ = join_at(c, form, &mut values, required(Type::U8), None);
            assert_eq!(
                found(c),
                vec![(Code::NoValue, at(1)), (Code::NoValue, at(2))]
            );
        });
        with_params(UNITS, "", |c, _| {
            // GIVEN `let xs: [u8] = [if c { u() } else { u() }]`: a nested join of `unit` leaves
            let nested_ty = c.table.fresh(Kind::Any, at(3));
            let nested = if_else(unit_call(c, "u", 1), unit_call(c, "u", 2), nested_ty, 3);
            let mut values = vec![Ok(nested)];
            let _ = join_at(c, JoinForm::Array, &mut values, required(Type::U8), None);
            // THEN one E0043, at the head of the nested `if`
            assert_eq!(found(c), vec![(Code::NoValue, at(3))]);
        });
        with_params(UNITS, "", |c, _| {
            // GIVEN `let y: unit? = if c { g() } else { g() }`
            let mut values = vec![Ok(unit_call(c, "g", 1)), Ok(unit_call(c, "g", 2))];
            let optional = Type::optional(Type::UNIT);
            let joined = join_at(c, JoinForm::If, &mut values, required(optional), None);
            // THEN it is valid
            assert_eq!(joined.ok(), Some(optional));
            assert_eq!(found(c).len(), 0);
        });
    }

    #[test]
    fn join_under_a_required_type_should_report_once_when_the_members_agree() {
        let hole = Symbol::intern("T");
        let forms = [
            JoinForm::If,
            JoinForm::Array,
            JoinForm::Call { param: hole },
            JoinForm::Loop,
        ];
        for form in forms {
            with_params("", "a: u8, b: u8", |c, types| {
                // GIVEN the members `a: u8` and `b: u8` under the required type `string`
                let mut values = vec![Ok(local(types[0], 1)), Ok(local(types[1], 2))];
                // WHEN they join
                let _ = join_at(c, form, &mut values, required(Type::STRING), None);
                // THEN one E0032, at the whole join
                assert_eq!(found(c), vec![(Code::Mismatch, head())]);
            });
        }
    }

    #[test]
    fn join_under_a_required_type_should_report_at_the_member_when_one_member_gives_the_value() {
        with_params("", "n: u8", |c, types| {
            // GIVEN `let b: string = id(n)`: one member `n: u8` at a `T`
            let mut values = vec![Ok(local(types[0], 1))];
            let form = JoinForm::Call {
                param: Symbol::intern("T"),
            };
            // WHEN it joins under the required type `string`
            let _ = join_at(c, form, &mut values, required(Type::STRING), None);
            // THEN the directed E0032 stays at `n`, not at the call
            assert_eq!(found(c), vec![(Code::Mismatch, at(1))]);
        });
    }

    #[test]
    fn join_under_a_required_type_should_report_each_member_that_does_not_convert() {
        with_params("", "a: u8", |c, types| {
            // GIVEN `[a, "s"]` with `a: u8`, under the element type `string` that a `let` declares
            let origin = Origin::Declared {
                at: at(30),
                ty: Type::array(Type::STRING),
            };
            let mut values = vec![Ok(local(types[0], 1)), Ok(string(2))];
            // WHEN it joins
            let expected = Expectation::Required(Type::STRING, origin);
            let _ = join_at(c, JoinForm::Array, &mut values, expected, None);
            // THEN one E0032, at `a`, with a label at the declaration
            assert_eq!(found(c), vec![(Code::Mismatch, at(1))]);
            assert_eq!(labels(c), vec![at(30)]);
        });
    }
}
