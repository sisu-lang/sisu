# Launch reviewers

Step 3 of `SKILL.md` chooses the primary seat, and the outside seat's model, CLI
and effort. This file gives the launch steps for each harness and outside CLI.
Run each outside CLI through Bash with `run_in_background` and the sandbox off.
Each CLI needs the network and writes under its own home directory. The
completion notification ends the wait; do not poll. Write `RUN`, `<effort>` and
each path as a literal.

Each launch can write anywhere, so `verify` is the guard (step 3). The outside
agy launch (`## agy`) cannot write in the repository: its sandbox mounts the
repository read-only for terminal commands, and its `deny` rule refuses a
file-tool write. The agy primary seat inherits the writable workspace, so
`verify` guards it too.

## Primary seat

The primary seat's report stem is `opus` in Claude Code, and the active model's
short name (such as `neon`) in `agy`.

- **Claude Code:** call the `Agent` tool with the table's `subagent_type`, and pass
  no `model` or `effort`. Prompt: `Read RUN/brief.md and follow it. Your scratch
  directory is RUN/opus-scratch. Your final reply is the whole report.` Use
  `brief-<round>.md` in a later round. A read-only reviewer cannot write a file.
  Save its reply word for word with `python3 <skill base>/scripts/review_round.py
  save-report RUN opus --agent <agent ID> --round <round>`.
- **Antigravity CLI (`agy`):** call `invoke_subagent` with `TypeName: "self"`,
  `Model: "inherit"`, and `Workspace: "inherit"`. Prompt: `Read RUN/brief.md and
  follow it. Your scratch directory is RUN/opus-scratch. Do not edit or write any
  file in <repository root>. Start your report with Model: <model> on line 1. Write
  your report to RUN/<stem>.md; your final reply is that exact report.` Use
  `brief-<round>.md` and `<stem>-<round>.md` in a later round. Stop background work
  with `manage_task` in place of `TaskStop`.

## agy

For a model that agy (Antigravity CLI) serves, such as `gemini-4-neon-medium`.
The model name holds the effort, so the launch has no `--effort`. The
reviewer runs in agy's terminal sandbox, with `RUN` as its workspace.

`review_round.py prepare` writes `RUN/agy-settings.json` on every round. Each
`read_file` rule mounts its directory read-only, and `write_file(RUN)` makes
`RUN` writable. The generated file allows the main checkout (so worktree git works) and every
skill directory in the brief. It also allows every `sisu` build in `RUN/bin`,
the safe read commands, and `mcp(GitHits/*)`. It denies writes to the main
checkout.

Run it from `RUN` with `set -euo pipefail` so a failed step stops the launch:

```
set -euo pipefail
cd RUN && PAGER=cat agy -p "Read RUN/brief.md and follow it. Your scratch directory is RUN/outside-scratch. Run only the commands that RUN/agy-settings.json allows, each in its own call, as git -C <repository root> ... for git. Use no pipe, no redirection and no \$(...). A command that the settings do not allow ends your run with no report. Name each check that you could not run under Unverified. Use rg and rg --files, not grep or find. To export a commit, run git -C <repository root> archive -o RUN/outside-scratch/<sha>.tar <sha>, then python3 -m tarfile -e RUN/outside-scratch/<sha>.tar <dir>. Do not run cargo or make, also not through python3. Your final reply is the whole report." --settings RUN/agy-settings.json --sandbox --model <model> --log-file RUN/<stem>-agy.log < /dev/null > RUN/<stem>.md 2> RUN/<stem>-cli.log
```

`PAGER=cat` stops `git` from waiting for a pager in the sandbox. `agy -p`
buffers stdout until exit, so `RUN/<stem>.md` stays empty while it runs.
`--log-file RUN/<stem>-agy.log` logs the conversation ID and live tool checks
during the run. Keep it separate from `2> RUN/<stem>-cli.log` so stderr and the
log file do not overwrite each other. `agy -p` can print status lines before
`Model:`. Strip them before `review_round.py verify` with
`sed -i '/^Model:/,$!d' RUN/<stem>.md`.

A rule matches the first words of a command, not a glob, so a rule for a
directory does not match a binary in it. An `mcp` rule names
`<server>/<tool>`, and `*` matches each tool. A command or a tool that no
rule allows ends the run at once, with exit status 0 and an empty
`RUN/<stem>.md`. Then `RUN/<stem>-cli.log` names the permission that the
tool needed. A pipe, a redirection or `$(...)` turns off the prefix match,
so the command fails in the same way. A `deny` rule gives the model an
error, and the run goes on.

The prompt tells the reviewer which commands to use in place of `grep`,
`find`, `tar`, `cargo` and `make`. A script that runs git with no `-C`, such
as `scripts/golden_diff.py`, fails in `RUN`. Run these scripts yourself, and
put their output into `RUN`.

## kiro-cli

For a model that `kiro-cli` serves, such as `claude-opus-5.5`. Run it from
`RUN/outside-scratch` (or `RUN/opus-scratch`). Send stdout to a `.out` file so
a later round does not read it as a second Markdown report:

```
set -euo pipefail
cd RUN/outside-scratch && kiro-cli chat --no-interactive --trust-all-tools --model <model> --effort <effort> "Read RUN/brief.md and follow it. Your scratch directory is RUN/outside-scratch. Do not edit or write any file in <repository root>. Write your report to RUN/<stem>.md; your final reply is that report." < /dev/null > RUN/<stem>-cli.out 2> RUN/<stem>-cli.log
```

## copilot

For a model that copilot serves, such as `gpt-6.1-sol`. Start the call with
`copilot`, because the permission rule matches that word:

```
copilot -p "Read RUN/brief.md and follow it. Your scratch directory is RUN/outside-scratch. Write your report to RUN/<stem>.md; your final reply is that report." --model <model> --reasoning-effort <effort> --allow-all -s -C RUN/outside-scratch --add-dir <repository root> --add-dir RUN
```

Add one more `--add-dir` for the directory of each skill in the brief.

## codex

For a model that codex serves, such as `gpt-6.1-sol` or `gpt-6-astra`, when
`sol-review` is on `PATH`. The script runs `codex exec` in a new scratch
directory, and tells the reviewer not to edit the repository. It tries again
while the model is at capacity, and exits 1 when no report comes back:

```
CODEX_MODEL=<model> sol-review <effort> RUN/brief.md RUN/<stem>.md <repository root>
```

## claude -p

For a Claude model that the Agent tool does not give, such as Fable 5.2: a
`fable-*` subagent runs Fable 5.1. Use it too when no Agent tool is available.
Run it from the repository root:

```
claude -p "Read RUN/brief.md and follow it. Your scratch directory is RUN/outside-scratch. Write your report to RUN/<stem>.md; your final reply is that report." --model <model ID, such as claude-fable-5-2> --effort <effort> --permission-mode dontAsk --allowedTools "Read Grep Glob Bash Edit(/RUN/**)" --add-dir RUN --add-dir <each skill directory in the brief> --no-session-persistence < /dev/null > RUN/<stem>-cli.log 2>&1
```

`Edit(/RUN/**)` starts with a second `/` before the absolute path of `RUN`,
for example `Edit(//tmp/x/run/**)`: a rule with one `/` is relative to the
project root. An `Edit` rule also lets the reviewer write a new file there;
`claude -p` warns that a `Write` rule matches no file check.

When no Agent tool is available, the same command runs an Opus or Sonnet seat
with `--model opus` or `--model sonnet`.
