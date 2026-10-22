#!/usr/bin/env bash
# Fuzzes the compiler with cargo-fuzz. front_end (fuzz/fuzz_targets/front_end.rs) runs the front
# end on every input, and codegen on each input that checks clean. program
# (fuzz/fuzz_targets/program.rs) turns each input into a well-typed program, builds and runs it
# three ways with its rewritten copy, and judges the runs (ADR 0031).
# Every libFuzzer flag lives here, so CI never repeats one.
#   scripts/fuzz.sh [--target <target>] [seconds]   fuzz for <seconds>, 1800 by default
#   scripts/fuzz.sh [--target <target>] --replay <input>   run one input
#   scripts/fuzz.sh [--target <target>] --tmin <input>   minimize one input into fuzz/artifacts/<target>/
#   scripts/fuzz.sh [--target <target>] --cmin   minimize fuzz/corpus/<target> in place
#   scripts/fuzz.sh [--target <target>] --coverage   line coverage per compiler stage of the corpus
#                                     and seeds, and how many inputs reach codegen
# <target> is front_end, the default, or program. program builds the runtime archive first and
# points SISU_RUNTIME at it; a replay of a program input also writes its program, rewritten
# copy, stdin and failure to fuzz/artifacts/program/<input name>.dump/ (the input itself is the
# file <input name>).
# A fuzz run writes fuzz/artifacts/fuzz.log, run.txt and summary.md, and failing inputs
# land in fuzz/artifacts/<target>/. Each new crash- input is then replayed once, and its
# output appended to fuzz.log. The run exits 1 when it produced a crash-, timeout- or
# oom- artifact, and otherwise with cargo-fuzz's status.
set -euo pipefail

usage="usage: scripts/fuzz.sh [--target front_end|program] [seconds] | --replay <input> | --tmin <input> | --cmin | --coverage"
target=front_end
if [[ ${1:-} == --target ]]; then
    if (($# < 2)); then
        echo "$usage" >&2
        exit 2
    fi
    target=$2
    shift 2
fi
case $target in
front_end)
    limits=(-max_len=16384 -timeout=60 -rss_limit_mb=2048)
    replay_seconds=120
    dict=fuzz/front_end.dict
    ;;
program)
    limits=(-max_len=4096 -timeout=300)
    replay_seconds=330
    dict=
    ;;
*)
    echo "$usage" >&2
    exit 2
    ;;
esac

# Resolve the input before the cd, so a path relative to the caller's directory works.
case ${1:-} in
--replay | --tmin) input=$(realpath "${2:?$1 needs an input}") ;;
esac
# The script's own checkout, not the caller's: run from another worktree, it still fuzzes this one.
cd "$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
export RUST_BACKTRACE=1
if [[ $target == program ]]; then
    cargo build --release -p sisu-runtime-mimalloc
    export SISU_RUNTIME=$PWD/target/release/libsisu_runtime_mimalloc.a
fi
# How many crashes are replayed, each for at most replay_seconds (set per target above, past
# libFuzzer's own -timeout, for the program's start-up): a regression can leave one crash- per
# corpus input, and the job has a limit.
replay_limit=3
corpus=fuzz/corpus/$target
found=fuzz/artifacts/$target
# Where a replay of the input at $1 leaves the program fuzzer's record of it; front_end ignores it.
dump_dir() { echo "$PWD/$found/${1##*/}.dump"; }

