#!/usr/bin/env bash
# Fuzzes the compiler with cargo-fuzz. front_end (fuzz/fuzz_targets/front_end.rs) runs the front
# end on every input, and codegen on each input that checks clean. program
# (fuzz/fuzz_targets/program.rs) turns each input into a well-typed program, builds and runs it
# three ways with its rewritten copy, and judges the runs (ADR 0033).
# Every libFuzzer flag lives here, so CI never repeats one.
#   scripts/fuzz.sh [--target <target>] [seconds]   fuzz for <seconds>, 1800 by default
#   scripts/fuzz.sh [--target <target>] --replay <input>   run one input.
#   scripts/fuzz.sh [--target <target>] --against <commit> <input>   run one input here and on an
#                                     export of <commit>, such as origin/main. It says where the
#                                     input fails. It exits 1 only when it fails here alone.
#   scripts/fuzz.sh [--target <target>] --tmin <input>   minimize one input into fuzz/artifacts/<target>/
#   scripts/fuzz.sh [--target <target>] --cmin   minimize fuzz/corpus/<target> in place
#   scripts/fuzz.sh [--target <target>] --coverage   line coverage per compiler stage of the corpus
#                                     and seeds, and how many inputs reach codegen
#   scripts/fuzz.sh --target program --gaps   the functions that a valid program reaches and no
#                                     fixed seed reaches (scripts/fuzz_gaps.py); exits 1 on a new
#                                     gap and 2 when the comparison cannot trust a measurement,
#                                     each with its section appended to fuzz/artifacts/summary.md.
#                                     A directory with no program exits 2, and a tool that fails
#                                     gives its own status. Neither writes a section.
# The seeds of front_end are the .sisu files of the test tree. The seeds of program are 128
# full-size inputs from scripts/fuzz_seeds.py: a short input runs out of bytes, and the fuzzer
# grows an input too slowly to reach full size.
# <target> is front_end, the default, or program. program builds the runtime archive first and
# points SISU_RUNTIME at it; a replay of a program input also writes its program, rewritten
# copy, stdin and failure to fuzz/artifacts/program/<input name>.dump/ (the input itself is the
# file <input name>).
# A fuzz run writes fuzz/artifacts/fuzz.log, run.txt and summary.md, and failing inputs
# land in fuzz/artifacts/<target>/. Each new crash- input is then replayed once, and its
# output appended to fuzz.log. The run exits 1 when it produced a crash-, timeout- or
# oom- artifact, and otherwise with cargo-fuzz's status. summary.md gives the panic location
# and the first line of the panic message of each replayed crash. The program target adds the
# section "Known cascades": the pairs of fuzz/known_cascades.txt that no input of the run gave.
# It is a warning, and the exit status stays the same.
set -euo pipefail

usage="usage: scripts/fuzz.sh [--target front_end|program] [seconds] | --replay <input> | --against <commit> <input> | --tmin <input> | --cmin | --coverage | --target program --gaps"
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
    # -len_control=0 tries full-length inputs at once: a short input runs out of bytes, and the
    # generator then takes the smallest choice, so libFuzzer's slow length growth kept programs small.
    limits=(-max_len=4096 -timeout=300 -len_control=0)
    replay_seconds=330
    dict=
    ;;
*)
    echo "$usage" >&2
    exit 2
    ;;
esac

# The gap check compares the generator with the valid programs, so only the program target has it.
if [[ ${1:-} == --gaps && $target != program ]]; then
    echo "$usage" >&2
    exit 2
fi

# Resolve the input before the cd, so a path relative to the caller's directory works.
case ${1:-} in
--replay | --tmin) input=$(realpath "${2:?$1 needs an input}") ;;
--against)
    ref=${2:?--against needs a commit}
    input=$(realpath "${3:?--against needs an input}")
    ;;
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

# The coverage of the profile that `cargo fuzz coverage` left for the target $1, as llvm-cov's
# JSON on stdout. Other arguments go to llvm-cov. `export`, not `report`: report shows names
# without their longest common directory, which depends on which other files the binary has;
# export has absolute paths.
export_coverage() {
    local host
    host=$(rustc -vV | awk '/^host:/ { print $2 }')
    "$(rustc --print sysroot)/lib/rustlib/$host/bin/llvm-cov" export "target/$host/coverage/$host/release/$1" \
        -instr-profile="fuzz/coverage/$1/coverage.profdata" \
        -ignore-filename-regex='(\.cargo|\.rustup|/rustc/|/fuzz/|/tests/|tests\.rs)' "${@:2}"
}

