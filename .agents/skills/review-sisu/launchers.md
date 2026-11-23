# Launch an outside seat

Step 3 of `SKILL.md` chooses the model, the CLI and the effort. This file
gives the command for each CLI. Run each one through Bash with
`run_in_background` and the sandbox off, because each CLI needs the network
and writes under its own home directory. The completion notification ends
the wait; do not poll. Write `RUN`, `<effort>` and each path as a literal.

Each launch can write anywhere, so `verify` is the guard (step 3). The agy
launch is the exception: its sandbox cannot write in the repository.

## agy

For a model that agy (Antigravity CLI) serves, such as `gemini-4-neon-medium`.
The reviewer runs in agy's terminal sandbox, with `RUN` as its workspace.
Only the workspace, the temporary directories and the build caches are
writable. Each
`read_file` rule mounts its directory read-only, so the repository cannot
change. Write `RUN/agy-settings.json` first. Give one `read_file` rule for
the repository root and one for each skill directory in the brief. Give one
`command` rule with the full path of each `sisu` build in `RUN/bin`:

```json
{
  "agentMode": "",
  "toolPermission": "request-review",
  "permissions": {
    "allow": [
      "read_file(<repository root>)",
      "read_file(<each skill directory in the brief>)",
      "command(git -C)", "command(rg)", "command(ls)", "command(cat)",
      "command(head)", "command(sed -n)", "command(python3)",
      "command(RUN/bin/<sha>/sisu)"
    ],
    "deny": ["write_file(<repository root>)"]
  }
}
```

Then run it from `RUN`:

```
cd RUN && agy -p "Read RUN/brief.md and follow it. Your scratch directory is RUN/outside-scratch. Run only the commands that RUN/agy-settings.json allows, each in its own call, as git -C <repository root> ... for git. Use no pipe, no redirection and no \$(...). Your final reply is the whole report." --settings RUN/agy-settings.json --sandbox --model <model> --print-timeout 60m > RUN/<stem>.md 2> RUN/<stem>-cli.log
```

A rule matches the first words of a command, not a glob, so a rule for a
directory does not match a binary in it. A command that no rule allows ends
the run at once, with exit status 0 and an empty `RUN/<stem>.md`. Then
`RUN/<stem>-cli.log` says that a tool needed the "command" permission. A
pipe, a redirection or `$(...)` turns off the prefix match, so the command
fails in the same way. A `deny` rule gives the model an error, and the run
goes on. The rules allow no `cargo` and no `make`, so put each output that
the reviewer needs, such as a test log, into `RUN`.

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
`fable-*` subagent runs Fable 5.1. Run it from the repository root:

```
claude -p "Read RUN/brief.md and follow it. Your scratch directory is RUN/outside-scratch. Write your report to RUN/<stem>.md; your final reply is that report." --model <model ID, such as claude-fable-5-2> --effort <effort> --permission-mode dontAsk --allowedTools "Read Grep Glob Bash Edit(/RUN/**)" --add-dir RUN --add-dir <each skill directory in the brief> --no-session-persistence < /dev/null > RUN/<stem>-cli.log 2>&1
```

`Edit(/RUN/**)` starts with a second `/` before the absolute path of `RUN`,
for example `Edit(//tmp/x/run/**)`: a rule with one `/` is relative to the
project root. An `Edit` rule also lets the reviewer write a new file there;
`claude -p` warns that a `Write` rule matches no file check.
