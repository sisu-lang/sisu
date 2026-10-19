# mir holds every count

Status: accepted, 2026-10-18. It replaces architecture plan 4's decision that
one pass labels each read of a local `Copy`, `Borrow` or `Move` in `tir`, and
carries over that plan's read-after-move fault.

Three modules (`last_use`, codegen and the loop hoist) each decided which
reads of a local own their value, and `last_use` restated codegen's
evaluation order in prose. No disagreement has shipped as a bug, but nothing
checked the modules against each other.

mir is a control-flow graph of basic blocks between `tir` and LLVM IR. It
holds every count as a statement, so one place says where each reference is
taken and given up.

- **Counts are statements.** `copy` adds a reference, `release` gives one up,
  `release?` gives one up only if the local still owns it, `make_unique`
  copies a shared block before a write, and `take` plus a store replaces a
  field. A move is an operand: a consuming read that leaves its local empty.
- **One pass places them.** The ownership pass turns each consuming read
  into a copy or a move, marks borrowed reads, and places their releases. The
  lowering from `tir` emits the releases that scopes and stores require: a
  `release?` before a whole-local reassignment, and scope releases on
  `return`, `break` and `continue`.
- **Codegen obeys.** It lowers each statement as written and decides nothing
  about counts.
- **The certifier checks.** In debug builds of `sisuc`, a dataflow over every
  function checks that each owned reference is consumed exactly once, that no
  empty local is read, and that a borrow does not outlive its owner.
- **The read-after-move fault stays.** Under `--rc-checks`, a read of a slot
  that a move emptied faults (`sisu_rc_fault` kind 2, `RC_FAULT_MOVED`).

## Considered options

- **Plan 4: labels on `tir` reads (`Read { Copy, Borrow, Move }`).** One walk
  decides each read, and codegen obeys the label. But codegen still places
  the retains and releases itself, and its evaluation order still has to
  match the pass's. A missed release is not visible to any checker.
- **mir with every count as a statement (this decision).** The labels become
  operand kinds and count statements, so a certifier can check them. It costs
  a new IR and a lowering for every `tir` construct that reaches codegen.

## Prior art

Checked with GitHits on 2026-10-18.

- **Swift's ownership verifier** is the certifier's precedent: ownership kinds
  and the consumed-exactly-once rule (SIL's Ownership.md), checked by
  SILOwnershipVerifier.cpp (swiftlang/swift@bc975a95).
- **Rust's MIR** is the shape's precedent: statements and terminators in
  basic blocks, places with projections, and `Copy`/`Move` operands
  (rust-lang/rust@bf9448e4, rustc_middle's mir/syntax.rs).
- Roc's and Lean 4's IR checkers check only that a counted symbol exists, so
  they are not precedent for the certifier.

## Consequences

- A new `tir` construct gets its ownership arm in the lowering to mir and in
  the ownership pass. The certifier turns a wrong placement into a failing
  test, in every `cargo nextest` run.
- `ExprKind::Move` leaves `tir`; `--emit mir` shows `move a#0`.
- `Capture { moved }` is decided by the ownership pass, not by `last_use`.
- Optimization passes (mir parts 2 to 5) work on counts as statements.
