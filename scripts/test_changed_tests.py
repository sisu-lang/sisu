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


def write(tmp, name, text):
    with open(os.path.join(tmp, name), 'w', encoding='utf-8') as file:
        file.write(text)


def changed_after(files, edit):
    """What `changed` finds after `edit(repo path)` changed the working tree of a repository."""
    with git_fixture.repo(files) as tmp:
        edit(tmp)
        return ct.changed('HEAD', tmp)


class ChangedTests(unittest.TestCase):
    def test_changed_tests_should_follow_only_body_lines(self):
        a, b = {('t.rs', 'a_should_x')}, {('t.rs', 'b_should_y')}
        rows = [
            ('a deleted first line of a body', THREE, lambda t: write(t, 't.rs', THREE.replace('    one();\n', '')), a),
            ('a deleted middle line of a body', THREE, lambda t: write(t, 't.rs', THREE.replace('    two();\n', '')), a),
            ('a deleted last line of a body', THREE, lambda t: write(t, 't.rs', THREE.replace('    three();\n', '')), a),
            ('a deleted blank line between two tests', THREE, lambda t: write(t, 't.rs', THREE.replace('}\n\n#', '}\n#')), set()),
        ]
        for message, files, edit, want in rows:
            self.assertEqual(changed_after({'t.rs': files}, edit), want, message)

    def test_a_pure_move_should_change_no_test(self):
        def move(t):
            git_fixture.git(t, 'mv', 't.rs', 'u.rs')
        self.assertEqual(changed_after({'t.rs': THREE}, move), set(), 'a pure git mv of the file')

    def test_a_move_with_an_edit_should_change_only_the_edited_test(self):
        def move_and_edit(t):
            git_fixture.git(t, 'mv', 't.rs', 'u.rs')
            write(t, 'u.rs', THREE.replace('four();', 'five();'))
        self.assertEqual(changed_after({'t.rs': THREE}, move_and_edit), {('u.rs', 'b_should_y')},
                         'git mv plus an edit in one test')

    def test_a_new_test_should_not_change_the_baseline_test_before_it(self):
        new = THREE + '\n#[test]\nfn c_should_z() {\n    six();\n}\n'
        # The new test goes right after `b_should_y`, with no line of its own between them.
        glued = THREE.rstrip('\n') + '\n#[test]\nfn c_should_z() {\n    six();\n}\n'
        for message, text in [('a new test after a blank line', new), ('a new test right after a test', glued)]:
            self.assertEqual(changed_after({'t.rs': THREE}, lambda t: write(t, 't.rs', text)),
                             {('t.rs', 'c_should_z')}, message)

    def test_an_edit_after_a_one_line_test_should_change_only_the_edited_test(self):
        text = '#[test]\nfn a_should_x() {}\n#[test]\nfn b_should_y() {\n    four();\n}\n'
        self.assertEqual(
            changed_after({'t.rs': text}, lambda t: write(t, 't.rs', text.replace('four', 'five'))),
            {('t.rs', 'b_should_y')}, 'an edit in a test that follows a one-line fn')

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
