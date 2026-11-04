//! Codegen units: the size rule that picks how many units a program builds in, and the
//! assignment of its functions to those units.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::num::NonZeroUsize;
use std::panic::resume_unwind;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use rustc_hash::FxHashSet;

use crate::mir;

/// The mir size of one unit. It matches 25k raw IR instructions.
///
/// The generated program of `make compile-speed` with 100 copies has 195,729 raw IR instructions
/// in `--emit ir-raw`. Its mir size in all is 38,903. So the value is 25,000 * 38,903 / 195,729,
/// which is 4,969. That program gets 7 units.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "Task 4 calls `count`, which reads it")
)]
pub(crate) const UNIT_SIZE: usize = 4969;

/// The most units a program builds in when the flag is not given.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "Task 4 calls `count`, which reads it")
)]
pub(crate) const MAX_UNITS: usize = 8;

/// The mir size of `function`: its statements, and one terminator for each block. Every
/// function has a block, so the size is at least 1.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "Task 4 measures the functions with it")
)]
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
#[cfg_attr(not(test), expect(dead_code, reason = "Task 4 calls it"))]
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
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "Task 4 calls `partition`, which calls it")
)]
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

/// What a unit does with a function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "Task 4 builds `Other` in `partition`")
)]
pub(crate) enum Placement {
    /// The unit defines the function.
    Own,
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

/// The `count` units of `mir`.
///
/// # Panics
///
/// With `count > 1`, if two functions have one name: a split build links by name.
#[cfg_attr(not(test), expect(dead_code, reason = "Task 4 calls it"))]
pub(crate) fn partition(mir: &mir::Program, count: usize) -> Vec<Unit> {
    if count > 1 {
        let mut names = FxHashSet::default();
        for function in &mir.functions {
            assert!(
                names.insert(function.name.as_str()),
                "two functions are named `{}`; a split build links by name",
                function.name
            );
        }
    }
    let sizes: Vec<usize> = mir.functions.iter().map(size).collect();
    let owner = assign(&sizes, count);
    (0..count)
        .map(|index| Unit {
            index,
            count,
            placement: owner
                .iter()
                .map(|&unit| {
                    if unit == index {
                        Placement::Own
                    } else {
                        Placement::Other
                    }
                })
                .collect(),
        })
        .collect()
}

/// How the units of a build run. `Forward` and `Reverse` are for tests of the order of units (the
/// `SISU_UNIT_ORDER` hook): they run every unit on one thread.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "Task 4 reads the order and gives it to `schedule`"
    )
)]
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
    #[expect(dead_code, reason = "Task 4 calls it")]
    pub(crate) fn from_env() -> Order {
        match std::env::var("SISU_UNIT_ORDER").as_deref() {
            Ok("forward") => Order::Forward,
            Ok("reverse") => Order::Reverse,
            _ => Order::Threads,
        }
    }
}

/// The number of threads for `count` units: one for each unit, and at most one for each core.
#[cfg_attr(not(test), expect(dead_code, reason = "Task 4 calls it"))]
pub(crate) fn threads(count: usize) -> usize {
    count.min(thread::available_parallelism().map_or(1, NonZeroUsize::get))
}

/// Runs `work` for each unit from 0 to `count`, and returns the results in unit order, whatever
/// the order in which the units end. With one unit, `work` runs on the calling thread. Else a
/// worker has the stack of the compiler. With `Threads`, up to `threads` workers take the units
/// one by one. With `Forward` or `Reverse`, one worker runs every unit in that order.
///
/// # Panics
///
/// If `work` panics, the panic goes on in the caller. If the system cannot start a thread.
#[cfg_attr(not(test), expect(dead_code, reason = "Task 4 calls it"))]
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
    let mut done: Vec<(usize, T)> = thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                thread::Builder::new()
                    .stack_size(crate::STACK_SIZE)
                    .spawn_scoped(scope, || {
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
                    })
                    .expect("the thread should start")
            })
            .collect();
        (handles.into_iter())
            .flat_map(|handle| handle.join().unwrap_or_else(|panic| resume_unwind(panic)))
            .collect()
    });
    done.sort_unstable_by_key(|&(unit, _)| unit);
    done.into_iter().map(|(_, result)| result).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::front_end::{PRELUDE_SOURCE, prelude_program};
    use crate::parser::parse_ok;
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
        let units = partition(&mir, 3);
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
        partition(&mir, 2);
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
    fn threads_should_be_at_most_the_unit_count_and_the_cores() {
        assert_eq!(threads(1), 1);
        assert_eq!(
            threads(usize::MAX),
            thread::available_parallelism().map_or(1, NonZeroUsize::get)
        );
    }
}
