"""Text tests for the nightly build workflow: python3 -m unittest discover -s scripts -p 'test_*.py'.

They read the workflow by its indentation and use no YAML library. A job is the lines under a
two-space key of `jobs:`, a job key is at four spaces, a step starts at six spaces with `- ` and
its keys are at eight. `mapping` turns the lines of one indent into a dict: a key with no value
holds the mapping one level deeper, and a `|` value holds its block, so that the text of a
`run: |` block is never a key. Limit: a flow map with a nested flow map that has a comma.
"""

import os
import re
import subprocess
import tempfile
import unittest
from collections.abc import Callable
from typing import Any

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
WORKFLOW = os.path.join(ROOT, ".github", "workflows", "nightly-build.yml")
PICKED = "${{ needs.pick.outputs.sha }}"
JOBS = [
    "pick",
    "build",
    "checksums",
    "clean-machine",
    "installer-tests",
    "publish",
    "live-check",
    "prune",
    "report",
]
# The `needs:` of the spec's table, and the `if:` of the plan's table.
NEEDS = {
    "pick": set(),
    "build": {"pick"},
    "checksums": {"pick", "build"},
    "clean-machine": {"pick", "checksums"},
    "installer-tests": {"pick", "checksums"},
    "publish": {"pick", "build", "checksums", "clean-machine", "installer-tests"},
    "live-check": {"pick", "publish"},
    "prune": {"publish", "live-check"},
    "report": set(JOBS) - {"report"},
}
IF = {
    "pick": None,
    "build": "needs.pick.outputs.build == 'true'",
    "checksums": None,
    "clean-machine": None,
    "installer-tests": None,
    "publish": "needs.pick.outputs.publish == 'true'",
    "live-check": "needs.publish.outputs.published == 'true'",
    "prune": None,
    "report": "always() && github.event_name == 'schedule' && "
    "(contains(needs.*.result, 'failure') || contains(needs.*.result, 'cancelled'))",
}
PERMISSIONS = {
    "pick": {"contents": "read", "actions": "read"},
    "build": {"contents": "read"},
    "checksums": {"contents": "read"},
    "clean-machine": {"contents": "read"},
    "installer-tests": {"contents": "read"},
    "publish": {"contents": "write", "actions": "read"},
    "live-check": {"contents": "read"},
    "prune": {"contents": "write"},
    "report": {"contents": "read", "issues": "write"},
}
TIMEOUT = {
    "pick": 5,
    "build": 90,
    "checksums": 5,
    "clean-machine": 20,
    "installer-tests": 20,
    "publish": 15,
    "live-check": 15,
    "prune": 5,
    "report": 5,
}
RUNS_ON = {
    "pick": "ubuntu-26.04",
    "build": "${{ matrix.runner }}",
    "checksums": "ubuntu-26.04",
    "clean-machine": "${{ matrix.runner }}",
    "installer-tests": "${{ matrix.runner }}",
    "publish": "ubuntu-26.04",
    "live-check": "ubuntu-24.04",
    "prune": "ubuntu-26.04",
    "report": "ubuntu-26.04",
}
CONTAINER = {
    "build": "${{ matrix.container }}",
    "clean-machine": "${{ matrix.image }}",
    "live-check": "ubuntu:22.04",
}
PICKED_CHECKOUT = {"build", "checksums", "clean-machine", "installer-tests", "live-check"}
TOKEN_JOBS = ("pick", "publish", "prune", "report")
# The one `run:` of each job that holds a write token.
WRITE_RUN = {
    "publish": "python3 scripts/nightly_build.py publish --sha " + PICKED,
    "prune": "python3 scripts/nightly_build.py prune",
    "report": "python3 scripts/nightly_build.py report",
}
OUTPUTS = {
    "pick": {
        "sha": "${{ steps.pick.outputs.sha }}",
        "build": "${{ steps.pick.outputs.build }}",
        "publish": "${{ steps.pick.outputs.publish }}",
    },
    "publish": {"published": "${{ steps.publish.outputs.published }}"},
}
PULL_REQUEST_PATHS = [
    ".github/workflows/nightly-build.yml",
    "install.sh",
    "scripts/nightly_*",
    "scripts/test_install.py",
    "scripts/test_nightly_*.py",
    ".github/actions/install-llvm/action.yml",
    "scripts/install-llvm.sh",
    ".config/about.toml",
    ".config/about.hbs",
    ".config/notices/**",
]
COMMIT_CHECK = (
    '"$HOME/.sisu/bin/sisu" --version --verbose | grep -Fx '
    "\"commit: $(printf %.7s '" + PICKED + "')\""
)
INSTALL = "SISU_DOWNLOAD_DIR=$PWD/dl sh dl/install.sh"
SMOKE = 'scripts/nightly_smoke.sh "$HOME/.sisu/bin" site/programs/run'
DIFF = "diff orders.actual site/programs/run/orders.out"
CHECKS = [
    ("python3 scripts/nightly_check.py version target/release/sisu " + PICKED, None),
    ("python3 scripts/nightly_check.py linux target/release/sisu", "runner.os == 'Linux'"),
    (
        (
            "python3 scripts/nightly_check.py macos target/release/sisu .llvm/23/lib/libLLVMSupport.a "
            '"$(brew --prefix zstd)/lib/libzstd.a"'
        ),
        "runner.os == 'macOS'",
    ),
    (
        (
            "python3 scripts/nightly_check.py pack ${{ matrix.target }} target/release "
            "target/release/THIRD-PARTY-LICENSES.txt out"
        ),
        None,
    ),
]
CARGO_BUILD = "cargo build --release --locked --workspace"
CARGO_ENV = {
    "CARGO_PROFILE_RELEASE_STRIP": "symbols",
    "MACOSX_DEPLOYMENT_TARGET": "${{ runner.os == 'macOS' && '15.0' || '' }}",
}
COUNT_LINE = 'test "$(wc -l < SHA256SUMS)" = 3'
THIRD_PARTY = "target/release/THIRD-PARTY-LICENSES.txt"
NOTICES_LINE = (
    "cat .config/notices/LLVM-LICENSE.txt .config/notices/LLVM-EMBEDDED.txt >> " + THIRD_PARTY
)
STD_LINE = (
    "sed -e 's/<[^>]*>//g' -e 's/&#34;/\"/g' -e \"s/&#39;/'/g\" -e 's/&#60;/</g' "
    "-e 's/&#62;/>/g' \"$(rustc --print sysroot)/share/doc/rust/COPYRIGHT-library.html\" "
    ">> " + THIRD_PARTY
)
EMBEDDED_AUTHORS = (
    "Yann Collet",
    "Henry Spencer",
    "Regents of the University of California",
    "Todd C. Miller",
    "Unicode, Inc.",
)
# A token in any spelling: `github.token`, `github['token']`, `secrets.X`, `GH_TOKEN`.
TOKEN = re.compile(r"github\W*token|secrets\b|\b(?:GH|GITHUB)_TOKEN\b", re.IGNORECASE)
GATE = re.compile(r"continue-on-error|\|\|\s*(?:true\b|:|exit\s+0)|set\s+\+e")


