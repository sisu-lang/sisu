//! The program fuzzer's generator: it turns bytes, read through `arbitrary::Unstructured`, into a
//! Sisu program that is well typed by construction, the stdin it reads, and a rewritten copy that
//! must print the same (ADR 0033). Each rule writes one construct of a wanted type, and the bytes
//! choose among the rules that can make that type in the current scope. Choice 0 is always the
//! smallest valid one, which is what every choice is once the bytes run out.

mod expr;
mod generic;
mod item;
mod mistake;
mod mut_call;
mod scope;
mod stmt;
mod writer;

#[cfg(test)]
pub(crate) mod coverage;

use arbitrary::Unstructured;

use scope::{Scope, Ty};
use writer::Writer;

pub(crate) use mistake::{Kind, Mistake};

pub(crate) struct Generated {
    pub(crate) program: String,
    /// `program` with the rewrites of decision 2, which must print the same.
    pub(crate) rewritten: String,
    pub(crate) stdin: String,
    /// The known sites `main` prints, in the order it prints them (decision 3).
    pub(crate) known: Vec<Known>,
    /// The lines `@d<n> <value>` that the marked statements of `shapes` and of the functions it
    /// calls print, in the order a run of `shapes` to its end prints them.
    pub(crate) expected: Vec<String>,
    /// The bound sites of `program`, in the order it holds them (property 6).
    pub(crate) bound_sites: Vec<BoundSite>,
    /// At most one mistake of each kind, in kind order (property 7).
    pub(crate) mistakes: Vec<Mistake>,
    /// The program's cost, as decision 5 counts it.
    #[cfg(test)]
    pub(crate) cost: u64,
    /// `program` without its known sites and the forms of `gap_forms`, which the coverage test
    /// walks so they hide nothing.
    #[cfg(test)]
    pub(crate) plain: String,
    /// The rewrites `rewritten` holds.
    #[cfg(test)]
    pub(crate) made: Vec<writer::Rewrite>,
    /// The most blocks open around an `if` of which every branch jumps.
    #[cfg(test)]
    pub(crate) leaving_if_depth: u32,
    /// For each `mut` parameter of `program`: its name, and how many elements its place holds at
    /// least (`Adds::len`), which the text does not say.
    #[cfg(test)]
    pub(crate) mut_lens: Vec<(String, u64)>,
}

/// An argument of a call of a bounded generic function or class, whose parameter is a bare type
/// parameter `T` that no earlier argument names. Replaced by `broken`, it breaks the bound of `T`,
/// and the checker must report that and nothing else (property 6).
pub(crate) struct BoundSite {
    /// Where `program` holds the argument.
    pub(crate) at: std::ops::Range<usize>,
    /// The bound of `T`, as the program writes it.
    pub(crate) bound: String,
    /// The text that does not conform to the bound.
    pub(crate) broken: &'static str,
}

/// The class that `NO_BOUND` builds. It conforms to no interface.
pub(crate) const NO_BOUND_CLASS: &str = "class NoBound {}\n";
/// A value of that class: it breaks every bound but `Display`.
const NO_BOUND: &str = "NoBound()";
/// A value that is no class, which breaks `AnyObject`. It is not `5`: when another argument gives
/// `T` a class, `5` takes that class as its type and the error is E0032, not E0154.
const NOT_A_CLASS: &str = "true";

