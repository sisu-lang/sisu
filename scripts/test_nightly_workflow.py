"""Text tests for the nightly build workflow: python3 -m unittest discover -s scripts -p 'test_*.py'.

They read the workflow by its indentation and use no YAML library. A job is the lines under a
two-space key of `jobs:`, a job key is at four spaces and a step starts at six spaces with `- `.
"""
import os
import re
import unittest

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..')
WORKFLOW = os.path.join(ROOT, '.github', 'workflows', 'nightly-build.yml')
PICKED = 'ref: ${{ needs.pick.outputs.sha }}'
JOBS = ['pick', 'build', 'checksums', 'clean-machine', 'installer-tests', 'publish', 'live-check',
        'prune', 'report']
# The `needs:` of the spec's table, and the `if:` of the plan's table.
NEEDS = {
    'pick': set(),
    'build': {'pick'},
    'checksums': {'pick', 'build'},
    'clean-machine': {'pick', 'checksums'},
    'installer-tests': {'pick', 'checksums'},
    'publish': {'pick', 'build', 'checksums', 'clean-machine', 'installer-tests'},
    'live-check': {'pick', 'publish'},
    'prune': {'publish', 'live-check'},
    'report': set(JOBS) - {'report'},
}
IF = {
    'pick': None,
    'build': "needs.pick.outputs.build == 'true'",
    'checksums': None,
    'clean-machine': None,
    'installer-tests': None,
    'publish': "needs.pick.outputs.publish == 'true'",
    'live-check': "needs.publish.outputs.published == 'true'",
    'prune': None,
    'report': "always() && github.event_name == 'schedule' && "
              "(contains(needs.*.result, 'failure') || contains(needs.*.result, 'cancelled'))",
}
PERMISSIONS = {
    'pick': {'contents': 'read', 'actions': 'read'},
    'build': {'contents': 'read'},
    'checksums': {'contents': 'read'},
    'clean-machine': {'contents': 'read'},
    'installer-tests': {'contents': 'read'},
    'publish': {'contents': 'write', 'actions': 'read'},
    'live-check': {'contents': 'read'},
    'prune': {'contents': 'write'},
    'report': {'contents': 'read', 'issues': 'write'},
}
TIMEOUT = {'pick': 5, 'build': 90, 'checksums': 5, 'clean-machine': 20, 'installer-tests': 20,
           'publish': 15, 'live-check': 15, 'prune': 5, 'report': 5}
PICKED_CHECKOUT = {'build', 'checksums', 'clean-machine', 'installer-tests', 'live-check'}
PULL_REQUEST_PATHS = [
    '.github/workflows/nightly-build.yml', 'install.sh', 'scripts/nightly_*',
    'scripts/test_install.py', 'scripts/test_nightly_*.py', '.github/actions/install-llvm/action.yml',
    'scripts/install-llvm.sh', '.config/about.toml', '.config/about.hbs', '.config/notices/**']
COMMIT_CHECK = ('"$HOME/.sisu/bin/sisu" --version --verbose | grep -Fx '
                '"commit: $(printf %.7s \'${{ needs.pick.outputs.sha }}\')"')


def lines_of(path):
    with open(path, encoding='utf-8') as f:
        return f.read().splitlines()


def workflow(test):
    """The workflow's lines without comments. A missing file fails the test on an assertion."""
    if not os.path.isfile(WORKFLOW):
        test.fail(f'{WORKFLOW} does not exist')
    return [line for line in lines_of(WORKFLOW) if not line.lstrip().startswith('#')]


def jobs(test):
    """Job name -> its lines, from the line after the name to the line before the next job."""
    found = {}
    name = None
    in_jobs = False
    for line in workflow(test):
        if line == 'jobs:':
            in_jobs = True
        elif in_jobs and (m := re.fullmatch(r'  ([a-z-]+):', line)):
            name = m[1]
            found[name] = []
        elif in_jobs and name and line.strip():
            found[name].append(line)
    return found


