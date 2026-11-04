//! Codegen units: the size rule that picks how many units a program builds in, and the
//! assignment of its functions to those units.

#[cfg(test)]
use std::cell::Cell;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::num::NonZeroUsize;
use std::panic::resume_unwind;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use rustc_hash::FxHashSet;

use super::OptLevel;
use crate::mir;

/// The mir size of one unit. It matches 25k raw IR instructions.
///
/// The generated program of `make compile-speed` with 100 copies has 195,729 raw IR instructions
/// in `--emit ir-raw`. Its mir size in all is 38,903. So the value is 25,000 * 38,903 / 195,729,
/// which is 4,969. That program gets 7 units.
pub(crate) const UNIT_SIZE: usize = 4969;

/// The most units a program builds in when the flag is not given.
pub(crate) const MAX_UNITS: usize = 8;

/// The largest mir size of a function that a unit may copy from another unit (C6). The spike
/// copied functions of at most 40 raw IR instructions. A mir size of `UNIT_SIZE` (4,969) matches
/// 25,000 raw IR instructions, so 40 raw IR instructions are 40 * 4,969 / 25,000 = 7.95 mir
/// size, which rounds to 8.
pub(crate) const SMALL: usize = 8;

/// The mir size of `function`: its statements, and one terminator for each block. Every
/// function has a block, so the size is at least 1.
pub(crate) fn size(function: &mir::Function) -> usize {
    function
        .blocks
        .iter()
        .map(|block| block.stmts.len() + 1)
        .sum()
}

/// The number of units for functions of `sizes`: `flag` if it is given, else one unit for each
/// `UNIT_SIZE`, from 1 to `MAX_UNITS`. A unit always holds a function, so the number is at most
/// the function count.
pub(crate) fn count(sizes: &[usize], flag: Option<usize>) -> usize {
    let wanted =
        flag.unwrap_or_else(|| (sizes.iter().sum::<usize>() / UNIT_SIZE).clamp(1, MAX_UNITS));
    wanted.min(sizes.len()).max(1)
}

/// The unit of each function of `sizes`, indexed by the function's id. The largest function goes
/// first, and each goes to the unit with the least size so far; ties go to the lower index.
///
/// # Panics
///
/// If `count` is 0 and `sizes` is not empty: a function needs a unit.
pub(crate) fn assign(sizes: &[usize], count: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    // The sort is stable, so equal sizes stay in id order.
    order.sort_by_key(|&function| Reverse(sizes[function]));
    // The least loaded unit pops first; a tie pops the lower unit index.
    let mut loads: BinaryHeap<Reverse<(usize, usize)>> =
        (0..count).map(|unit| Reverse((0, unit))).collect();
    let mut units = vec![0; sizes.len()];
    for function in order {
        let Reverse((load, unit)) = loads.pop().expect("`count` should be at least 1");
        units[function] = unit;
        loads.push(Reverse((load + sizes[function], unit)));
    }
    units
}

/// The root of the group of `function` in `parent`, by path halving.
fn root(parent: &mut [usize], mut function: usize) -> usize {
    while parent[function] != function {
        parent[function] = parent[parent[function]];
        function = parent[function];
    }
    function
}

/// The group of each function, as the lowest function id in the group. A function with exactly
/// one direct call site, in another function `c`, joins the group of `c` when the two groups
/// together are at most `cap` in size. LLVM can then inline it, and use its constant arguments,
/// because both stay in one unit. `sizes[f]` is the size of `f`. `callees[f]` lists the functions
/// that `f` calls, one entry for each call. Functions are visited in id order, and the result
/// depends only on `sizes` and `callees`. The work is linear in the calls, apart from the path
/// compression of the groups.
pub(crate) fn groups(sizes: &[usize], callees: &[Vec<usize>], cap: usize) -> Vec<usize> {
    // For each function, its number of call sites and the function of the last one.
    let mut calls = vec![(0_usize, 0_usize); sizes.len()];
    for (caller, list) in callees.iter().enumerate() {
        for &callee in list {
            calls[callee] = (calls[callee].0 + 1, caller);
        }
    }
    let mut parent: Vec<usize> = (0..sizes.len()).collect();
    let mut total = sizes.to_vec();
    for (function, &(count, caller)) in calls.iter().enumerate() {
        if count != 1 || caller == function {
            continue;
        }
        let (a, b) = (root(&mut parent, function), root(&mut parent, caller));
        if a != b && total[a] + total[b] <= cap {
            // The lower id becomes the root.
            let (low, high) = (a.min(b), a.max(b));
            parent[high] = low;
            total[low] += total[high];
        }
    }
    (0..sizes.len()).map(|f| root(&mut parent, f)).collect()
}

