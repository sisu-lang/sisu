//! The program fuzzer's generator: it turns bytes, read through `arbitrary::Unstructured`, into a
//! Sisu program that is well typed by construction, the stdin it reads, and a rewritten copy that
//! must print the same (ADR 0033). Each rule writes one construct of a wanted type, and the bytes
//! choose among the rules that can make that type in the current scope. Choice 0 is always the
//! smallest valid one, which is what every choice is once the bytes run out.

mod expr;
mod generic;
mod item;
mod scope;
mod stmt;
mod writer;

#[cfg(test)]
pub(crate) mod coverage;

use arbitrary::Unstructured;

use scope::{Scope, Ty};
use writer::Writer;

pub(crate) struct Generated {
    pub(crate) program: String,
    /// `program` with the rewrites of decision 2, which must print the same.
    pub(crate) rewritten: String,
    pub(crate) stdin: String,
    /// The known sites `main` prints, in the order it prints them (decision 3).
    pub(crate) known: Vec<Known>,
    /// The program's cost, as decision 5 counts it.
    #[cfg(test)]
    pub(crate) cost: u64,
    /// `program` without its known sites, which the coverage test walks so they hide nothing.
    #[cfg(test)]
    pub(crate) plain: String,
    /// The rewrites `rewritten` holds.
    #[cfg(test)]
    pub(crate) made: Vec<writer::Rewrite>,
}

pub(crate) struct Known {
    /// `k<n>`; `main` prints its result as `@k<n> <value>`.
    pub(crate) marker: String,
    pub(crate) answer: Answer,
}

/// The line `main` prints just before the panic site `marker`: the one place its text is
/// written, for the generator and for property 5.
pub(crate) fn panic_line(marker: &str) -> String {
    format!("@{marker} panics")
}

pub(crate) enum Answer {
    Value(String),
    /// The runtime's message: `integer overflow` or `division by zero`.
    Panic(&'static str),
}

/// The most work a program may do, counted by `Cost` (decision 5).
const BUDGET: u64 = 1_000_000;
/// The most elements or nodes of a value the program builds, chars for a `string` (decision 5).
const MAX_SIZE: u64 = 1_000;
/// The most iterations of a loop.
const MAX_BOUND: u64 = 8;
/// The most loops around a statement.
const MAX_LOOPS: usize = 3;
/// The `depth`s a call can pass: `main` passes at most 4.
const DEPTHS: usize = 5;
/// Statements and expression nodes a program may hold; each rule spends one.
const FUEL: u64 = 400;
/// The most an expression nests.
const MAX_NESTING: u32 = 6;
/// The most chars of a line of the stdin.
const LINE: u64 = 12;
/// The most a statement of `main` costs: inside every loop it may nest in, where the condition of
/// a `while` runs once more than its body.
const STATEMENT_MOST: u64 = (MAX_BOUND + 1) * (MAX_BOUND + 1) * (MAX_BOUND + 1);

/// Accepts every byte string. When `data` runs out, every choice is the smallest valid one.
pub(crate) fn generate(data: &[u8]) -> Generated {
    // The rewrites and the known sites read the bytes from the end, the program from the start.
    let reversed: Vec<u8> = data.iter().rev().copied().collect();
    let mut g = Gen::new(data);
    g.ru = Unstructured::new(&reversed);
    let (cost, known) = g.program();
    // Only the tests read the cost.
    #[cfg(not(test))]
    let _ = cost;
    #[cfg(test)]
    let plain = g
        .known_text
        .iter()
        .rev()
        .fold(g.w.out.program.clone(), |mut text, at| {
            text.replace_range(at.clone(), "");
            text
        });
    g.w.out.rewritten += &g.w.consts;
    Generated {
        program: g.w.out.program,
        rewritten: g.w.out.rewritten,
        stdin: g.stdin,
        known,
        #[cfg(test)]
        cost,
        #[cfg(test)]
        plain,
        #[cfg(test)]
        made: g.w.made,
    }
}

/// The generator's state while it writes one program.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is a separate fact about the point being written"
)]
struct Gen<'a> {
    u: Unstructured<'a>,
    w: Writer,
    scope: Scope,
    /// What rules may still spend.
    fuel: u64,
    /// How deep the expression being written nests.
    nesting: u32,
    /// The lines `read_line` reads, each ending in a line break.
    stdin: String,
    /// The cost of the function being written, or of `main`.
    cost: Cost,
    /// Inside an interpolation, where a line break ends the string.
    one_line: bool,
    /// What the calls `main` may still make may cost: the budget less what `main`'s own
    /// statements can cost.
    budget: u64,
    /// The jumps a statement may make here.
    jumps: stmt::Jumps,
    /// How many blocks are open around the point being written.
    blocks: u32,
    /// Whether the next expression is in a position with no expected type, where a `u8` must
    /// be one by its own type (`Gen::expr`).
    synth: bool,
    /// Whether the next expression is the value of a `let` that writes its type, so that a
    /// `Result` may be built as `Ok(..)` or `Err(..)`, which take their type from it.
    typed: bool,
    /// Whether the variant that `Gen::variant` writes next may be written as `Ok(..)` or `Err(..)`.
    bare: bool,
    /// What chooses the rewrites and the known sites, apart from `u`, so the program is the
    /// same whatever they are.
    ru: Unstructured<'a>,
    /// Whether an expression of the statement being written so far has an effect or may
    /// panic, so one after it may not move before the statement (decision 2).
    effect: bool,
    /// Whether `main` holds a `return`.
    returns: bool,
    /// Where `program` holds the known sites' text, in order.
    #[cfg(test)]
    known_text: Vec<std::ops::Range<usize>>,
}

