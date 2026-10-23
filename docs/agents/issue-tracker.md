# Issue tracker: GitHub issues

Issues for this repo are GitHub issues on `sisu-lang/sisu`. The repo is public, so every issue is public.

Specs, plans, review notes and wayfinding maps are private. They stay in the untracked `.scratch/` folder and in `docs/superpowers/`. Do not put them, or links to them, in an issue.

## Issue format

The title gives the symptom that a user sees, or a concrete task. The body has these sections:

- `## What`: what happens today, in one or two sentences.
- `## Why`: the effect on a Sisu user or contributor.
- `## Repro`: for a bug, a slow path or a diagnostic. Give a minimal Sisu program, the command, the actual output and the expected output. Run it on `main` before you file.
- `## Options`: only when a decision is open. Number each option.

Write the text in ASD-STE100 Simplified Technical English (`CODING_STANDARDS.md`, Writing). Do not write reviewer or model names, plan or task references, mutant notes, `/tmp` paths, or commit hashes that exist only on a branch.

Give each issue one kind label, one area label and one triage label. A `bug`, `diagnostic` or `perf` issue also gets one severity label. `triage-labels.md` lists the labels.

## Commands

Use `gh api` with the REST endpoints. Do not use `gh issue`: it calls GraphQL, and that fails here with Forbidden. The calls need the network, so run them outside the sandbox.

- Create: `gh api repos/sisu-lang/sisu/issues -f title='<title>' -F body=@<file> -f 'labels[]=bug' -f 'labels[]=mir' -f 'labels[]=needs-triage' -f 'labels[]=severity:high'`
- Read: `gh api repos/sisu-lang/sisu/issues/<N>` and `gh api repos/sisu-lang/sisu/issues/<N>/comments`
- Comment: `gh api repos/sisu-lang/sisu/issues/<N>/comments -F body=@<file>`
- Change labels: `gh api -X PUT repos/sisu-lang/sisu/issues/<N>/labels -f 'labels[]=<label>' …`
- Close: `gh api -X PATCH repos/sisu-lang/sisu/issues/<N> -f state=closed -f state_reason=completed` (or `not_planned` for `wontfix`)
- List: `gh api 'repos/sisu-lang/sisu/issues?state=open&labels=ready-for-agent&per_page=100'`. The result also contains pull requests; skip each item that has a `pull_request` field.

A pull request that fixes an issue says `Closes #<N>` in its body.

## When a skill says "publish to the issue tracker"

Create a GitHub issue in the format above.

If GitHub cannot be reached (no network, or an auth error), write the issue to `.scratch/<feature-slug>/issues/<NN>-<slug>.md` in the same format, with a `GitHub: pending` line at the top. Tell the user. Publish it at the next chance, then change the line to `GitHub: #<N>`.

## When a skill says "fetch the relevant ticket"

Read the GitHub issue and its comments. The user normally gives the issue number. A path under `.scratch/` is a fallback issue or a private spec: read the file.

## Specs

A feature spec is private. It is `.scratch/<feature-slug>/spec.md`. The GitHub issues that come from it do not link to it.

## Wayfinding operations

Used by `/wayfinder`. Wayfinding is private planning, so it stays in `.scratch/`. The **map** is a file with one **child** file per ticket.

- **Map**: `.scratch/<effort>/map.md` (the Notes / Decisions-so-far / Fog body).
- **Child ticket**: `.scratch/<effort>/issues/NN-<slug>.md`, numbered from `01`, with the question in the body. A `Type:` line records the ticket type (`research`/`prototype`/`grilling`/`task`); a `Status:` line records `claimed`/`resolved`.
- **Blocking**: a `Blocked by: NN, NN` line near the top. A ticket is unblocked when every file it lists is `resolved`.
- **Frontier**: scan `.scratch/<effort>/issues/` for files that are open, unblocked, and unclaimed; first by number wins.
- **Claim**: set `Status: claimed` and save before any work.
- **Resolve**: append the answer under an `## Answer` heading, set `Status: resolved`, then append a context pointer (gist + link) to the map's Decisions-so-far in `map.md`.
