//! Codegen units: the size rule that picks how many units a program builds in, and the
//! assignment of its functions to those units.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use rustc_hash::FxHashSet;

use crate::mir;

/// The mir size of one unit. It matches 25k raw IR instructions. The generated program of
/// `make compile-speed` with 100 copies has 195,729 raw IR instructions in `--emit ir-raw` and a
/// mir size of 38,903 in all: 25,000 * 38,903 / 195,729 is 4,969. That program gets 7 units.
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
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "Task 2 calls `partition`, which calls it")
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
        let Some(Reverse((load, unit))) = loads.pop() else {
            break;
        };
        units[function] = unit;
        loads.push(Reverse((load + sizes[function], unit)));
    }
    units
}

/// What a unit does with a function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), expect(dead_code, reason = "Task 2 reads it"))]
pub(crate) enum Placement {
    /// The unit defines the function.
    Own,
    /// The unit declares the function; another unit defines it.
    Other,
}

/// One codegen unit of a build.
#[derive(Clone, Debug)]
#[cfg_attr(not(test), expect(dead_code, reason = "Task 2 builds the units"))]
pub(crate) struct Unit {
    /// The place of this unit among the `count` units, from 0.
    pub(crate) index: usize,
    /// The number of units in the build.
    pub(crate) count: usize,
    /// What this unit does with each function, indexed by the function's id.
    pub(crate) placement: Vec<Placement>,
}

#[cfg_attr(not(test), expect(dead_code, reason = "Task 2 calls them"))]
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
#[cfg_attr(not(test), expect(dead_code, reason = "Task 2 calls it"))]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::front_end::{PRELUDE_SOURCE, prelude_program};
    use crate::parser::parse_ok;
    use std::fs;

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

    #[test]
    fn count_should_split_at_the_limit() {
        assert_eq!(count(&vec![1; 2 * UNIT_SIZE - 1], None), 1);
        assert_eq!(count(&vec![1; 2 * UNIT_SIZE], None), 2);
        assert_eq!(count(&vec![UNIT_SIZE; 100], None), MAX_UNITS);
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
}
