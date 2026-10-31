# Ledger

`.scratch/<batch>/progress.md` holds the state of one batch. `<batch>` names
it, for example `issues-2026-11`. Add to it after each event, so that a new
session can go on from its last line.

```markdown
# Batch <batch>

Started <date> on origin/main <sha>.

## Issues

| Issue | State | Group | Note |
| --- | --- | --- | --- |
| #<N> | open / closed: fixed on main / closed: intended / held / in group | <group> | <one line> |

## Groups

| Group | Issues | Order | Review | Branch | Worktree | State |
| --- | --- | --- | --- | --- | --- | --- |
| A | #<N>, #<N> | 1 | full / light | fix/<group> | .claude/worktrees/<group> | waiting / root cause / task <N> / final review / PR #<N> |

## Decisions

- <date> Stop 1: <what the user chose>.
- <date> Stop 2, group <X>: <cause and fix the user approved; the option chosen>.

## Tasks

| Group | Task | Issues | Base | Head | Mutation (last line) | Review run | Verdict |
| --- | --- | --- | --- | --- | --- | --- | --- |

## Events

- <date time> <one line: dispatched, reported, ruled, merged main, and so on>.

## After the merge

- <deferred finding or diagnostic issue>: <runnable repro>.
```
