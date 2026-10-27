"""Unit tests for changed_tests.py: python3 -m unittest discover -s scripts -p 'test_*.py'."""
import os
import unittest

import changed_tests as ct
import git_fixture

THREE = """#[test]
fn a_should_x() {
    one();
    two();
    three();
}

#[test]
fn b_should_y() {
    four();
}
"""


write = git_fixture.write
RAW = '#[test]\nfn a_should_x() {\n    let s = "\n-- x\n";\n    one();\n}\n\n#[test]\nfn b_should_y() {\n    four();\n}\n'
ONE_LINE = '#[test]\nfn a_should_x() {}\n#[test]\nfn b_should_y() {\n    four();\n}\n'
NO_NEWLINE = '#[test]\nfn a_should_x() {\n    one();\n}'
FAR = THREE + '\n#[test]\nfn c_should_z() {\n    six();\n}\n'


def changed_after(files, edit):
    """What `changed` finds after `edit(repo path)` changed the working tree of a repository."""
    with git_fixture.repo(files) as tmp:
        edit(tmp)
        return ct.changed('HEAD', tmp)


def rewrite(new):
    """An edit that writes `new` over t.rs."""
    return lambda tmp: write(tmp, 't.rs', new)


def move(tmp):
    git_fixture.git(tmp, 'mv', 't.rs', 'u.rs')


def move_and_edit(tmp):
    move(tmp)
    write(tmp, 'u.rs', THREE.replace('four();', 'five();'))


def edit_next_to_a_non_utf8_file(tmp):
    write(tmp, 't.rs', THREE.replace('four', 'five'))
    write(tmp, 'x.sisu', b'a\n\xe9\n')


def edit_with_diff_opts(tmp):
    os.environ['GIT_DIFF_OPTS'] = '--unified=7'  # `repo()` restores the environment at its exit
    write(tmp, 't.rs', THREE.replace('one', 'uno'))


def edit_a_sql_file_and_a_rust_test(tmp):
    write(tmp, 'a.sql', '++ b.rs\nq\nz\nQ\n')
    write(tmp, 't.rs', THREE.replace('four', 'five'))


def edit_the_last_line_of_a_file_with_no_final_newline_and_another_file(tmp):
    write(tmp, 't.rs', NO_NEWLINE + ' // end')
    write(tmp, 'u.rs', THREE.replace('four', 'five'))


def edit_a_rust_file_to_hold_a_non_utf8_byte(tmp):
    write(tmp, 't.rs', THREE.replace('four();', 'four(); // \u00e9').encode('latin-1'))