impl Generated {
    /// `program` with the argument of `site` replaced by `site.broken`, and `NO_BOUND_CLASS`
    /// at its end.
    pub(crate) fn broken(&self, site: &BoundSite) -> String {
        let mut text = self.program.clone();
        text.replace_range(site.at.clone(), site.broken);
        text + NO_BOUND_CLASS
    }
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
const BUDGET: u64 = 1_000_000 + MUT_BUDGET;
/// The part of it that the calls of `mut` calls in `main` may take, apart from `main`'s own.
const MUT_BUDGET: u64 = 100_000;
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
/// The most size of a compound part of an interpolation (decision 5): an array, a class object or
/// an enum value that `{x:#}` can lay out over several lines.
const PART_SIZE: u64 = 16;
/// The most an expression nests.
const MAX_NESTING: u32 = 6;
/// The most chars of a line of the stdin.
const LINE: u64 = 12;
/// The most a statement of `main` costs: inside every loop it may nest in, where the condition of
/// a `while` runs once more than its body.
const STATEMENT_MOST: u64 = (MAX_BOUND + 1) * (MAX_BOUND + 1) * (MAX_BOUND + 1);

/// The remainder, modulo 8, of the XOR of all input bytes that gives `main` the parameter
/// `gap_args`. The gate makes no draw, and the gate writes only known text, which `plain` leaves
/// out. So a gated seed keeps every choice that it had, and its `plain` is the same. Seed 1 has the
/// remainder 1, so `seed_1_program_passes` and `seed_1_rewritten_passes` run the form with the
/// properties 1 to 3. The value 1 is not 5, the gate of `leaves`.
pub(crate) const MAIN_ARGS_GATE: u8 = 1;

/// Accepts every byte string. When `data` runs out, every choice is the smallest valid one.
pub(crate) fn generate(data: &[u8]) -> Generated {
    // The rewrites and the known sites read the bytes from the end, the program from the start.
    let reversed: Vec<u8> = data.iter().rev().copied().collect();
    // The aliases read a stream of their own, so that the choices of every other rule stay where
    // they were, with or without aliases.
    let rotated: Vec<u8> = data.iter().map(|b| b.rotate_left(3)).collect();
    // So do the compound parts of an interpolation: a program with none is the same as it was.
    let mixed: Vec<u8> = data.iter().map(|b| b.rotate_left(5)).collect();
    // The `mut` functions and the `mut` calls choose by their own streams, which stand in for
    // the four above while they are written (`Gen::swap_streams`).
    let places: Vec<u8> = data.iter().map(|b| b.rotate_left(7)).collect();
    let spare: [Vec<u8>; 4] = [1, 2, 4, 6].map(|n| data.iter().map(|b| b.rotate_left(n)).collect());
    let mut g = Gen::new(data);
    g.ru = Unstructured::new(&reversed);
    g.au = Unstructured::new(&rotated);
    g.cu = Unstructured::new(&mixed);
    g.mu = Unstructured::new(&places);
    g.spare = Spare::new(spare.each_ref().map(|bytes| Unstructured::new(bytes)));
    // One program in eight leaves with `let ... else` and `??`. The choice reads every byte, so
    // that it shares no bits with an early choice. The other programs are the same as they were.
    // The value 5 keeps the seeds that the tests pin outside the gate. The weight of the `??`
    // jump in `coalesce` is tuned to the same seeds.
    let fold = data.iter().fold(0, |fold, byte| fold ^ byte);
    g.leaves = fold % 8 == 5;
    // The gate of `main_args` is `MAIN_ARGS_GATE`.
    let (cost, known, expected) = g.program(fold % 8 == MAIN_ARGS_GATE);
    let mistakes = mistake::choose(&g.w.out.sites, &g.w.out.program, data);
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
        expected,
        bound_sites: g.bound_sites,
        mistakes,
        #[cfg(test)]
        cost,
        #[cfg(test)]
        plain,
        #[cfg(test)]
        made: g.w.made,
        #[cfg(test)]
        leaving_if_depth: g.leaving_if_depth,
        #[cfg(test)]
        mut_lens: g.mut_lens,
    }
}

/// What `Gen::swap_streams` swaps in: the streams that stand in for `u`, `ru`, `au` and `cu`, in
/// that order; what the calls of the `mut` calls in `main` may still cost; and how many
/// expressions the `mut` functions and `mut` calls have written.
struct Spare<'a> {
    streams: [Unstructured<'a>; 4],
    budget: u64,
    expressions: u64,
}