impl<'a> Gen<'a> {
    fn new(data: &'a [u8]) -> Gen<'a> {
        Gen {
            u: Unstructured::new(data),
            w: Writer::default(),
            scope: Scope::default(),
            fuel: FUEL,
            nesting: 0,
            stdin: String::new(),
            cost: Cost::default(),
            one_line: false,
            budget: BUDGET - FUEL * STATEMENT_MOST,
            jumps: stmt::Jumps::default(),
            blocks: 0,
            synth: false,
            typed: false,
            bare: false,
            ru: Unstructured::new(&[]),
            effect: false,
            returns: false,
            #[cfg(test)]
            known_text: Vec::new(),
        }
    }

    /// One of `0..n`, `n > 0`, for a rewrite or a known site.
    fn rewrite_pick(&mut self, n: usize) -> usize {
        self.ru.choose_index(n).unwrap_or(0)
    }

    /// One of `0..n`, `n > 0`.
    fn pick(&mut self, n: usize) -> usize {
        self.u.choose_index(n).unwrap_or(0)
    }

    /// `ty` as a declaration writes it: each optional is `T?`, or now and then `Option<T>`.
    fn spell(&mut self, ty: &Ty) -> String {
        match ty {
            Ty::Optional(payload) => {
                let payload = self.spell(payload);
                if self.pick(2) == 0 {
                    format!("{payload}?")
                } else {
                    format!("Option<{payload}>")
                }
            }
            Ty::Array(element) => format!("[{}]", self.spell(element)),
            _ => ty.to_string(),
        }
    }

    /// One of `lo..=hi`.
    fn int(&mut self, lo: i64, hi: i64) -> i64 {
        self.u.int_in_range(lo..=hi).unwrap_or(lo)
    }

    /// One of `0..=hi`.
    fn upto(&mut self, hi: u64) -> u64 {
        self.u.int_in_range(0..=hi).unwrap_or(0)
    }

    /// Writes one of `options`.
    fn one_of(&mut self, options: &[&str]) {
        let choice = self.pick(options.len());
        self.w.text(options[choice]);
    }

    /// Takes `units` of fuel, if that much is left.
    fn spend(&mut self, units: u64) -> bool {
        let spent = self.fuel >= units;
        if spent {
            self.fuel -= units;
        }
        spent
    }
}

/// The work of a function body, or of `main`, as decision 5 counts it: a statement costs 1, a
/// loop its bound times its body, and a call its callee's cost at the depth it passes.
#[derive(Default)]
pub(crate) struct Cost {
    /// The bounds of the loops around the point being written.
    loops: Vec<u64>,
    /// The statements so far, each times the bounds of its loops.
    statements: u64,
    calls: Vec<Call>,
}

/// A call in a body: how many times it runs per run of the body, its callee, and the depth it
/// passes, `None` for `depth - 1`.
struct Call {
    times: u64,
    callee: usize,
    depth: Option<usize>,
}

impl Cost {
    /// How many times a statement at this point runs per run of the body.
    pub(crate) fn times(&self) -> u64 {
        self.times_since(0)
    }

    /// How many times a statement at this point runs per run of the loops after the first
    /// `outer` ones.
    pub(crate) fn times_since(&self, outer: usize) -> u64 {
        self.loops[outer..]
            .iter()
            .fold(1, |times: u64, bound| times.saturating_mul(*bound))
    }

    pub(crate) fn enter_loop(&mut self, bound: u64) {
        self.loops.push(bound);
    }

    /// The condition of a `while` of `bound` runs, which runs once more than its body; leave it
    /// with `exit_loop`.
    pub(crate) fn enter_condition(&mut self, bound: u64) {
        self.loops.push(bound + 1);
    }

    pub(crate) fn exit_loop(&mut self) {
        self.loops.pop();
    }

    /// The loops around the point being written.
    pub(crate) fn depth(&self) -> usize {
        self.loops.len()
    }

    /// A call of a generic function whose body costs `units`.
    pub(crate) fn generic_call(&mut self, units: u64) {
        let times = self.times();
        self.statements = self.statements.saturating_add(times.saturating_mul(units));
    }

    pub(crate) fn statement(&mut self) {
        let times = self.times();
        debug_assert!(times <= STATEMENT_MOST, "a statement runs {times} times");
        self.statements = self.statements.saturating_add(times);
    }

    /// A call of the function `callee` passing `depth`, `None` for `depth - 1`.
    pub(crate) fn call(&mut self, callee: usize, depth: Option<usize>) {
        let times = self.times();
        self.calls.push(Call {
            times,
            callee,
            depth,
        });
    }

    /// The cost of a call of the function whose body this is, at each depth, where `costs` are
    /// the functions' and `own` is this one's index, whose entry is not set yet. The
    /// `depth <= 0` check costs 1.
    pub(crate) fn of_function(&self, costs: &[[u64; DEPTHS]], own: usize) -> [u64; DEPTHS] {
        let mut cost = [1; DEPTHS];
        for depth in 1..DEPTHS {
            let at = self
                .calls
                .iter()
                .fold(self.statements.saturating_add(1), |sum: u64, call| {
                    let callee = if call.callee == own {
                        &cost
                    } else {
                        &costs[call.callee]
                    };
                    sum.saturating_add(call.times.saturating_mul(callee[depth - 1]))
                });
            cost[depth] = at;
        }
        cost
    }

    /// The cost of `main`, whose calls pass literal depths, where `costs` are the functions'.
    pub(crate) fn of_main(&self, costs: &[[u64; DEPTHS]]) -> u64 {
        self.calls.iter().fold(self.statements, |sum: u64, call| {
            let depth = call
                .depth
                .expect("a call in `main` should pass a literal depth");
            sum.saturating_add(call.times.saturating_mul(costs[call.callee][depth]))
        })
    }
}
