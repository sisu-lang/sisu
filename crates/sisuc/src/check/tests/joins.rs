//! The tests of the inference table and the join that is built on it.

use super::*;
use crate::check::infer::{Kind, Written};

/// A result that holds no type, so that `with_table` has nothing to write back in the test.
struct Nothing<R>(R);

impl<R> WriteBack for Nothing<R> {
    fn write_back_all(&mut self, _table: &mut infer::Table) -> Written {
        Written::default()
    }
}

/// Runs `f` on a checker over `src`, whose names and interface signatures are collected, inside
/// `with_table`: the checker has the table of a function with no variable.
pub(in crate::check) fn with_checker<R>(src: &str, f: impl FnOnce(&mut Checker<'_>) -> R) -> R {
    let program = parse_ok(src);
    let mut checker = Checker::new(&program, src, "");
    let items: Vec<&ast::Item> = program.items.iter().collect();
    checker.collect_names(&items, items.len(), &program.broken);
    checker.interface_signatures(&items);
    checker.with_table(|c| Nothing(f(c))).0
}

#[test]
fn cache_should_not_keep_an_inference_variable_across_functions() {
    // GIVEN two functions whose first open `Any` variable has the same id and a different type
    let src = "interface Holds<T> {\n    fn get(self) -> T\n}\n";
    let program = parse_ok(src);
    let mut checker = Checker::new(&program, src, "");
    let items: Vec<&ast::Item> = program.items.iter().collect();
    checker.collect_names(&items, items.len(), &program.broken);
    checker.interface_signatures(&items);
    let Some(Declared::Known(holds)) = checker.names.interface(Symbol::intern("Holds")) else {
        panic!("Holds should be an interface");
    };
    let any_of = |arg| Type::intern(TypeKind::Any(holds, TypeArgs::intern(&[arg])));
    let mut closures = Vec::new();
    for settled in [Type::I64, Type::STRING] {
        // WHEN each function asks for the interfaces of `any Holds<A>`
        let closure = checker
            .with_table(|c| {
                let var = c.table.fresh(Kind::Any, Span::new(0, 1));
                assert_eq!(var, Type::intern(TypeKind::Infer(crate::types::InferId(0))));
                assert!(c.unify(var, settled), "A should unify with {settled:?}");
                let ty = any_of(var);
                let closure = c.any_interfaces(ty);
                // THEN the key that holds the variable is not kept, so the next lookup is computed
                assert!(!c.any_closures.contains_key(&ty));
                Nothing(closure.to_vec())
            })
            .0;
        closures.push(closure);
    }
    // A key with no variable is kept, as before.
    checker.any_interfaces(any_of(Type::I64));
    assert!(checker.any_closures.contains_key(&any_of(Type::I64)));
    assert!(closures.iter().all(|closure| !closure.is_empty()));
}

#[test]
fn cache_should_not_keep_a_conformance_leaf_or_probe_of_a_type_that_holds_a_variable() {
    // GIVEN a generic class and an interface with an associated type
    // The first interface stands for each language interface, which `conforms` answers at once.
    let src = "interface Pad {\n    fn pad(self) -> bool\n}\ninterface Source {\n    type Item\n    fn next(self) -> Item\n}\nclass Box<T> {\n    item: T\n}\n";
    let program = parse_ok(src);
    let mut checker = Checker::new(&program, src, "");
    let items: Vec<&ast::Item> = program.items.iter().collect();
    checker.collect_names(&items, items.len(), &program.broken);
    checker.interface_signatures(&items);
    let Some(Declared::Known(source)) = checker.names.interface(Symbol::intern("Source")) else {
        panic!("Source should be an interface");
    };
    let Some(Declared::Known(boxed)) = checker.names.class(Symbol::intern("Box")) else {
        panic!("Box should be a class");
    };
    // The marks that the pass before the bodies makes: `Box` does not grow.
    checker.growing_classes = vec![false];
    checker.with_table(|c| {
        let var = c.table.fresh(Kind::Any, Span::new(0, 1));
        let held = Type::intern(TypeKind::Class(boxed, TypeArgs::intern(&[var])));
        // WHEN the checker asks about `Box<A>`: a conformance, its failing part and its probe
        c.conforms(held, source, TypeArgs::NONE);
        c.failing_leaf(held, source, TypeArgs::NONE);
        c.probe_associated(held, source, TypeArgs::NONE);
        let closed = Type::intern(TypeKind::Class(boxed, TypeArgs::intern(&[Type::I64])));
        c.conforms(closed, source, TypeArgs::NONE);
        c.failing_leaf(closed, source, TypeArgs::NONE);
        // The key with no variable is kept, as before.
        assert!(c.conformed.contains_key(&(closed, source, TypeArgs::NONE)));
        // THEN no check-wide cache keeps a key that holds the variable
        assert!(
            !c.conformed
                .keys()
                .any(|(ty, _, args)| ty.holds_infer() || args.holds_infer())
        );
        assert!(
            !c.leaves
                .keys()
                .any(|(ty, _, args)| ty.holds_infer() || args.holds_infer())
        );
        assert!(
            !c.probed_types
                .iter()
                .any(|(ty, _, args)| ty.holds_infer() || args.holds_infer())
        );
        assert!(!c.probed_assoc.iter().any(|ty| ty.holds_infer()));
        Nothing(())
    });
}
