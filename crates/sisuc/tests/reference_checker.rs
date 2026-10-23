//! The checker of the language reference's examples (`reference_check`), fed small made-up
//! pages: pages that must pass, pages whose examples must fail, and malformed pages.

use std::path::Path;

mod reference_check;
#[expect(dead_code, reason = "this binary uses only part of the shared helpers")]
mod support;

use reference_check::{check, output_lines, pending_result, read_page};

/// Checks the one example of `page` in a fresh case directory named after `test`. It asserts
/// that the page has no fault and one example, whose line is the line of the page's `sisu` fence.
fn verdict(test: &str, page: &str) -> Result<(), String> {
    let read = read_page(page);
    assert!(read.faults.is_empty(), "faults in {page}");
    let [example] = read.examples.as_slice() else {
        panic!("{} examples in {page}", read.examples.len())
    };
    let fence = page
        .lines()
        .position(|line| line.starts_with("```sisu"))
        .expect("the page should have a sisu fence");
    assert_eq!(example.line, fence + 1, "the example's line in {page}");
    let dir = support::case_dir(&Path::new("reference_checker").join(test));
    check(example, &dir)
}

/// Checks that the one example of `page` fails, with `fragment` in the checker's error.
fn fails_with(test: &str, page: &str, fragment: &str) {
    let failure = verdict(test, page).expect_err("the example should fail");
    assert!(failure.contains(fragment), "{failure}");
}

/// The lines of the faults in `page`, each fault with a message.
fn faults(page: &str) -> Vec<usize> {
    let read = read_page(page);
    for fault in &read.faults {
        assert!(!fault.message.is_empty(), "fault on line {}", fault.line);
    }
    read.faults.iter().map(|fault| fault.line).collect()
}