def workflow(test: unittest.TestCase) -> list[str]:
    """The workflow's lines without comments. A missing file fails the test on an assertion."""
    if not os.path.isfile(WORKFLOW):
        test.fail(f"{WORKFLOW} does not exist")
    with open(WORKFLOW, encoding="utf-8") as f:
        return [line for line in f.read().splitlines() if not line.lstrip().startswith("#")]


def jobs(test: unittest.TestCase) -> dict[str, list[str]]:
    """Job name -> its lines, from the line after the name to the line before the next job."""
    found: dict[str, list[str]] = {}
    name: str | None = None
    in_jobs = False
    for line in workflow(test):
        if line == "jobs:":
            in_jobs = True
        elif line.strip() and not line.startswith(" "):
            in_jobs = False
            name = None
        elif in_jobs and (m := re.fullmatch(r"  ([^\s#:]+):(?:\s.*)?", line)):
            name = m[1]
            found[name] = []
        elif in_jobs and name and line.strip():
            found[name].append(line)
    return found


def top(test: unittest.TestCase, key: str) -> list[str]:
    """The lines under a key at column 0."""
    lines = workflow(test)
    if f"{key}:" not in lines:
        test.fail(f"no {key}: in the workflow")
    body: list[str] = []
    for line in lines[lines.index(f"{key}:") + 1 :]:
        if line.strip() and not line.startswith(" "):
            break
        if line.strip():
            body.append(line)
    return body


def flow_list(text: str) -> list[str]:
    return [item.strip().strip("'") for item in text.strip()[1:-1].split(",") if item.strip()]


def flow_map(text: str) -> dict[str, Any]:
    pairs = (pair.split(": ", 1) for pair in re.split(r",\s*", text.strip()[1:-1]))
    return {k: flow_map(v) if v.startswith("{") else v for k, v in pairs}


