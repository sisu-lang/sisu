//! Tests of the generic rules.

use super::{Body, GField, GFunc, GTy, GVariant, Generic, MAX_SOLVE, Shape, unify};
use crate::generate::scope::Ty;
use crate::generate::{BUDGET, FUEL, Gen, MAX_NESTING, MAX_SIZE, STATEMENT_MOST, generate};
use crate::tir::Rank;

/// No generic function.
const NONE: [usize; 0] = [];

fn param(i: usize) -> GTy {
    GTy::Param(i)
}

fn plain(ty: Ty) -> GTy {
    GTy::Plain(ty)
}

fn array(inner: GTy) -> GTy {
    GTy::Array(Box::new(inner))
}

fn optional(inner: GTy) -> GTy {
    GTy::Optional(Box::new(inner))
}

fn field(name: &str, ty: GTy, mutable: bool) -> GField {
    GField {
        name: name.to_string(),
        ty,
        mutable,
    }
}

fn variant(name: &str, fields: Vec<GTy>, recursive: bool) -> GVariant {
    GVariant {
        name: name.to_string(),
        fields,
        recursive,
    }
}

/// `class G0<T, U> { g1: T  g2: [U]  g3: T? }`.
fn pair_class() -> Generic {
    Generic {
        params: 2,
        body: Body::Class(vec![
            field("g1", param(0), false),
            field("g2", array(param(1)), true),
            field("g3", optional(param(0)), false),
        ]),
    }
}

/// `enum G<n><T> { V1  V2(T) }`.
fn wrapper() -> Generic {
    Generic {
        params: 1,
        body: Body::Enum(vec![
            variant("V1", Vec::new(), false),
            variant("V2", vec![param(0)], false),
        ]),
    }
}

/// `fn h<T>(x: T) -> T`.
fn identity(cost: u64) -> GFunc {
    GFunc {
        name: "h1".to_string(),
        params: 1,
        args: vec![param(0)],
        ret: param(0),
        cost,
        extra: 0,
    }
}

/// Declares the generics `generics` in `g`, in order, as the program text and the scope.
fn declare(g: &mut Gen, generics: Vec<Generic>) {
    for (id, generic) in generics.into_iter().enumerate() {
        match generic.body {
            Body::Enum(variants) => g.declare_enum(id, generic.params, variants),
            Body::Class(_) => g.scope.generics.push(generic),
        }
    }
}

#[test]
fn unify_should_bind_a_parameter_to_the_type_it_stands_for() {
    let pattern = GTy::Applied(0, vec![param(0), array(param(1))]);
    let target = GTy::Applied(0, vec![plain(Ty::I64), array(plain(Ty::String))]);
    let mut binds = vec![None, None];
    assert!(unify(&pattern, &target, &mut binds));
    assert_eq!(binds, [Some(plain(Ty::I64)), Some(plain(Ty::String))]);
}

#[test]
fn unify_should_refuse_a_parameter_bound_to_two_types() {
    let pattern = GTy::Applied(0, vec![param(0), param(0)]);
    let same = GTy::Applied(0, vec![plain(Ty::I64), plain(Ty::I64)]);
    let other = GTy::Applied(0, vec![plain(Ty::I64), plain(Ty::U8)]);
    assert!(unify(&pattern, &same, &mut [None]));
    assert!(!unify(&pattern, &other, &mut [None]));
}

#[test]
fn unify_should_refuse_an_optional_for_a_parameter() {
    // E0138: an optional is no type argument.
    assert!(!unify(&param(0), &optional(plain(Ty::I64)), &mut [None]));
    assert!(unify(
        &optional(param(0)),
        &optional(plain(Ty::I64)),
        &mut [None]
    ));
}

#[test]
fn unify_should_refuse_another_generic_or_another_shape() {
    let target = GTy::Applied(0, vec![plain(Ty::I64)]);
    let mut binds = [None];
    assert!(!unify(
        &GTy::Applied(1, vec![param(0)]),
        &target,
        &mut binds
    ));
    assert!(!unify(&array(param(0)), &target, &mut binds));
    assert!(!unify(&plain(Ty::I64), &target, &mut binds));
    assert!(!unify(&plain(Ty::I64), &plain(Ty::U8), &mut binds));
    assert_eq!(binds, [None]);
}

#[test]
fn lift_should_give_an_instance_its_generic_and_arguments() {
    let mut g = Gen::new(&[]);
    g.scope.generics.push(pair_class());
    let ty = g.instance(0, &[Ty::I64, Ty::String]);
    let lifted = GTy::lift(&Ty::Array(Box::new(Ty::Optional(Box::new(ty)))));
    let applied = GTy::Applied(0, vec![plain(Ty::I64), plain(Ty::String)]);
    assert_eq!(lifted, array(optional(applied)));
    assert_eq!(GTy::lift(&Ty::class(0, 9)), plain(Ty::class(0, 9)));
}

#[test]
fn subst_should_put_the_arguments_in_every_position() {
    let ty = GTy::Applied(1, vec![param(1), array(param(0))]);
    let args = [plain(Ty::I64), plain(Ty::Bool)];
    let want = GTy::Applied(1, vec![plain(Ty::Bool), array(plain(Ty::I64))]);
    assert_eq!(optional(ty).subst(&args), optional(want));
    assert_eq!(plain(Ty::U8).subst(&args), plain(Ty::U8));
}

#[test]
fn name_all_should_find_a_parameter_in_every_position() {
    let nested = GTy::Applied(0, vec![array(param(1))]);
    assert!(GTy::name_all([&param(0), &nested], 2));
    assert!(GTy::name_all([&optional(param(0))], 1));
    assert!(!GTy::name_all([&param(0), &plain(Ty::I64)], 2));
    assert!(!GTy::name_all([&array(param(1))], 2));
}

#[test]
fn an_instance_should_put_its_arguments_into_its_fields() {
    let mut g = Gen::new(&[]);
    declare(&mut g, vec![pair_class()]);
    let ty = g.instance(0, &[Ty::I64, Ty::String]);
    assert_eq!(ty.to_string(), "G0<i64, string>");
    let class = &g.scope.classes[0];
    let types: Vec<String> = class.fields.iter().map(|f| f.ty.to_string()).collect();
    assert_eq!(types, ["i64", "[string]", "i64?"]);
    // The object, and the `[string]` field's room: `GCAP` beyond its floor.
    let caps: Vec<u64> = class.fields.iter().map(|f| f.cap).collect();
    assert_eq!(caps, [0, super::GCAP, 0]);
    assert_eq!(class.size, 1 + super::GCAP);
    let mutable: Vec<bool> = class.fields.iter().map(|f| f.mutable).collect();
    assert_eq!(mutable, [false, true, false]);
    assert_eq!(ty, g.scope.class_ty(0));
    // The same arguments name the same class; others, another.
    assert_eq!(g.instance(0, &[Ty::I64, Ty::String]), ty);
    assert_eq!(g.scope.classes.len(), 1);
    assert_ne!(g.instance(0, &[Ty::U8, Ty::String]), ty);
    assert_eq!(g.scope.classes.len(), 2);
}

