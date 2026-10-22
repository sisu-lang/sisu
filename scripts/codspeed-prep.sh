#!/usr/bin/env bash
# Prepares the CodSpeed benchmarks listed in codspeed.yml and codspeed-main.yml:
# builds release sisu, compiles reduced copies of the bench/ programs,
# generates a 200-section and a 20-section compiler input, then runs every
# benchmark once and checks what it prints. Writes .codspeed-main-merged.yml,
# the entries of both files, which pushes to main run. Exits non-zero, naming
# the benchmark or file, when a reduction no longer applies or a benchmark
# misbehaves.
#   scripts/codspeed-prep.sh
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
out=target/codspeed
large_sections=200
small_sections=20

cargo build --release --locked -p sisuc -p sisu-runtime-mimalloc
rm -rf "$out"
mkdir -p "$out"/{fib,sieve,binary_trees,quicksort,nbody,mandelbrot,fannkuch_redux,large,large_20,build_hello,build_binary_trees,build_large,build_large_20,strings,enum_match,shared_arrays,compare,tree_rewrite}

fail() {
    echo "codspeed-prep: $*" >&2
    exit 1
}

# Copies bench/$1/main.sisu with sed script $2 applied and compiles it.
reduce() {
    local b=$1 script=$2
    sed "$script" "bench/$b/main.sisu" >"$out/$b/main.sisu"
    if cmp -s "bench/$b/main.sisu" "$out/$b/main.sisu"; then
        fail "the reduction of $b no longer applies"
    fi
    target/release/sisu build -O2 "$out/$b/main.sisu" -o "$out/$b/main" || fail "$b failed to compile"
}
reduce fib 's/fib(40)/fib(27)/'
reduce sieve 's/sieve(50_000_000)/sieve(1_000_000)/'
reduce binary_trees 's/max_depth = 18/max_depth = 10/'
reduce quicksort 's/let n = 10_000_000/let n = 100_000/'
reduce nbody 's/let steps = 5_000_000/let steps = 100_000/'
reduce mandelbrot 's/let n = 4000/let n = 400/'
reduce fannkuch_redux 's/fannkuch(10)/fannkuch(9)/'
# bench/features/ programs are sized for simulation and Sisu-only: no reduction.
for b in strings enum_match shared_arrays compare tree_rewrite; do
    target/release/sisu build -O2 "bench/features/$b/main.sisu" -o "$out/$b/main" || fail "$b failed to compile"
done

# step0 returns its argument. stepi adds 1 through an array, a class and an
# optional chain, and 0 through constants, an enum, f64 methods, a string built
# with += and format specs, while let, u8 and char; then it calls step(i-1).
# main prints step(N-1)(0) = N-1. Not fi: f64 is a built-in type.
large_input() {
    local sections=$1 dir=$2
{
    echo 'fn step0(x: i64) -> i64 { x }'
    for ((i = 1; i < sections; i++)); do
        cat <<SISU

const K$i = $i
const HALF$i = 0.5

enum Shape$i {
    Dot
    Circle(f64)
    Rect(w: f64, h: f64)

    fn is_dot(self) -> bool { self == Shape$i.Dot }
}

class C$i {
    v: i64
    label: string

    fn get(self) -> i64 { self.v }
}

fn below$i(k: i64) -> i64? {
    if k > 0 { k - 1 } else { None }
}

fn step$i(x: i64) -> i64 {
    let xs = [x, K$i, 1]
    let mut sum = 0
    for v in xs { sum += v }
    let r = f64(K$i) * HALF$i
    let area = (r * r * f64.pi).sqrt().max(0.0)
    let shapes = [Shape$i.Circle(area), Shape$i.Rect(w: r, h: 2.0), Shape$i.Dot]
    let mut text = "s$i"
    text += "{area:>10.3}|{K$i:<4}|"
    let mut k: i64? = 2
    let mut spins = 0
    while let n = k {
        spins += 1
        k = below$i(n)
    }
    let b = u8(spins) ?? 0
    let first = text.chars()[0]
    let zero = if shapes[2].is_dot() && first == 's' { i64(b) - spins } else { 1 }
    let c: C$i? = C$i(v: sum - x - xs[1], label: text)
    step$((i - 1))(x + (c?.get() ?? 0) + zero)
}
SISU
    done
    printf '\nfn main() {\n    print(step%d(0))\n}\n' $((sections - 1))
} >"$dir/main.sisu"
}
large_input "$large_sections" "$out/large"
large_input "$small_sections" "$out/large_20"