# Line coverage of the corpus and seeds, summed per stage, then how many of those inputs reach
# codegen. Run `cargo fuzz coverage` over the corpus first; its profile feeds llvm-cov.
coverage() {
    mkdir -p "$corpus"
    # cargo-fuzz keeps earlier raw profiles and merges them (cargo-fuzz 0.13.2 src/project.rs, coverage_for).
    rm -rf "fuzz/coverage/$target"
    cargo fuzz coverage --sanitizer none "$target" "$corpus" "$seeds"
    local lines
    lines=$(export_coverage "$target" -summary-only |
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
        # An input that fails the clean check is a crash, never a corpus entry, and the coverage run
        # above passed over every one of them: each reached codegen.
        for f in "$corpus"/* "$seeds"/*; do
            [[ -f $f ]] && inputs=$((inputs + 1))
        done
        echo "inputs reaching codegen: all ${inputs} (corpus and seeds; a failing input is a crash, never a corpus entry)"
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

# Copies each .sisu file under the directories $2 and after into the directory $1 under a unique
# name. cargo-fuzz runs every regular file of a directory, and not the files below it.
copy_programs() {
    local to=$1 i=0 f
    shift
    # A pipe, not `< <(find ...)`: pipefail sees the status of a pipe, and not of a substitution.
    find "$@" -name '*.sisu' -print0 | while IFS= read -r -d '' f; do
        i=$((i + 1))
        cp "$f" "$to/$i-${f##*/}"
    done
}

# The generator gaps (scripts/fuzz_gaps.py): the functions that a valid program reaches and no
# fixed seed reaches. The seeds alone are the generator's side, with no corpus, so the result
# depends on the commit and not on what a night kept. The valid side is every program of tests/run
# and tests/run-fail, and no other directory: tests/diagnostics holds invalid programs.
gaps() {
    local dir found
    # A directory that holds no program, or is a symlink, reads as a reference without it. The
    # stage floors of fuzz_gaps.py cannot see that, because the other directory still reaches them.
    for dir in crates/sisuc/tests/run crates/sisuc/tests/run-fail; do
        found=$(find "$dir" -name '*.sisu' -print -quit)
        if [[ -z $found ]]; then
            echo "scripts/fuzz.sh: ${dir} holds no .sisu program" >&2
            exit 2
        fi
    done
    gaps_dir=$(mktemp -d)
    mkdir "$gaps_dir/programs"
    copy_programs "$gaps_dir/programs" crates/sisuc/tests/run crates/sisuc/tests/run-fail
    # cargo-fuzz merges old raw profiles, so a stale profile could hide a gap.
    rm -rf fuzz/coverage/program fuzz/coverage/front_end
    cargo fuzz coverage --sanitizer none program "$seeds"
    export_coverage program >"$gaps_dir/seeds.json"
    cargo fuzz coverage --sanitizer none front_end "$gaps_dir/programs"
    export_coverage front_end >"$gaps_dir/valid.json"
    local status=0 output
    output=$(python3 scripts/fuzz_gaps.py "$gaps_dir/seeds.json" "$gaps_dir/valid.json" fuzz/known_gaps.txt) || status=$?
    echo "$output"
    mkdir -p fuzz/artifacts
    { echo; echo "$output"; } >>fuzz/artifacts/summary.md
    exit "$status"
}