#[test]
fn an_enum_instance_should_name_itself_in_a_recursive_variant() {
    let mut g = Gen::new(&[]);
    declare(
        &mut g,
        vec![Generic {
            params: 1,
            body: Body::Enum(vec![
                variant("V1", Vec::new(), false),
                variant("V2", vec![GTy::Applied(0, vec![param(0)]), param(0)], true),
            ]),
        }],
    );
    let ty = g.instance(0, &[Ty::String]);
    let e = &g.scope.enums[0];
    assert_eq!(e.least, 1);
    assert_eq!(g.scope.enum_ty(0), ty);
    let [first, second] = &e.variants[..] else {
        panic!("two variants")
    };
    assert!(!first.recursive && first.fields.is_empty());
    assert!(second.recursive);
    assert_eq!(second.fields, [ty, Ty::String]);
    assert_eq!(g.scope.enums.len(), 1);
}

#[test]
fn an_enum_instance_should_know_whether_it_may_hold_a_class() {
    let mut g = Gen::new(&[]);
    declare(&mut g, vec![wrapper()]);
    g.scope.classes.push(crate::generate::scope::Class {
        fields: Vec::new(),
        size: 1,
        applied: None,
        inferable: false,
    });
    let _ = g.instance(0, &[Ty::class(0, 1)]);
    let _ = g.instance(0, &[Ty::I64]);
    assert!(g.scope.enums[0].has_class);
    assert!(!g.scope.enums[1].has_class);
}

#[test]
fn the_instances_of_an_open_pair_should_name_each_other() {
    let mut g = Gen::new(&[]);
    declare(&mut g, vec![wrapper()]);
    g.open_pair(&[0]);
    let m = g.instance(1, &[Ty::I64]);
    // `M<i64>` holds `G0<G2<i64>>`, which holds `N<i64>`, which holds `M<i64>` again.
    let names: Vec<String> = g
        .scope
        .instances
        .iter()
        .map(|(_, ty)| ty.to_string())
        .collect();
    assert_eq!(names.len(), 3, "{names:?}");
    assert!(names.contains(&"G0<G2<i64>>".to_string()), "{names:?}");
    let n = g
        .scope
        .instances
        .iter()
        .find(|(k, _)| k.generic == 2)
        .unwrap()
        .1
        .clone();
    let holds = |e: usize| g.scope.enums[e].variants[1].fields.clone();
    let (Ty::Enum { id: m_id, .. }, Ty::Enum { id: n_id, .. }) = (&m, &n) else {
        panic!("two enums")
    };
    assert_eq!(holds(*n_id), std::slice::from_ref(&m));
    assert!(g.scope.enums[*m_id].variants[1].recursive);
    assert!(g.scope.enums[*n_id].variants[1].recursive);
}

#[test]
fn a_variant_should_be_inferable_when_its_fields_name_every_parameter() {
    let mut g = Gen::new(&[]);
    declare(&mut g, vec![wrapper()]);
    let _ = g.instance(0, &[Ty::I64]);
    let variants = &g.scope.enums[0].variants;
    assert!(!variants[0].inferable, "`V1` has no field");
    assert!(variants[1].inferable);
}

#[test]
fn a_class_should_be_inferable_when_its_fields_name_every_parameter() {
    let mut g = Gen::new(&[]);
    g.scope.generics.push(pair_class());
    g.scope.generics.push(Generic {
        params: 2,
        body: Body::Class(vec![field("g1", param(0), false)]),
    });
    let first = g.instance(0, &[Ty::I64, Ty::I64]);
    let second = g.instance(1, &[Ty::I64, Ty::I64]);
    assert!(g.scope.classes[0].inferable);
    assert!(!g.scope.classes[1].inferable, "`U` is named nowhere");
    assert_eq!((first, second), (g.scope.class_ty(0), g.scope.class_ty(1)));
}

#[test]
fn a_type_argument_should_be_small_and_no_optional() {
    for byte in 0..=u8::MAX {
        let data = [byte, 0, 0];
        let mut g = Gen::new(&data);
        g.scope.classes.push(crate::generate::scope::Class {
            fields: Vec::new(),
            size: 99,
            applied: None,
            inferable: false,
        });
        g.scope.types.push(Ty::class(0, 99));
        let ty = g.type_arg();
        assert!(ty.floor() <= super::ARG_FLOOR, "{ty}");
        assert!(!matches!(ty, Ty::Optional(_) | Ty::Unit), "{ty}");
    }
}

#[test]
fn a_body_should_use_each_argument_once() {
    let mut g = Gen::new(&[]);
    let mut env = vec![("a".to_string(), param(0), false)];
    assert_eq!(g.solve(&param(0), &mut env, 0).unwrap().text, "a");
    assert!(g.solve(&param(0), &mut env, 0).is_none());
    assert!(env[0].2);
}

#[test]
fn a_body_that_fails_should_leave_the_arguments_unused() {
    let mut g = Gen::new(&[]);
    // `G0<T, T>` needs two `T`s for its first and its second field; it has one.
    g.scope.generics.push(Generic {
        params: 1,
        body: Body::Class(vec![
            field("g1", param(0), false),
            field("g2", param(0), false),
        ]),
    });
    let mut env = vec![("a".to_string(), param(0), false)];
    let target = GTy::Applied(0, vec![param(0)]);
    assert!(g.solve(&target, &mut env, 0).is_none());
    assert!(!env[0].2);
}

#[test]
fn a_body_should_write_a_scalar_that_is_of_its_own_type() {
    let mut g = Gen::new(&[3, 0, 5]);
    let text = g.scalar(&plain(Ty::U8)).unwrap().text;
    assert!(text.starts_with("(u8(") && text.contains(") ?? "), "{text}");
    assert!(g.scalar(&plain(Ty::class(0, 1))).is_none());
    assert!(g.scalar(&param(0)).is_none());
}

#[test]
fn a_body_should_wrap_an_optional_and_an_array_around_a_value() {
    let mut g = Gen::new(&[]);
    let mut env = vec![
        ("a".to_string(), param(0), false),
        ("b".to_string(), param(0), false),
    ];
    let text = g.build(&optional(param(0)), &mut env, 0).unwrap().text;
    assert_eq!(text, "(if true { a } else { None })");
    let built = g.build(&array(param(0)), &mut env, 0).unwrap();
    assert_eq!(built.text, "[b]");
    assert_eq!(built.extra, 1);
}

#[test]
fn a_body_should_match_a_generic_enum_for_a_field_and_fall_back() {
    let mut g = Gen::new(&[]);
    declare(&mut g, vec![wrapper()]);
    let mut env = vec![
        ("o".to_string(), GTy::Applied(0, vec![param(0)]), false),
        ("d".to_string(), param(0), false),
    ];
    let solved = g.destructure(&param(0), &mut env, 0).unwrap();
    assert_eq!(solved.text, "(match o { G0.V2(p) => p; _ => d })");
    assert_eq!(solved.cost, 1);
    // The enum is used, so it fails the second time.
    assert!(g.destructure(&param(0), &mut env, 0).is_none());
}

