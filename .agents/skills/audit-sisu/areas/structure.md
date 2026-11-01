# Area: structure

Whether the known structural debt grew since the register last counted it.
`docs/register.md` lists each debt with its Rust items and, where there is
one, a count. This area counts each one again on this checkout, compares the
count with the register, and looks for new debt of the same kinds. It writes
no probe programs: its probes are the counts.

## Counts

Count outside test code: skip `tests/` directories, `tests.rs` files and
inline `#[cfg(test)] mod` blocks. Write the command and the count of each
row in `probes.md`.

- **Fields of the checker's state.** The fields of `check::Body` and of
  `check::Checker`. The `Body` debt row gives its count; compare `Checker`
  with the last audit's `probes.md` when there is one. Read the struct, or
  count its field lines:

  ```console
  $ awk '/^struct Body \{/,/^\}/' crates/sisuc/src/check/mod.rs | grep -cE '^    [a-z_]+:'
  ```

- **Wildcard arms at decision points.** The `_ =>` arms in each function
  that the wildcard debt row names. Read each function with CodeGraph
  (`codegraph explore "<function>"`). Also look for a new `_ =>` arm in a
  match over `tir`, `mir` or `Type` that decides a result for a variant.
- **Direct name-map reads.** The uses of the five name maps that the
  name-map row names, in `check/`:

  ```console
  $ grep -rnoE --include=*.rs '\.(function_ids|class_ids|enum_ids|interface_ids|constants)\b' crates/sisuc/src/check | grep -v '/tests/' | wc -l
  ```

- **Number-type lists.** The `matches!` patterns, `|` patterns and slices
  that name two or more number types (`Type::I64`, `Type::U8`, `Type::F64`,
  `TypeKind::*`). Start from this search and read each hit, because a line
  can hold one list and a list can span lines:

  ```console
  $ grep -rnE --include=*.rs '(Type|TypeKind)::(I64|U8|F64)\b.*(Type|TypeKind)::(I64|U8|F64)\b' crates/sisuc/src | grep -v '/tests/'
  ```

- **Module order.** The entries of `BACK_EDGES` in
  `crates/sisuc/tests/architecture.rs`. The test holds the rule, so only
  count the entries and compare them with the last audit.
- **The other rows.** For each other debt row, check that its Rust items
  still exist and that the problem it states is still there.

## A new hub

Look for a struct with more than 30 fields, a function that all of
`check/`, `mono`, `mir` and `codegen` call with a flag argument, and a rule
that two modules each state, such as a list of types or a calling
convention. A new one that no row names is a lead.

## Findings

A finding is a count above the register's, or a debt that no row names,
with the command that measures it as its Repro. Its severity is `gap`
([finding.md](../finding.md)). A count below the register's is a lead: the
row needs the smaller count, or the debt is paid. The suggested fix is a
register edit (a new row or a new count) or the change that pays the debt;
the suggested test is the command and the count it must not pass.
