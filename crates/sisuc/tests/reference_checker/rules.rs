//! The reference checker's reading of rules and their IDs: which list items are rules, what a
//! page's `{#` items need, the faults of a bad ID, and the proof of each rule: its hash, its
//! tests and its section.

use crate::reference_check::markers::{RuleMarker, read_markers};
use crate::reference_check::rules::{id_faults, proof_faults};
use crate::reference_check::{Kind, read_page};

/// The faults that `id_faults` finds on the page `stem` with `page` as text, each as a line and a
/// message.
fn id_fault_list(stem: &str, page: &str) -> Vec<(usize, String)> {
    id_faults(stem, &read_page(page))
        .into_iter()
        .map(|fault| (fault.line, fault.message))
        .collect()
}

/// A table row: its name, a page, and the faults that the page should give.
type Row<'a> = (&'a str, &'a str, &'a [(usize, &'a str)]);

/// Asserts that `got` holds one fault for each `(line, fragment)` of `want`, in order, and that
/// the message of each holds its fragment. `row` names the table row in a failure.
fn assert_faults(row: &str, got: &[(usize, String)], want: &[(usize, &str)]) {
    assert_eq!(got.len(), want.len(), "row {row}: faults are {got:?}");
    for ((line, message), (want_line, fragment)) in got.iter().zip(want) {
        assert_eq!(line, want_line, "row {row}: the line of {message:?}");
        assert!(
            message.contains(fragment),
            "row {row}: {message:?} should hold {fragment:?}"
        );
    }
}

/// GIVEN a page where no list item starts with `{#`
/// WHEN `read_page` reads it
/// THEN the page does not join, so `reference.rs` makes no `rules` trial for it
/// AND its examples and grammar faults are as before
#[test]
fn page_without_an_id_item_should_not_join() {
    let text = "\
**Rules**

- one
- two

```sisu
fn main() {}
```
";
    let page = read_page(text);
    assert!(page.id_lines.is_empty(), "id lines are {:?}", page.id_lines);
    assert_eq!(page.rules.len(), 2, "rules are {:?}", page.rules);
    assert_eq!(page.examples.len(), 1, "examples are {:?}", page.examples);
    assert_eq!(page.examples[0].kind, Kind::Run);
    assert!(page.faults.is_empty(), "faults are {:?}", page.faults);
}

/// GIVEN a page with two rules, where only the first has an ID
/// WHEN its faults are found
/// THEN it fails with "this rule has no ID" at the line of the second rule
#[test]
fn rule_without_an_id_should_fail_when_the_page_has_ids() {
    let text = "**Rules**\n\n- {#strings.a} first\n- second\n";
    assert_faults(
        "second rule",
        &id_fault_list("strings", text),
        &[(4, "has no ID")],
    );
}

/// GIVEN a page whose only `{#` item is in a place that is not a top-level item of a list of rules
/// WHEN `read_page` reads it and its faults are found
/// THEN the page joins
/// AND the result is "an ID is only for a top-level item" at that line
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "a table of pages, one row for each place"
)]
fn id_should_fail_on_an_item_that_is_not_a_rule() {
    // Each row is a name, a page, and the faults. The `{#` item is the only one in its page.
    let rows: [Row<'_>; 19] = [
        (
            "a list after a plain sentence",
            "Some words.\n\n- {#strings.x} x\n",
            &[(3, "only for a top-level item")],
        ),
        (
            // The parent is a rule with no ID, so the page also needs one for it.
            "a nested item of a list of rules",
            "**Rules**\n\n- parent\n  - {#strings.x} nested\n",
            &[(3, "has no ID"), (4, "only for a top-level item")],
        ),
        (
            "an ordered list after a bold label",
            "**Rules**\n\n1. {#strings.x} x\n",
            &[(3, "only for a top-level item")],
        ),
        (
            "a list after a bold label inside a list item",
            "- item\n\n  **Rules**\n\n  - {#strings.x} x\n",
            &[(5, "only for a top-level item")],
        ),
        (
            "a list after a bold label and a note with a code span and a link",
            "**Rules**\n\nSee `x` and [y](z).\n\n- {#strings.note} x\n",
            &[],
        ),
        (
            "a list after a bold label and two paragraphs",
            "**Rules**\n\none\n\ntwo\n\n- {#strings.x} x\n",
            &[(7, "only for a top-level item")],
        ),
        (
            "an ID with no space after a plain sentence",
            "Some words.\n\n- {#strings.x}foo\n",
            &[(3, "only for a top-level item")],
        ),
        (
            "a list after a bold label inside a block quote",
            "> **Rules**\n>\n> - {#strings.x} x\n",
            &[(3, "only for a top-level item")],
        ),
        (
            "a loose list of rules with two IDs",
            "**Rules**\n\n- {#strings.a} one\n\n- {#strings.b} two\n",
            &[],
        ),
        (
            "a nested item on the line of its parent rule",
            "**Rules**\n\n- - {#strings.x} z\n",
            &[(3, "has no ID"), (3, "only for a top-level item")],
        ),
        (
            "a nested item on the line of its parent rule, in a loose list",
            "**Rules**\n\n- - {#strings.x} z\n\n- w\n",
            &[
                (3, "has no ID"),
                (3, "only for a top-level item"),
                (5, "has no ID"),
            ],
        ),
        (
            "a nested item on the line of its parent rule, with `*` bullets",
            "**Rules**\n\n* * {#strings.x} z\n",
            &[(3, "has no ID"), (3, "only for a top-level item")],
        ),
        (
            "a nested item on the line of its parent rule, with `+` bullets",
            "**Rules**\n\n+ + {#strings.x} z\n",
            &[(3, "has no ID"), (3, "only for a top-level item")],
        ),
        (
            "an item two levels deep on the line of its parent rule",
            "**Rules**\n\n- - - {#strings.x} z\n",
            &[(3, "has no ID"), (3, "only for a top-level item")],
        ),
        (
            "a nested item under a parent rule that has an ID",
            "**Rules**\n\n- {#strings.a} x\n  - {#strings.b} y\n",
            &[(4, "only for a top-level item")],
        ),
        (
            "a heading between the label and the list",
            "**Rules**\n\n## H\n\n- {#strings.a} x\n",
            &[(5, "only for a top-level item")],
        ),
        (
            "a block quote between the label and the list",
            "**Rules**\n\n> q\n\n- {#strings.a} x\n",
            &[(5, "only for a top-level item")],
        ),
        (
            // The site parses with HTML off, so this block is a paragraph there: the note.
            "an HTML block between the label and the list",
            "**Rules**\n\n<!-- c -->\n\n- {#strings.a} x\n",
            &[],
        ),
        (
            "an HTML block and a paragraph between the label and the list",
            "**Rules**\n\n<!-- c -->\n\nnote\n\n- {#strings.a} x\n",
            &[(7, "only for a top-level item")],
        ),
    ];
    for (row, text, want) in rows {
        let page = read_page(text);
        assert!(!page.id_lines.is_empty(), "row {row}: the page should join");
        assert_faults(row, &id_fault_list("strings", text), want);
    }
}