impl<'a> Spare<'a> {
    fn new(streams: [Unstructured<'a>; 4]) -> Spare<'a> {
        Spare {
            streams,
            budget: MUT_BUDGET,
            expressions: 0,
        }
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
    /// Inside the argument of `dbg`, whose text the rewrites leave as it is: the line it writes
    /// holds that text.
    frozen: bool,
    /// How many expressions `Gen::expr` has written.
    expressions: u64,
    /// What the calls `main` may still make may cost: the budget less what `main`'s own
    /// statements can cost.
    budget: u64,
    /// The jumps a statement may make here.
    jumps: stmt::Jumps,
    /// How many blocks are open around the point being written.
    blocks: u32,
    /// The most blocks open around an `if` of which every branch jumps (`Gen::if_stmt`).
    #[cfg(test)]
    leaving_if_depth: u32,
    /// The arms of the `if` that `Gen::if_expr` wrote last, as text: the outermost `if` of a
    /// rule, since it ends last.
    #[cfg(test)]
    last_arms: Vec<String>,
    /// The right operand of the equality that `Rule::Equal` wrote last, as text.
    #[cfg(test)]
    last_right: String,
    /// Whether the next expression is in a position with no expected type, where a `u8` must
    /// be one by its own type (`Gen::expr`).
    synth: bool,
    /// Whether the point being written is below a later branch of an `if` that has no expected
    /// type, or below the right operand of an equality whose left operand has none. The checker
    /// may give a value there no expected type, though the generator gives it one.
    untyped_branch: bool,
    /// Whether the next expression is the value of a `let` that writes its type, so that a
    /// `Result` may be built as `Ok(..)` or `Err(..)`, which take their type from it.
    typed: bool,
    /// Whether the variant that `Gen::variant` writes next may be written as `Ok(..)` or `Err(..)`.
    bare: bool,
    /// What chooses the rewrites and the known sites, apart from `u`, so the program is the
    /// same whatever they are.
    ru: Unstructured<'a>,
    /// What chooses the type aliases and where they are used, apart from `u` and `ru`.
    au: Unstructured<'a>,
    /// What chooses whether a part of an interpolation is a compound value, apart from `u`,
    /// `ru` and `au`, so that a part that is not one draws what it drew before.
    cu: Unstructured<'a>,
    /// What chooses where a `mut` function or a `mut` call goes, apart from the streams above.
    mu: Unstructured<'a>,
    /// What stands in for the streams, the budget and the count of expressions while a `mut`
    /// function or a `mut` call is written: no choice of any other rule moves for it.
    spare: Spare<'a>,
    /// How many statements have begun, and how many `mut` calls ran as an argument of another.
    /// A `mut` call reads it to see whether a statement or a `mut` call ran in one of its
    /// arguments: either may change a local that the call passes.
    statements: u64,
    /// Whether a `mut` call that is the argument of another is being written, so no call in it
    /// holds another one.
    nested: bool,
    /// Whether the function being written has a `mut` parameter, so its body holds `mut` calls.
    mut_fn: bool,
    /// Whether the program holds `let ... else`, and `return`, `break` and `continue` after `??`.
    leaves: bool,
    /// Whether an expression of the statement being written so far has an effect or may
    /// panic, so one after it may not move before the statement (decision 2).
    effect: bool,
    /// Whether `main` holds a `return`.
    returns: bool,
    /// The bound sites of `program`, in order.
    bound_sites: Vec<BoundSite>,
    /// Where `program` holds the literal that `literal` wrote last, for `declared`.
    last_literal: Option<std::ops::Range<usize>>,
    /// Where `program` holds the text of the known sites and of the forms of `gap_forms`, in
    /// order.
    #[cfg(test)]
    known_text: Vec<std::ops::Range<usize>>,
    /// See `Generated::mut_lens`.
    #[cfg(test)]
    mut_lens: Vec<(String, u64)>,
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
            frozen: false,
            expressions: 0,
            budget: BUDGET - MUT_BUDGET - FUEL * STATEMENT_MOST,
            jumps: stmt::Jumps::default(),
            blocks: 0,
            #[cfg(test)]
            leaving_if_depth: 0,
            #[cfg(test)]
            last_arms: Vec::new(),
            #[cfg(test)]
            last_right: String::new(),
            synth: false,
            untyped_branch: false,
            typed: false,
            bare: false,
            ru: Unstructured::new(&[]),
            au: Unstructured::new(&[]),
            cu: Unstructured::new(&[]),
            mu: Unstructured::new(&[]),
            spare: Spare::new([(); 4].map(|()| Unstructured::new(&[]))),
            statements: 0,
            nested: false,
            mut_fn: false,
            leaves: false,
            effect: false,
            returns: false,
            bound_sites: Vec::new(),
            last_literal: None,
            #[cfg(test)]
            known_text: Vec::new(),
            #[cfg(test)]
            mut_lens: Vec::new(),
        }
    }

    /// Where `program` ends now.
    fn here(&self) -> usize {
        self.w.out.program.len()
    }

    /// Records a site of property 7 in the function being written.
    fn record(&mut self, kind: Kind, at: std::ops::Range<usize>, error: usize) {
        let function = self.scope.func;
        self.w.record(mistake::MistakeSite {
            kind,
            at,
            error,
            function,
        });
    }

    /// One of `0..n`, `n > 0`, for a rewrite or a known site.
    fn rewrite_pick(&mut self, n: usize) -> usize {
        self.ru.choose_index(n).unwrap_or(0)
    }

    /// One of `0..n`, `n > 0`.
    fn pick(&mut self, n: usize) -> usize {
        self.u.choose_index(n).unwrap_or(0)
    }

    /// One of `0..n`, `n > 0`, for where a `mut` function or a `mut` call goes.
    fn mut_pick(&mut self, n: usize) -> usize {
        self.mu.choose_index(n).unwrap_or(0)
    }

    /// Swaps `u`, `ru`, `au` and `cu` with the spare streams, and the budget and the count of
    /// expressions with their own, and back. A `mut` function and a `mut` call are written
    /// between two swaps, so no other rule's choice moves for them.
    fn swap_streams(&mut self) {
        let spare = &mut self.spare;
        std::mem::swap(&mut self.budget, &mut spare.budget);
        std::mem::swap(&mut self.expressions, &mut spare.expressions);
        let [u, ru, au, cu] = &mut spare.streams;
        std::mem::swap(&mut self.u, u);
        std::mem::swap(&mut self.ru, ru);
        std::mem::swap(&mut self.au, au);
        std::mem::swap(&mut self.cu, cu);
    }

    /// One of `0..n`, `n > 0`, for a type alias.
    fn alias_pick(&mut self, n: usize) -> usize {
        self.au.choose_index(n).unwrap_or(0)
    }

    /// `ty` as a declaration writes it: each optional is `T?`, or now and then `Option<T>`.
    fn spell(&mut self, ty: &Ty) -> String {
        // The text is written first, whatever the alias: its choices are those of a program with
        // no aliases.
        let plain = self.spell_plain(ty);
        self.alias_of(ty).unwrap_or(plain)
    }

    /// `spell` with no alias for `ty` itself.
    fn spell_plain(&mut self, ty: &Ty) -> String {
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

    /// An alias that makes `ty`, as a declaration writes it, now and then: `Al1`, or `Al2<i64>` for
    /// an alias of a type over its parameter. The choice is made only when some alias fits, and
    /// choice 0 is the type itself.
    fn alias_of(&mut self, ty: &Ty) -> Option<String> {
        let fitting: Vec<usize> = (0..self.scope.aliases.len())
            .filter(|&a| {
                let alias = &self.scope.aliases[a];
                alias.plain.as_ref() == Some(ty)
                    || alias.wrap.is_some_and(|wrap| wrap.argument(ty).is_some())
            })
            .collect();
        if fitting.is_empty() {
            return None;
        }
        let pick = self.alias_pick(fitting.len() + 1).checked_sub(1)?;
        let alias = &self.scope.aliases[fitting[pick]];
        let name = alias.name.clone();
        let Some(argument) = alias.wrap.and_then(|wrap| wrap.argument(ty)) else {
            return Some(name);
        };
        Some(format!("{name}<{argument}>"))
    }

    /// The name that writes `ty` before a `(` or a `.`: an alias of exactly that type, now and
    /// then, else `name`.
    fn constructor(&mut self, ty: &Ty, name: String) -> String {
        let fitting: Vec<String> = (self.scope.aliases.iter())
            .filter(|alias| alias.plain.as_ref() == Some(ty))
            .map(|alias| alias.name.clone())
            .collect();
        if fitting.is_empty() {
            return name;
        }
        match self.alias_pick(fitting.len() + 1).checked_sub(1) {
            Some(pick) => fitting[pick].clone(),
            None => name,
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
