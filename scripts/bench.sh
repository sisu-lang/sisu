#!/usr/bin/env bash
# Builds each benchmark in bench/ in every language whose toolchain is on PATH,
# checks that it prints what the Sisu version prints, and times it pinned to
# one CPU. Builds and runs each program RUNS times. Prints a markdown table
# for docs/benchmarks.md.
#   scripts/bench.sh [benchmark...]
# RUNS (default 5) sets the runs per program, BENCH_CPU (default 2) the CPU,
# PYTHON (default python3.14) the Python interpreter.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
runs=${RUNS:-5}
cpu=${BENCH_CPU:-2}
python=${PYTHON:-python3.14}
out=target/bench
benchmarks=("$@")
[[ ${#benchmarks[@]} -gt 0 ]] || benchmarks=(hello fib sieve binary_trees quicksort nbody mandelbrot fannkuch_redux)

cargo build --release --quiet -p sisuc -p sisu-runtime

have() { command -v "$1" >/dev/null; }

# Builds benchmark $2 in language $1 into directory $3, writing the build's
# wall time in seconds, to the millisecond, into $3/compile. Prints the
# command that runs it.
build() {
    local lang=$1 src=bench/$2 dir=$3 start=$EPOCHREALTIME
    case $lang in
        sisu) target/release/sisuc -O2 "$src/main.sisu" "$dir/main" ;;
        c) clang -O2 -o "$dir/main" "$src/main.c" -lm ;;
        rust) rustc -C opt-level=3 -o "$dir/main" "$src/main.rs" ;;
        go) go build -o "$dir/main" "$src/main.go" ;;
        java) javac -d "$dir" "$src/Main.java" ;;
        python) echo "$python $src/main.py" && return ;;
    esac
    awk -v a="$start" -v b="$EPOCHREALTIME" 'BEGIN { printf "%.3f\n", b - a }' >"$dir/compile"
    if [[ $lang == java ]]; then echo "java -cp $dir Main"; else echo "$dir/main"; fi
}

# The median of the numbers on standard input.
median() { sort -n | awk '{ v[NR] = $1 } END { print v[int((NR + 1) / 2)] }'; }

declare -A names=([sisu]=Sisu [c]=C [rust]=Rust [go]=Go [java]=Java [python]=Python)
available=(sisu)
have clang && available+=(c)
have rustc && available+=(rust)
have go && available+=(go)
have javac && have java && available+=(java)
have "$python" && available+=(python)

echo "Commit $(git rev-parse --short HEAD), $(date -u +%F), $(lscpu | sed -n 's/^Model name: *//p'), $(uname -sr)."
echo "Median of $runs runs on CPU $cpu."
for bench in "${benchmarks[@]}"; do
    rows=()
    for lang in "${available[@]}"; do
        dir=$out/$bench/$lang
        rm -rf "$dir" && mkdir -p "$dir"
        for i in $(seq "$runs"); do
            cmd=$(build "$lang" "$bench" "$dir")
            if [[ -f $dir/compile ]]; then mv "$dir/compile" "$dir/compile.$i"; fi
        done
        for i in $(seq "$runs"); do
            # shellcheck disable=SC2086 # $cmd is a command and its arguments.
            taskset -c "$cpu" /usr/bin/time -f "%e %U %S %M" -o "$dir/time.$i" $cmd >"$dir/stdout"
            if ! cmp -s "$dir/stdout" "$out/$bench/sisu/stdout"; then
                echo "$bench/$lang printed something other than the Sisu version:" >&2
                diff "$out/$bench/sisu/stdout" "$dir/stdout" >&2
                exit 1
            fi
        done
        wall=$(cat "$dir"/time.* | awk '{ print $1 }' | median)
        cpu_s=$(cat "$dir"/time.* | awk '{ printf "%.2f\n", $2 + $3 }' | median)
        rss=$(cat "$dir"/time.* | awk '{ printf "%.1f\n", $4 / 1024 }' | median)
        binary=-
        if [[ -f $dir/main ]]; then
            strip -o "$dir/main.stripped" "$dir/main"
            binary=$(($(stat -c %s "$dir/main.stripped") / 1024))
        fi
        compile=-
        [[ -f $dir/compile.1 ]] && compile=$(cat "$dir"/compile.* | median)
        cells=("${names[$lang]}" "$wall" "$cpu_s" "$rss" "$binary" "$compile")
        if [[ $lang == sisu ]]; then
            for i in "${!cells[@]}"; do
                [[ ${cells[i]} == - ]] || cells[i]="**${cells[i]}**"
            done
        fi
        printf -v row '%s | ' "${cells[@]}"
        # The wall time before the tab is the sort key, cut off before printing.
        rows+=("$wall	| ${row% }")
    done
    echo
    echo "### $bench"
    echo
    echo "| Language | Wall (s) | CPU (s) | Peak RSS (MiB) | Binary (KiB) | Build (s) |"
    echo "| --- | ---: | ---: | ---: | ---: | ---: |"
    printf '%s\n' "${rows[@]}" | sort -n | cut -f2-
done

echo
echo "Toolchains:"
echo "- sisuc -O2 (LLVM 23)"
have clang && echo "- $(clang --version | head -1), -O2"
have rustc && echo "- $(rustc --version), -C opt-level=3"
have go && echo "- $(go version)"
have java && echo "- $(java -version 2>&1 | head -1)"
have "$python" && echo "- $($python --version)"
exit 0
