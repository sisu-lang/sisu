#!/usr/bin/env bash
# Opens a GitHub issue for a failed scheduled fuzz run, or comments on the issue that is open.
# The issue has a fixed title for each target and the label `fuzz`, so a failure on a later night
# goes to the same issue. The comment or the issue holds the run's summary
# (fuzz/artifacts/summary.md) and the run's URL.
# The script reads the summary from a file and sends it as JSON through jq. It never puts the
# summary into a command line or a script, because a crash input can hold any bytes.
# The script gives no severity label, because it cannot judge one. The maintainer who triages the
# issue adds it (docs/agents/triage-labels.md).
# The environment has GH_TOKEN (with `issues: write`), GITHUB_REPOSITORY and RUN_URL.
#   scripts/fuzz_issue.sh <target> <summary file>
set -euo pipefail

target=${1:?usage: scripts/fuzz_issue.sh <target> <summary file>}
summary=${2:?usage: scripts/fuzz_issue.sh <target> <summary file>}
repo=${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is not set}
title="Nightly fuzz failure: ${target}"
body=$(mktemp)
trap 'rm -f "$body"' EXIT

{
    echo "The scheduled fuzz run of the \`${target}\` target failed."
    echo
    echo "Run: ${RUN_URL:?RUN_URL is not set}"
    echo
    echo "A maintainer adds the severity label when the issue is triaged. The maintainer also writes the"
    echo "text again as What, Why, Repro and Options."
    echo
    if [[ -s $summary ]]; then
        echo "The summary of the run:"
        echo
        # The API accepts at most 65536 characters; the summary is cut well below that.
        head -c 50000 "$summary"
    else
        echo "The run wrote no summary. Read the log of the run."
    fi
} >"$body"

# An issue of an earlier night: open, with the label and the title of this target. The issues
# list holds pull requests too, and they have a `pull_request` key.
number=$(gh api --paginate "repos/${repo}/issues?state=open&labels=fuzz&per_page=100" |
    jq -r --arg title "$title" '.[] | select(.pull_request == null and .title == $title) | .number' |
    sed -n 1p)

if [[ -n $number ]]; then
    jq -n --rawfile body "$body" '{body: $body}' |
        gh api "repos/${repo}/issues/${number}/comments" --input - >/dev/null
    echo "scripts/fuzz_issue.sh: commented on issue #${number}"
else
    jq -n --arg title "$title" --rawfile body "$body" \
        '{title: $title, body: $body, labels: ["needs-triage", "bug", "fuzz"]}' |
        gh api "repos/${repo}/issues" --input - --jq '"scripts/fuzz_issue.sh: opened issue #\(.number)"'
fi
