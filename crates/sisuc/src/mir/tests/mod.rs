use super::*;
use crate::check::check;
use crate::parser::parse_ok;

mod certify;
mod elaborate;
pub(crate) mod hoist;
mod lower;
mod ownership;
mod pairs;
mod pattern;

/// A program whose `f` holds every kind of expression and statement lowering handles.
pub(super) const EVERY_KIND: &str = "class O {\n    mut n: i64\n    mut s: string\n}\n\
    enum E {\n    V(i64)\n    W\n}\n\
    fn g(x: i64) -> i64 { x }\n\
    fn name() -> string { \"n\" }\n\
    fn f(o: O, p: O, xs: [i64], s: string, cs: [char], c: bool, x: f64, k: char) {\n\
        let m = O(n: 1, s: \"\")\n\
        let v = E.V(2)\n\
        let w: i64? = g(3)\n\
        let none: i64? = None\n\
        let b = u8(o.n)\n\
        print(x.abs())\n\
        print(-o.n)\n\
        print(1.5 * x)\n\
        print(k == 'a')\n\
        print(s != \"t\")\n\
        print(o is p)\n\
        print(1 < xs[0] < 3)\n\
        print(c && xs.len() > 0)\n\
        print(c || false)\n\
        print(xs.contains(2))\n\
        print([4; 2].sum())\n\
        print(s.words().len())\n\
        let ws = s.words()\n\
        let bs = s.bytes()\n\
        let chs = s.chars()\n\
        print(string(cs))\n\
        print(\"{x:>8}\")\n\
        let line = read_line()\n\
        eprint(1)\n\
        name()\n\
        for i in 0..2 {\n\
            if c { continue }\n\
            print(i)\n\
        }\n\
    }";

/// Code after a `return`, an endless loop, a function that always panics, an `if` whose
/// branches both return and a `match` whose last arm is `_`.
pub(super) const UNREACHABLE_CODE: &str = "fn f() -> i64 { panic(\"x\") }\n\
     fn g() {\n    while true {\n    }\n}\n\
     fn h(c: bool) -> i64 { if c { return 1 } else { return 2 } }\n\
     fn r() -> i64 {\n    return 1\n    print(2)\n    3\n}\n\
     fn m(c: bool) -> i64 {\n    match c {\n        true => 1\n        _ => 0\n    }\n}";

/// The typed tree of `src`, which checks without an error, with an empty `main` added when it
/// has none.
fn checked(src: &str) -> tir::Program {
    let src = if src.contains("fn main") {
        src.to_owned()
    } else {
        format!("{src}\nfn main() {{}}")
    };
    let (program, diagnostics) = check(&parse_ok(&src), &src);
    program.unwrap_or_else(|| panic!("{diagnostics:?}"))
}

/// The mir of `src` as lowering leaves it, before any pass.
pub(super) fn lowered_program(src: &str) -> Program {
    lower(&checked(src))
}

/// The printed mir of `src` as lowering leaves it, before any pass.
pub(super) fn lowered(src: &str) -> String {
    let program = checked(src);
    print(&lower(&program), &program)
}

/// The printed mir of `src` as `build` gives it, with its copies, moves and releases placed.
pub(super) fn built(src: &str) -> String {
    let program = checked(src);
    print(&build(&program), &program)
}

/// The printed function `name`, from its `fn` line through its closing `}`, with every
/// temporary written `_`.
pub(super) fn function(printed: &str, name: &str) -> String {
    let head = format!("fn {name}(");
    let start = printed
        .match_indices(&head)
        .map(|(at, _)| at)
        .find(|&at| at == 0 || printed[..at].ends_with('\n'))
        .unwrap_or_else(|| panic!("no function {name} in {printed}"));
    let rest = &printed[start..];
    let end = rest.find("\n}").map_or(rest.len(), |at| at + 2);
    normalized(&rest[..end])
}

/// `text` with each temporary, `_` and digits not inside a name, written `_`.
fn normalized(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut previous = ' ';
    while let Some(c) = chars.next() {
        out.push(c);
        let in_name = previous.is_alphanumeric() || previous == '_';
        if c == '_' && !in_name {
            while chars.next_if(char::is_ascii_digit).is_some() {}
        }
        previous = c;
    }
    out
}

/// Asserts that function `name` of `printed` holds each of `parts` in order, each after the
/// previous one, once its temporaries are normalized.
pub(super) fn has(printed: &str, name: &str, parts: &[&str]) {
    let f = function(printed, name);
    let mut from = 0;
    for part in parts {
        let at = f[from..]
            .find(part)
            .unwrap_or_else(|| panic!("`{part}` after byte {from} of\n{f}"));
        from += at + part.len();
    }
}