/// What a unit does with a function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Placement {
    /// The unit defines the function.
    Own,
    /// The unit defines an `available_externally` copy of the function, so that LLVM can inline
    /// it. Another unit defines the function.
    Copy,
    /// The unit declares the function; another unit defines it.
    Other,
}

/// One codegen unit of a build.
#[derive(Clone, Debug)]
pub(crate) struct Unit {
    /// The place of this unit among the `count` units, from 0.
    pub(crate) index: usize,
    /// The number of units in the build.
    pub(crate) count: usize,
    /// What this unit does with each function, indexed by the function's id.
    pub(crate) placement: Vec<Placement>,
}

impl Unit {
    /// The only unit of a build with one unit: it defines all `functions`.
    pub(crate) fn whole(functions: usize) -> Unit {
        Unit {
            index: 0,
            count: 1,
            placement: vec![Placement::Own; functions],
        }
    }

    /// Whether the build has more than one unit.
    pub(crate) fn is_split(&self) -> bool {
        self.count > 1
    }
}

/// The units of `mir` for the flag `flag` (see `count`) and the optimization level `level`. It
/// measures the functions once.
pub(crate) fn split(mir: &mir::Program, flag: Option<usize>, level: OptLevel) -> Vec<Unit> {
    let sizes: Vec<usize> = mir.functions.iter().map(size).collect();
    partition(mir, &sizes, count(&sizes, flag), level)
}

/// The `count` units of `mir`, whose functions have the sizes `sizes`. One unit is `Unit::whole`,
/// with no assignment. With more, and `level` above `O0`, a unit also copies the small functions
/// of the other units that it calls (see `copies`).
///
/// # Panics
///
/// With `count > 1`, if two functions have one name: a split build links by name.
pub(crate) fn partition(
    mir: &mir::Program,
    sizes: &[usize],
    count: usize,
    level: OptLevel,
) -> Vec<Unit> {
    if count == 1 {
        return vec![Unit::whole(mir.functions.len())];
    }
    let mut names = FxHashSet::default();
    for function in &mir.functions {
        assert!(
            names.insert(function.name.as_str()),
            "two functions are named `{}`; a split build links by name",
            function.name
        );
    }
    let callees: Vec<Vec<usize>> = mir.functions.iter().map(mir::Function::callees).collect();
    // A function with one call site goes with its caller, up to one unit's share of the program.
    let group = groups(sizes, &callees, sizes.iter().sum::<usize>() / count);
    // The groups are the items of the greedy assignment, each with its total size.
    let mut totals = vec![0; sizes.len()];
    for (function, &root) in group.iter().enumerate() {
        totals[root] += sizes[function];
    }
    // A unit is never empty: a program of fewer groups than `count` gets that many units.
    let count = count.min(totals.iter().filter(|&&total| total > 0).count());
    if count == 1 {
        return vec![Unit::whole(mir.functions.len())];
    }
    let unit_of_group = assign(&totals, count);
    let owner: Vec<usize> = group.iter().map(|&root| unit_of_group[root]).collect();
    let copied = if level == OptLevel::O0 {
        vec![Vec::new(); count]
    } else {
        copies(sizes, &callees, &owner, count)
    };
    copied
        .into_iter()
        .enumerate()
        .map(|(index, copied)| {
            let mut placement: Vec<Placement> = (owner.iter())
                .map(|&unit| {
                    if unit == index {
                        Placement::Own
                    } else {
                        Placement::Other
                    }
                })
                .collect();
            for function in copied {
                placement[function] = Placement::Copy;
            }
            Unit {
                index,
                count,
                placement,
            }
        })
        .collect()
}

#[cfg(test)]
thread_local! {
    /// How many callee edges `copies` has visited on this thread.
    static COPY_STEPS: Cell<usize> = const { Cell::new(0) };
}

