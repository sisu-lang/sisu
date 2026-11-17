# A join takes the type of one of its members

Status: accepted, 2026-11-16.

Before this decision, the checker gave a join the type of the first member that
it checked. It gave that type to the members after it as a hint. So the result
of a join changed with the order of its members (#756, #378, #755). The
body-inference spec makes these choices. They are recorded here because later
stages and later specs cite them.

## Decision

- **Rule A.** The type of a join is the type of one of its members, which each
  other member converts to. When no member's type takes all the others, the
  join is an error in each order of its members.
- **The `?`-added candidate.** When a member is optional, the type of each
  other member with `?` added is also a candidate. With `e: any Seen`,
  `[e, Some(Eye(v: 1))]` is `[(any Seen)?]` in each order.
- **No implicit `any`.** No join infers an `any I` that no member states
  (#755). So a new interface cannot make valid code an error.
- **Unify before wrap.** A member that converts to a candidate by unification
  gets no `Wrap` and no `ToAny`. Of two candidates that each take all members,
  the join is the one that needs the fewer wraps.
- **A nested join has no type of its own.** An `if` or a `match` at a value
  position inside a member of a join decides no type. Its members convert to
  the part of the outer candidate at their position, and a conflict names each
  member at each depth.
- **The report of a join has no direction.** With no required type, the E0032
  of a join names the form in its headline ("the branches of this `if` have
  different types"). It has its primary label at the head of the join and a
  label with the type of each group of members. So the report is the same in
  each order.
- **Under a required type, one report when the members agree.** The required
  type is the only candidate. When the members agree with each other and only
  the required type differs, one directed E0032 goes at the whole join, in
  each form. Else each member that does not convert gets the directed E0032
  (Q33).
- **Generic calls in a join.** The arguments at the places of one type
  parameter of a generic call are a join. Each type parameter that is not
  written is an inference variable, made at the call.
  - **Closed first** (Q22). The call decides each type parameter from its own
    arguments, and its result converts at its top only. A call under a
    required type does the same (Q26).
  - **Open when closed fails** (Q19). The closed result can fail to convert
    to the outer candidate or to the required type. Then the call can take the
    part of that type at the places of the type parameter in its return type.
  - **The `?`-added candidate applies to the parts of arguments** (Q20).
    `f([Some(1)], a)` with `fn f<T>(x: [T], y: T) -> T` and `a: u8` is valid,
    with `T = u8?`.
  - **One wrap per conversion point** (Q24, Q27). A closed call's result wraps
    at most once at its top, and each argument of an open call wraps at most
    once. So `let v: i64?? = h(1, Some(2))` with `fn h<T>(a: T, b: T?) -> T` is
    an error. The program writes the inner wrap.
  - **A place that names two type parameters is closed only** (Q28). An inner
    call that is an argument at `x: Two<T, U>` of an outer call does not open.

ADR 0023 still holds: a generic body is checked once, at its definition. Its
type arguments also come from the other members of a join, as before this
decision.

## Considered options

- **Rule B: infer `any I` when exactly one interface fits all members.**
  Rejected. The validity of a program would then depend on which interfaces
  exist, so a new interface could make valid code an error. The report of
  rule A names the interface as a help instead.
- **Make `Rank` complete.** The smallest change, but the rank reads from syntax
  which member gives a type. It is a second copy of the type rules, and each
  new feature needs a rank arm or the order bugs come back. Rejected.

## Consequences

- The checker has one entry that decides a join and one function that settles
  its members. The rank, the hints and the copies of the join rule go.
- Some programs that `main` accepts in one order only are valid in each order.
  Some programs with two wraps through a generic call become errors (Q24).
- The program fuzzer's judge checks that each join gives the same result in
  each order of its members.

## Reconsider when

- Sisu gets a number type that is not `i64`, `u8` or `f64`. An integer literal
  then has more types to settle to, and a float literal needs a `Float` kind.
- Milestone 9 adds closures. A closure's body shares the inference table of
  the function around it. So a join inside a closure can settle a type of the
  outer body.