def job_value(job, key):
    """The text after `key: ` on a line of the job at four spaces, or None."""
    values = [m[1] for line in job if (m := re.fullmatch(rf'    {re.escape(key)}: (.*)', line))]
    assert len(values) <= 1, (key, values)
    return values[0] if values else None


def steps(job):
    """The steps of the job as lists of lines. The `- ` of the first line is a space."""
    found = []
    for line in job:
        if line.startswith('      - '):
            found.append(['        ' + line[8:]])
        elif found and line.startswith('        '):
            found[-1].append(line)
    return found


def flow_list(text):
    return [item.strip().strip("'") for item in text.strip()[1:-1].split(',') if item.strip()]


def flow_map(text):
    return dict(pair.split(': ', 1) for pair in re.split(r',\s*', text.strip()[1:-1]))


def uses(job):
    return re.findall(r'^\s*(?:- )?uses: (\S+)', '\n'.join(job), re.M)


def unquote(value):
    return value.strip("'")


class NightlyWorkflowTest(unittest.TestCase):
    def test_jobs_should_have_the_needs_and_if_of_the_table(self):
        found = jobs(self)
        self.assertEqual(sorted(found), sorted(JOBS))
        for name, job in found.items():
            needs = job_value(job, 'needs')
            self.assertEqual(set(flow_list(needs)) if needs else set(), NEEDS[name], name)
            self.assertEqual(job_value(job, 'if'), IF[name], name)

    def test_publish_should_not_use_always(self):
        found = jobs(self)
        for name in ('publish', 'live-check', 'prune'):
            self.assertNotIn('always()', '\n'.join(found[name]), name)

    def test_publish_should_run_only_the_checkout_action(self):
        job = jobs(self)['publish']
        self.assertEqual([u.split('@')[0] for u in uses(job)], ['actions/checkout'])
        self.assertNotIn('cargo', '\n'.join(job))

    def test_clean_machine_should_check_the_commit_of_the_install(self):
        job = jobs(self)['clean-machine']
        texts = ['\n'.join(step) for step in steps(job)]
        position = {}
        for key, needle in (('install', 'sh dl/install.sh'), ('check', COMMIT_CHECK),
                            ('smoke', 'scripts/nightly_smoke.sh')):
            hits = [i for i, text in enumerate(texts) if needle in text]
            self.assertEqual(len(hits), 1, key)
            position[key] = hits[0]
        self.assertIn(f'          {COMMIT_CHECK}', texts[position['check']].splitlines())
        self.assertLess(position['install'], position['check'])
        self.assertLess(position['check'], position['smoke'])

    def test_jobs_should_have_the_exact_permissions_and_token(self):
        self.assertIn('permissions: {contents: read}', workflow(self))
        found = jobs(self)
        for name, job in found.items():
            self.assertEqual(flow_map(job_value(job, 'permissions') or '{}'), PERMISSIONS[name], name)
            has_token = '          GH_TOKEN: ${{ github.token }}' in job
            self.assertEqual(has_token, name in ('pick', 'publish', 'prune', 'report'), name)
        self.assertIn('          HEAD_SHA: ${{ github.event.pull_request.head.sha || github.sha }}',
                      found['pick'])

    def test_every_action_should_be_pinned_by_full_sha(self):
        actions = [u for job in jobs(self).values() for u in uses(job) if not u.startswith('./')]
        self.assertTrue(actions)
        for action in actions:
            self.assertRegex(action, r'^[\w./-]+@[0-9a-f]{40}$')

    def test_every_checkout_should_keep_no_credentials(self):
        checkouts = [step for job in jobs(self).values() for step in steps(job)
                     if any('uses: actions/checkout@' in line for line in step)]
        self.assertEqual(len(checkouts), 9)
        for step in checkouts:
            self.assertIn('          persist-credentials: false', step)

    def test_checkouts_should_use_the_picked_sha_where_the_spec_says(self):
        for name, job in jobs(self).items():
            refs = [line.strip() for line in job if re.fullmatch(r'\s+ref: .*', line)]
            self.assertEqual(refs, [PICKED] if name in PICKED_CHECKOUT else [], name)

    def test_jobs_with_the_picked_sha_should_need_pick(self):
        for name, job in jobs(self).items():
            if any('needs.pick' in line for line in job):
                self.assertIn('pick', flow_list(job_value(job, 'needs') or '[]'), name)

    def test_every_job_should_have_a_time_limit(self):
        for name, job in jobs(self).items():
            self.assertEqual(job_value(job, 'timeout-minutes'), str(TIMEOUT[name]), name)

    def test_rust_cache_should_be_off(self):
        setups = [step for job in jobs(self).values() for step in steps(job)
                  if any('uses: actions-rust-lang/setup-rust-toolchain@' in line for line in step)]
        self.assertTrue(setups)
        for step in setups:
            self.assertIn('          cache: false', step)

    def test_triggers_should_be_the_three_of_the_spec(self):
        lines = workflow(self)
        top = lines[:lines.index('permissions: {contents: read}')]
        self.assertEqual([line for line in top if re.fullmatch(r'  \w+:.*', line)],
                         ["  schedule: [{cron: '17 2 * * *'}]", '  workflow_dispatch:', '  pull_request:'])
        text = '\n'.join(top)
        paths = re.search(r'    paths: \[(.*?)\]', text, re.S)
        self.assertIsNotNone(paths)
        listed = flow_list('[' + re.sub(r'\s+', ' ', paths[1]) + ']')
        self.assertEqual(listed, PULL_REQUEST_PATHS)
        for path in listed:
            if '*' not in path:
                self.assertTrue(os.path.isfile(os.path.join(ROOT, path)), path)

    def test_concurrency_should_cancel_only_pull_requests(self):
        lines = workflow(self)
        i = lines.index('concurrency:')
        self.assertEqual(lines[i + 1:i + 3], [
            '  group: ${{ github.workflow }}-${{ github.ref }}',
            "  cancel-in-progress: ${{ github.event_name == 'pull_request' }}"])

    def test_matrices_should_have_the_rows_of_the_spec(self):
        found = jobs(self)

        def rows(name):
            return [flow_map(m[1]) for line in found[name] if (m := re.fullmatch(r'\s+- (\{.*\})', line))]

        def unquoted(maps):
            return [{k: unquote(v) for k, v in row.items()} for row in maps]

        self.assertEqual(unquoted(rows('build')), [
            {'target': 'x86_64-linux', 'runner': 'ubuntu-24.04', 'container': 'ubuntu:22.04'},
            {'target': 'aarch64-linux', 'runner': 'ubuntu-24.04-arm', 'container': 'ubuntu:22.04'},
            {'target': 'aarch64-macos', 'runner': 'macos-15'}])
        want = [{'runner': runner, 'image': image}
                for runner in ('ubuntu-24.04', 'ubuntu-24.04-arm')
                for image in ('ubuntu:22.04', 'debian:12', 'ubuntu:24.04')]
        want += [{'runner': 'macos-15'}, {'runner': 'macos-26'}]
        self.assertEqual(unquoted(rows('clean-machine')), want)
        runners = [line for line in found['installer-tests'] if re.fullmatch(r'\s+runner: \[.*\]', line)]
        self.assertEqual([flow_list(line.split(': ', 1)[1]) for line in runners],
                         [['ubuntu-24.04', 'macos-15']])
        for name in ('clean-machine', 'installer-tests'):
            self.assertIn('      fail-fast: false', found[name], name)
        self.assertEqual(job_value(found['build'], 'container'), '${{ matrix.container }}')
        self.assertEqual(job_value(found['clean-machine'], 'container'), '${{ matrix.image }}')

    def test_prek_should_watch_the_new_files(self):
        with open(os.path.join(ROOT, 'prek.toml'), encoding='utf-8') as f:
            text = f.read()
        block = text[text.index('id = "python-tests"'):]
        pattern = re.search(r"^files = '(.*)'$", block, re.M)[1]
        for path in ('install.sh', '.github/workflows/nightly-build.yml', 'scripts/nightly_smoke.sh'):
            self.assertRegex(path, pattern)


if __name__ == '__main__':
    unittest.main()