def mapping(lines: list[str], indent: int) -> dict[str, Any]:
    """The keys at `indent` of the lines. The lines below a key, deeper than `indent`, are its
    value: a mapping when the key has no value, the block text for `|` and `>`, else they join the
    value, as in a flow list on two lines.
    """
    found: dict[str, Any] = {}
    i = 0
    while i < len(lines):
        m = re.fullmatch(" " * indent + r"([^\s:#-][^:]*): ?(.*)", lines[i])
        if not m:
            i += 1
            continue
        j = i + 1
        while j < len(lines) and len(lines[j]) - len(lines[j].lstrip()) > indent:
            j += 1
        key, value, body = m[1].strip("'\""), m[2], lines[i + 1 : j]
        if value in ("|", ">", "|-", ">-"):
            found[key] = "\n".join(line[indent + 2 :] for line in body)
        elif not value:
            found[key] = mapping(body, indent + 2)
        else:
            value = re.sub(r"\s+#.*", "", " ".join([value] + [line.strip() for line in body]))
            found[key] = flow_map(value) if value.startswith("{") else value
        i = j
    return found


def job_map(job: list[str]) -> dict[str, Any]:
    return mapping(job, 4)


def part(step: dict[str, Any], key: str) -> dict[str, Any]:
    """The `with:` or `env:` of a step as a dict."""
    value = step.get(key)
    return value if isinstance(value, dict) else {}


def steps(job: list[str]) -> list[dict[str, Any]]:
    """The steps of the job as dicts of their keys; `- {uses: x}` is a step too."""
    if "    steps:" not in job:
        return []
    found: list[list[str]] = []
    for line in job[job.index("    steps:") + 1 :]:
        if not line.startswith("      "):
            break
        if line.startswith("      - "):
            found.append(["        " + line[8:]])
        elif found:
            found[-1].append(line)
    parsed: list[dict[str, Any]] = []
    for lines in found:
        first = lines[0].strip()
        step = flow_map(first) if first.startswith("{") else {}
        step.update(mapping(lines[1:] if first.startswith("{") else lines, 8))
        parsed.append(step)
    return parsed


def run_lines(step: dict[str, Any]) -> list[str]:
    return str(step.get("run", "")).splitlines()


def uses(job: list[str]) -> list[str]:
    """The `uses:` of the job and of its steps."""
    values = [job_map(job).get("uses")] + [step.get("uses") for step in steps(job)]
    return [value for value in values if value]


def checkouts(job: list[str]) -> list[dict[str, Any]]:
    return [
        step for step in steps(job) if str(step.get("uses", "")).startswith("actions/checkout@")
    ]


def unquote(value: str) -> str:
    return value.strip("'")


