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
mkdir -p "$out"/{fib,sieve,binary_trees,quicksort,nbody,mandelbrot,fannkuch_redux,large,large_20,build_hello,build_binary_trees,build_large,build_large_20,strings,enum_match,shared_arrays,compare,tree_rewrite,interp,interfaces,show}

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
for b in strings enum_match shared_arrays compare tree_rewrite interp interfaces show; do
    target/release/sisu build -O2 "bench/features/$b/main.sisu" -o "$out/$b/main" || fail "$b failed to compile"
done

# step0 returns its argument. stepi adds 1 through an array, a class and an
# optional chain, and 0 through constants, an enum, f64 methods, a string built
# with += and format specs, while let, u8 and char, a generic function,
# class and enum, each instantiated twice, a match on a prelude Result,
# an i64?? that two ?? peel to 0, an interface with an associated type that
# a class conforms to in its declaration and another in `extend` blocks, a
# bound with &, and a call through each bound, a bounded `largest` over
# numbers and over a class with a written `Eq` and `Ord`, `==` on an array of
# such classes, `is` on a class through a bound `T: AnyObject`, and the text of an array,
# an enum and an interpolated array, which add 0, and a `dbg` of a class in a
# branch that never runs;
# then it calls step(i-1).
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
    _label: string

    fn get(self) -> i64 { self.v }
}

fn pick$i<T>(xs: [T], fallback: T) -> T {
    if xs.len() > 0 { xs[0] } else { fallback }
}

class Pair$i<A, B> {
    a: A
    b: B

    fn first(self) -> A { self.a }
}

enum Tree$i<T> {
    Leaf
    Node(Tree$i<T>, T, Tree$i<T>)

    fn size(self) -> i64 {
        match self {
            Tree$i.Leaf => 0
            Tree$i.Node(l, _, r) => l.size() + 1 + r.size()
        }
    }
}

interface Gauge$i {
    type Unit
    fn read(self) -> i64
    fn unit(self) -> Unit
}

interface Tag$i {
    fn tag(self) -> i64
}

class Dial$i: Gauge$i, Tag$i {
    type Unit = i64
    n: i64

    fn read(self) -> i64 { self.n }
    fn unit(self) -> Unit { self.n }
    fn tag(self) -> i64 { 1 }
}

class Bar$i {
    n: i64
}

extend Bar$i: Gauge$i {
    type Unit = string

    fn read(self) -> i64 { self.n }
    fn unit(self) -> Unit { "u" }
}

extend Bar$i: Tag$i {
    fn tag(self) -> i64 { 1 }
}

fn both$i<T: Gauge$i & Tag$i>(t: T) -> i64 { t.read() + t.tag() }

fn unit_of$i<T: Gauge$i>(t: T) -> T.Unit { t.unit() }

fn below$i(k: i64) -> i64? {
    if k > 0 { k - 1 } else { None }
}

fn parse$i(k: i64) -> Result<i64, string> {
    if k > 0 { Result.Ok(k) } else { Result.Err("none") }
}

class Disc$i: Eq, Ord {
    r: i64

    fn eq(self, other: Self) -> bool { self.r == other.r }
    fn less(self, other: Self) -> bool { self.r < other.r }
}

fn largest$i<T: Ord>(xs: [T]) -> T? {
    let mut best: T? = None
    for v in xs {
        if let b = best {
            if v > b { best = v }
        } else {
            best = v
        }
    }
    best
}

fn same$i<T: AnyObject>(a: T, b: T) -> bool { a is b }

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
    let nested: i64?? = k
    let peeled = (nested ?? None) ?? 0
    let first = text.chars()[0]
    let zero = if shapes[2].is_dot() && first == 's' { i64(b) - spins } else { 1 }
    let c: C$i? = C$i(v: sum - x - xs[1], _label: text)
    let p = Pair$i(a: pick$i([K$i], 0), b: pick$i(["g"], ""))
    let t = Tree$i.Node(Tree$i.Leaf, p.first(), Tree$i.Leaf)
    let u = Tree$i.Node(Tree$i.Leaf, p.b, Tree$i.Node(Tree$i.Leaf, "h", Tree$i.Leaf))
    let q = Pair$i(a: p.b, b: p.first())
    let parsed = match parse$i(K$i) {
        Result.Ok(v) => v - K$i
        Result.Err(_) => 1
    }
    let gauge = both$i(Dial$i(n: K$i)) - both$i(Bar$i(n: K$i)) + unit_of$i(Dial$i(n: K$i)) - K$i + unit_of$i(Bar$i(n: K$i)).len() - 1
    let top = (largest$i([Disc$i(r: K$i), Disc$i(r: 0)]) ?? Disc$i(r: 0)).r - K$i + (largest$i([0, K$i, 1]) ?? 0) - K$i
    let kept = if [Disc$i(r: top)] == [Disc$i(r: 0)] { 0 } else { 1 }
    let alias = Bar$i(n: K$i)
    let same = if same$i(alias, alias) { 0 } else { 1 }
    let shown = shapes.to_string().len() + t.to_string().len() + "{[x, K$i]}".len() + "{[x, K$i]:#}".len()
    // The run keeps standard error empty, so this dbg never runs.
    if shown < 0 {
        let _ = dbg(q)
    }
    let generic = t.size() + u.size() - 3 + q.b - K$i + parsed + peeled + gauge + top + kept + same + (if shown > 0 { 0 } else { 1 })
    step$((i - 1))(x + (c?.get() ?? 0) + zero + generic)
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
        enum_match) printf '%s\n' 457364 816672 ;;
        shared_arrays) printf '%s\n' 20000 519500 ;;
        compare) printf '%s\n' 1494 640 2880 ;;
        tree_rewrite) printf '%s\n' 732347 163697 ;;
        interp) printf '%s\n' 549000 126 ;;
        interfaces) printf '%s\n' 699988 949994 1010000 ;;
        show) printf '%s\n' 1091750 40900 ;;
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