#[test]
fn a_body_should_call_an_earlier_function_and_count_its_cost() {
    // The first byte picks the argument for the call's own; the second leaves the type
    // arguments out when 0, and writes them when 1.
    for (data, text) in [(&[0u8, 0][..], "h1(a)"), (&[0, 1][..], "h1<T>(a)")] {
        let mut g = Gen::new(data);
        g.scope.gfuncs.push(identity(4));
        let mut env = vec![("a".to_string(), param(0), false)];
        let solved = g.gcall_in(&param(0), &mut env, 0).unwrap();
        assert_eq!(solved.text, text);
        assert_eq!(solved.cost, 4);
        assert!(env[0].2);
    }
}

#[test]
fn a_body_should_skip_a_function_whose_result_is_another_type() {
    let mut g = Gen::new(&[]);
    g.scope.gfuncs.push(GFunc {
        ret: array(param(0)),
        ..identity(1)
    });
    g.scope.gfuncs.push(GFunc {
        name: "h2".to_string(),
        ..identity(2)
    });
    let mut env = vec![("a".to_string(), param(0), false)];
    let solved = g.gcall_in(&param(0), &mut env, 0).unwrap();
    assert!(solved.text.starts_with("h2"), "{}", solved.text);
    assert_eq!(solved.cost, 2);
}

#[test]
fn an_argument_should_be_used_once() {
    let mut env = vec![
        ("a".to_string(), param(0), true),
        ("b".to_string(), param(1), false),
    ];
    assert!(Gen::argument(&param(0), &mut env).is_none());
    assert!(Gen::argument(&param(1), &mut env).is_some());
    assert!(Gen::argument(&param(1), &mut env).is_none());
}

#[test]
fn a_parameter_or_a_scalar_should_not_be_built() {
    let mut g = Gen::new(&[]);
    let mut env = vec![("a".to_string(), param(0), false)];
    assert!(g.build(&param(0), &mut env, 0).is_none());
    assert!(g.build(&plain(Ty::I64), &mut env, 0).is_none());
    assert!(!env[0].2);
}

#[test]
fn a_type_with_a_parameter_should_name_no_closed_type() {
    let g = Gen::new(&[]);
    for ty in [
        param(0),
        array(param(0)),
        optional(param(0)),
        GTy::Applied(0, vec![param(0)]),
    ] {
        assert_eq!(g.closed(&ty), None, "{ty}");
        assert!(!g.small(&ty), "{ty}");
    }
    assert_eq!(
        g.closed(&array(plain(Ty::U8))),
        Some(Ty::Array(Box::new(Ty::U8)))
    );
    let optional_string = optional(plain(Ty::String));
    assert_eq!(
        g.closed(&optional_string),
        Some(Ty::Optional(Box::new(Ty::String)))
    );
    assert!(g.small(&plain(Ty::I64)));
}

#[test]
fn a_type_argument_should_not_be_small_when_its_instance_does_not_exist() {
    let mut g = Gen::new(&[]);
    g.scope.generics.push(wrapper());
    let applied = GTy::Applied(0, vec![plain(Ty::I64)]);
    assert_eq!(g.closed(&applied), None);
    assert!(!g.small(&applied));
    let ty = g.instance(0, &[Ty::I64]);
    assert_eq!(g.closed(&applied), Some(ty));
    assert!(g.small(&applied));
}

#[test]
fn a_call_in_main_should_need_the_budget_for_its_cost() {
    let mut g = Gen::new(&[]);
    g.scope.gfuncs.push(identity(4));
    g.budget = 3;
    assert_eq!(g.gcallees(&Ty::I64, 0), NONE);
    g.budget = 4;
    assert_eq!(g.gcallees(&Ty::I64, 0), [0]);
    // In a function the cost goes into the function's own.
    g.budget = 0;
    g.scope.func = Some(0);
    assert_eq!(g.gcallees(&Ty::I64, 0), [0]);
}

#[test]
fn a_call_should_not_bind_a_parameter_to_a_large_type() {
    let mut g = Gen::new(&[]);
    g.scope.gfuncs.push(identity(1));
    assert_eq!(g.gcallees(&Ty::class(0, super::ARG_FLOOR), 100), [0]);
    assert_eq!(g.gcallees(&Ty::class(0, super::ARG_FLOOR + 1), 100), NONE);
}

#[test]
fn a_call_should_need_room_for_the_size_it_adds_to_a_sized_result() {
    let mut g = Gen::new(&[]);
    g.scope.gfuncs.push(GFunc {
        name: "h1".to_string(),
        params: 1,
        args: vec![param(0)],
        ret: array(param(0)),
        cost: 1,
        extra: 10,
    });
    let ty = Ty::Array(Box::new(Ty::I64));
    assert_eq!(g.gcallees(&ty, 10 + super::GARG - 1), NONE);
    assert_eq!(g.gcallees(&ty, 10 + super::GARG), [0]);
    // A result without a size needs no room.
    g.scope.gfuncs[0].ret = param(0);
    assert_eq!(g.gcallees(&Ty::I64, 0), [0]);
    assert_eq!(g.gcallees(&Ty::Unit, 100), NONE);
}

#[test]
fn a_call_should_write_the_type_arguments_it_cannot_leave_out() {
    // `fn h1<T>() -> [T]` names `T` in no argument.
    let mut g = Gen::new(&[]);
    g.scope.gfuncs.push(GFunc {
        name: "h1".to_string(),
        params: 1,
        args: Vec::new(),
        ret: array(param(0)),
        cost: 2,
        extra: 0,
    });
    let ty = Ty::Array(Box::new(Ty::String));
    let info = g.gcall(&ty, MAX_SIZE);
    assert_eq!(g.w.out.program, "h1<string>()");
    assert!(!info.pure);
    // In `main`, the cost comes out of the budget; it goes into the statements, too.
    assert_eq!(g.cost.statements, 2);
    assert_eq!(g.budget, BUDGET - FUEL * STATEMENT_MOST - 2);
}

#[test]
fn a_call_should_leave_the_type_arguments_to_the_arguments_that_name_them() {
    // Byte 0 chooses to infer, and the argument is then of its own type.
    let mut g = Gen::new(&[]);
    g.scope.gfuncs.push(identity(1));
    g.gcall(&Ty::U8, MAX_SIZE);
    assert!(g.w.out.program.starts_with("h1("), "{}", g.w.out.program);
    assert!(g.w.out.program.contains("u8("), "{}", g.w.out.program);
    // Another byte writes them out, and the argument takes its type from them.
    let mut g = Gen::new(&[1]);
    g.scope.gfuncs.push(identity(1));
    g.gcall(&Ty::U8, MAX_SIZE);
    assert!(
        g.w.out.program.starts_with("h1<u8>("),
        "{}",
        g.w.out.program
    );
}

#[test]
fn a_call_in_a_loop_should_cost_the_cost_of_the_callee_each_run() {
    let mut g = Gen::new(&[]);
    g.scope.gfuncs.push(identity(7));
    g.cost.enter_loop(3);
    g.gcall(&Ty::I64, MAX_SIZE);
    // Three runs of a call that costs 7, and a literal argument.
    assert_eq!(g.cost.statements, 21);
}

