//! The reference checker's reading of rules and their IDs: which list items are rules, what a
//! page's `{#` items need, and the faults of a bad ID.

use crate::reference_check::rules::id_faults;
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
fn id_should_fail_on_an_item_that_is_not_a_rule() {
    // Each row is a name, a page, and the faults. The `{#` item is the only one in its page.
    let rows: [Row<'_>; 9] = [
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
    let rows: [Row<'_>; 5] = [
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
        faults[0].1.contains('3'),
        "the message should name line 3: {:?}",
        faults[0].1
    );
}