/// For each of the `count` units, the functions it copies (C6). A unit copies a small function
/// (`sizes` at most `SMALL`) that it does not own. It copies the ones that its functions call.
/// It also copies the ones that its copies call. `callees[f]` lists the functions that `f` calls,
/// in the order of its calls, and may repeat one. `unit_of[f]` is the unit that owns `f`. The
/// copies of a unit depend only on these, never on a hash map.
///
/// A function is marked when it is pushed on the unit's work list, so each is expanded at most
/// once for each unit. One list of marks serves every unit and is cleared through the unit's own
/// list. The work is O(units * (small functions + their edges)) plus every edge once.
pub(crate) fn copies(
    sizes: &[usize],
    callees: &[Vec<usize>],
    unit_of: &[usize],
    count: usize,
) -> Vec<Vec<usize>> {
    let mut owned = vec![Vec::new(); count];
    for (function, &unit) in unit_of.iter().enumerate() {
        owned[unit].push(function);
    }
    let mut marked = vec![false; sizes.len()];
    (0..count)
        .map(|unit| {
            let mut list: Vec<usize> = Vec::new();
            // Pushes the small callees of `source` that `unit` does not own and has not met.
            let mut expand = |source: usize, list: &mut Vec<usize>| {
                for &callee in &callees[source] {
                    #[cfg(test)]
                    COPY_STEPS.set(COPY_STEPS.get() + 1);
                    if sizes[callee] <= SMALL && unit_of[callee] != unit && !marked[callee] {
                        marked[callee] = true;
                        list.push(callee);
                    }
                }
            };
            for &function in &owned[unit] {
                expand(function, &mut list);
            }
            // The list grows while it is read: a copy's own small callees follow it.
            let mut at = 0;
            while let Some(&function) = list.get(at) {
                expand(function, &mut list);
                at += 1;
            }
            for &function in &list {
                marked[function] = false;
            }
            list
        })
        .collect()
}

/// How the units of a build run. `Forward` and `Reverse` are for tests of the order of units (the
/// `SISU_UNIT_ORDER` hook): they run every unit on one thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Order {
    /// On several threads.
    Threads,
    /// On one thread, unit 0 first.
    Forward,
    /// On one thread, the last unit first.
    Reverse,
}

impl Order {
    /// The order that `SISU_UNIT_ORDER` names: `forward` or `reverse`. Any other value, or none,
    /// gives `Threads`.
    pub(crate) fn from_env() -> Order {
        match std::env::var("SISU_UNIT_ORDER").as_deref() {
            Ok("forward") => Order::Forward,
            Ok("reverse") => Order::Reverse,
            _ => Order::Threads,
        }
    }
}

/// The number of threads for `count` units: one for each unit, and at most one for each core.
pub(crate) fn threads(count: usize) -> usize {
    count.min(thread::available_parallelism().map_or(1, NonZeroUsize::get))
}