/// GIVEN a rule whose text follows `{#strings.text.null}` in a form that is not one space and then
/// the text on the same line
/// WHEN its faults are found
/// THEN the result is a fault for the form of the prefix at that line
#[test]
fn prefix_should_be_one_space_then_the_text() {
    let rows: [Row<'_>; 7] = [
        (
            "the text on the next line",
            "**Rules**\n\n- {#strings.text.null}\n  A string\n",
            &[(3, "one space")],
        ),
        (
            "no space",
            "**Rules**\n\n- {#strings.text.null}A string\n",
            &[(3, "one space")],
        ),
        (
            "no space, and the message names the ID",
            "**Rules**\n\n- {#strings.text.null}A string\n",
            &[(3, "after `{#strings.text.null}`")],
        ),
        (
            "two spaces",
            "**Rules**\n\n- {#strings.text.null}  A string\n",
            &[(3, "one space")],
        ),
        (
            "no closing brace",
            "**Rules**\n\n- {#strings.text.null A map {k} is\n",
            &[(3, "one space")],
        ),
        (
            "an empty ID, and the message names it",
            "**Rules**\n\n- {#} x\n",
            &[(3, "the ID \"\" has a bad form")],
        ),
        (
            "a code span after the space",
            "**Rules**\n\n- {#strings.text.join} `+` joins two strings\n",
            &[],
        ),
    ];
    for (row, text, want) in rows {
        assert_faults(row, &id_fault_list("strings", text), want);
    }
}

