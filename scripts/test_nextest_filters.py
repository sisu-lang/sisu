"""Unit tests for nextest_filters.py: python3 -m unittest discover -s scripts -p 'test_*.py'."""
import contextlib
import io
import json
import os
import tempfile
import unittest

import nextest_filters as nf

# A skipped suite has no `testcases` key.
TESTS = {
    'sisuc': {'testcases': {'check::tests::a_should_x': {}, 'check::tests::b_should_y': {}}},
    'sisuc::cases': {'testcases': {'run::fib.sisu': {}}},
    'skipped': {},
}


def overrides(*filters, table='overrides', profile='default'):
    """A config with one `[[profile.<profile>.<table>]]` per filter, each filter on its own line."""
    return ''.join(f"[[profile.{profile}.{table}]]\nfilter = '{each}'\npriority = 1\n\n" for each in filters)


def problems(config, tests=TESTS):
    """What `check` finds for a config text and a nextest list of `tests` (a name per suite)."""
    with tempfile.TemporaryDirectory() as tmp:
        config_path = os.path.join(tmp, 'nextest.toml')
        list_path = os.path.join(tmp, 'list.json')
        with open(config_path, 'w', encoding='utf-8') as file:
            file.write(config)
        with open(list_path, 'w', encoding='utf-8') as file:
            json.dump({'rust-suites': tests}, file)
        return [line.replace(config_path, 'CONFIG') for line in nf.check(config_path, list_path)]


def no_match(line, leaf):
    return f"CONFIG:{line}: filter leaf `test({leaf})` matches no test; the test was renamed or deleted"


def other_kind(line, leaf):
    return (f"CONFIG:{line}: filter leaf `test({leaf})` is not a name leaf; "
            "use test(=name) or test(substring)")