/// GIVEN a page with the example `print(42) // 42`
/// WHEN the checker checks the page
/// THEN the result is pass
#[test]
fn output_line_should_be_checked() {
    let page = "\
```sisu
fn main() {
    print(42) // 42
}
```
";
    assert_eq!(verdict("output_line_should_be_checked", page), Ok(()));
}

/// GIVEN a page with the example `print(42) // 43`
/// WHEN the checker checks the page
/// THEN the result is fail
#[test]
fn wrong_output_line_should_fail() {
    let page = "\
```sisu
fn main() {
    print(42) // 43
}
```
";
    fails_with(
        "wrong_output_line_should_fail",
        page,
        r#"line 2: the output line is "43""#,
    );
}

/// GIVEN a page with the example `print(1)` and `print(2) // 2`
/// WHEN the checker checks the page
/// THEN the result is fail
#[test]
fn unlisted_output_should_fail() {
    let page = "\
```sisu
fn main() {
    print(1)
    print(2) // 2
}
```
";
    fails_with(
        "unlisted_output_should_fail",
        page,
        r#"line 3: the output line is "2", and the program prints "1\n""#,
    );
}

/// GIVEN a page with a loop that prints 1 and 2, with an `output` block "1\n2"
/// WHEN the checker checks the page
/// THEN the result is pass
#[test]
fn output_block_should_be_the_whole_output() {
    let page = "\
```sisu
fn main() {
    for i in [1, 2] {
        print(i)
    }
}
```
```output
1
2
```
";
    assert_eq!(
        verdict("output_block_should_be_the_whole_output", page),
        Ok(())
    );
}

/// GIVEN a page with `let x = 1 // a note` then `print(x)`, with an `output` block "1"
/// WHEN the checker checks the page
/// THEN the result is pass
#[test]
fn comments_should_be_plain_with_an_output_block() {
    let page = "\
```sisu
fn main() {
    let x = 1 // a note
    print(x)
}
```
```output
1
```
";
    assert_eq!(
        verdict("comments_should_be_plain_with_an_output_block", page),
        Ok(())
    );
}

/// GIVEN a page with `print(\"a // b\")` with an `output` block "a // b"
/// WHEN the checker checks the page
/// THEN the result is pass
#[test]
fn slashes_in_a_string_should_not_be_output_with_a_block() {
    let page = "\
```sisu
fn main() {
    print(\"a // b\")
}
```
```output
a // b
```
";
    assert_eq!(
        verdict(
            "slashes_in_a_string_should_not_be_output_with_a_block",
            page
        ),
        Ok(())
    );
}

/// GIVEN a page with `print(read_line() ?? \"none\") // hi` with an `input` block "hi"
/// WHEN the checker checks the page
/// THEN the result is pass
#[test]
fn input_block_should_be_standard_input() {
    let page = "\
```sisu
fn main() {
    print(read_line() ?? \"none\") // hi
}
```
```input
hi
```
";
    assert_eq!(
        verdict("input_block_should_be_standard_input", page),
        Ok(())
    );
}

/// GIVEN a page with `print(read_line() ?? \"none\") // hi` with no `input` block
/// WHEN the checker checks the page
/// THEN the result is fail
#[test]
fn missing_input_block_should_fail() {
    let page = "\
```sisu
fn main() {
    print(read_line() ?? \"none\") // hi
}
```
";
    fails_with(
        "missing_input_block_should_fail",
        page,
        r#"the program prints "none\n""#,
    );
}

/// GIVEN a page with a `sisu args=\"a b\"` example that prints each of its arguments
/// AND an `output` block of what it prints
/// WHEN the checker checks the page
/// THEN the result is pass
#[test]
fn arguments_should_reach_the_program() {
    let page = "\
```sisu args=\"a b\"
fn main(args: [string]) {
    for arg in args {
        print(arg)
    }
}
```
```output
a
b
```
";
    assert_eq!(verdict("arguments_should_reach_the_program", page), Ok(()));
}

/// GIVEN a page with the example `eprint(1)` with no `stderr` block
/// WHEN the checker checks the page
/// THEN the result is fail
#[test]
fn unexpected_standard_error_should_fail() {
    let page = "\
```sisu
fn main() {
    eprint(1)
}
```
";
    fails_with(
        "unexpected_standard_error_should_fail",
        page,
        "the standard error is not empty",
    );
}

/// GIVEN a page with the example `print(1000_) // error[E0008]` on line 2
/// WHEN the checker checks the page
/// THEN the result is pass
#[test]
fn error_marker_should_be_checked() {
    let page = "\
```sisu
fn main() {
    print(1000_) // error[E0008]
}
```
";
    assert_eq!(verdict("error_marker_should_be_checked", page), Ok(()));
}

/// GIVEN a page with the example `print(1000_) // error[E0009]` on line 2
/// WHEN the checker checks the page
/// THEN the result is fail
#[test]
fn wrong_code_should_fail() {
    let page = "\
```sisu
fn main() {
    print(1000_) // error[E0009]
}
```
";
    fails_with(
        "wrong_code_should_fail",
        page,
        "the markers give [E0009 on line 2], and sisu check reports [E0008 on line 2]",
    );
}

/// GIVEN a page with the E0008 line on line 2 and its marker on line 3
/// WHEN the checker checks the page
/// THEN the result is fail
#[test]
fn marker_on_the_wrong_line_should_fail() {
    let page = "\
```sisu
fn main() {
    print(1000_)
    print(1) // error[E0008]
}
```
";
    fails_with(
        "marker_on_the_wrong_line_should_fail",
        page,
        "the markers give [E0008 on line 3], and sisu check reports [E0008 on line 2]",
    );
}

/// GIVEN a page with a `let mut` never changed, marked `// warning[W0002]`, then `print(n) // 42`
/// WHEN the checker checks the page
/// THEN the result is pass
#[test]
fn warning_example_should_also_run() {
    let page = "\
```sisu
fn main() {
    let mut n = 42 // warning[W0002]
    print(n) // 42
}
```
";
    assert_eq!(verdict("warning_example_should_also_run", page), Ok(()));
}

/// GIVEN a page with an E0008 line with its marker and an E0009 line with none
/// WHEN the checker checks the page
/// THEN the result is fail
#[test]
fn unmarked_diagnostic_should_fail() {
    // The E0009 line is in a function of its own: after an error, `sisu` skips the rest of the
    // function body, so a second literal error in `main` is never reported.
    let page = "\
```sisu
fn big() {
    print(9223372036854775808)
}

fn main() {
    print(1000_) // error[E0008]
}
```
";
    fails_with(
        "unmarked_diagnostic_should_fail",
        page,
        "sisu check reports [E0008 on line 6, E0009 on line 2]",
    );
}

/// GIVEN a page with `todo` under `sisu panics` with `// warning[W0003]` and its `stderr` block
/// WHEN the checker checks the page
/// THEN the result is pass
#[test]
fn markers_should_work_with_any_kind() {
    let page = "\
```sisu panics
fn main() {
    todo // warning[W0003]
}
```
```stderr
sisu: panic at example.sisu:2:5: not yet implemented
```
";
    assert_eq!(verdict("markers_should_work_with_any_kind", page), Ok(()));
}

/// GIVEN a page with the W0002 example with `print(n) // 43`
/// WHEN the checker checks the page
/// THEN the result is fail
#[test]
fn warning_example_output_should_be_checked() {
    let page = "\
```sisu
fn main() {
    let mut n = 42 // warning[W0002]
    print(n) // 43
}
```
";
    fails_with(
        "warning_example_output_should_be_checked",
        page,
        r#"line 3: the output line is "43""#,
    );
}

/// GIVEN a page with a `let mut` never changed, with no marker
/// WHEN the checker checks the page
/// THEN the result is fail
#[test]
fn working_example_with_a_warning_should_fail() {
    let page = "\
```sisu
fn main() {
    let mut n = 42
    print(n) // 42
}
```
";
    fails_with(
        "working_example_with_a_warning_should_fail",
        page,
        "sisu build warns, and the example has no marker",
    );
}

/// GIVEN a page with a `sisu panics` example that adds 1 to the largest i64
/// AND a stderr block with the panic message at example.sisu
/// WHEN the checker checks the page
/// THEN it passes
/// AND it fails when the stderr block names another column
/// AND it fails when the example exits 0
#[test]
fn panics_example_should_check_its_exit_and_message() {
    let test = "panics_example_should_check_its_exit_and_message";
    let overflow = "\
```sisu panics
fn main() {
    let big = 9_223_372_036_854_775_807
    print(big + 1)
}
```
```stderr
sisu: panic at example.sisu:3:11: integer overflow
```
";
    assert_eq!(verdict(test, overflow), Ok(()));
    let column = overflow.replace("3:11", "3:12");
    fails_with(
        test,
        &column,
        "the standard error is not the `stderr` block",
    );
    // The same output and standard error as the panic, so only the exit code differs.
    let exits_0 = "\
```sisu panics
fn main() {
    eprint(\"sisu: panic at example.sisu:3:11: integer overflow\")
}
```
```stderr
sisu: panic at example.sisu:3:11: integer overflow
```
";
    fails_with(test, exits_0, "the program should exit 101");
}

/// GIVEN a page with a `sisu pending` example that compiles
/// WHEN the checker checks the page
/// THEN it fails and says to make the example a working one
#[test]
fn pending_example_that_compiles_should_fail() {
    let page = "\
```sisu pending
fn main() {}
```
";
    let failure = verdict("pending_example_that_compiles_should_fail", page)
        .expect_err("a pending example that compiles should fail");
    assert!(failure.contains("working example"), "{failure}");
}

/// GIVEN a page with the info string `sisu waiting` on line 3
/// WHEN the checker checks the page
/// THEN it fails and names the line of the fault
#[test]
fn unknown_word_should_be_a_fault() {
    let page = "\
# Page

```sisu waiting
fn main() {}
```
";
    assert_eq!(faults(page), [3]);
}

/// GIVEN a page with the info string `sisu pending panics` on line 3
/// WHEN the checker checks the page
/// THEN it fails and names the line of the fault
#[test]
fn exclusive_words_should_be_a_fault() {
    let page = "\
# Page

```sisu pending panics
fn main() {}
```
";
    assert_eq!(faults(page), [3]);
}

/// GIVEN a page with an `output` block after a paragraph, on line 11
/// WHEN the checker checks the page
/// THEN it fails and names the line of the fault
#[test]
fn companion_after_a_paragraph_should_be_a_fault() {
    let page = "\
# Page

```sisu
fn main() {
    print(1)
}
```

Some text.

```output
1
```
";
    assert_eq!(faults(page), [11]);
}

/// GIVEN a page with an indented code block on line 5
/// WHEN the checker checks the page
/// THEN it fails and names the line of the fault
#[test]
fn indented_code_should_be_a_fault() {
    let page = "\
# Page

Some text.

    let x = 1
    print(x)
";
    assert_eq!(faults(page), [5]);
}

/// GIVEN a page with a `rust` fence on line 3
/// WHEN the checker checks the page
/// THEN it fails and names the line of the fault
#[test]
fn unknown_fence_should_be_a_fault() {
    let page = "\
# Page

```rust
fn main() {}
```
";
    assert_eq!(faults(page), [3]);
}

/// GIVEN a page with an `output` block after an error example, on line 8
/// WHEN the checker checks the page
/// THEN it fails and names the line of the fault
#[test]
fn output_after_an_error_example_should_be_a_fault() {
    let page = "\
# Page

```sisu
fn main() {
    print(1000_) // error[E0008]
}
```
```output
```
";
    assert_eq!(faults(page), [8]);
}

/// GIVEN a page with a `sisu` fence inside a `<div>` HTML block, on line 3
/// WHEN the checker checks the page
/// THEN it fails and names the line of the fault
#[test]
fn fence_in_html_should_be_a_fault() {
    let page = "\
# Page

<div>
```sisu
fn main() {}
```
</div>
";
    assert_eq!(faults(page), [3]);
}

/// GIVEN pages with a `~~~` fence, a repeated word, a repeated `args` and `args` without quotes,
/// each on line 3
/// AND a page with a second `output` block on line 8
/// WHEN the checker checks each page
/// THEN each fails and names the line of the fault
#[test]
fn malformed_fences_should_each_be_a_fault() {
    for fence in [
        "~~~sisu\nfn main() {}\n~~~\n",
        "```sisu panics panics\nfn main() {}\n```\n",
        "```sisu args=\"a b\" args=\"c\"\nfn main() {}\n```\n",
        "```sisu args=a\nfn main() {}\n```\n",
    ] {
        assert_eq!(faults(&format!("# Page\n\n{fence}")), [3], "{fence}");
    }
    let second = "# Page\n\n```sisu\nfn main() {}\n```\n```output\n```\n```output\n```\n";
    assert_eq!(faults(second), [8]);
}

/// GIVEN a `sisu check` of a pending example that cannot read its file and exits 1
/// AND one that has no exit code
/// WHEN the checker judges each result
/// THEN each fails, names the status and does not say that the example compiles
#[test]
fn pending_check_failure_should_not_say_it_compiles() {
    let unread = pending_result(Some(1), "sisu: cannot read example.sisu: Permission denied")
        .expect_err("an exit 1 with no error line should fail");
    assert!(unread.contains("exit code 1"), "{unread}");
    assert!(!unread.contains("compiles"), "{unread}");
    let killed = pending_result(None, "").expect_err("a check with no exit code should fail");
    assert!(killed.contains("no exit code"), "{killed}");
    assert!(!killed.contains("compiles"), "{killed}");
}

/// GIVEN an example whose output line is the replacement character U+FFFD
/// AND a standard output of the byte 0xFF and a newline, which is not UTF-8
/// WHEN the checker compares them
/// THEN it fails and says that the output is not UTF-8
#[test]
fn output_that_is_not_utf8_should_be_named() {
    let source = "fn main() {\n    print(\"\u{FFFD}\") // \u{FFFD}\n}\n";
    let failure =
        output_lines(source, b"\xFF\n").expect_err("bytes that are not UTF-8 should fail");
    assert!(failure.contains("not UTF-8"), "{failure}");
}

/// GIVEN a page with a `sisu pending` example that calls a function that does not exist
/// AND the marker `// error[E0008]`, which is the wrong code
/// WHEN the checker checks the page
/// THEN the result is pass
#[test]
fn pending_example_should_not_check_its_markers() {
    let page = "\
```sisu pending
fn main() {
    not_written_yet() // error[E0008]
}
```
";
    assert_eq!(
        verdict("pending_example_should_not_check_its_markers", page),
        Ok(())
    );
}

/// GIVEN a page with an error example whose line 6 is `print(1) // 1`
/// WHEN the checker checks the page
/// THEN it fails and names the line of the output comment
#[test]
fn output_comment_on_an_error_example_should_be_a_fault() {
    let page = "\
# Page

```sisu
fn main() {
    print(1000_) // error[E0008]
    print(1) // 1
}
```
";
    assert_eq!(faults(page), [6]);
}

/// GIVEN a page with a `sisu no-run` example whose line 5 is `print(1) // 1`, then `loop {}`
/// WHEN the checker checks the page
/// THEN it fails and names the line of the output comment
#[test]
fn output_comment_on_a_no_run_example_should_be_a_fault() {
    let page = "\
# Page

```sisu no-run
fn main() {
    print(1) // 1
    loop {}
}
```
";
    assert_eq!(faults(page), [5]);
}

/// GIVEN a page with a `sisu no-run` example whose main is an endless loop
/// WHEN the checker checks the page
/// THEN it passes without running the program
#[test]
fn no_run_example_should_only_build() {
    // A run would never end, so a pass shows that the checker did not run the program.
    let page = "\
```sisu no-run
fn main() {
    loop {}
}
```
";
    assert_eq!(verdict("no_run_example_should_only_build", page), Ok(()));
}

/// GIVEN a page with a `sisu no-run args=\"a b\"` example on line 3
/// WHEN the checker checks the page
/// THEN it fails and names the line of the fence
#[test]
fn arguments_on_a_no_run_example_should_be_a_fault() {
    let page = "\
# Page

```sisu no-run args=\"a b\"
fn main() {
    loop {}
}
```
";
    assert_eq!(faults(page), [3]);
}

/// GIVEN a page with a `sisu panics` example with an error marker, on line 3
/// WHEN the checker checks the page
/// THEN it fails and names the line of the fence
#[test]
fn error_marker_on_a_panics_example_should_be_a_fault() {
    let page = "\
# Page

```sisu panics
fn main() {
    print(1000_) // error[E0008]
}
```
";
    assert_eq!(faults(page), [3]);
}

/// GIVEN a page with a `sisu no-run` example with an error marker, on line 3
/// WHEN the checker checks the page
/// THEN it fails and names the line of the fence
#[test]
fn error_marker_on_a_no_run_example_should_be_a_fault() {
    let page = "\
# Page

```sisu no-run
fn main() {
    print(1000_) // error[E0008]
}
```
";
    assert_eq!(faults(page), [3]);
}

/// GIVEN a page with a `sisu pending` example with an error marker and `print(1) // ignored`
/// WHEN the checker checks the page
/// THEN the result is pass
#[test]
fn pending_example_should_not_check_its_output_comments() {
    let page = "\
```sisu pending
fn main() {
    not_written_yet() // error[E0008]
    print(1) // ignored
}
```
";
    assert_eq!(
        verdict("pending_example_should_not_check_its_output_comments", page),
        Ok(())
    );
}

/// GIVEN a page with a link reference definition on line 1
/// AND code indented with four spaces, or with a tab, directly under it on line 2
/// WHEN the checker checks each page
/// THEN each fails and names line 2
#[test]
fn indented_code_after_a_link_definition_should_be_a_fault() {
    // markdown-it shows the indented lines as a code block, and pulldown-cmark reads them as
    // text of the definition's paragraph.
    for indent in ["    ", "\t"] {
        let page = format!(
            "[link]: https://example.com\n{indent}fn main() {{\n{indent}    print(42) // 43\n\
             {indent}}}\n\n```sisu\nfn main() {{}}\n```\n"
        );
        assert_eq!(faults(&page), [2], "{page}");
    }
}

/// GIVEN a page with a `sisu` fence in a block quote, on line 3
/// WHEN the checker checks the page
/// THEN it fails and names the line of the fence
#[test]
fn fence_in_a_block_quote_should_be_a_fault() {
    let page = "\
# Page

> ```sisu
> fn main() {}
> ```
";
    assert_eq!(faults(page), [3]);
}

/// GIVEN a page with `- item` then a `sisu` fence indented two spaces, on line 4
/// WHEN the checker checks the page
/// THEN it fails and names the line of the fence
#[test]
fn fence_in_a_list_item_should_be_a_fault() {
    let page = "\
# Page

- item
  ```sisu
  fn main() {}
  ```
";
    assert_eq!(faults(page), [4]);
}

/// GIVEN a page with `print(\"a\\tb\")` and an output comment with a tab between `a` and `b`
/// WHEN the checker checks the page
/// THEN it passes
/// AND it fails when the comment has a space for the tab
#[test]
fn tab_in_an_output_comment_should_be_compared() {
    let test = "tab_in_an_output_comment_should_be_compared";
    let tab = "\
```sisu
fn main() {
    print(\"a\\tb\") // a\tb
}
```
";
    assert_eq!(verdict(test, tab), Ok(()));
    fails_with(
        test,
        &tab.replace("a\tb", "a b"),
        r#"line 2: the output line is "a b""#,
    );
}

/// GIVEN a page with `fn main() {}` and no output lines
/// AND the same program with an empty `output` block
/// AND `print(1)` with an empty `output` block
/// WHEN the checker checks each page
/// THEN the first two pass and the third fails
#[test]
fn empty_output_block_should_mean_no_output() {
    let test = "empty_output_block_should_mean_no_output";
    let silent = "\
```sisu
fn main() {}
```
";
    assert_eq!(verdict(test, silent), Ok(()));
    let empty = format!("{silent}```output\n```\n");
    assert_eq!(verdict(test, &empty), Ok(()));
    let prints = empty.replace("fn main() {}", "fn main() {\n    print(1)\n}");
    fails_with(
        test,
        &prints,
        "the standard output is not the `output` block",
    );
}