/// GIVEN `README.md` with the item `- {#README.a.b} x`
/// WHEN `read_page` reads it and its faults are found
/// THEN the result is the fault "the index cannot have rule IDs" at that line
#[test]
fn index_should_fail_on_each_id_item() {
    // The third rule has no ID, and the index never gets "has no ID".
    let text = "**Rules**\n\n- {#README.a.b} x\n- {#README.a.c} y\n- z\n";
    assert_faults(
        "README",
        &id_fault_list("README", text),
        &[
            (3, "the index cannot have rule IDs"),
            (4, "the index cannot have rule IDs"),
        ],
    );
}

/// GIVEN a page whose only `{#` item is the rule `- {#Text.null} A string can hold ...`
/// WHEN its faults are found
/// THEN it fails with a fault for the form of the ID at that line
#[test]
fn malformed_id_should_make_the_page_join() {
    let text = "**Rules**\n\n- {#Text.null} A string can hold ...\n";
    let page = read_page(text);
    assert_eq!(page.id_lines, [3], "the page should join");
    assert_faults(
        "Text.null",
        &id_fault_list("strings", text),
        &[(3, "has a bad form")],
    );
}

/// GIVEN a rule on `strings.md` with an ID
/// WHEN its faults are found
/// THEN the result is a fault when the ID does not start with the stem or has a bad form
#[test]
fn id_should_have_the_stem_and_a_valid_form() {
    let rows: [(&str, &[(usize, &str)]); 6] = [
        ("strings.text.null", &[]),
        ("strings.null", &[]),
        ("text.null", &[(3, "must start with")]),
        ("strings.Text.null", &[(3, "has a bad form")]),
        ("strings.text..null", &[(3, "has a bad form")]),
        ("strings.2text", &[(3, "has a bad form")]),
    ];
    for (id, want) in rows {
        let text = format!("**Rules**\n\n- {{#{id}}} A string.\n");
        assert_faults(id, &id_fault_list("strings", &text), want);
    }
}

/// GIVEN `strings.md` with two rules whose ID is `strings.text.null`
/// WHEN its faults are found
/// THEN it fails with "is also the ID of" and names both lines
#[test]
fn repeated_id_should_fail_with_both_lines() {
    let text = "**Rules**\n\n- {#strings.text.null} a\n- {#strings.text.null} b\n";
    let faults = id_fault_list("strings", text);
    assert_faults("repeat", &faults, &[(4, "is also the ID of")]);
    assert!(
        faults[0].1.ends_with("at line 3."),
        "the message should name line 3: {:?}",
        faults[0].1
    );
}