/// The blocks of `f` that no path of successors from `bb0` reaches.
pub(super) fn unreachable_blocks(f: &Function) -> Vec<BlockId> {
    let mut seen = vec![false; f.blocks.len()];
    let mut stack = vec![BlockId(0)];
    while let Some(block) = stack.pop() {
        if std::mem::replace(&mut seen[block.0], true) {
            continue;
        }
        stack.extend(f.blocks[block.0].term.successors());
    }
    (0..f.blocks.len())
        .filter(|&i| !seen[i])
        .map(BlockId)
        .collect()
}

/// Builds a `Function` by hand, for mir that lowering cannot produce. Each local's scope is
/// `bb0` until `scope` moves it; a block's terminator may name blocks not built yet.
pub(crate) struct Builder {
    function: Function,
}

impl Builder {
    pub(crate) fn new(name: &str) -> Self {
        Builder {
            function: Function {
                name: name.to_owned(),
                params: 0,
                ret: Type::UNIT,
                locals: Vec::new(),
                blocks: Vec::new(),
                loops: Vec::new(),
                owners: FxHashMap::default(),
            },
        }
    }

    /// A parameter, declared before any other local.
    pub(crate) fn param(&mut self, name: &str, ty: Type) -> LocalId {
        assert_eq!(
            self.function.params,
            self.function.locals.len(),
            "parameters should come first"
        );
        self.function.params += 1;
        self.local(name, ty)
    }

    /// A named local of `ty`, which holds no enum: counted when `ty` is.
    pub(crate) fn local(&mut self, name: &str, ty: Type) -> LocalId {
        self.function.locals.push(Local {
            name: Some(name.to_owned()),
            ty,
            counted: ty.is_counted(&[]),
            scope: BlockId(0),
        });
        LocalId(self.function.locals.len() - 1)
    }

    pub(crate) fn scope(&mut self, local: LocalId, block: BlockId) {
        self.function.locals[local.0].scope = block;
    }

    /// Records `binding` as a `match` binding that borrows from `owner`.
    pub(crate) fn owner(&mut self, binding: LocalId, owner: LocalId) {
        self.function.owners.insert(binding, owner);
    }

    pub(crate) fn add_loop(&mut self, pre_header: BlockId, header: BlockId, blocks: Vec<BlockId>) {
        self.function.loops.push(Loop {
            pre_header,
            header,
            blocks,
        });
    }

    /// The next block, `bb{n}` for the `n` blocks built before it.
    pub(crate) fn block(&mut self, stmts: Vec<Statement>, term: Terminator) -> BlockId {
        self.function.blocks.push(Block { stmts, term });
        BlockId(self.function.blocks.len() - 1)
    }

    pub(crate) fn finish(self) -> Function {
        self.function
    }
}

// Statements and operands for hand-built mir, written short.

pub(super) fn whole(local: LocalId) -> Place {
    Place {
        local,
        projections: Vec::new(),
        borrow: false,
        guard: false,
    }
}

pub(super) fn read(local: LocalId) -> Operand {
    Operand::Read(whole(local))
}

pub(super) fn assign(dest: LocalId, value: Rvalue) -> Statement {
    Statement::Assign {
        dest: Some(dest),
        value,
    }
}

/// `dest = call fresh()`: an owned value.
pub(super) fn fresh(dest: LocalId) -> Statement {
    assign(
        dest,
        Rvalue::Call {
            func: FuncId(0),
            args: Vec::new(),
        },
    )
}

/// `dest = copy "x"`.
pub(super) fn literal(dest: LocalId) -> Statement {
    assign(dest, Rvalue::CopyStr("x".to_owned()))
}

pub(super) fn goto(block: usize) -> Terminator {
    Terminator::Goto(BlockId(block))
}

pub(super) fn branch(cond: LocalId, then: usize, else_: usize) -> Terminator {
    Terminator::Branch {
        cond: read(cond),
        then: BlockId(then),
        else_: BlockId(else_),
    }
}

pub(super) fn ret() -> Terminator {
    Terminator::Return(None)
}

/// Asserts that `one`, `two` and `three`, the reads for inputs of size `n`, `2n` and `3n`,
/// grow no faster than `n log n`: the second step adds at most a tenth more than the first.
/// The wavelet matrix and its queries add one bit level per doubling of the depths they hold,
/// a few percent from `2n` to `3n`; quadratic work makes the second step two thirds larger.
pub(crate) fn assert_near_linear(one: usize, two: usize, three: usize) {
    assert!(one > 0);
    let (first, second) = (two - one, three - two);
    assert!(
        second <= first + first / 10,
        "{one}, {two}, {three} reads: the second step adds {second}, the first {first}"
    );
}