# Line coverage of the corpus and seeds, summed per stage, then how many of those inputs reach
# codegen. Run `cargo fuzz coverage` over the corpus first; its profile feeds llvm-cov.
coverage() {
    local host bin
    host=$(rustc -vV | awk '/^host:/ { print $2 }')
    bin=$(rustc --print sysroot)/lib/rustlib/$host/bin/llvm-cov
    mkdir -p "$corpus"
    cargo fuzz coverage --sanitizer none "$target" "$corpus" "$seeds"
    # `export`, not `report`: report shows names without their longest common directory, which
    # depends on which other files the binary has; export has absolute paths.
    local lines
    lines=$("$bin" export "target/$host/coverage/$host/release/$target" \
        -instr-profile="fuzz/coverage/$target/coverage.profdata" \
        -ignore-filename-regex='(\.cargo|\.rustup|/rustc/|/fuzz/|/tests/|tests\.rs)' \
        -summary-only |
        jq -r '
            [.data[0].files[]
             | select(.filename | test("/crates/sisuc/src/"))
             | {stage: (.filename | sub("^.*/crates/sisuc/src/"; "") | sub("(\\.rs|/.*)$"; "")),
                count: .summary.lines.count, covered: .summary.lines.covered}]
            | group_by(.stage)
            | map({key: .[0].stage,
                   value: {count: (map(.count) | add), covered: (map(.covered) | add)}})
            | from_entries
            | . as $by
            | ["lexer", "parser", "check", "mir", "codegen"][]
            | "\(.) \($by[.].count // 0) \($by[.].covered // 0)"')
    local stage count covered empty=false
    while read -r stage count covered; do
        if ((count == 0)); then
            echo "scripts/fuzz.sh: no coverage lines for stage ${stage}" >&2
            empty=true
            continue
        fi
        awk -v s="$stage" -v c="$count" -v v="$covered" \
            'BEGIN { printf "%-8s %5.1f%% of %d lines\n", s, 100 * v / c, c }'
    done <<<"$lines"
    if [[ $empty == true ]]; then exit 1; fi
    local reached=0 inputs=0 f
    if [[ $target == program ]]; then
        # An input that fails property 1 is a crash, never a corpus entry, and the coverage run
        # above passed over every one of them: each reached codegen.
        for f in "$corpus"/*; do
            [[ -f $f ]] && inputs=$((inputs + 1))
        done
        echo "inputs reaching codegen: all ${inputs} (a failing input is a crash, never a corpus entry)"
    else
        cargo build --quiet -p sisuc
        for f in "$corpus"/* "$seeds"/*; do
            [[ -f $f ]] || continue
            inputs=$((inputs + 1))
            if target/debug/sisu check "$f" >/dev/null 2>&1; then reached=$((reached + 1)); fi
        done
        echo "inputs reaching codegen: ${reached} of ${inputs}"
    fi
}

# These exec cargo, so the trap below would never run: they come before the seeds exist.
case ${1:-} in
--replay) SISU_FUZZ_DUMP=$(dump_dir "$input") exec cargo fuzz run --sanitizer none "$target" "$input" -- "${limits[@]}" ;;
--tmin) exec cargo fuzz tmin --sanitizer none "$target" "$input" -- "${limits[@]}" ;;
--cmin) exec cargo fuzz cmin --sanitizer none "$target" "$corpus" -- "${limits[@]}" ;;
esac

# Every .sisu file under the test tree, copied under a unique name into a temp directory.
seeds=$(mktemp -d)
marker=$(mktemp)
trap 'rm -rf "$seeds" "$marker"' EXIT
i=0
while IFS= read -r -d '' f; do
    i=$((i + 1))
    cp "$f" "$seeds/$i-${f##*/}"
done < <(if [[ $target == front_end ]]; then find crates/sisuc/tests -name '*.sisu' -print0; fi)

if [[ ${1:-} == --coverage ]]; then
    coverage
    exit 0
fi

seconds=${1:-1800}
if [[ ! $seconds =~ ^[0-9]+$ ]]; then
    echo "$usage" >&2
    exit 2
fi

# libFuzzer exits 1 when an explicit corpus directory is missing, as on a first run.
mkdir -p "$corpus" "$found"
log=fuzz/artifacts/fuzz.log
: >"$log"

# Artifacts are named by the input's hash, so an old file can be rewritten: only those
# newer than this marker belong to this run.
touch "$marker"

# Runs a command with its output teed into the log. Under pipefail `|| status=$?` would
# take tee's status, so status comes from PIPESTATUS; a failed tee only marks the log.
status=0
log_complete=true
logged() {
    local pipe=(0 0)
    "$@" 2>&1 | tee -a "$log" || pipe=("${PIPESTATUS[@]}")
    status=${pipe[0]}
    if ((pipe[1] != 0)); then log_complete=false; fi
}

# cargo-fuzz adds -fork=N only when N != 1, so a one-CPU machine still gets 2 jobs.
jobs=$(nproc)
if ((jobs < 2)); then jobs=2; fi
cmd=(cargo fuzz run --sanitizer none --jobs "$jobs" "$target" "$corpus" "$seeds"
    -- ${dict:+-dict="$PWD/$dict"} -max_total_time="$seconds" "${limits[@]}")
logged cargo fuzz build --sanitizer none "$target"
start=$SECONDS
if ((status == 0)); then logged "${cmd[@]}"; fi
elapsed=$((SECONDS - start))

{
    echo "commit: $(git rev-parse HEAD)"
    if [[ -n $(git status --porcelain) ]]; then echo "tree: dirty"; else echo "tree: clean"; fi
    rustc -V
    cargo fuzz --version
    echo "command: ${cmd[*]}"
} >fuzz/artifacts/run.txt

failures=()
while IFS= read -r -d '' f; do
    case ${f##*/} in
    crash-*) failures+=("crash $f") ;;
    timeout-*) failures+=("timeout $f") ;;
    oom-*) failures+=("oom $f") ;;
    esac