#[test]
fn a_class_result_should_count_the_size_of_its_class() {
    let mut g = Gen::new(&[]);
    g.scope.generics.push(pair_class());
    let ty = g.instance(0, &[Ty::I64, Ty::I64]);
    g.scope.gfuncs.push(GFunc {
        name: "h1".to_string(),
        params: 2,
        args: vec![param(0), array(param(1))],
        ret: GTy::Applied(0, vec![param(0), param(1)]),
        cost: 1,
        extra: 0,
    });
    let info = g.gcall(&ty, MAX_SIZE);
    assert_eq!(info.size, ty.floor());
}

#[test]
fn a_class_should_be_built_with_its_type_arguments_unless_they_can_be_left_out() {
    for (bytes, start) in [(&[0u8][..], "G0("), (&[1][..], "G0<i64, string>(")] {
        let mut g = Gen::new(bytes);
        g.scope.generics.push(pair_class());
        let ty = g.instance(0, &[Ty::I64, Ty::String]);
        g.object(&ty);
        assert!(g.w.out.program.starts_with(start), "{}", g.w.out.program);
    }
}

#[test]
fn a_variant_should_leave_out_its_type_arguments_only_with_fields_that_name_them() {
    let mut g = Gen::new(&[]);
    declare(&mut g, vec![wrapper()]);
    let ty = g.instance(0, &[Ty::I64]);
    g.w.out.program.clear();
    // `V1` has no field, so its type arguments stay.
    g.variant(&ty, 1, None);
    assert_eq!(g.w.out.program, "G0<i64>.V1");
    let mut g = Gen::new(&[]);
    declare(&mut g, vec![wrapper()]);
    let ty = g.instance(0, &[Ty::I64]);
    g.w.out.program.clear();
    g.variant(&ty, 3, Some(0));
    assert!(g.w.out.program.starts_with("G0.V2("), "{}", g.w.out.program);
    let mut g = Gen::new(&[1]);
    declare(&mut g, vec![wrapper()]);
    let ty = g.instance(0, &[Ty::I64]);
    g.w.out.program.clear();
    g.variant(&ty, 3, Some(0));
    assert!(
        g.w.out.program.starts_with("G0<i64>.V2("),
        "{}",
        g.w.out.program
    );
}

#[test]
fn a_class_should_leave_out_type_arguments_only_if_each_field_fits_its_own_type() {
    // `class G0<T> { g1: [T] }`: an array of its own type holds an element, so its
    // field needs room for one (`Ty::least`).
    for (cap, start) in [(0, "G0<i64>("), (1, "G0(")] {
        let mut g = Gen::new(&[]);
        g.scope.generics.push(Generic {
            params: 1,
            body: Body::Class(vec![field("g1", array(param(0)), false)]),
        });
        let ty = g.instance(0, &[Ty::I64]);
        g.scope.classes[0].fields[0].cap = cap;
        g.object(&ty);
        assert!(g.w.out.program.starts_with(start), "{}", g.w.out.program);
    }
}

#[test]
fn a_variant_should_leave_out_type_arguments_only_if_its_fields_fit_their_own_types() {
    // `enum G0<T> { V1  V2([T]) }`: a `[T]` of its own type holds an element, so `V2`
    // needs room for it in the variant, beyond its own node.
    for (max, start) in [(1, "G0<i64>.V2("), (2, "G0.V2(")] {
        let mut g = Gen::new(&[]);
        declare(
            &mut g,
            vec![Generic {
                params: 1,
                body: Body::Enum(vec![
                    variant("V1", Vec::new(), false),
                    variant("V2", vec![array(param(0))], false),
                ]),
            }],
        );
        let ty = g.instance(0, &[Ty::I64]);
        g.w.out.program.clear();
        g.variant(&ty, max, Some(0));
        assert!(g.w.out.program.starts_with(start), "{}", g.w.out.program);
    }
}

#[test]
fn a_generic_type_should_be_written_as_the_program_writes_it() {
    let ty = GTy::Applied(1, vec![array(param(0)), plain(Ty::I64)]);
    assert_eq!(optional(ty).to_string(), "G1<[T], i64>?");
    assert_eq!(array(param(1)).to_string(), "[U]");
    assert_eq!(super::angled(&[Ty::I64, Ty::Bool]), "<i64, bool>");
}

#[test]
fn a_type_parameter_list_should_be_written_on_one_line_or_over_lines() {
    let written = |params, id| {
        let mut g = Gen::new(&[]);
        g.tparams(params, id);
        g.w.out.program.clone()
    };
    assert_eq!(written(1, 0), "<T>");
    assert_eq!(written(2, 2), "<T, U>");
    assert_eq!(written(1, 1), "<\nT\n>");
    assert_eq!(written(2, 3), "<\nT,\nU\n>");
    let source = format!(
        "fn _f{}(_x: T, _y: U) {{}}\nfn main() {{}}\n",
        written(2, 1)
    );
    let (program, diagnostics) = crate::front_end::checked(&source);
    assert!(
        program.is_some() && diagnostics.is_empty(),
        "{diagnostics:?}"
    );
}

#[test]
fn unify_should_refuse_applied_types_of_other_arguments_or_other_counts() {
    let one = GTy::Applied(0, vec![plain(Ty::I64)]);
    let two = GTy::Applied(0, vec![plain(Ty::I64), plain(Ty::I64)]);
    let other = GTy::Applied(0, vec![plain(Ty::U8)]);
    assert!(!unify(&one, &two, &mut []));
    assert!(!unify(&two, &one, &mut []));
    assert!(!unify(&one, &other, &mut []));
    assert!(unify(&one, &one, &mut []));
}

#[test]
fn the_generics_of_a_given_size_should_be_found_by_their_parameters_and_kind() {
    let mut g = Gen::new(&[]);
    declare(&mut g, vec![wrapper(), pair_class()]);
    let pair = Generic {
        params: 2,
        body: Body::Enum(vec![variant("V9", vec![param(0), param(1)], false)]),
    };
    g.declare_enum(2, 2, vec![variant("V9", vec![param(0), param(1)], false)]);
    assert_eq!(pair.params, 2);
    assert_eq!(g.generic_enums(1), [0]);
    assert_eq!(g.generic_enums(2), [2]);
    assert_eq!(g.all_generic_enums(), [0, 2]);
}