# Reads a config file strictly: a "benchmarks:" line, then per entry the lines
# "  - name: X", "    id: X" (same X) and "    exec: plain words", in that
# order. Anything else fails, so no entry can escape the check below.
names=()
execs=()
read_config() {
    local file=$1 line e state=start
    [[ -f $file ]] || fail "$file: missing"
    local before=${#names[@]}
    while IFS= read -r line; do
        [[ -z $line || $line == \#* ]] && continue
        case $state:$line in
            start:"benchmarks:") state=name ;;
            name:"  - name: "?* | id:"  - name: "?*)
                [[ $state == name ]] || fail "$file: entry lacks id/exec before: $line"
                names+=("${line#  - name: }")
                state=id
                ;;
            id:"    id: "?*)
                [[ ${line#    id: } == "${names[-1]}" ]] || fail "$file: id differs from name: $line"
                state=run
                ;;
            run:"    exec: "?*)
                e=${line#    exec: }
                [[ $e != *[\"\'\\#]* ]] || fail "$file: exec must be plain words: $line"
                execs+=("$e")
                state=name
                ;;
            *) fail "$file: unexpected line: $line" ;;
        esac
    done <"$file"
    [[ $state == name && ${#names[@]} -gt $before ]] || fail "$file: no complete benchmarks found"
}
read_config codspeed.yml
read_config codspeed-main.yml

for i in "${!names[@]}"; do
    for ((j = i + 1; j < ${#names[@]}; j++)); do
        [[ ${names[i]} != "${names[j]}" ]] || fail "benchmark ${names[i]} is listed twice"
    done
done

# Pushes to main run the entries of both files. Written from the parsed arrays,
# so CodSpeed reads only entries checked above. It sits at the repository root
# because CodSpeed runs a config's commands from the config's own directory.
merged=.codspeed-main-merged.yml
{
    echo "# Written by scripts/codspeed-prep.sh from codspeed.yml and codspeed-main.yml; do not edit."
    echo "benchmarks:"
    for i in "${!names[@]}"; do
        printf '  - name: %s\n    id: %s\n    exec: %s\n' "${names[i]}" "${names[i]}" "${execs[i]}"
    done
} >"$merged"

# Runs the exec line of every entry once. check_* and build_* must print
# nothing; a program must print its expected output and nothing on stderr.
expected() {
    case $1 in
        fib) echo 196418 ;;
        sieve) echo 78498 ;;
        binary_trees) printf '%s\n' 4095 1024 31744 256 32512 64 32704 16 32752 2047 ;;
        quicksort) echo 5315747952549993 ;;
        nbody) printf '%s\n' -0.169075164 -0.169079859 ;;
        mandelbrot) echo 63528 ;;
        fannkuch_redux) printf '%s\n' 8629 'Pfannkuchen(9) = 30' ;;
        strings) printf '%s\n' 186741 9800 10000 12073291 ;;
        enum_match) printf '%s\n' 987548 816672 ;;
        shared_arrays) printf '%s\n' 20000 519500 ;;
        compare) printf '%s\n' 1494 640 2880 ;;
        tree_rewrite) printf '%s\n' 732347 163697 ;;
        *) return 1 ;;
    esac
}
for i in "${!names[@]}"; do
    name=${names[i]}
    read -ra cmd <<<"${execs[i]}"
    "${cmd[@]}" >"$out/stdout" 2>"$out/stderr" || fail "$name exited non-zero"
    [[ ! -s $out/stderr ]] || fail "$name printed to stderr"
    case $name in
        check_* | build_*)
            [[ ! -s $out/stdout ]] || fail "$name printed output"
            ;;
        *)
            expected "$name" >"$out/expected" || fail "no expected output for $name"
            cmp -s "$out/expected" "$out/stdout" || fail "$name printed something other than its expected output"
            ;;
    esac
done

# Runs the program a build_* benchmark compiled: it prints sections - 1.
large_program() {
    local name=$1 sections=$2
    "$out/$name/main" >"$out/stdout" 2>"$out/stderr" || fail "$name's program exited non-zero"
    [[ ! -s $out/stderr ]] || fail "$name's program printed to stderr"
    printf '%d\n' $((sections - 1)) | cmp -s - "$out/stdout" || fail "$name's program printed something other than $((sections - 1))"
}
large_program build_large "$large_sections"
large_program build_large_20 "$small_sections"
echo "codspeed-prep: ok"