/// A table row: its name, a page, its markers, and the faults that the page should give.
type ProofRow<'a> = (&'a str, &'a str, Vec<RuleMarker>, &'a [(usize, &'a str)]);

/// A marker of the rule `id`, with `hash`, on `line` of `file`.
fn marker(file: &str, line: usize, id: &str, hash: &str) -> RuleMarker {
    RuleMarker {
        file: file.to_string(),
        line,
        id: id.to_string(),
        hash: hash.to_string(),
    }
}

/// The faults of the page `stem` with `page` as text and `markers`, as the `rules` trial finds
/// them: those of `id_faults` and `proof_faults`, sorted by line.
fn rule_fault_list(stem: &str, page: &str, markers: &[RuleMarker]) -> Vec<(usize, String)> {
    let read = read_page(page);
    let mut faults = id_faults(stem, &read);
    faults.extend(proof_faults(stem, &read, markers));
    faults.sort_by_key(|fault| fault.line);
    faults
        .into_iter()
        .map(|fault| (fault.line, fault.message))
        .collect()
}

/// The hash of each rule of `page` that has an ID, as a marker in a test file.
fn current_markers(page: &str) -> Vec<RuleMarker> {
    read_page(page)
        .rules
        .iter()
        .filter_map(|rule| {
            let id = rule.id.as_deref()?;
            Some(marker("crates/sisuc/tests/run/t.sisu", 1, id, &rule.hash))
        })
        .collect()
}

/// GIVEN the one rule of `**Rules**` and `- {#strings.text.null} A string can hold the null
/// character `\0`. It counts as one byte.`
/// WHEN its hash is found
/// THEN the hash is `3958`
#[test]
fn hash_should_be_fnv1a_of_the_normalized_text() {
    let text = "**Rules**\n\n- {#strings.text.null} A string can hold the null character `\\0`. \
                It counts as one byte.\n";
    let page = read_page(text);
    let [rule] = page.rules.as_slice() else {
        panic!("rules are {:?}", page.rules)
    };
    assert_eq!(rule.hash, "3958");
}

/// GIVEN a joined page whose one rule has the ID `strings.text.null`
/// AND a `tests/run` case with a marker for `strings.text.null` that has the rule's hash
/// WHEN its faults are found
/// THEN it passes
#[test]
fn rule_with_a_current_marker_should_pass() {
    let text = "**Rules**\n\n- {#strings.text.null} A string can hold the null character.\n";
    let hash = read_page(text).rules[0].hash.clone();
    let markers = [marker(
        "crates/sisuc/tests/run/null.sisu",
        1,
        "strings.text.null",
        &hash,
    )];
    assert_faults(
        "current marker",
        &rule_fault_list("strings", text, &markers),
        &[],
    );
}

/// GIVEN a joined page with a rule `strings.text.null` that no test names
/// WHEN its faults are found
/// THEN it fails with "strings.text.null has no test"
/// AND the message holds the marker with the current hash
#[test]
fn rule_without_a_marker_should_fail_with_its_hash() {
    let text = "**Rules**\n\n- {#strings.text.null} A string can hold the null character `\\0`. \
                It counts as one byte.\n";
    let faults = rule_fault_list("strings", text, &[]);
    assert_faults(
        "no marker",
        &faults,
        &[(3, "strings.text.null has no test")],
    );
    assert!(
        faults[0].1.contains("r[verify strings.text.null@3958]"),
        "the message should hold the marker: {:?}",
        faults[0].1
    );
}

/// GIVEN a joined page where the ID of a rule has a fault
/// WHEN the faults of its IDs and of its proofs are found
/// THEN the rule has the fault of its ID and no fault for a missing or a changed test
#[test]
fn rule_with_a_bad_id_should_get_no_proof_fault() {
    // The second page has the ID of line 3 twice, with different texts.
    let repeat = "**Rules**\n\n- {#strings.text.null} first\n- {#strings.text.null} second\n";
    let hash = read_page(repeat).rules[0].hash.clone();
    let rows: [ProofRow<'_>; 2] = [
        (
            "a wrong stem",
            "**Rules**\n\n- {#text.null} x\n",
            Vec::new(),
            &[(3, "must start with")],
        ),
        (
            "a repeated ID",
            repeat,
            vec![marker("crates/a.sisu", 1, "strings.text.null", &hash)],
            &[(4, "is also the ID of")],
        ),
    ];
    for (row, text, markers, want) in rows {
        assert_faults(row, &rule_fault_list("strings", text, &markers), want);
    }
}

/// GIVEN a joined page with the rules `- {#text.null} x` and `- {#strings.a.b} y`
/// AND no marker
/// WHEN the faults of its IDs and of its proofs are found
/// THEN the first rule has the fault of its ID only
/// AND the second rule has a fault for a missing test
#[test]
fn proof_check_should_skip_a_rule_and_not_the_page() {
    let text = "**Rules**\n\n- {#text.null} x\n- {#strings.a.b} y\n";
    assert_faults(
        "two rules",
        &rule_fault_list("strings", text, &[]),
        &[(3, "must start with"), (4, "strings.a.b has no test")],
    );
}

/// GIVEN a rule whose marker holds the hash of its old text
/// WHEN its faults are found
/// THEN it fails with "the text of strings.text.null changed"
/// AND the message names each file and line of a marker with the old hash
/// AND it gives the new hash
#[test]
fn changed_rule_should_list_each_stale_marker() {
    let text = "**Rules**\n\n- {#strings.text.null} A string can hold the null character.\n";
    let hash = read_page(text).rules[0].hash.clone();
    let markers = [
        marker(
            "crates/sisuc/tests/run/a.sisu",
            3,
            "strings.text.null",
            "0000",
        ),
        marker(
            "crates/sisuc/tests/run/c.sisu",
            1,
            "strings.text.null",
            &hash,
        ),
        marker("crates/sisuc/tests/b.rs", 9, "strings.text.null", "0000"),
    ];
    let faults = rule_fault_list("strings", text, &markers);
    assert_faults(
        "stale",
        &faults,
        &[(3, "the text of strings.text.null changed")],
    );
    let message = &faults[0].1;
    for part in [
        format!("@{hash}"),
        "a.sisu:3".to_string(),
        "b.rs:9".to_string(),
        "@0000".to_string(),
    ] {
        assert!(message.contains(&part), "{message:?} should hold {part:?}");
    }
    assert!(
        !message.contains("c.sisu"),
        "{message:?} names a current marker"
    );
    let (first, second) = (message.find("a.sisu:3"), message.find("b.rs:9"));
    assert!(
        first < second,
        "{message:?} should list the markers in order"
    );
}

/// GIVEN a rule with a current marker
/// WHEN the rule changes by a change in the rows
/// AND its faults are found
/// THEN a change to the white space, a link target, a code span or emphasis gives no fault
/// AND a change to a word, a link text, a code text or a nested item gives a stale-hash fault
#[test]
fn hash_should_change_only_with_the_meaning() {
    let base = "**Rules**\n\n- {#strings.a} A string with [E0068](x.md) and `len` and \
                [`cap`](c.md) and a word.\n  - nested note\n";
    let change = |from: &str, to: &str| (base.to_string(), base.replace(from, to));
    // Each row is a name, the old page, the new page, and whether the hash should change.
    let rows: [(&str, (String, String), bool); 11] = [
        (
            "a new line break",
            change(" and `len`", "\n  and `len`"),
            false,
        ),
        ("a new link target", change("(x.md)", "(y.md)"), false),
        ("a changed word", change("a word.", "a verb."), true),
        (
            "a changed diagnostic code",
            change("[E0068]", "[E0069]"),
            true,
        ),
        (
            "a changed nested item",
            change("nested note", "nested other"),
            true,
        ),
        (
            "a word that becomes a code span",
            change("a word.", "a `word`."),
            false,
        ),
        (
            "a word that gets emphasis",
            change("a word.", "a *word*."),
            false,
        ),
        (
            "a changed word in a code span",
            change("`len`", "`size`"),
            true,
        ),
        (
            "a changed word in a code link",
            change("[`cap`]", "[`top`]"),
            true,
        ),
        (
            "a letter moves into the nested item",
            (
                "**Rules**\n\n- {#strings.a} ab\n  - c\n".to_string(),
                "**Rules**\n\n- {#strings.a} a\n  - bc\n".to_string(),
            ),
            true,
        ),
        (
            "the list becomes loose",
            (
                "**Rules**\n\n- {#strings.a} one\n- {#strings.b} two\n".to_string(),
                "**Rules**\n\n- {#strings.a} one\n\n- {#strings.b} two\n".to_string(),
            ),
            false,
        ),
    ];
    for (row, (old, new), stale) in rows {
        assert_ne!(old, new, "row {row}: the page should change");
        let faults = rule_fault_list("strings", &new, &current_markers(&old));
        let want: &[(usize, &str)] = if stale { &[(3, "changed")] } else { &[] };
        assert_faults(row, &faults, want);
    }
}

/// GIVEN a joined page with a rule with an ID, and a place for a pending example
/// WHEN its faults are found
/// THEN a rule in a `###` section with a `sisu pending` example needs no test
/// AND a stale marker of it still fails
/// AND a pending example before the first `##` or under a `####` heading puts no rule in
/// development
/// AND a `##` section is a section in development, as a `###` section is
#[test]
fn rule_in_development_should_need_no_test() {
    let pending = "```sisu pending\nfn main() {}\n```\n";
    let stale = [marker("crates/a.sisu", 1, "strings.a", "0000")];
    let section = format!("### S\n\n{pending}\n**Rules**\n\n- {{#strings.a}} x\n");
    // The rule is in the same region as the pending example, under a `#` title.
    let before = format!("# T\n\n{pending}\n**Rules**\n\n- {{#strings.a}} x\n\n## S\n");
    let level_two = format!("## S\n\n{pending}\n**Rules**\n\n- {{#strings.a}} x\n");
    let level_four = format!("#### T\n\n{pending}\n**Rules**\n\n- {{#strings.a}} x\n");
    let deeper = format!("### S\n\n**Rules**\n\n- {{#strings.a}} x\n\n#### T\n\n{pending}");
    let rows: [(&str, &str, &[RuleMarker], &str, usize); 6] = [
        ("pending in the section", &section, &[], "", 0),
        ("pending and a stale marker", &section, &stale, "changed", 9),
        ("pending in a `##` section", &level_two, &[], "", 0),
        (
            "pending before the first `##`, under a title",
            &before,
            &[],
            "has no test",
            9,
        ),
        (
            "pending under a `####` heading, with the rule",
            &level_four,
            &[],
            "has no test",
            9,
        ),
        (
            "pending under a deeper heading",
            &deeper,
            &[],
            "has no test",
            5,
        ),
    ];
    for (row, text, markers, fragment, line) in rows {
        let faults = rule_fault_list("strings", text, markers);
        let want: &[(usize, &str)] = if fragment.is_empty() {
            &[]
        } else {
            &[(line, fragment)]
        };
        assert_faults(row, &faults, want);
    }
}

/// GIVEN a joined page with a `###` section that holds a `sisu pending` example and a plain `sisu`
/// example
/// WHEN its faults are found
/// THEN it fails with a fault that says to split the section, at the line of its heading
#[test]
fn mixed_section_should_fail_at_its_heading() {
    let text = "\
## One

**Rules**

- {#strings.a} x

### Two

```sisu pending
fn main() {}
```

```sisu
fn main() {}
```
";
    let page = read_page(text);
    assert_eq!(
        page.examples.len(),
        2,
        "the examples are {:?}",
        page.examples
    );
    let markers = current_markers(text);
    assert_faults(
        "mixed",
        &rule_fault_list("strings", text, &markers),
        &[(7, "a pending example and a plain example")],
    );
}

/// GIVEN a joined page with the rules `strings.a` and `strings.b`
/// AND one test file with a marker line for each, with current hashes
/// WHEN its faults are found
/// THEN there is no fault
#[test]
fn one_test_should_prove_two_rules() {
    let text = "**Rules**\n\n- {#strings.a} one\n- {#strings.b} two\n";
    let hashes: Vec<String> = read_page(text)
        .rules
        .iter()
        .map(|r| r.hash.clone())
        .collect();
    let file = format!(
        "// r[verify strings.a@{}]\n// r[verify strings.b@{}]\n",
        hashes[0], hashes[1]
    );
    let (markers, faults) = read_markers("crates/sisuc/tests/run/a.sisu", file.as_bytes());
    assert!(faults.is_empty(), "marker faults are {faults:?}");
    let got: Vec<(usize, &str)> = markers.iter().map(|m| (m.line, m.id.as_str())).collect();
    assert_eq!(
        got,
        [(1, "strings.a"), (2, "strings.b")],
        "markers are {markers:?}"
    );
    assert_faults(
        "two rules",
        &rule_fault_list("strings", text, &markers),
        &[],
    );
}

/// GIVEN a joined page with a `###` section whose `sisu pending` example has a fault
/// WHEN its faults are found
/// THEN the example gives its fault in the page
/// AND the rule of the section gives no fault for a missing test
#[test]
fn pending_example_with_a_fault_should_keep_its_section_in_development() {
    let text =
        "### S\n\n```sisu pending args=\"a\"\nfn main() {}\n```\n\n**Rules**\n\n- {#strings.a} x\n";
    let page = read_page(text);
    let lines: Vec<usize> = page.faults.iter().map(|fault| fault.line).collect();
    assert_eq!(lines, [3], "the faults of the page are {:?}", page.faults);
    assert_faults("args", &rule_fault_list("strings", text, &[]), &[]);
}
