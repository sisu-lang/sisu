//! `--timings`: the wall time of each stage of one compile, as the `timings:` lines that the
//! reporter prints to stderr.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::lexer::{Lexed, TokenKind};

/// One stage of the compiler, as the glossary lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Stage {
    Lex,
    Parse,
    /// Not one of the glossary's stages: the lex and parse of the prelude.
    Prelude,
    Check,
    Mono,
    Mir,
    Codegen,
    Optimize,
    Object,
    Link,
}

impl Stage {
    /// The name on the `timings:` line.
    fn name(self) -> &'static str {
        match self {
            Stage::Lex => "lex",
            Stage::Parse => "parse",
            Stage::Prelude => "prelude",
            Stage::Check => "check",
            Stage::Mono => "mono",
            Stage::Mir => "mir",
            Stage::Codegen => "codegen",
            Stage::Optimize => "optimize",
            Stage::Object => "object",
            Stage::Link => "link",
        }
    }
}

/// The time of each stage that ran. An off `Timings` reads no clock and counts nothing.
pub(crate) struct Timings(Option<Recording>);

struct Recording {
    start: Instant,
    stages: Vec<(Stage, Duration)>,
    /// The lines and the tokens of the user's file, unless the lex stopped.
    counts: Option<(usize, usize)>,
    /// The number of codegen units, once `merge_units` has run.
    units: Option<usize>,
}

impl Timings {
    /// A `Timings` that records nothing.
    pub(crate) fn off() -> Timings {
        Timings(None)
    }

    /// A `Timings` whose total starts now.
    pub(crate) fn start() -> Timings {
        Timings(Some(Recording {
            start: Instant::now(),
            stages: Vec::new(),
            counts: None,
            units: None,
        }))
    }

    /// A recorder for one codegen unit, on when `self` is on. It runs on the unit's thread, and
    /// `merge_units` takes its stages back.
    #[cfg_attr(not(test), expect(dead_code, reason = "Task 4 gives one to each unit"))]
    pub(crate) fn for_unit(&self) -> Timings {
        Timings(self.0.is_some().then(|| Recording {
            start: Instant::now(),
            stages: Vec::new(),
            counts: None,
            units: None,
        }))
    }

    /// Records the stages of `units`: for each stage that any unit recorded, in stage order, the
    /// longest time that one unit took for that stage. This is not the elapsed time of the stage:
    /// a worker can run several units one after the other. It also records the number of units
    /// for the `codegen units` line.
    #[cfg_attr(not(test), expect(dead_code, reason = "Task 4 merges the units"))]
    pub(crate) fn merge_units(&mut self, units: Vec<Timings>) {
        if let Some(recording) = &mut self.0 {
            recording.units = Some(units.len());
            let mut slowest: BTreeMap<Stage, Duration> = BTreeMap::new();
            for (stage, time) in units
                .into_iter()
                .filter_map(|unit| unit.0)
                .flat_map(|r| r.stages)
            {
                let longest = slowest.entry(stage).or_default();
                *longest = (*longest).max(time);
            }
            recording.stages.extend(slowest);
        }
    }

    /// Runs `work` and records its time as `stage`.
    pub(crate) fn time<T>(&mut self, stage: Stage, work: impl FnOnce() -> T) -> T {
        let Some(recording) = &mut self.0 else {
            return work();
        };
        let begin = Instant::now();
        let out = work();
        recording.stages.push((stage, begin.elapsed()));
        out
    }

    /// Records the lines and the tokens of the user's file. `counts` runs only when recording.
    pub(crate) fn count(&mut self, counts: impl FnOnce() -> (usize, usize)) {
        if let Some(recording) = &mut self.0 {
            recording.counts = Some(counts());
        }
    }

    /// The `timings:` lines, or none when off. The total ends now.
    pub(crate) fn report(self) -> Vec<String> {
        match self.0 {
            Some(r) => render(&r.stages, r.start.elapsed(), r.counts, r.units),
            None => Vec::new(),
        }
    }
}

/// The lines of `source` and the tokens of `lexed` without `Newline` and `Eof`.
pub(crate) fn counts(source: &str, lexed: &Lexed<'_>) -> (usize, usize) {
    let tokens = (lexed.tokens.iter())
        .filter(|t| !matches!(t.kind, TokenKind::Newline | TokenKind::Eof))
        .count();
    (source.lines().count(), tokens)
}

/// A line for each of `stages`, then the `codegen units` line when `units` is known, then `other`
/// and `total`. Lex, parse and check have throughput when `counts` is known; lex also gives tokens.
fn render(
    stages: &[(Stage, Duration)],
    total: Duration,
    counts: Option<(usize, usize)>,
    units: Option<usize>,
) -> Vec<String> {
    let percent = |time: Duration| {
        if total.is_zero() {
            0.0
        } else {
            100.0 * time.as_secs_f64() / total.as_secs_f64()
        }
    };
    let line = |name: &str, time: Duration| {
        format!(
            "timings: {name:<8}{:>9.1} ms{:>7.1}%",
            time.as_secs_f64() * 1000.0,
            percent(time)
        )
    };
    let mut lines: Vec<String> = stages
        .iter()
        .map(|&(stage, time)| {
            let throughput = match (counts, stage) {
                (Some((line_count, tokens)), Stage::Lex) => format!(
                    "  {line_count} lines  {} lines/s  {tokens} tokens  {} tokens/s",
                    rate(line_count, time),
                    rate(tokens, time)
                ),
                (Some((line_count, _)), Stage::Parse | Stage::Check) => {
                    format!("  {line_count} lines  {} lines/s", rate(line_count, time))
                }
                _ => String::new(),
            };
            line(stage.name(), time) + &throughput
        })
        .collect();
    if let Some(count) = units {
        lines.push(format!("timings: codegen units: {count}"));
    }
    let spent: Duration = stages.iter().map(|&(_, time)| time).sum();
    lines.push(line("other", total.saturating_sub(spent)));
    lines.push(line("total", total));
    lines
}

