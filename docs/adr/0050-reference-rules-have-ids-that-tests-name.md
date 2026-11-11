# Reference rules have IDs that tests name

Status: accepted, 2026-11-11.

The language reference states what a program does, one rule for each bullet. Only
the examples on a page are checked. A rule that has no example, or an example
that does not show the rule, can go without a test. A rule can also change while
its test stays the same.

The decision has five parts:

- **A rule has an ID.** A rule is one top-level item of a list of rules. A page
  joins when one of its list items starts with `{#`. From then on, each rule of
  the page starts with its ID, such as `{#strings.text.null} `. The ID starts with
  the page's name, and it does not change when the wording of the rule changes.
- **A test names the rule.** A comment in a test, `r[verify <id>@<hash>]`, names
  the rule that the test proves. One test can name many rules, and one rule can
  have many tests. The comment is in a `.rs` file or a `.sisu` case under `crates/`.
- **The hash finds a change.** The hash is the high 16 bits of the 32-bit FNV-1a
  hash of the normalized text of the rule, as four hex digits. `hash_of` in
  `crates/sisuc/tests/reference_check/rules.rs` defines it. When the text
  changes, the hash of the rule no longer matches the hash in the marker. Then a
  person reads the tests again and changes each marker. No command rewrites the
  hashes.
- **`make test` checks all of it.** The `reference::<page>::rules` trial fails
  in these cases:
  - A rule has no ID, a bad ID or a repeated ID.
  - An ID is on an item that is not a rule, or in the index.
  - A rule has no test, or its text changed after its tests.
  - A section holds a pending example and a plain example.

  A rule in a section that holds a `sisu pending` example needs an ID but no
  test. That section starts at a `##` or `###` heading and ends at the next
  heading of any level.

  The `reference::markers` trial fails in three cases. A marker names no rule. A
  marker line does not parse. A marker is in a file that the site shows.
- **The site hides the ID.** `site/build.py` removes the prefix, sets the `id` of
  the rule's `<li>`, and lists the tests that name the rule, with a link to each
  marker. A link such as `strings.html#strings.text.null` opens the page at that
  rule.

The check does not decide that a marker is in a test that runs and proves its
rule. The review does.

## Considered options

- **Tracey, the tool**: rejected. Its command line scans only the file extensions
  on a fixed list. `.sisu` is not on it, and its config cannot add one. It reads a
  rule only as a paragraph that starts with `r[id]` or in a block quote, and the
  reference writes rules as bullets. It has no way to fail on a rule with no test.
- **The `tracey-core` crate as a dev-dependency**: rejected. It would replace
  about twenty lines of code. The page reading, the strict check and the hashes
  stay ours in any case.
- **A test for each function, or a mark on the code that implements a rule**:
  rejected. The compiler has about 7,100 functions, so it costs too much, and it
  checks the wrong thing: the reference states what a program does.
- **Proof by the examples of a page**: rejected. An example does not name the
  rule that it proves.

## Consequences

- The marker form is the form of Tracey, so a later move to Tracey needs only a
  script that converts the markers.
- A rule that no test can prove leaves the list of rules and goes into the prose
  of the page. For example, a rule about how the compiler is built.
- Each page joins in its own pull request. `strings.md` is the first page. A page
  that has not joined needs no ID and no test.
- A change to the definition of the hash makes every marker of every joined page
  stale, so the definition is fixed.
- The check reads the file system and not `git`, because `make mutation-test`
  copies the tree without `.git`.
- Each ADR numbered 0050 or later needs a `## Reconsider when` section. The
  `adr_problems` check in `site/build.py` fails without it. ADR 0046 names the
  same idea `## What would reverse this`. The next pull request that amends it
  renames the heading.

## Reconsider when

- Tracey gets all three abilities: it reads a rule in a bullet, its config can add the
  `.sisu` file type, and it can fail on a rule with no test.
- The check doubles from about 900 lines. That is the size of the check today. It is in
  `rules.rs`, `markers.rs`, and the rule parts of `reference_check/mod.rs`,
  `reference.rs` and `site/build.py`.
- `tracey-core` could replace more than the marker reader.
