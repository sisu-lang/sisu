"""Unit tests for diag_diff.py: python3 -m unittest discover -s scripts -p 'test_*.py'."""
import contextlib
import io
import os
import tempfile
import unittest

import diag_diff as dd

E0032 = ('{"kind":"diagnostic","code":"E0032","severity":"error",'
         '"primary":{"line":2,"column":5},"secondary":[{"line":2,"column":9}]}')
W0001 = ('{"kind":"diagnostic","code":"W0001","severity":"warning",'
         '"primary":{"line":1,"column":1},"secondary":[]}')


def fake_sisuc(tmp, name, outputs):
    """A sisuc stand-in that prints the JSON lines `outputs` gives a file's stem, or crashes."""
    path = os.path.join(tmp, name)
    cases = ''.join(f"  {stem}) printf '%s\\n' '{out}' >&2;;\n" for stem, out in outputs.items())
    with open(path, 'w', encoding='utf-8') as f:
        f.write(f'#!/bin/sh\ncase $(basename "$4" .sisu) in\n{cases}  *) exit 101;;\nesac\n')
    os.chmod(path, 0o755)
    return path


class DiagDiff(unittest.TestCase):
    def test_reports_each_program_whose_diagnostics_differ(self):
        with tempfile.TemporaryDirectory() as tmp:
            for stem in ['same', 'accepted', 'rejected', 'moved', 'crash']:
                open(os.path.join(tmp, f'{stem}.sisu'), 'w').close()
            moved = E0032.replace('"column":9', '"column":10')
            base = fake_sisuc(tmp, 'base', {'same': E0032, 'accepted': E0032, 'rejected': W0001,
                                            'moved': E0032, 'crash': E0032})
            head = fake_sisuc(tmp, 'head', {'same': E0032, 'accepted': W0001, 'rejected': E0032,
                                            'moved': moved})
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                status = dd.main(base, head, tmp)
        self.assertEqual(status, 1)
        lines = out.getvalue().splitlines()
        self.assertEqual([line.split(':')[0] for line in lines if not line.startswith(' ')][:-1],
                         ['newly accepted', 'changed', 'changed', 'newly rejected'])
        self.assertIn('  head: crash 101:0', lines)
        self.assertIn('  head: E0032 2:5 +2:10', lines)
        self.assertEqual(lines[-1], 'diag_diff: 5 programs, 2 changed, 1 newly accepted, '
                                    '1 newly rejected')


if __name__ == '__main__':
    unittest.main()