# Replays the input here and on an export of the commit $ref, and says which of the two fail. So
# a crash that this checkout brought is told from one that $ref has too. The export stays in
# target/against/<commit>, so a later replay on the same commit does not build it again.
against() {
    local sha dir dump here=0 there=0
    sha=$(git rev-parse --verify "$ref^{commit}")
    dir=$PWD/target/against/$sha
    dump=$(dump_dir "$input")
    if [[ ! -d $dir ]]; then
        # Into a temporary name first, so a stopped export is never taken as a whole one.
        rm -rf "$dir.part"
        mkdir -p "$dir.part"
        git archive "$sha" | tar -x -C "$dir.part"
        mv "$dir.part" "$dir"
    fi
    # .cargo/config.toml finds LLVM at .llvm/23 relative to the checkout.
    ln -sfn "$PWD/.llvm" "$dir/.llvm"
    echo "scripts/fuzz.sh: replay at HEAD"
    SISU_FUZZ_DUMP=$dump cargo fuzz run --sanitizer none "$target" "$input" -- "${limits[@]}" || here=$?
    echo "scripts/fuzz.sh: replay at ${ref} (${sha})"
    (
        cd "$dir"
        if [[ $target == program ]]; then
            cargo build --release -p sisu-runtime-mimalloc
            export SISU_RUNTIME=$PWD/target/release/libsisu_runtime_mimalloc.a
        fi
        SISU_FUZZ_DUMP=$dump.against cargo fuzz run --sanitizer none "$target" "$input" -- "${limits[@]}"
    ) || there=$?
    if ((here == 0)); then
        echo "scripts/fuzz.sh: the input passes at HEAD"
    elif ((there == 0)); then
        echo "scripts/fuzz.sh: the input fails at HEAD only: this checkout brought the failure"
        exit 1
    else
        echo "scripts/fuzz.sh: the input fails at HEAD and at ${ref}: compare the two panic messages above"
    fi
    exit 0
}

# These exec cargo, so the trap below would never run: they come before the seeds exist.
case ${1:-} in
--against) against ;;
--replay) SISU_FUZZ_DUMP=$(dump_dir "$input") exec cargo fuzz run --sanitizer none "$target" "$input" -- "${limits[@]}" ;;
--tmin)
    # A minimizing step sets the input's length itself, and libFuzzer asserts that -max_len did not.
    tmin_limits=()
    for limit in "${limits[@]}"; do
        if [[ $limit != -max_len=* ]]; then tmin_limits+=("$limit"); fi
    done
    exec cargo fuzz tmin --sanitizer none "$target" "$input" -- "${tmin_limits[@]}"
    ;;
--cmin) exec cargo fuzz cmin --sanitizer none "$target" "$corpus" -- "${limits[@]}" ;;
esac

# The seeds go into a temp directory: for front_end every .sisu file under the test tree, copied
# under a unique name, and for program the files of scripts/fuzz_seeds.py.
seeds=$(mktemp -d)
marker=$(mktemp)
replay_out=$(mktemp)
gaps_dir=
# The pairs that the single-mistake check of the program target gave (fuzz_program.rs). Only the
# fuzz command gets this file: the build and the replays do not.
cascades=
trap 'rm -rf "$seeds" "$marker" "$replay_out" "$gaps_dir" "$cascades"' EXIT
if [[ $target == program ]]; then
    cascades=$(mktemp)
    python3 scripts/fuzz_seeds.py "$seeds"
else
    copy_programs "$seeds" crates/sisuc/tests
fi

if [[ ${1:-} == --coverage ]]; then
    coverage
    exit 0
fi
if [[ ${1:-} == --gaps ]]; then gaps; fi

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
fuzz_env=()
if [[ -n $cascades ]]; then fuzz_env=(SISU_FUZZ_CASCADES="$cascades"); fi
logged cargo fuzz build --sanitizer none "$target"
start=$SECONDS
fuzzed=false
if ((status == 0)); then
    fuzzed=true
    logged env "${fuzz_env[@]}" "${cmd[@]}"
fi
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
# The panic location and the first line of the panic message of the replay in file $1, as
# "`<location>`: `<message>`", or the location alone when the message is empty. Nothing is
# printed when the replay has no panic message.
# The output holds binary bytes, so grep reads it with -a. The message is cut to 300 bytes
# first. Then it loses its control characters, its invalid UTF-8 and its backticks. It is
# printed in a code span, so a mention or a link in it does not work in an issue.
valid_utf8() {
    iconv -f UTF-8 -t UTF-8 -c || true
}

panic_note() {
    local found location message
    # `valid_utf8` drops invalid bytes; some iconv builds exit 1 when they do, so its status is not used.
    found=$(grep -a -m1 -A1 'panicked at ' "$1" | LC_ALL=C tr -d '\000-\010\013-\037\177`' | valid_utf8) ||
        return 0
    [[ -n $found ]] || return 0
    location=$(sed -n '1s/.*panicked at //; 1s/:$//p' <<<"$found" | valid_utf8)
    # `head -c` counts bytes; `cut -c` counts characters in some coreutils (uutils on Ubuntu 26.04).
    message=$(sed -n 2p <<<"$found" | head -c 300 | valid_utf8)
    if [[ -n $message ]]; then
        printf "\`%s\`: \`%s\`\n" "$location" "$message"
    else
        printf "\`%s\`\n" "$location"
    fi
}