/// Runs `work` for each unit from 0 to `count - 1`, and returns the results in unit order,
/// whatever the order in which the units end. With one unit, `work` runs on the calling thread.
/// Else every worker thread has a stack of `STACK_SIZE` and the name `codegen`. With `Threads`, up
/// to `threads` workers take the units one by one. With `Forward` or `Reverse`, one worker runs
/// every unit in that order.
///
/// A thread that the system cannot start is no error. Then no more threads start, and the calling
/// thread, which has the stack of the compiler, takes the units that are left.
///
/// # Panics
///
/// If `work` panics, the panic goes on in the caller. A panic on a worker names the thread
/// `codegen`.
pub(crate) fn schedule<T: Send>(
    count: usize,
    threads: usize,
    order: Order,
    work: impl Fn(usize) -> T + Sync,
) -> Vec<T> {
    if count == 1 {
        return vec![work(0)];
    }
    // Every worker takes the next turn from `next`. `Reverse` counts the turns from the last unit.
    let next = AtomicUsize::new(0);
    let workers = if order == Order::Threads {
        threads.min(count).max(1)
    } else {
        1
    };
    let take_turns = || {
        let mut done = Vec::new();
        loop {
            let turn = next.fetch_add(1, Ordering::Relaxed);
            if turn >= count {
                break done;
            }
            let unit = if order == Order::Reverse {
                count - 1 - turn
            } else {
                turn
            };
            done.push((unit, work(unit)));
        }
    };
    let mut done: Vec<(usize, T)> = thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 0..workers {
            let spawned = thread::Builder::new()
                .name("codegen".to_owned())
                .stack_size(crate::STACK_SIZE)
                .spawn_scoped(scope, take_turns);
            match spawned {
                Ok(handle) => handles.push(handle),
                Err(_) => break,
            }
        }
        // The calling thread takes a turn too only when a worker did not start.
        let mut done = if handles.len() < workers {
            take_turns()
        } else {
            Vec::new()
        };
        for handle in handles {
            done.extend(handle.join().unwrap_or_else(|panic| resume_unwind(panic)));
        }
        done
    });
    done.sort_unstable_by_key(|&(unit, _)| unit);
    done.into_iter().map(|(_, result)| result).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::front_end::{PRELUDE_SOURCE, prelude_program};
    use crate::mir::tests::assert_near_linear;
    use crate::parser::parse_ok;
    use std::fmt::Write as _;
    use std::fs;
    use std::sync::{Condvar, Mutex};
    use std::time::Duration;

    /// The mir of `source`, built as `codegen` builds it.
    fn mir_of(source: &str) -> mir::Program {
        let program = parse_ok(source);
        let (program, diagnostics) =
            crate::check::check(&program, &prelude_program(PRELUDE_SOURCE), source, &[]);
        let program = program.unwrap_or_else(|| panic!("{diagnostics:?}"));
        let program = crate::mono::instantiate(program).unwrap_or_else(|found| panic!("{found:?}"));
        mir::build(&program)
    }

    /// The sizes of the functions of `mir`.
    fn sizes_of(mir: &mir::Program) -> Vec<usize> {
        mir.functions.iter().map(size).collect()
    }

    /// A program of six functions.
    const SIX: &str = "fn a(n: i64) -> i64 { n + 1 }
