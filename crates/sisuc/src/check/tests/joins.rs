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
                assert!(!c.any_entries.keys().any(|(_, args)| args.holds_infer()));
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
