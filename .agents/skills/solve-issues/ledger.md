# Ledger

`.scratch/<batch>/progress.md` holds the state of one batch. `<batch>` names
it, for example `issues-2026-11`. Add a line after each event. A new session
goes on from the last line of "Events".

```markdown
# Batch <batch>

Started <date> on origin/main <sha>. Build: <path of the origin/main sisu>.

## Issues

| Issue | State | Group | Note |
| --- | --- | --- | --- |
| #<N> | open / closed: fixed on main / closed: wontfix / held / in branch <branch> / in group | <group> | <one line> |

## Groups

| Group | Issues | Order | Flow | Branch | Worktree | State |
| --- | --- | --- | --- | --- | --- | --- |
| A | #<N>, #<N> | 1 | full / light | fix/<batch>-<group> | .claude/worktrees/<batch>-<group> | waiting / root cause / task <N> / final review / PR #<N> |

## Decisions

- <date> Stop 1: <what the user chose>.
- <date> Stop 2, group <X>: <cause and fix the user approved; the option chosen>.

## Tasks

| Group | Task | Issues | Base | Head | Mutation (commit, last line) | Review run | Rulings | Verdict |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| A | 1 | #<N> | <sha> | <sha> | <sha>: <line> / light | <RUN path> | rulings-1.md: <n> accepted | approve / round <n> |

## After the merge

- <deferred finding or diagnostic issue>: <runnable repro>.

## Events

- <date time> <one line: dispatched, reported, ruled, merged main, and so on>.
```
