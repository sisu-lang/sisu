//! The origin of an expected type: an E0032 says where its required type comes from, as a
//! secondary label (S21). These tests read positions and counts, never text.

use super::*;

/// How an E0032 explains its required type.
enum Explained {
    /// Exactly one secondary label, at this line and column, and no note.
    Label(u32, u32),
    /// No secondary label and no note.
    Nothing,
}

/// The line and column of `span` in `src`.
fn position(src: &str, span: Span) -> (u32, u32) {
    let (line, col) = at(src, span);
    let narrow = |n: usize| u32::try_from(n).expect("a test program is short");
    (narrow(line), narrow(col))
}

/// For each row (name, program, primary position, explanation): the program gives one error, an
/// E0032 at that position, explained as the row says. Warnings do not count. A failure names
/// each red row.
fn origin_rows(rows: &[(&str, &str, (u32, u32), Explained)]) {
    let mut red = Vec::new();
    for (name, src, primary, explained) in rows {
        let (_, ds) = checked_with(PRELUDE_SOURCE, src);
        let errors: Vec<_> = (ds.iter())
            .filter(|d| d.code().severity() == Severity::Error)
            .collect();
        let [d] = errors[..] else {
            red.push(format!("row `{name}`: {} errors, wanted 1", errors.len()));
            continue;
        };
        let found = (d.code(), position(src, d.span()));
        if found != (Code::Mismatch, *primary) {
            red.push(format!(
                "row `{name}`: found {found:?}, wanted E0032 at {primary:?}"
            ));
            continue;
        }
        let labels: Vec<_> = (d.secondary().iter())
            .map(|(s, _)| position(src, *s))
            .collect();
        let notes = d.notes().len();
        let wanted = match explained {
            Explained::Label(line, col) => vec![(*line, *col)],
            Explained::Nothing => Vec::new(),
        };
        if labels != wanted || notes != 0 {
            red.push(format!(
                "row `{name}`: labels at {labels:?} and {notes} notes, wanted labels at \
                 {wanted:?} and no note"
            ));
        }
    }
    assert!(
        red.is_empty(),
        "{} red rows:\n{}",
        red.len(),
        red.join("\n")
    );
}

#[test]
fn a_break_origin_should_reach_each_part_of_the_value() {
    origin_rows(&[
        (
            "an element of a `break` value",
            "fn f(c: bool) {\n    let a = loop {\n        if c { break [1] }\n        break [2, \"x\"]\n    }\n    print(a)\n}\nfn main() {\n    f(true)\n}\n",
            (4, 19),
            Explained::Label(3, 16),
        ),
        (
            "a block of a `break` value",
            "fn f(c: bool) {\n    let a = loop {\n        if c { break 1 }\n        break { None }\n    }\n    print(a)\n}\nfn main() {\n    f(true)\n}\n",
            (4, 17),
            Explained::Label(3, 16),
        ),
        (
            "a branch of a `break` value",
            "fn main() {\n    let a = loop {\n        if true { break 1 }\n        break if true { 1 } else { \"x\" }\n    }\n    print(a)\n}\n",
            (4, 36),
            Explained::Label(3, 19),
        ),
        (
            "a branch of a `break` value after a `string` branch",
            "fn f(c: bool, d: bool) {\n    let a = loop {\n        if c { break 1 }\n        break if d { \"x\" } else { 2 }\n    }\n    print(a)\n}\nfn main() {\n    f(true, true)\n}\n",
            (4, 22),
            Explained::Label(3, 16),
        ),
        (
            "a `break` value, the row of S21",
            "fn f(c: bool) {\n    let a = loop {\n        if c { break 1 }\n        break \"x\"\n    }\n    print(a)\n}\nfn main() {\n    f(true)\n}\n",
            (4, 15),
            Explained::Label(3, 16),
        ),
        (
            "a `Some` payload in a `break` value, a form that waits",
            "fn f(c: bool) {\n    let a = loop {\n        if c { break Some(1) }\n        break Some(\"x\")\n    }\n    print(a)\n}\nfn main() { f(true) }\n",
            (4, 20),
            Explained::Nothing,
        ),
        (
            "an `if` in a `break` value with a hint from its siblings",
            "fn f(c: bool, d: bool) {\n    let a = loop {\n        if c { break 1 }\n        break if d { [] } else { \"x\" }\n    }\n    print(a)\n}\nfn main() {\n    f(true, true)\n}\n",
            (4, 22),
            Explained::Label(3, 16),
        ),
    ]);
}