crashes=0
panic_notes=()
for i in "${!failures[@]}"; do
    failure=${failures[i]}
    if [[ ${failure%% *} != crash ]]; then continue; fi
    crashes=$((crashes + 1))
    if ((crashes > replay_limit)); then continue; fi
    path=${failure#* }
    : >"$replay_out"
    {
        echo
        echo "==> scripts/fuzz.sh: replay of ${path}"
        replay=0
        SISU_FUZZ_DUMP=$(dump_dir "$path") timeout -k 10 "$replay_seconds" \
            cargo fuzz run --sanitizer none "$target" "$path" -- "${limits[@]}" 2>&1 | tee "$replay_out" || replay=$?
        echo "==> scripts/fuzz.sh: the replay exited ${replay}"
    } >>"$log" 2>&1 || log_complete=false
    panic_notes[i]=$(panic_note "$replay_out")
done
not_replayed=0
if ((crashes > replay_limit)); then
    not_replayed=$((crashes - replay_limit))
    echo "==> scripts/fuzz.sh: ${not_replayed} more crashes not replayed; scripts/fuzz.sh --target ${target} --replay <path> replays any of them" >>"$log" || log_complete=false
fi

# Fork mode interleaves the children's output, whose lines start `#<n>\t`; the parent's
# status lines start `#<n>: cov:`, and the last one holds the total.
executions=$(awk '/^#[0-9]+: cov:/ { n = substr($1, 2, length($1) - 2) } END { print (n == "" ? "unknown" : n) }' "$log")
# libFuzzer runs the whole corpus before job 1 (FuzzerFork.cpp, FuzzWithFork). A parent line gives
# the time since the start. So the first such line shows about when that pass ended.
first_pass=$(awk '/^#[0-9]+: cov:/ { for (i = 1; i < NF; i++) if ($i == "time:") { print substr($(i + 1), 1, length($(i + 1)) - 1); exit } }' "$log")

{
    echo "# Fuzzing: ${target}"
    echo
    echo "- Fuzzed for ${elapsed} s (asked for ${seconds} s)."
    echo "- Executions in processed jobs: ${executions}."
    if [[ -z $first_pass ]]; then
        echo "- No job finished, so the number of new inputs is not known."
    else
        echo "- First pass over the corpus: about ${first_pass} s."
        if ((first_pass * 2 > seconds)); then
            echo "- The first pass took over half of the run, so this run tried few new inputs. Ask for more seconds."
        fi
    fi
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
    for i in "${!failures[@]}"; do
        failure=${failures[i]}
        class=${failure%% *}
        path=${failure#* }
        echo "- **${class}**: \`${path}\`. Replay: \`scripts/fuzz.sh --target ${target} --replay ${path}\`."
        if [[ $class == crash && -n ${panic_notes[i]:-} ]]; then
            echo "  Panic at ${panic_notes[i]}"
            echo "  Its replay is at the end of \`fuzz.log\`."
        elif [[ $class == crash ]]; then
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

# The pairs of the baseline that no input of this run gave. The section is written only when the
# fuzz command ran: after a failed build, every pair would be missing. It does not change the
# exit status, because a pair can be missing for a short run.
if [[ $fuzzed == true && -n $cascades ]]; then
    baseline=fuzz/known_cascades.txt
    # A pair line is a line that is not a comment and not empty (fuzz_program.rs, parse_cascades).
    # The fuzzer splits it on white space, so the script prints its two words with one space.
    missing=$(LC_ALL=C comm -23 \
        <(awk '{ gsub(/\r/, "") } /^#/ || !NF { next } { print $1 " " $2 }' "$baseline" | LC_ALL=C sort -u) \
        <(LC_ALL=C sort -u "$cascades"))
    {
        echo
        echo "## Known cascades"
        echo
        if [[ -z $missing ]]; then
            echo "Each pair of \`$baseline\` occurred."
        else
            echo "Warning: no input of this run gave these pairs of \`$baseline\`. A pair that no nightly run gives can go:"
            echo
            while IFS= read -r pair; do echo "- \`${pair}\`"; done <<<"$missing"
        fi
    } >>fuzz/artifacts/summary.md
fi

cat fuzz/artifacts/summary.md
if ((${#failures[@]} > 0)); then
    exit 1
fi
exit "$status"