class NextestFilters(unittest.TestCase):
    def test_a_filter_that_matches_no_test_should_fail(self):
        config = overrides('test(=check::tests::a_should_x)', 'test(=check::tests::gone)')
        self.assertEqual(problems(config), [no_match(6, '=check::tests::gone')])
        scripts = overrides('test(gone_)', table='scripts')
        self.assertEqual(problems(scripts), [no_match(2, 'gone_')])
        ci = overrides('test(gone_)', profile='ci')
        self.assertEqual(problems(ci), [no_match(2, 'gone_')])

    def test_a_leaf_of_another_kind_should_fail(self):
        for leaf in ['/^check::/', '/^(a|b)$/', '/^a{2}/', '~check', '#check{a,b}', '#check*', r'check\.']:
            with self.subTest(leaf=leaf):
                self.assertEqual(problems(overrides(f'test({leaf})')), [other_kind(2, leaf)])
        spaced = overrides('test( /a)b/)')
        self.assertEqual(problems(spaced), [other_kind(2, '/a)b/')])

    def test_each_leaf_should_match_as_nextest_does(self):
        # (row, filter, the leaf that matches no test, if any)
        rows = [
            ('an = leaf equals a whole name', 'test(=run::fib.sisu)', None),
            ('an = leaf is not a prefix', 'test(=run::fib)', '=run::fib'),
            ('an = leaf is not a part of a name', 'test(=_should_x)', '=_should_x'),
            ('a bare leaf is a prefix', 'test(check::tests::a)', None),
            ('a bare leaf is a part of a name', 'test(_should_y)', None),
            ('space before a leaf argument is trimmed', 'test( _should_y)', None),
            ('space before a missing leaf is trimmed', 'test( _should_z)', '_should_z'),
            ('space after a leaf argument is kept', 'test(_should_y )', '_should_y '),
            ('a bare leaf that is no part fails', 'test(_should_z)', '_should_z'),
            ('a leaf in a group matches', 'binary(sisuc) & (test(=check::tests::b_should_y) | kind(test))', None),
            ('the missing leaf of a group fails', 'binary(sisuc) & (test(_should_x) | test(=no::test))', '=no::test'),
            ('a kind leaf is not a test leaf', 'kind(test) | package(sisuc)', None),
        ]
        for row, filter_, missing in rows:
            with self.subTest(row=row):
                expected = [] if missing is None else [no_match(2, missing)]
                self.assertEqual(problems(overrides(filter_)), expected, row)

    def test_a_leaf_with_whitespace_should_be_checked(self):
        spaced = "[[profile.default.overrides]]\nfilter = 'binary(a) & test (missing)'\n"
        self.assertEqual(problems(spaced), [no_match(2, 'missing')])
        across = "[[profile.default.overrides]]\nfilter = '''\nbinary(a) &\n  test\n(missing)'''\n"
        self.assertEqual(problems(across), [no_match(4, 'missing')])

    def test_a_stale_leaf_should_be_reported_at_each_line_where_a_filter_holds_it(self):
        gone = "filter = 'test(gone)'\n"
        rows = [  # row, config, the lines that report `gone`
            ('an inline comment before the filter holds the leaf',
             "[[profile.default.overrides]]\npriority = 1 # test(gone) was renamed\n" + gone, [3]),
            ('a comment line above the filter holds the leaf',
             "[[profile.default.overrides]]\n# test(gone) was renamed\n" + gone, [3]),
            ('a comment with an apostrophe and a quote above the filter',
             "[[profile.default.overrides]]\n# it's \"test(gone)\"\n" + gone, [3]),
            ('a comment that holds a filter assignment',
             "[[profile.default.overrides]]\n# filter = 'test(gone)'\n" + gone, [3]),
            ('the leaf in the default and the ci profile',
             overrides('test(gone)') + overrides('test(gone)', profile='ci'), [2, 6]),
            ('the leaf twice in one filter, on two lines',
             "[[profile.default.overrides]]\nfilter = \'\'\'\ntest(gone) |\ntest(gone)\'\'\'\n", [3, 4]),
            ('the leaf twice on one line is one report',
             "[[profile.default.overrides]]\nfilter = 'test(gone) | test(gone)'\n", [2]),
            ('a basic string and a script filter',
             '[[profile.default.overrides]]\nfilter = "test(gone)"\n[[profile.default.scripts]]\n' + gone, [2, 4]),
            ('a key that ends in filter is no filter',
             "[profile.default]\ndefault-filter = 'test(gone)'\n[[profile.default.overrides]]\n" + gone, [4])]
        for row, config, lines in rows:
            with self.subTest(row=row):
                self.assertEqual(problems(config), [no_match(line, 'gone') for line in lines], row)

    def test_an_override_for_a_platform_should_not_be_checked(self):
        platform = "[[profile.default.overrides]]\nplatform = 'cfg(target_os = \"macos\")'\nfilter = 'test(gone)'\n"
        table = "[[profile.default.overrides]]\nplatform = { host = 'cfg(unix)' }\nfilter = 'test(gone)'\n"
        for row, config, lines in [('a platform string', platform, []), ('a platform table', table, []),
                                   ('a platform override and one without', platform + overrides('test(gone_)'), [5])]:
            with self.subTest(row=row):
                self.assertEqual(problems(config), [no_match(line, 'gone_') for line in lines], row)

    def test_a_bad_leaf_should_fail_the_run_and_print_its_line(self):
        with tempfile.TemporaryDirectory() as tmp:
            config = os.path.join(tmp, 'nextest.toml')
            tests = os.path.join(tmp, 'list.json')
            with open(config, 'w', encoding='utf-8') as file:
                file.write(overrides('test(=check::tests::a_should_x)', 'test(=gone)'))
            with open(tests, 'w', encoding='utf-8') as file:
                json.dump({'rust-suites': TESTS}, file)
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                status = nf.main([config, tests])
        self.assertEqual((status, out.getvalue()),
                         (1, f"{config}:6: filter leaf `test(=gone)` matches no test; the test was renamed or deleted\n"))

    def test_a_config_with_nothing_to_check_should_print_nothing_and_pass(self):
        with tempfile.TemporaryDirectory() as tmp:
            config = os.path.join(tmp, 'nextest.toml')
            tests = os.path.join(tmp, 'list.json')
            with open(config, 'w', encoding='utf-8') as file:
                file.write(overrides('binary(sisuc) | kind(test)'))
            with open(tests, 'w', encoding='utf-8') as file:
                json.dump({'rust-suites': TESTS}, file)
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                self.assertEqual(nf.main([config, tests]), 0)
        self.assertEqual(out.getvalue(), '')


if __name__ == '__main__':
    unittest.main()