done < <(find "$found" -type f -newer "$marker" -print0 | sort -z)

# Fork mode's first pass over the corpus sends the children's output to /dev/null, so a
# crash on a restored input leaves no panic message in the log. A plain replay prints it;
# its own status changes neither the run's status nor which inputs count.
crashes=0
for failure in "${failures[@]}"; do
    if [[ ${failure%% *} != crash ]]; then continue; fi
    crashes=$((crashes + 1))
    if ((crashes > replay_limit)); then continue; fi
    path=${failure#* }
    {
        echo
        echo "==> scripts/fuzz.sh: replay of ${path}"
        replay=0
        SISU_FUZZ_DUMP=$(dump_dir "$path") timeout -k 10 "$replay_seconds" \
            cargo fuzz run --sanitizer none "$target" "$path" -- "${limits[@]}" || replay=$?
        echo "==> scripts/fuzz.sh: the replay exited ${replay}"
    } >>"$log" 2>&1 || log_complete=false
done
not_replayed=0
if ((crashes > replay_limit)); then
    not_replayed=$((crashes - replay_limit))
    echo "==> scripts/fuzz.sh: ${not_replayed} more crashes not replayed; scripts/fuzz.sh --target ${target} --replay <path> replays any of them" >>"$log" || log_complete=false
fi

# Fork mode interleaves the children's output, whose lines start `#<n>\t`; the parent's
# status lines start `#<n>: cov:`, and the last one holds the total.
executions=$(awk '/^#[0-9]+: cov:/ { n = substr($1, 2, length($1) - 2) } END { print (n == "" ? "unknown" : n) }' "$log")

{
    echo "# Fuzzing: ${target}"
    echo
    echo "- Fuzzed for ${elapsed} s (asked for ${seconds} s)."
    echo "- Executions in processed jobs: ${executions}."
    echo "- cargo-fuzz exit status: ${status}."
    if [[ $log_complete == false ]]; then
        echo "- \`fuzz.log\` is incomplete: writing it failed."
    fi
    echo
    echo "## Failures"
    echo
    if ((${#failures[@]} == 0 && status != 0)); then
        echo "The fuzzer exited ${status} with no failing input; see \`fuzz.log\`."
    elif ((${#failures[@]} == 0)); then
        echo "None."
    else
        echo "Paths are the run's own. Replay a downloaded input by its own path: \`--replay\` resolves it from your directory."
        echo
    fi
    for failure in "${failures[@]}"; do
        class=${failure%% *}
        path=${failure#* }
        echo "- **${class}**: \`${path}\`. Replay: \`scripts/fuzz.sh --target ${target} --replay ${path}\`."
        if [[ $class == crash ]]; then
            echo "  Its replay is at the end of \`fuzz.log\`. A crash whose replay shows no panic message is a stack overflow or another fatal signal."
        else
            echo "  No Rust backtrace: without a sanitizer, libFuzzer prints no native stack for this class."
        fi
    done
    if ((not_replayed > 0)); then
        echo
        echo "${not_replayed} more crashes were not replayed. \`scripts/fuzz.sh --target ${target} --replay <path>\` replays any of them."
    fi
} >fuzz/artifacts/summary.md

cat fuzz/artifacts/summary.md
if ((${#failures[@]} > 0)); then
    exit 1
fi
exit "$status"