#[test]
fn each_count_of_generics_should_reach_its_most_and_stay_within_it() {
    // Each generic type and each generic function spends one fuel. A generic class has 3
    // fields at most; a generic enum, 3 variants and 3 fields in a variant. A function has 2
    // type parameters, and 4 arguments: the 3 of `(o: G<n><T, U>, d: T, e: U)` and one more.
    let (mut types, mut functions, mut params, mut args) = (0, 0, 0, 0);
    let (mut fields, mut variants, mut payload, mut mutable) = (0, (usize::MAX, 0), 0, [false; 2]);
    for state in 0..256 {
        let data = crate::generate::coverage::seed(state);
        let mut g = Gen::new(&data);
        g.generics();
        functions = functions.max(g.scope.gfuncs.len());
        let spent = usize::try_from(FUEL - g.fuel).unwrap();
        types = types.max(spent - g.scope.gfuncs.len());
        for generic in &g.scope.generics {
            params = params.max(generic.params);
            match &generic.body {
                Body::Class(list) => {
                    fields = fields.max(list.len());
                    for field in list {
                        mutable[usize::from(field.mutable)] = true;
                    }
                }
                Body::Enum(list) => {
                    variants = (variants.0.min(list.len()), variants.1.max(list.len()));
                    payload = payload.max(list.iter().map(|v| v.fields.len()).max().unwrap_or(0));
                }
            }
        }
        for func in &g.scope.gfuncs {
            params = params.max(func.params);
            args = args.max(func.args.len());
        }
    }
    assert_eq!(
        (
            types, functions, params, args, fields, variants, payload, mutable
        ),
        (5, 3, 2, 4, 3, (2, 3), 3, [true; 2])
    );
}

#[test]
fn a_generic_enum_of_the_plain_shape_should_have_a_second_variant_now_and_then() {
    for (data, variants) in [(&[0u8; 8][..], 2), (&[1; 8][..], 3)] {
        let mut g = Gen::new(data);
        g.generic_enum(Shape::Plain, &[]);
        let Body::Enum(list) = &g.scope.generics[0].body else {
            panic!("an enum")
        };
        assert_eq!(list.len(), variants);
        assert!(list.iter().all(|v| !v.recursive));
    }
    let mut g = Gen::new(&[]);
    g.generic_enum(Shape::Recursive, &[]);
    let Body::Enum(list) = &g.scope.generics[0].body else {
        panic!("an enum")
    };
    assert_eq!(
        list.iter().map(|v| v.recursive).collect::<Vec<_>>(),
        [false, true]
    );
    // The enum itself is its first field, which a `while let` can walk.
    assert_eq!(list[1].fields[0], GTy::Applied(0, vec![param(0)]));
}

#[test]
fn a_base_variant_should_hold_nothing_a_parameter_or_a_scalar() {
    let fields = |byte: u8| {
        let data = [byte, 1];
        let mut g = Gen::new(&data);
        g.base_variant(2).fields
    };
    assert_eq!(fields(0), []);
    assert_eq!(fields(1), [param(1)]);
    assert!(matches!(fields(2)[..], [GTy::Plain(_)]));
}

#[test]
fn a_field_should_be_a_parameter_an_array_an_optional_a_scalar_or_a_generic_enum() {
    let mut seen = [false; 5];
    for byte in 0..=u8::MAX {
        let data = [byte; 4];
        let mut g = Gen::new(&data);
        declare(&mut g, vec![wrapper()]);
        match g.gfield(1, &[0]) {
            GTy::Param(_) => seen[0] = true,
            GTy::Array(_) => seen[1] = true,
            GTy::Optional(_) => seen[2] = true,
            GTy::Plain(_) => seen[3] = true,
            GTy::Applied(..) => seen[4] = true,
        }
    }
    assert_eq!(seen, [true; 5]);
    // With no generic enum to apply, a field is never one.
    for byte in 0..=u8::MAX {
        let data = [byte; 4];
        assert!(!matches!(Gen::new(&data).gfield(1, &[]), GTy::Applied(..)));
    }
}

#[test]
fn a_parameter_field_should_name_its_parameter_alone_in_an_array_or_in_an_optional() {
    let fields: Vec<GTy> = (0..3)
        .map(|byte| {
            let data = [byte];
            Gen::new(&data).param_field(1)
        })
        .collect();
    assert_eq!(fields, [param(1), array(param(1)), optional(param(1))]);
}