/// `count` per second of `time`: `-` when either is zero, and an integer below 1000.
/// Else the rate has the first of `k`, `M` and `G` whose value, rounded to one decimal, is below
/// 1000.
#[expect(
    clippy::cast_precision_loss,
    reason = "a count of a source file is far below 2^52, and the rate has one decimal"
)]
fn rate(count: usize, time: Duration) -> String {
    if count == 0 || time.is_zero() {
        return "-".to_string();
    }
    let per_second = count as f64 / time.as_secs_f64();
    let whole = per_second.round();
    if whole < 1000.0 {
        return format!("{whole:.0}");
    }
    for (divisor, suffix) in [(1e3, 'k'), (1e6, 'M')] {
        let text = format!("{:.1}", per_second / divisor);
        // The text decides: "999.9" has 5 characters, and "1000.0" has 6.
        if text.len() < 6 {
            return format!("{text}{suffix}");
        }
    }
    format!("{:.1}G", per_second / 1e9)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn report_should_add_up_to_the_total() {
        let ms = Duration::from_millis;
        let stages = [
            (Stage::Lex, ms(1)),
            (Stage::Parse, ms(2)),
            (Stage::Prelude, ms(1)),
            (Stage::Check, ms(2)),
        ];
        assert_eq!(
            render(&stages, ms(10), Some((3, 12)), None),
            [
                "timings: lex           1.0 ms   10.0%  3 lines  3.0k lines/s  12 tokens  12.0k tokens/s",
                "timings: parse         2.0 ms   20.0%  3 lines  1.5k lines/s",
                "timings: prelude       1.0 ms   10.0%",
                "timings: check         2.0 ms   20.0%  3 lines  1.5k lines/s",
                "timings: other         4.0 ms   40.0%",
                "timings: total        10.0 ms  100.0%",
            ]
        );
    }

    #[test]
    fn report_should_have_no_throughput_when_the_lex_stops() {
        let ms = Duration::from_millis;
        assert_eq!(
            render(&[(Stage::Lex, ms(1))], ms(2), None, None),
            [
                "timings: lex           1.0 ms   50.0%",
                "timings: other         1.0 ms   50.0%",
                "timings: total         2.0 ms  100.0%",
            ]
        );
    }

    #[test]
    fn report_should_print_zero_percent_when_the_total_is_zero() {
        assert_eq!(
            render(&[(Stage::Lex, Duration::ZERO)], Duration::ZERO, None, None),
            [
                "timings: lex           0.0 ms    0.0%",
                "timings: other         0.0 ms    0.0%",
                "timings: total         0.0 ms    0.0%",
            ]
        );
    }

    #[test]
    fn rate_should_print_a_dash_for_zero_time_or_zero_count() {
        assert_eq!(rate(0, Duration::from_millis(1)), "-");
        assert_eq!(rate(5, Duration::ZERO), "-");
    }

    #[test]
    fn rate_should_scale_with_k_m_and_g() {
        let s = Duration::from_secs(1);
        assert_eq!(rate(999, s), "999");
        assert_eq!(rate(9_997, Duration::from_secs(10)), "1.0k");
        assert_eq!(rate(14_333, s), "14.3k");
        assert_eq!(rate(999_960, s), "1.0M");
        assert_eq!(
            rate(599_989_979, Duration::from_nanos(600_019_979_999)),
            "999.9k"
        );
        assert_eq!(rate(11_900_000, s), "11.9M");
        assert_eq!(rate(2_500_000_000, s), "2.5G");
    }

    #[test]
    fn report_should_be_empty_when_off() {
        let mut t = Timings::off();
        assert_eq!(t.time(Stage::Lex, || 7), 7);
        t.count(|| unreachable!("an off report counts nothing"));
        assert_eq!(t.report(), Vec::<String>::new());
    }

    /// A recorder that has run `stages`, as `time` would record them.
    fn recorded(stages: &[(Stage, u64)]) -> Timings {
        Timings(Some(Recording {
            start: Instant::now(),
            stages: (stages.iter())
                .map(|&(stage, millis)| (stage, Duration::from_millis(millis)))
                .collect(),
            counts: None,
            units: None,
        }))
    }

    #[test]
    fn merge_units_should_keep_the_slowest_unit_of_each_stage() {
        let mut timings = Timings::start();
        let units = vec![
            recorded(&[(Stage::Codegen, 3), (Stage::Optimize, 4)]),
            recorded(&[(Stage::Codegen, 5), (Stage::Optimize, 1)]),
        ];
        timings.merge_units(units);
        let lines = timings.report();
        assert!(
            lines[0].starts_with("timings: codegen       5.0 ms"),
            "{lines:?}"
        );
        assert!(
            lines[1].starts_with("timings: optimize      4.0 ms"),
            "{lines:?}"
        );
        assert_eq!(lines[2], "timings: codegen units: 2");
        assert!(lines[3].starts_with("timings: other"), "{lines:?}");
    }

    #[test]
    fn for_unit_should_be_on_only_when_the_report_is_on() {
        assert!(Timings::start().for_unit().0.is_some());
        assert!(Timings::off().for_unit().0.is_none());
    }
}