class ChangedTests(unittest.TestCase):
    def test_changed_tests_should_follow_only_body_lines(self):
        a, b = {('t.rs', 'a_should_x')}, {('t.rs', 'b_should_y')}
        glued = THREE.rstrip('\n') + '\n#[test]\nfn c_should_z() {\n    six();\n}\n'
        rows = [  # message, files, edit, the changed tests
            ('a deleted first line of a body', THREE, rewrite(THREE.replace('    one();\n', '')), a),
            ('a deleted middle line of a body', THREE, rewrite(THREE.replace('    two();\n', '')), a),
            ('a deleted last line of a body', THREE, rewrite(THREE.replace('    three();\n', '')), a),
            ('a deleted blank line between two tests', THREE, rewrite(THREE.replace('}\n\n#', '}\n#')), set()),
            ('a pure git mv of the file', THREE, move, set()),
            ('git mv plus an edit in one test', THREE, move_and_edit, {('u.rs', 'b_should_y')}),
            ('a new test after a blank line', THREE, rewrite(FAR), {('t.rs', 'c_should_z')}),
            ('a new test right after a baseline test', THREE, rewrite(glued), {('t.rs', 'c_should_z')}),
            ('an edit in a test that follows a one-line fn', ONE_LINE,
             rewrite(ONE_LINE.replace('four', 'five')), b),
            ('a removed line that starts with `-- ` and an edit in a later test', RAW,
             rewrite(RAW.replace('-- x\n', '').replace('four', 'five')), a | b),
            ('an added line that starts with `++ ` and an edit in a later test', RAW,
             rewrite(RAW.replace('-- x\n', '++ y\n').replace('four', 'five')), a | b),
            ('a non-UTF-8 line in a non-.rs file next to an edit', THREE, edit_next_to_a_non_utf8_file, b),
            ('a non-UTF-8 byte in a .rs file', THREE, edit_a_rust_file_to_hold_a_non_utf8_byte, b),
            ('GIT_DIFF_OPTS=--unified=7 and an edit in the test before a neighbour', THREE,
             edit_with_diff_opts, a),
            ('a `-- a` line of a text file changed to `++ b.rs` next to a .rs edit',
             {'t.rs': THREE, 'a.sql': '-- a\nq\nz\nq\n'}, edit_a_sql_file_and_a_rust_test, b),
            ('no final newline and an edit of its last line, and an edit in a second file',
             {'t.rs': NO_NEWLINE, 'u.rs': THREE},
             edit_the_last_line_of_a_file_with_no_final_newline_and_another_file,
             a | {('u.rs', 'b_should_y')}),
        ]
        for message, files, change, want in rows:
            with self.subTest(message):
                files = files if isinstance(files, dict) else {'t.rs': files}
                self.assertEqual(changed_after({'x.sisu': 'a\n', **files}, change), want, message)

    def test_the_users_git_config_should_not_change_the_result(self):
        text = FAR + '\n#[test]\nfn d_should_w() {\n    seven();\n}\n'
        with git_fixture.repo({'t.rs': text}) as tmp:
            for key, value in [('diff.mnemonicPrefix', 'true'), ('color.ui', 'always'),
                               ('diff.noprefix', 'true'), ('diff.interHunkContext', '100'),
                               ('diff.relative', 'true'), ('diff.external', 'false')]:
                git_fixture.git(tmp, 'config', key, value)
            # The tests between the two edits are unchanged, though one merged hunk would hold them.
            write(tmp, 't.rs', text.replace('one', 'uno').replace('seven', 'siete'))
            self.assertEqual(ct.changed('HEAD', tmp), {('t.rs', 'a_should_x'), ('t.rs', 'd_should_w')})

    def test_a_text_conversion_driver_should_not_change_the_result(self):
        with git_fixture.repo({'t.rs': THREE}) as tmp:
            write(tmp, '.gitattributes', '*.rs diff=cut\n')
            git_fixture.git(tmp, 'config', 'diff.cut.textconv', 'sed 1,5d')
            # With the first five lines cut from the diff, line 4 would be in `a_should_x`.
            write(tmp, 't.rs', THREE.replace('four', 'five'))
            self.assertEqual(ct.changed('HEAD', tmp), {('t.rs', 'b_should_y')})

    def test_a_deleted_test_should_not_be_changed(self):
        self.assertEqual(changed_after({'t.rs': THREE}, lambda t: write(t, 't.rs', THREE.split('\n\n')[0] + '\n')),
                         set(), 'a deleted test')

    def test_only_rust_files_should_count(self):
        self.assertEqual(changed_after({'t.txt': THREE}, lambda t: write(t, 't.txt', THREE.replace('one', 'uno'))),
                         set(), 'a file that is not .rs')

    def test_a_body_that_ends_early_at_a_raw_string_should_not_flag_an_unchanged_test(self):
        # `}` at the indent of the `fn` inside the raw string ends the body of `a` early.
        text = ('#[test]\nfn a_should_x() {\n    let s = r"\n}\n";\n    one();\n}\n\n'
                '#[test]\nfn b_should_y() {\n    four();\n}\n')
        for message, old, new, want in [
                ('a change below the raw string is missed', text, text.replace('one', 'uno'), set()),
                ('a change in the next test', text, text.replace('four', 'five'), {('t.rs', 'b_should_y')}),
                ('a change above the raw string', text, text.replace('let s', 'let t'), {('t.rs', 'a_should_x')})]:
            self.assertEqual(changed_after({'t.rs': old}, lambda t: write(t, 't.rs', new)), want, message)

    def test_the_body_should_end_at_the_closing_brace_of_its_fn(self):
        lines = ('#[test]\nfn a_should_x() {\n    let c = \'}\';\n    /* } */\n    one();\n}\n'
                 '#[test]\nfn b_should_y() {}\n#[test]\nfn c_should_z() {\n}\n').splitlines()
        self.assertEqual(ct.bodies(lines), [('a_should_x', 2, 6), ('b_should_y', 8, 8), ('c_should_z', 10, 11)])


if __name__ == '__main__':
    unittest.main()