fn b(n: i64) -> i64 { a(n) + 2 }
fn c(n: i64) -> i64 { b(n) + 3 }
fn d(n: i64) -> i64 { c(n) + 4 }
fn e(n: i64) -> i64 { d(n) + 5 }
fn main() { print(e(1)) }";

    #[test]
    fn bench_programs_should_stay_one_module() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
        let mut paths = vec![format!("{root}/selfhost/lexer.sisu")];
        let mut found = [0, 0];
        for (at, dir) in [format!("{root}/bench"), format!("{root}/bench/features")]
            .iter()
            .enumerate()
        {
            for entry in fs::read_dir(dir).expect("the bench directory exists") {
                let path = entry
                    .expect("the entry is readable")
                    .path()
                    .join("main.sisu");
                if path.is_file() {
                    paths.push(path.display().to_string());
                    found[at] += 1;
                }
            }
        }
        // The selfhost_lexer benchmark runs `selfhost/lexer.sisu`, and has no `main.sisu`.
        assert_eq!(found, [8, 11], "the benchmark programs: {paths:?}");
        for path in paths {
            let mir = mir_of(&fs::read_to_string(&path).expect("the program is readable"));
            let sizes: Vec<usize> = mir.functions.iter().map(size).collect();
            assert_eq!(
                count(&sizes, None),
                1,
                "{path}: {} in all",
                sizes.iter().sum::<usize>()
            );
        }
    }

    /// `main` of `SIX`, with `blocks` in place of its body.
    fn function_of(blocks: Vec<mir::Block>) -> mir::Function {
        let mut function = mir_of(SIX)
            .functions
            .into_iter()
            .find(|function| function.name == "main")
            .expect("the program has a main");
        function.blocks = blocks;
        function
    }

    fn statement() -> mir::Statement {
        mir::Statement::Assign {
            dest: None,
            value: mir::Rvalue::Use(mir::Operand::Const(mir::Const::Bool(true))),
        }
    }

    fn block(statements: usize, term: mir::Terminator) -> mir::Block {
        mir::Block {
            stmts: (0..statements).map(|_| statement()).collect(),
            term,
        }
    }

    #[test]
    fn size_should_count_each_statement_and_one_terminator_per_block() {
        let ret = || mir::Terminator::Return(None);
        let go = |to| mir::Terminator::Goto(mir::BlockId(to));
        // A block of 4 statements and its terminator.
        assert_eq!(size(&function_of(vec![block(4, ret())])), 5);
        // Blocks of 2, 0 and 3 statements: 5 statements and 3 terminators.
        let three = vec![block(2, go(1)), block(0, go(2)), block(3, ret())];
        assert_eq!(size(&function_of(three)), 8);
        // The smallest function: one block, no statement.
        assert_eq!(size(&function_of(vec![block(0, ret())])), 1);
    }

    #[test]
    fn count_should_split_at_the_limit() {
        assert_eq!(count(&vec![1; 2 * UNIT_SIZE - 1], None), 1);
        assert_eq!(count(&vec![1; 2 * UNIT_SIZE], None), 2);
        assert_eq!(count(&vec![UNIT_SIZE; 100], None), 8);
    }

    #[test]
    fn count_should_be_one_with_the_flag_1() {
        assert_eq!(count(&vec![UNIT_SIZE; 100], Some(1)), 1);
    }

    #[test]
    fn count_should_never_pass_the_function_count() {
        assert_eq!(count(&[5, 5, 5], Some(64)), 3);
        assert_eq!(count(&[UNIT_SIZE * 20], None), 1);
    }

    #[test]
    fn assign_should_put_the_largest_first_on_the_lightest_unit() {
        assert_eq!(assign(&[5, 3, 3, 1], 2), [0, 1, 1, 0]);
        assert_eq!(assign(&[2, 2], 2), [0, 1]);
        // The sizes are not in order: the largest still goes first.
        assert_eq!(assign(&[1, 3, 3, 5], 2), [0, 1, 1, 0]);
    }

    #[test]
    fn assign_should_give_every_unit_a_function() {
        for sizes in [vec![1; 10], vec![9, 1, 1]] {
            let count = sizes.len();
            let units = assign(&sizes, count);
            for unit in 0..count {
                assert!(
                    units.contains(&unit),
                    "{sizes:?}: unit {unit} is empty in {units:?}"
                );
            }
        }
    }

    #[test]
    fn partition_should_own_each_function_in_one_unit() {
        let mir = mir_of(SIX);
        let units = partition(&mir, &sizes_of(&mir), 3, OptLevel::O0);
        assert_eq!(units.len(), 3);
        for (index, unit) in units.iter().enumerate() {
            assert_eq!((unit.index, unit.count), (index, 3));
            assert!(unit.is_split(), "unit {index} of 3");
            assert_eq!(unit.placement.len(), mir.functions.len());
        }
        for function in 0..mir.functions.len() {
            let owners = units
                .iter()
                .filter(|unit| unit.placement[function] == Placement::Own)
                .count();
            let others = units
                .iter()
                .filter(|unit| unit.placement[function] == Placement::Other)
                .count();
            assert_eq!((owners, others), (1, 2), "function {function}");
        }
    }

    #[test]
    #[should_panic(expected = "two functions are named")]
    fn partition_should_reject_two_functions_with_one_name() {
        let mut mir = mir_of(SIX);
        mir.functions[1].name = mir.functions[0].name.clone();
        partition(&mir, &sizes_of(&mir), 2, OptLevel::O0);
    }

    #[test]
    fn whole_should_own_every_function_in_one_unit() {
        let unit = Unit::whole(4);
        assert_eq!((unit.index, unit.count, unit.is_split()), (0, 1, false));
        assert_eq!(unit.placement, [Placement::Own; 4]);
    }

    /// Adds the caller to those that arrived, and waits until two have. Returns whether two did
    /// within `timeout`. The timeout is a guard against a hang, never a measure of speed.
    fn rendezvous(arrived: &(Mutex<usize>, Condvar), timeout: Duration) -> bool {
        let (lock, condvar) = arrived;
        let mut count = lock.lock().expect("the lock should not be poisoned");
        *count += 1;
        condvar.notify_all();
        let (count, _) = condvar
            .wait_timeout_while(count, timeout, |count| *count < 2)
            .expect("the lock should not be poisoned");
        *count == 2
    }

    #[test]
    fn units_should_run_at_the_same_time() {
        let arrived = (Mutex::new(0), Condvar::new());
        let met = schedule(2, 2, Order::Threads, |_| {
            rendezvous(&arrived, Duration::from_secs(10))
        });
        assert_eq!(met, [true, true]);
    }

    #[test]
    fn one_thread_should_not_meet_the_other_unit() {
        let arrived = (Mutex::new(0), Condvar::new());
        let met = schedule(2, 1, Order::Threads, |_| {
            rendezvous(&arrived, Duration::from_millis(100))
        });
        assert_eq!(met, [false, true]);
    }

    #[test]
    fn schedule_should_return_results_in_unit_order() {
        for order in [Order::Threads, Order::Forward, Order::Reverse] {
            let log = Mutex::new(Vec::new());
            let results = schedule(9, 4, order, |unit| {
                log.lock()
                    .expect("the lock should not be poisoned")
                    .push(unit);
                unit
            });
            assert_eq!(results, (0..9).collect::<Vec<_>>(), "{order:?}");
            let log = log.into_inner().expect("the lock should not be poisoned");
            match order {
                Order::Threads => {
                    let mut sorted = log;
                    sorted.sort_unstable();
                    assert_eq!(sorted, (0..9).collect::<Vec<_>>(), "{order:?}");
                }
                Order::Forward => assert_eq!(log, (0..9).collect::<Vec<_>>(), "{order:?}"),
                Order::Reverse => assert_eq!(log, (0..9).rev().collect::<Vec<_>>(), "{order:?}"),
            }
        }
    }

    #[test]
    fn schedule_should_run_one_unit_on_the_calling_thread() {
        let ids = schedule(1, 4, Order::Threads, |_| thread::current().id());
        assert_eq!(ids, [thread::current().id()]);
    }

    #[test]
    fn threads_should_be_at_least_one_and_at_most_the_unit_count() {
        assert_eq!(threads(1), 1);
        assert!((1..=64).all(|count| (1..=count).contains(&threads(count))));
    }

    #[test]
    fn one_worker_should_run_the_units_of_forward_and_reverse_in_turn() {
        for (order, want) in [
            (Order::Forward, [false, true]),
            (Order::Reverse, [true, false]),
        ] {
            let arrived = (Mutex::new(0), Condvar::new());
            let met = schedule(2, 2, order, |_| {
                rendezvous(&arrived, Duration::from_millis(100))
            });
            assert_eq!(met, want, "{order:?}");
        }
    }

    /// Recurses with a 64 KiB array in each of 65 frames: about 4 MiB of stack. The 2 MiB stack of
    /// a default thread overflows.
    #[expect(
        clippy::large_stack_arrays,
        reason = "the test needs a frame that holds much stack"
    )]
    fn deep(frames: usize) -> usize {
        let array = std::hint::black_box([1_u8; 64 << 10]);
        let below = if frames == 0 { 0 } else { deep(frames - 1) };
        below + usize::from(std::hint::black_box(array)[0])
    }

    #[test]
    fn workers_should_have_the_stack_of_the_compiler() {
        assert_eq!(schedule(2, 2, Order::Threads, |_| deep(64)), [65, 65]);
    }

    /// The copy list of each unit, sorted, so that a test does not depend on the order in which
    /// `copies` finds the functions.
    fn sorted(mut copied: Vec<Vec<usize>>) -> Vec<Vec<usize>> {
        for list in &mut copied {
            list.sort_unstable();
        }
        copied
    }

    /// A function that is small.
    const S: usize = SMALL;
    /// A function that is not.
    const L: usize = SMALL + 1;

    #[test]
    fn copies_should_close_over_small_callees() {
        // a -> b -> c -> d, each in its own unit; b and c are small, d is not.
        let callees = [vec![1], vec![2], vec![3], vec![]];
        let copied = sorted(copies(&[S, S, S, L], &callees, &[0, 1, 2, 3], 4));
        // `a` copies `b` and, through `b`, `c`, but not the large `d`. `b` copies `c`. No unit
        // copies a function it owns.
        assert_eq!(copied, [vec![1, 2], vec![2], vec![], vec![]]);
    }

    #[test]
    fn copies_should_end_on_a_cycle() {
        // a -> b <-> c, in three units, all small.
        let callees = [vec![1], vec![2], vec![1]];
        let copied = sorted(copies(&[S, S, S], &callees, &[0, 1, 2], 3));
        assert_eq!(copied, [vec![1, 2], vec![2], vec![1]]);
    }

    #[test]
    fn copies_should_expand_a_shared_callee_once() {
        // a -> b, c; b -> d; c -> d; d -> e; each in its own unit, all small.
        let callees = [vec![1, 2], vec![3], vec![3], vec![4], vec![]];
        COPY_STEPS.set(0);
        let copied = copies(&[S; 5], &callees, &[0, 1, 2, 3, 4], 5);
        assert_eq!(copied[0], [1, 2, 3, 4]);
        // The steps of each unit: `a` 5 (a->b, a->c, b->d, c->d, d->e), `b` 2 (b->d, d->e),
        // `c` 2 (c->d, d->e), `d` 1 (d->e), `e` 0. An expansion for each push gives 11.
        assert_eq!(COPY_STEPS.get(), 10);
    }

    /// The steps `copies` takes for `callees`, whose functions are all small, in 64 units round
    /// robin.
    fn copy_steps(callees: &[Vec<usize>]) -> usize {
        let unit_of: Vec<usize> = (0..callees.len()).map(|function| function % 64).collect();
        COPY_STEPS.set(0);
        copies(&vec![S; callees.len()], callees, &unit_of, 64);
        COPY_STEPS.get()
    }

    #[test]
    fn copies_should_take_linear_steps() {
        // A chain: each function calls the next.
        let chain = |n: usize| -> Vec<Vec<usize>> {
            (0..n)
                .map(|f| if f + 1 < n { vec![f + 1] } else { vec![] })
                .collect()
        };
        assert_near_linear(
            copy_steps(&chain(200)),
            copy_steps(&chain(400)),
            copy_steps(&chain(600)),
        );
        // Layers of two: each function calls both functions of the next layer. A function that
        // is expanded once for each caller makes 2^layers steps.
        let layers = |n: usize| -> Vec<Vec<usize>> {
            (0..n)
                .map(|f| {
                    let next = (f / 2 + 1) * 2;
                    if next < n {
                        vec![next, next + 1]
                    } else {
                        vec![]
                    }
                })
                .collect()
        };
        assert_near_linear(
            copy_steps(&layers(200)),
            copy_steps(&layers(400)),
            copy_steps(&layers(600)),
        );
    }

    /// A chain of 16,000 small functions in 64 units makes about a million steps. A step counter
    /// does not see a scan of the unit's list for a repeat in place of its mark: each edge still
    /// takes one step, and the scan makes the work quadratic. A chain of 1,600 already takes
    /// 13 times longer with the scan (0.134 s against 0.010 s), so this input is 10 times that
    /// size. Alone, the run takes 0.04 s. With the scan it takes 12.8 s. Both were measured at a
    /// load average of 8 to 9, in the default profile.
    #[test]
    fn copies_should_finish_a_long_chain_in_linear_time() {
        let n = 16_000;
        let callees: Vec<Vec<usize>> = (0..n)
            .map(|f| if f + 1 < n { vec![f + 1] } else { vec![] })
            .collect();
        let unit_of: Vec<usize> = (0..n).map(|function| function % 64).collect();
        let copied = copies(&vec![S; n], &callees, &unit_of, 64);
        // Unit 0 owns the functions 0, 64, 128 and so on, and copies all the others after 0.
        let owned_after_0 = (n - 1) / 64;
        assert_eq!(copied[0].len(), n - 1 - owned_after_0);
    }

    #[test]
    fn partition_should_copy_nothing_at_o0() {
        let mir = mir_of(CALLS_STEP);
        let units = partition(&mir, &sizes_of(&mir), 2, OptLevel::O0);
        assert!(
            units
                .iter()
                .all(|unit| !unit.placement.contains(&Placement::Copy)),
            "{units:?}"
        );
    }

    /// A `main` that calls the small function `step`.
    const CALLS_STEP: &str = "fn step(i: i64) -> i64 { i * 3 + 1 }