#[test]
fn an_applied_generic_should_take_a_parameter_an_array_of_one_or_a_scalar() {
    let mut seen = [false; 3];
    for byte in 0..=u8::MAX {
        let data = [byte; 6];
        let mut g = Gen::new(&data);
        declare(&mut g, vec![wrapper()]);
        let GTy::Applied(0, args) = g.applied(0, 2) else {
            panic!("an application of `G0`")
        };
        match &args[..] {
            [GTy::Param(i)] => {
                assert!(*i < 2);
                seen[0] = true;
            }
            [GTy::Array(inner)] => {
                assert!(matches!(**inner, GTy::Param(i) if i < 2));
                seen[1] = true;
            }
            [GTy::Plain(_)] => seen[2] = true,
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(seen, [true; 3]);
}

#[test]
fn a_type_should_follow_the_choices_of_values_declared_types_instances_arrays_and_optionals() {
    // The choices, in order: the 6 values, a declared type, an instance, an array, an
    // optional. Each is present only when it can be made.
    for (declared, instances) in [(false, false), (true, false), (false, true), (true, true)] {
        for byte in 0..=u8::MAX {
            let data = [byte; 8];
            let mut kinds = Ty::VALUES.to_vec();
            let mut g = Gen::new(&data);
            if declared {
                g.scope.types.push(Ty::class(0, 1));
                kinds.push(Ty::class(0, 1));
            }
            if instances {
                g.scope.generic_types.push(Ty::enumeration(1, 1));
                kinds.push(Ty::enumeration(1, 1));
            }
            let choice = Gen::new(&data).pick(kinds.len() + 2);
            let ty = g.any_type();
            match kinds.get(choice) {
                Some(kind) => assert_eq!(ty, *kind, "choice {choice}"),
                None if choice == kinds.len() => assert!(matches!(ty, Ty::Array(_))),
                None => assert!(matches!(ty, Ty::Optional(_))),
            }
        }
    }
}

#[test]
fn a_declared_class_should_be_built_with_fields_typed_by_the_class() {
    // `u8` literals come bare, not as `(u8(n) ?? n)`, which only a generic class's fields
    // use.
    let mut g = Gen::new(&[]);
    g.scope.classes.push(crate::generate::scope::Class {
        fields: vec![crate::generate::scope::Field {
            name: "g1".to_string(),
            ty: Ty::U8,
            mutable: false,
            cap: 0,
        }],
        size: 1,
        applied: None,
        inferable: false,
    });
    g.object(&Ty::class(0, 1));
    assert!(
        g.w.out.program.starts_with("K0(g1: "),
        "{}",
        g.w.out.program
    );
    assert!(!g.w.out.program.contains("u8("), "{}", g.w.out.program);
}

#[test]
fn a_generic_enum_value_should_end_in_a_variant_that_holds_no_enum() {
    // At the depth where a recursive variant is no longer allowed, only the other is.
    let recursive = vec![
        variant("V1", Vec::new(), false),
        variant("V2", vec![GTy::Applied(0, vec![param(0)])], true),
    ];
    for (nesting, start) in [(MAX_SOLVE - 1, "G0<i64>.V1"), (MAX_SOLVE - 2, "G0.V2(")] {
        let mut g = Gen::new(&[1]);
        declare(
            &mut g,
            vec![Generic {
                params: 1,
                body: Body::Enum(recursive.clone()),
            }],
        );
        let mut env = Vec::new();
        let solved = g
            .construct(0, &[plain(Ty::I64)], &mut env, nesting)
            .unwrap();
        assert!(solved.text.starts_with(start), "{}", solved.text);
    }
}

#[test]
fn a_construction_should_leave_out_its_type_arguments_only_for_fields_that_name_them() {
    let enum_text = |byte: u8, fields: Vec<GTy>| {
        let data = [byte];
        let mut g = Gen::new(&data);
        declare(
            &mut g,
            vec![Generic {
                params: 1,
                body: Body::Enum(vec![variant("V1", fields, false)]),
            }],
        );
        let mut env = vec![("a".to_string(), param(0), false)];
        let solved = g.construct(0, &[param(0)], &mut env, 0).unwrap();
        (solved.text, solved.extra)
    };
    assert_eq!(enum_text(0, vec![param(0)]), ("G0.V1(a)".to_string(), 1));
    assert_eq!(enum_text(1, vec![param(0)]), ("G0<T>.V1(a)".to_string(), 1));
    // No field, so no type parameter is named: they stay, and `a` stays unused.
    assert_eq!(enum_text(0, Vec::new()), ("G0<T>.V1".to_string(), 1));
    let class_text = |byte: u8| {
        let data = [byte];
        let mut g = Gen::new(&data);
        g.scope.generics.push(Generic {
            params: 1,
            body: Body::Class(vec![field("g1", param(0), false)]),
        });
        let mut env = vec![("a".to_string(), param(0), false)];
        let solved = g.construct(0, &[param(0)], &mut env, 0).unwrap();
        (solved.text, solved.extra)
    };
    assert_eq!(class_text(0), ("G0(g1: a)".to_string(), 0));
    assert_eq!(class_text(1), ("G0<T>(g1: a)".to_string(), 0));
}

#[test]
fn a_body_should_offer_a_call_and_a_match_only_above_the_depth_that_ends_it() {
    let solved = |byte: u8, nesting: u32| {
        let data = [byte];
        let mut g = Gen::new(&data);
        declare(&mut g, vec![wrapper()]);
        g.scope.gfuncs.push(identity(1));
        let mut env = vec![
            ("o".to_string(), GTy::Applied(0, vec![param(0)]), false),
            ("d".to_string(), param(0), false),
        ];
        g.solve(&param(0), &mut env, nesting).unwrap().text
    };
    // At the depth limit, only the argument itself remains.
    for byte in 0..3 {
        assert_eq!(solved(byte, MAX_SOLVE), "d");
    }
    // Above it, a byte picks the argument, the call or the `match`.
    assert_eq!(solved(0, 0), "d");
    assert!(solved(1, 0).starts_with("h1"), "{}", solved(1, 0));
    assert!(solved(2, 0).starts_with("(match o"), "{}", solved(2, 0));
}

#[test]
fn a_body_should_not_offer_a_match_on_an_argument_that_is_a_class() {
    let mut g = Gen::new(&[1]);
    declare(&mut g, vec![pair_class()]);
    let mut env = vec![
        (
            "o".to_string(),
            GTy::Applied(0, vec![param(0), param(1)]),
            false,
        ),
        ("d".to_string(), param(0), false),
    ];
    let before = g.u.len();
    assert_eq!(g.solve(&param(0), &mut env, 0).unwrap().text, "d");
    // One option leaves nothing to pick, so the byte stays.
    assert_eq!(g.u.len(), before);
}

#[test]
fn a_scalar_should_be_written_as_its_literal() {
    let text = |ty: Ty, byte: u8| {
        let data = [byte, 3, 2, 1];
        let mut g = Gen::new(&data);
        g.scalar(&plain(ty)).unwrap().text
    };
    assert_eq!(text(Ty::I64, 0), "3");
    assert_eq!(text(Ty::U8, 0), "(u8(0) ?? 0)");
    assert_eq!(text(Ty::Bool, 0), "false");
    assert_eq!(text(Ty::Bool, 1), "true");
    assert_eq!(text(Ty::Char, 0), "'a'");
    assert!(text(Ty::F64, 0).starts_with(|c: char| c.is_ascii_digit() || c == 'f'));
    let string = text(Ty::String, 2);
    assert!(string.starts_with('"') && string.ends_with('"'), "{string}");
    assert!(string.len() - 2 <= 4, "{string}");
}

#[test]
fn an_optional_and_an_array_should_come_in_their_two_forms() {
    let make = |byte: u8, target: GTy| {
        let data = [byte, 1];
        let mut g = Gen::new(&data);
        let mut env = vec![
            ("a".to_string(), param(0), false),
            ("b".to_string(), param(0), false),
        ];
        let solved = g.build(&target, &mut env, 0).unwrap();
        (solved.text, solved.extra)
    };
    assert_eq!(
        make(0, optional(param(0))).0,
        "(if true { a } else { None })"
    );
    assert_eq!(
        make(1, optional(param(0))).0,
        "(if false { a } else { None })"
    );
    // An array of one element takes the first byte as 0, and of two as 1.
    assert_eq!(make(0, array(param(0))), ("[a]".to_string(), 1));
    assert_eq!(make(1, array(param(0))), ("[a, b]".to_string(), 2));
}

#[test]
fn a_generic_function_should_be_written_with_its_type_parameters_and_arguments() {
    // Byte by byte: one type parameter; the argument `T` and no other; the result `T`.
    let mut g = Gen::new(&[]);
    g.generic_function();
    assert_eq!(g.w.out.program, "fn h1<T>(v2: T) -> T {\n    v2\n}\n");
    // Two type parameters, an argument for each, the result the first.
    let mut g = Gen::new(&[1]);
    g.generic_function();
    assert_eq!(
        g.w.out.program,
        "fn h1<T, U>(v2: T, v3: U) -> T {\n    v2\n}\n"
    );
    assert_eq!(g.scope.gfuncs[0].cost, 1);
    assert_eq!(g.scope.gfuncs[0].params, 2);
    assert_eq!(g.scope.gfuncs[0].args, [param(0), param(1)]);
    assert_eq!(g.scope.gfuncs[0].ret, param(0));
}

#[test]
fn a_function_over_an_enum_and_a_parameter_should_match_the_enum_for_the_parameter() {
    // Byte by byte: two type parameters; the unwrap form (0 of 4); no extra argument; and
    // the `match`, the second of two ways to make a `T`.
    let mut g = Gen::new(&[1, 0, 0, 1]);
    declare(&mut g, vec![wrapper()]);
    g.w.out.program.clear();
    g.generic_function();
    let text = &g.w.out.program;
    assert!(
        text.starts_with("fn h1<T, U>(v2: G0<T>, v3: T, v4: U) -> T {"),
        "{text}"
    );
    assert!(
        text.contains("(match v2 { G0.V2(p) => p; _ => v3 })"),
        "{text}"
    );
}

#[test]
fn a_match_should_leave_the_other_fields_of_its_variant_unnamed() {
    let mut g = Gen::new(&[]);
    g.declare_enum(
        0,
        1,
        vec![
            variant("V1", Vec::new(), false),
            variant("V2", vec![plain(Ty::I64), param(0), plain(Ty::Bool)], false),
        ],
    );
    let mut env = vec![
        ("o".to_string(), GTy::Applied(0, vec![param(0)]), false),
        ("d".to_string(), param(0), false),
    ];
    let solved = g.destructure(&param(0), &mut env, 0).unwrap();
    assert_eq!(solved.text, "(match o { G0.V2(_, p, _) => p; _ => d })");
}

#[test]
fn a_call_should_skip_a_function_that_does_not_give_the_type() {
    let mut g = Gen::new(&[]);
    g.scope.gfuncs.push(GFunc {
        ret: array(param(0)),
        ..identity(1)
    });
    assert_eq!(g.gcallees(&Ty::I64, 0), NONE);
    g.scope.gfuncs.push(identity(1));
    assert_eq!(g.gcallees(&Ty::I64, 0), [1]);
}

#[test]
fn a_call_should_separate_its_arguments_with_commas() {
    let mut g = Gen::new(&[]);
    g.scope.gfuncs.push(GFunc {
        args: vec![param(0), param(0)],
        ..identity(1)
    });
    g.gcall(&Ty::I64, MAX_SIZE);
    assert_eq!(g.w.out.program, "h1(0, 0)");
}

#[test]
fn a_sized_result_should_hold_the_nodes_it_adds() {
    let mut g = Gen::new(&[]);
    g.scope.gfuncs.push(GFunc {
        ret: array(param(0)),
        extra: 3,
        ..identity(1)
    });
    // The argument is a `string` of no chars, and the call adds 3.
    assert_eq!(g.gcall(&Ty::Array(Box::new(Ty::String)), MAX_SIZE).size, 3);
}

#[test]
fn a_type_argument_should_have_a_floor_of_the_most_and_no_more() {
    let mut found = [false; 2];
    for byte in 0..=u8::MAX {
        let data = [byte, 0, 0];
        let mut g = Gen::new(&data);
        g.scope.types.push(Ty::class(0, super::ARG_FLOOR));
        g.scope.types.push(Ty::class(1, super::ARG_FLOOR + 1));
        match g.type_arg() {
            Ty::Class { id: 0, .. } => found[0] = true,
            Ty::Class { id: 1, .. } => found[1] = true,
            _ => {}
        }
    }
    assert_eq!(found, [true, false]);
}

#[test]
fn an_enum_instance_should_count_its_first_variant_as_its_least_value() {
    let mut g = Gen::new(&[]);
    g.scope.classes.push(crate::generate::scope::Class {
        fields: Vec::new(),
        size: 5,
        applied: None,
        inferable: false,
    });
    g.declare_enum(
        0,
        1,
        vec![
            variant("V1", vec![param(0), plain(Ty::I64)], false),
            variant("V2", Vec::new(), false),
        ],
    );
    let ty = g.instance(0, &[Ty::class(0, 5)]);
    // The node, and the object.
    assert!(matches!(ty, Ty::Enum { least: 6, .. }), "{ty:?}");
    assert_eq!(g.scope.enums[0].least, 6);
}

#[test]
fn a_value_of_a_recursive_enum_should_take_the_recursive_variant_only_while_fuel_lasts() {
    // Byte 1 picks the second of two variants when both can come.
    for (fuel, nesting, start) in [
        (0, 0, "G0<i64>.V1"),
        (1, MAX_NESTING, "G0<i64>.V1"),
        (1, MAX_NESTING - 1, "G0.V2("),
    ] {
        let mut g = Gen::new(&[1]);
        declare(
            &mut g,
            vec![Generic {
                params: 1,
                body: Body::Enum(vec![
                    variant("V1", Vec::new(), false),
                    variant("V2", vec![GTy::Applied(0, vec![param(0)])], true),
                ]),
            }],
        );
        let ty = g.instance(0, &[Ty::I64]);
        g.w.out.program.clear();
        g.fuel = fuel;
        g.nesting = nesting;
        g.variant(&ty, 10, None);
        assert!(g.w.out.program.starts_with(start), "{}", g.w.out.program);
    }
}

#[test]
fn generics_should_name_each_instance_once_among_the_types() {
    let mut instances = 0;
    for state in 0..64 {
        let data = crate::generate::coverage::seed(state);
        let mut g = Gen::new(&data);
        g.generics();
        let types = &g.scope.generic_types;
        instances += types.len();
        for (i, ty) in types.iter().enumerate() {
            assert!(!types[..i].contains(ty), "{ty} twice");
            assert!(ty.to_string().starts_with('G'), "{ty}");
        }
    }
    assert!(instances > 0, "no seed names an instance");
}

#[test]
fn a_generic_class_field_should_be_mutable_by_the_second_choice_only() {
    let mutable = |data: &[u8]| {
        let mut g = Gen::new(data);
        g.generic_class();
        let Body::Class(fields) = &g.scope.generics[0].body else {
            panic!("a class")
        };
        fields.iter().filter(|f| f.mutable).count()
    };
    assert_eq!(mutable(&[]), 0);
    assert!(mutable(&[1; 16]) > 0);
}

#[test]
fn a_signature_type_should_nest_as_deep_as_it_may_and_no_deeper() {
    // The same byte at every level picks an array or an optional at every level it may.
    let depth = |ty: &GTy| {
        let mut depth = 0;
        let mut at = ty;
        while let GTy::Array(inner) | GTy::Optional(inner) = at {
            depth += 1;
            at = inner;
        }
        depth
    };
    let mut deepest = [0; 3];
    for byte in 0..=u8::MAX {
        for nesting in 0..3 {
            let data = [byte; 8];
            let ty = Gen::new(&data).gsig(2, nesting);
            let d = depth(&ty);
            assert!(d <= nesting as usize, "{ty} at {nesting}");
            deepest[nesting as usize] = deepest[nesting as usize].max(d);
        }
    }
    assert_eq!(deepest, [0, 1, 2]);
}

#[test]
fn a_nested_value_should_be_one_level_deeper_than_the_value_around_it() {
    // At the depth limit's last level, the value inside an array, an optional, a class field or
    // a `match` has no call to offer; the byte that would pick one then changes nothing.
    let at = MAX_SOLVE - 1;
    let setup = |g: &mut Gen| {
        declare(g, vec![wrapper()]);
        g.scope.generics.push(Generic {
            params: 1,
            body: Body::Class(vec![field("g1", param(0), false)]),
        });
        g.scope.gfuncs.push(identity(1));
    };
    let env = || {
        vec![
            ("o".to_string(), GTy::Applied(0, vec![param(0)]), false),
            ("a".to_string(), param(0), false),
        ]
    };
    let data = [0, 1];
    let mut g = Gen::new(&data);
    setup(&mut g);
    assert_eq!(
        g.build(&array(param(0)), &mut env(), at).unwrap().text,
        "[a]"
    );
    let mut g = Gen::new(&data);
    setup(&mut g);
    assert_eq!(
        g.build(&optional(param(0)), &mut env(), at).unwrap().text,
        "(if true { a } else { None })"
    );
    let data = [1];
    let mut g = Gen::new(&data);
    setup(&mut g);
    let class = g.construct(1, &[param(0)], &mut env(), at).unwrap();
    assert_eq!(class.text, "G1<T>(g1: a)");
    let mut g = Gen::new(&data);
    setup(&mut g);
    let matched = g.destructure(&param(0), &mut env(), at).unwrap();
    assert_eq!(matched.text, "(match o { G0.V2(p) => p; _ => a })");
    let mut g = Gen::new(&data);
    setup(&mut g);
    let call = g.gcall_in(&param(0), &mut env(), at).unwrap();
    assert_eq!(call.text, "h1<T>(a)");
}

#[test]
fn a_call_in_a_body_should_add_its_nodes_to_those_of_its_arguments() {
    let mut g = Gen::new(&[]);
    g.scope.gfuncs.push(GFunc {
        args: vec![array(param(0))],
        extra: 2,
        ..identity(4)
    });
    let mut env = vec![("a".to_string(), param(0), false)];
    let solved = g.gcall_in(&param(0), &mut env, 0).unwrap();
    // `h1([a])`: the 2 of the function, and the 1 of the array.
    assert_eq!(solved.text, "h1([a])");
    assert_eq!((solved.extra, solved.cost), (3, 4));
}

#[test]
fn an_argument_of_a_call_should_have_room_beyond_its_floor() {
    // With no fuel, a `string` is a literal of at most `GARG` chars; some byte makes it long.
    let literal = |byte: u8| {
        let data = [byte; 4];
        let mut g = Gen::new(&data);
        g.fuel = 0;
        g.scope.gfuncs.push(identity(1));
        g.gcall(&Ty::String, MAX_SIZE);
        let text = g.w.out.program;
        let (_, inner) = text.split_once("(\"").expect("a string argument");
        inner
            .strip_suffix("\")")
            .expect("a closed call")
            .to_string()
    };
    let literals: Vec<String> = (0..=u8::MAX).map(literal).collect();
    assert!(literals.iter().any(|text| !text.is_empty()), "{literals:?}");
}

#[test]
fn an_optional_in_a_signature_should_hold_a_type_one_level_shallower() {
    // The first byte picks an optional, and the others an array at every level they may.
    let pick = |want: usize| (0..=u8::MAX).find(|&b| Gen::new(&[b]).pick(5) == want);
    let (optional_byte, array_byte) = (pick(4).unwrap(), pick(3).unwrap());
    let mut data = vec![optional_byte];
    data.extend([array_byte; 8]);
    // At one level of nesting, `T?` is all there is below the optional.
    let ty = Gen::new(&data).gsig(2, 1);
    assert!(
        matches!(ty, GTy::Optional(ref inner) if !matches!(**inner, GTy::Array(_))),
        "{ty}"
    );
    // At two, an array may sit in it.
    let ty = Gen::new(&data).gsig(2, 2);
    assert!(
        matches!(ty, GTy::Optional(ref inner) if matches!(**inner, GTy::Array(_))),
        "{ty}"
    );
}

#[test]
fn a_pattern_should_name_a_generic_enum_without_its_type_arguments() {
    let mut g = Gen::new(&[1]);
    declare(&mut g, vec![wrapper()]);
    let ty = g.instance(0, &[Ty::I64]);
    let (pattern, _) = g.variant_pattern(&ty, 0, None, 0);
    assert_eq!(pattern, "G0.V1");
    assert_eq!(ty.bare(), "G0");
    assert_eq!(Ty::enumeration(3, 1).bare(), "E3");
}

#[test]
fn no_bytes_should_make_no_generic() {
    for data in [&[][..], &[0; 64][..]] {
        let program = generate(data).program;
        assert!(
            !program.contains("G0") && !program.contains("fn h"),
            "{program}"
        );
    }
}

#[test]
fn a_type_should_be_an_instance_as_often_as_a_declared_type() {
    // One byte picks the type; the others, with an array or optional, pick the rest.
    let mut counts = [0; 3];
    for byte in 0..=u8::MAX {
        let data = [byte; 8];
        let mut g = Gen::new(&data);
        g.scope.types.push(Ty::class(0, 1));
        g.scope.generic_types.push(Ty::enumeration(1, 1));
        match g.any_type() {
            Ty::Class { .. } => counts[0] += 1,
            Ty::Enum { .. } => counts[1] += 1,
            _ => counts[2] += 1,
        }
    }
    assert!(counts[0] > 0 && counts[1] > 0, "{counts:?}");
    assert_eq!(counts[0], counts[1], "{counts:?}");
}

/// The `tir::Rank` of each payload field of each enum of `source`, which must check.
fn ranks(source: &str) -> Vec<(String, Vec<Rank>)> {
    let (program, diagnostics) = crate::front_end::checked(source);
    let program = program.unwrap_or_else(|| panic!("{diagnostics:?}\n{source}"));
    program
        .enums
        .iter()
        .map(|e| {
            let fields = e.variants.iter().flat_map(|v| &v.fields);
            (e.name.clone(), fields.map(|p| p.rank).collect())
        })
        .collect()
}

#[test]
fn each_shape_should_give_the_rank_it_is_for() {
    let program = |build: &dyn Fn(&mut Gen)| {
        let mut g = Gen::new(&[]);
        declare(&mut g, vec![wrapper()]);
        build(&mut g);
        g.w.text("\nfn main() {}\n");
        ranks(&g.w.out.program)
    };
    // `G0` is `enum G0<T> { V1  V2(T) }`; its field is a `Never`.
    let recursive = program(&|g| g.generic_enum(Shape::Recursive, &[0]));
    assert_eq!(recursive[0], ("G0".to_string(), vec![Rank::Never]));
    assert!(recursive[1].1.contains(&Rank::Own), "{recursive:?}");
    let wrapped = program(&|g| g.generic_enum(Shape::Wrapped, &[0]));
    assert!(wrapped[1].1.contains(&Rank::Own), "{wrapped:?}");
    let open = program(&|g| g.open_pair(&[0]));
    assert_eq!(open[1].1, [Rank::Open], "{open:?}");
    assert_eq!(open[2].1, [Rank::Never, Rank::Open], "{open:?}");
    let plain = program(&|g| g.generic_enum(Shape::Plain, &[0]));
    assert!(!plain[1].1.contains(&Rank::Own), "{plain:?}");
}

#[test]
fn a_pair_should_give_a_late_field() {
    let mut g = Gen::new(&[]);
    let pair = Generic {
        params: 2,
        body: Body::Enum(vec![variant("V1", vec![param(0), param(1)], false)]),
    };
    declare(&mut g, vec![pair]);
    g.generic_enum(Shape::Late, &[0]);
    g.w.text("\nfn main() {}\n");
    let ranks = ranks(&g.w.out.program);
    assert_eq!(ranks[1].1, [Rank::Late], "{ranks:?}");
}