class NightlyWorkflowTest(unittest.TestCase):
    def test_jobs_should_have_the_needs_and_if_of_the_table(self) -> None:
        found = jobs(self)
        self.assertEqual(sorted(found), sorted(JOBS))
        for name, job in found.items():
            needs = job_map(job).get("needs")
            self.assertEqual(set(flow_list(needs)) if needs else set(), NEEDS[name], name)
            self.assertEqual(job_map(job).get("if"), IF[name], name)

    def test_publish_should_not_use_always(self) -> None:
        found = jobs(self)
        for name in ("publish", "live-check", "prune"):
            self.assertNotIn("always()", "\n".join(found[name]), name)

    def test_write_jobs_should_run_only_their_script(self) -> None:
        found = jobs(self)
        for name, line in WRITE_RUN.items():
            self.assertEqual(
                [step["run"] for step in steps(found[name]) if "run" in step], [line], name
            )
            self.assertEqual(
                [u.split("@")[0] for u in uses(found[name])], ["actions/checkout"], name
            )

    def test_clean_machine_should_check_the_commit_of_the_install(self) -> None:
        st = steps(jobs(self)["clean-machine"])

        def where(has: Callable[[dict[str, Any]], bool]) -> int:
            hits = [i for i, step in enumerate(st) if has(step)]
            self.assertEqual(len(hits), 1)
            return hits[0]

        install = where(lambda step: step.get("run") == INSTALL)
        check = where(lambda step: COMMIT_CHECK in run_lines(step))
        smoke = where(lambda step: step.get("run") == SMOKE)
        for i in (install, check, smoke):
            self.assertNotIn("if", st[i])
            self.assertNotIn("continue-on-error", st[i])
        self.assertLess(install, check)
        self.assertLess(check, smoke)

    def test_live_check_should_compare_the_output_last(self) -> None:
        st = steps(jobs(self)["live-check"])
        self.assertEqual(st[-1].get("run"), DIFF)
        self.assertNotIn("if", st[-1])
        self.assertNotIn("continue-on-error", st[-1])

    def test_gates_should_not_be_bypassed(self) -> None:
        for line in workflow(self):
            self.assertIsNone(GATE.search(line), line)

    def test_job_outputs_should_be_exact(self) -> None:
        for name, job in jobs(self).items():
            self.assertEqual(job_map(job).get("outputs", {}), OUTPUTS.get(name, {}), name)

    def test_build_should_run_every_check_in_order(self) -> None:
        found = jobs(self)
        st = steps(found["build"])
        positions = []
        for run, condition in CHECKS:
            hits = [i for i, step in enumerate(st) if step.get("run") == run]
            self.assertEqual(len(hits), 1, run)
            self.assertEqual(st[hits[0]].get("if"), condition, run)
            self.assertNotIn("continue-on-error", st[hits[0]], run)
            positions.append(hits[0])
        self.assertEqual(positions, sorted(positions))
        cargo = [step for step in st if step.get("run") == CARGO_BUILD]
        self.assertEqual([part(step, "env") for step in cargo], [CARGO_ENV])
        self.assertTrue(any(COUNT_LINE in run_lines(step) for step in steps(found["checksums"])))

    def test_jobs_should_have_the_exact_permissions_and_token(self) -> None:
        self.assertIn("permissions: {contents: read}", workflow(self))
        found = jobs(self)
        for name, job in found.items():
            self.assertEqual(job_map(job).get("permissions"), PERMISSIONS[name], name)
            has_token = "          GH_TOKEN: ${{ github.token }}" in job
            self.assertEqual(has_token, name in TOKEN_JOBS, name)
        # A token line is allowed only as a line of the four jobs: not in another job, not above
        # `jobs:` and not in a key after the jobs.
        allowed = {line for name in TOKEN_JOBS for line in found[name]}
        self.assertEqual(
            [line for line in workflow(self) if TOKEN.search(line) and line not in allowed], []
        )
        self.assertIn(
            "          HEAD_SHA: ${{ github.event.pull_request.head.sha || github.sha }}",
            found["pick"],
        )

    def test_every_action_should_be_pinned_by_full_sha(self) -> None:
        actions = [u for job in jobs(self).values() for u in uses(job) if not u.startswith("./")]
        self.assertTrue(actions)
        for action in actions:
            self.assertRegex(action, r"^[\w./-]+@[0-9a-f]{40}$")

    def test_every_checkout_should_keep_no_credentials(self) -> None:
        found = [step for job in jobs(self).values() for step in checkouts(job)]
        self.assertEqual(len(found), 9)
        for step in found:
            self.assertEqual(part(step, "with").get("persist-credentials"), "false")

    def test_checkouts_should_use_the_picked_sha_where_the_spec_says(self) -> None:
        for name, job in jobs(self).items():
            self.assertEqual(
                [part(step, "with").get("ref") for step in checkouts(job)],
                [PICKED if name in PICKED_CHECKOUT else None],
                name,
            )

    def test_jobs_with_the_picked_sha_should_need_pick(self) -> None:
        for name, job in jobs(self).items():
            if any("needs.pick" in line for line in job):
                self.assertIn("pick", flow_list(job_map(job).get("needs", "[]")), name)

    def test_every_job_should_have_a_time_limit(self) -> None:
        for name, job in jobs(self).items():
            self.assertEqual(job_map(job).get("timeout-minutes"), str(TIMEOUT[name]), name)

    def test_every_job_should_have_its_runner(self) -> None:
        for name, job in jobs(self).items():
            self.assertEqual(job_map(job).get("runs-on"), RUNS_ON[name], name)
            self.assertEqual(job_map(job).get("container"), CONTAINER.get(name), name)

    def test_rust_cache_should_be_off(self) -> None:
        setups = [
            step
            for job in jobs(self).values()
            for step in steps(job)
            if str(step.get("uses", "")).startswith("actions-rust-lang/setup-rust-toolchain@")
        ]
        self.assertTrue(setups)
        for step in setups:
            self.assertEqual(part(step, "with").get("cache"), "false")

    def test_triggers_should_be_the_three_of_the_spec(self) -> None:
        on = mapping(top(self, "on"), 2)
        self.assertEqual(sorted(on), ["pull_request", "schedule", "workflow_dispatch"])
        self.assertEqual(on["schedule"], "[{cron: '17 2 * * *'}]")
        self.assertEqual(on["workflow_dispatch"], {})
        self.assertEqual(list(on["pull_request"]), ["paths"])
        listed = flow_list(on["pull_request"]["paths"])
        self.assertEqual(listed, PULL_REQUEST_PATHS)
        for path in listed:
            if "*" not in path:
                self.assertTrue(os.path.isfile(os.path.join(ROOT, path)), path)

    def test_concurrency_should_cancel_only_pull_requests(self) -> None:
        self.assertEqual(
            mapping(top(self, "concurrency"), 2),
            {
                "group": "${{ github.workflow }}-${{ github.ref }}",
                "cancel-in-progress": "${{ github.event_name == 'pull_request' }}",
            },
        )

    def test_matrices_should_have_the_rows_of_the_spec(self) -> None:
        found = jobs(self)

        def rows(name: str) -> list[dict[str, Any]]:
            return [
                flow_map(m[1])
                for line in found[name]
                if (m := re.fullmatch(r"\s+- (\{.*\})", line))
            ]

        def unquoted(maps: list[dict[str, Any]]) -> list[dict[str, str]]:
            return [{k: unquote(v) for k, v in row.items()} for row in maps]

        self.assertEqual(
            unquoted(rows("build")),
            [
                {"target": "x86_64-linux", "runner": "ubuntu-24.04", "container": "ubuntu:22.04"},
                {
                    "target": "aarch64-linux",
                    "runner": "ubuntu-24.04-arm",
                    "container": "ubuntu:22.04",
                },
                {"target": "aarch64-macos", "runner": "macos-15"},
            ],
        )
        want = [
            {"runner": runner, "image": image}
            for runner in ("ubuntu-24.04", "ubuntu-24.04-arm")
            for image in ("ubuntu:22.04", "debian:12", "ubuntu:24.04")
        ]
        want += [{"runner": "macos-15"}, {"runner": "macos-26"}]
        self.assertEqual(unquoted(rows("clean-machine")), want)
        strategy = job_map(found["installer-tests"])["strategy"]
        self.assertEqual(flow_list(strategy["matrix"]["runner"]), ["ubuntu-24.04", "macos-15"])
        for name in ("clean-machine", "installer-tests"):
            self.assertEqual(job_map(found[name])["strategy"]["fail-fast"], "false", name)

    def test_license_step_should_append_the_embedded_and_std_notices(self) -> None:
        st = steps(jobs(self)["build"])
        found = [
            run_lines(step) for step in st if "cargo about generate" in str(step.get("run", ""))
        ]
        self.assertEqual(len(found), 1)
        lines = found[0]
        want = [NOTICES_LINE, STD_LINE]
        positions = [lines.index(line) if line in lines else -1 for line in want]
        self.assertNotIn(-1, positions, lines)
        self.assertEqual(positions, sorted(positions))

    def test_std_notices_step_should_write_plain_text(self) -> None:
        st = steps(jobs(self)["build"])
        lines = [line for step in st for line in run_lines(step) if line.startswith("sed -e ")]
        self.assertEqual(len(lines), 1)
        source = '"$(rustc --print sysroot)/share/doc/rust/COPYRIGHT-library.html"'
        sink = ">> " + THIRD_PARTY
        self.assertIn(source, lines[0])
        self.assertTrue(lines[0].endswith(sink), lines[0])
        with tempfile.TemporaryDirectory() as tmp:
            html = os.path.join(tmp, "in.html")
            out = os.path.join(tmp, "out.txt")
            with open(html, "w", encoding="utf-8") as f:
                f.write("<p>say &#34;a&#34; and &#39;b&#39; &#60;c&#62;</p>\n")
            command = lines[0].replace(source, f"'{html}'").replace(sink, f"> '{out}'")
            subprocess.run(["bash", "-c", command], check=True)
            with open(out, encoding="utf-8") as f:
                self.assertEqual(f.read(), "say \"a\" and 'b' <c>\n")

    def test_embedded_notices_should_name_each_author(self) -> None:
        path = os.path.join(ROOT, ".config", "notices", "LLVM-EMBEDDED.txt")
        self.assertTrue(os.path.isfile(path), path)
        with open(path, encoding="utf-8") as f:
            text = f.read()
        for author in EMBEDDED_AUTHORS:
            self.assertIn(author, text)

    def test_prek_should_watch_the_new_files(self) -> None:
        with open(os.path.join(ROOT, "prek.toml"), encoding="utf-8") as f:
            text = f.read()
        block = text[text.index('id = "python-tests"') :]
        match = re.search(r"^files = '(.*)'$", block, re.MULTILINE)
        assert match is not None
        pattern = match[1]
        for path in (
            "install.sh",
            ".github/workflows/nightly-build.yml",
            "scripts/nightly_smoke.sh",
        ):
            self.assertRegex(path, pattern)


if __name__ == "__main__":
    unittest.main()