fn main() {
    let mut sum = 0
    for i in 0..10 {
        sum += step(i) % 7
    }
    print(sum)
}";

    #[test]
    fn partition_should_copy_a_small_callee_at_o2_and_o3() {
        let mir = mir_of(CALLS_STEP);
        let at = |name: &str| {
            (mir.functions.iter().position(|f| f.name == name)).expect("the program has it")
        };
        let (step, main) = (at("step"), at("main"));
        for level in [OptLevel::O2, OptLevel::O3] {
            let units = partition(&mir, &sizes_of(&mir), 2, level);
            let own = (units
                .iter()
                .position(|u| u.placement[main] == Placement::Own))
            .expect("a unit owns `main`");
            assert_eq!(units[own].placement[step], Placement::Copy);
            assert_eq!(units[1 - own].placement[step], Placement::Own);
        }
    }

    #[test]
    fn groups_should_join_a_one_call_site_function_to_its_caller() {
        // main -> a -> b, each called once: one group, named by its lowest id.
        assert_eq!(
            groups(&[1, 1, 1], &[vec![1], vec![2], vec![]], 100),
            [0, 0, 0]
        );
        // The same chain with the ids reversed: 0 is called by 3, and 3 by 2, and 2 by 1.
        let callees = [vec![], vec![2], vec![3], vec![0]];
        assert_eq!(groups(&[1; 4], &callees, 100), [0, 0, 0, 0]);
    }

    #[test]
    fn groups_should_keep_a_function_with_two_call_sites_apart() {
        // main calls `b` twice.
        assert_eq!(groups(&[1, 1], &[vec![1, 1], vec![]], 100), [0, 1]);
        // `c` is called by `a` and by `b`.
        assert_eq!(groups(&[1; 3], &[vec![2], vec![2], vec![]], 100), [0, 1, 2]);
    }

    #[test]
    fn groups_should_stop_at_the_cap() {
        // main -> a -> b -> c, each of size 2 and called once, with a cap of 6. `a` and `b` join
        // `main` (4, then 6). `c` would make 8, so it stays out.
        let callees = [vec![1], vec![2], vec![3], vec![]];
        assert_eq!(groups(&[2; 4], &callees, 6), [0, 0, 0, 3]);
    }

    #[test]
    fn groups_should_end_on_a_cycle() {
        // a and b call only each other.
        assert_eq!(groups(&[1, 1], &[vec![1], vec![0]], 100), [0, 0]);
    }

    #[test]
    fn groups_should_not_join_a_self_call() {
        // The one call site of `b` is in `b`, so `b` has no caller and `main` does not call it.
        assert_eq!(groups(&[1, 1], &[vec![], vec![1]], 100), [0, 1]);
        // `b` is called by `main` and by itself: two call sites.
        assert_eq!(groups(&[1, 1], &[vec![1], vec![1]], 100), [0, 1]);
    }

    /// A program of ten functions: `main` calls `helper` once, and eight functions that nothing
    /// calls are large, so that a unit of one eighth of the program holds `main` and `helper`.
    fn one_call_site_program() -> String {
        let mut source =
            "fn helper(n: i64) -> i64 { n + 1 }\nfn main() { print(helper(1)) }\n".to_owned();
        for pad in 0..8 {
            let sum = (0..20)
                .map(|k| format!("n + {k}"))
                .collect::<Vec<_>>()
                .join(" + ");
            let _ = writeln!(source, "fn pad{pad}(n: i64) -> i64 {{ {sum} }}");
        }
        source
    }

    #[test]
    fn partition_should_put_a_one_call_site_function_with_its_caller() {
        let mir = mir_of(&one_call_site_program());
        let sizes = sizes_of(&mir);
        let at = |name: &str| {
            (mir.functions.iter().position(|f| f.name == name)).expect("the program has it")
        };
        let (helper, main) = (at("helper"), at("main"));
        assert_eq!(mir.functions.len(), 10);
        assert!(
            sizes[helper] + sizes[main] <= sizes.iter().sum::<usize>() / 8,
            "the premise: a unit's share holds both: {sizes:?}"
        );
        for (level, name) in [(OptLevel::O0, "O0"), (OptLevel::O2, "O2")] {
            let units = partition(&mir, &sizes, 8, level);
            let owner = |function: usize| {
                (units.iter())
                    .position(|unit| unit.placement[function] == Placement::Own)
                    .expect("a unit owns it")
            };
            assert_eq!(owner(helper), owner(main), "{name}");
            // Every unit still owns a function.
            for unit in &units {
                assert!(unit.placement.contains(&Placement::Own), "{unit:?}");
            }
        }
    }
}
