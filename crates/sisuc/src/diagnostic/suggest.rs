//! Did-you-mean: the candidate nearest to a name that was not found.

use super::Definition;

/// A name in scope that the missing name may have meant, and what it is.
#[derive(Clone)]
pub(crate) struct Suggestion {
    pub(crate) kind: Definition,
    pub(crate) name: String,
}

/// The candidate of `name`'s kind nearest to it, if any; each candidate is a name and its kind.
pub(crate) fn suggestion<'a>(
    name: &str,
    candidates: impl IntoIterator<Item = (&'a str, Definition)>,
) -> Option<Suggestion> {
    let candidates: Vec<_> = candidates.into_iter().collect();
    let best = closest(name, candidates.iter().map(|&(candidate, _)| candidate))?;
    let (_, kind) = candidates
        .iter()
        .find(|&&(candidate, _)| candidate == best)?;
    Some(Suggestion {
        kind: *kind,
        name: best.to_string(),
    })
}

/// The prelude function that another language's name for it means, for a name too far from it
/// for `closest`: `readln` and `input` are `read_line`.
pub(crate) fn alias(name: &str) -> Option<&'static str> {
    match name {
        "readln" | "input" => Some("read_line"),
        _ => None,
    }
}

/// The candidate nearest to `name`: within `max(1, len / 3)` edits of it, or equal to it
/// ignoring ASCII case. A tie goes to the alphabetically first. `name` itself never qualifies.
pub(crate) fn closest<'a>(
    name: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    let cutoff = (name.chars().count() / 3).max(1);
    candidates
        .into_iter()
        .filter(|&candidate| candidate != name)
        .filter_map(|candidate| {
            let distance = if candidate.eq_ignore_ascii_case(name) {
                0
            } else {
                edit_distance(name, candidate)
            };
            (distance <= cutoff).then_some((distance, candidate))
        })
        .min()
        .map(|(_, candidate)| candidate)
}

/// The Levenshtein distance between `a` and `b`, over chars.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    // `row[j]` is the distance from the chars of `a` read so far to the first `j` chars of `b`.
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (diagonal + usize::from(ca != *cb))
                .min(above + 1)
                .min(row[j] + 1);
            diagonal = above;
        }
    }
    row[b.len()]
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::closest;

    #[test]
    fn close_names_qualify() {
        assert_eq!(closest("totl", ["total"]), Some("total"));
        assert_eq!(closest("x", ["y"]), Some("y"));
    }

    #[test]
    fn far_names_do_not() {
        assert_eq!(closest("ab", ["xy"]), None);
        assert_eq!(closest("abcdef", ["abcxyz"]), None);
        assert_eq!(closest("abcdef", ["abcdxy"]), Some("abcdxy"));
        // A swap is two edits, so a four-letter name needs another one to qualify.
        assert_eq!(closest("nrom", ["norm"]), None);
    }

    #[test]
    fn case_only_always_qualifies() {
        assert_eq!(closest("COUNT", ["count", "COUNTS"]), Some("count"));
        assert_eq!(closest("ab", ["AB"]), Some("AB"));
        // Past the cutoff, but the same letters.
        assert_eq!(closest("ABCDEFGH", ["abcdefgh"]), Some("abcdefgh"));
    }

    #[test]
    fn ties_go_alphabetically() {
        assert_eq!(closest("cat", ["hat", "bat"]), Some("bat"));
        assert_eq!(closest("cat", ["bat", "hat"]), Some("bat"));
    }

    #[test]
    fn the_name_itself_never() {
        assert_eq!(closest("total", ["total"]), None);
        assert_eq!(closest("total", ["total", "totals"]), Some("totals"));
    }

    #[test]
    fn nearer_beats_alphabetical() {
        assert_eq!(closest("abcd", ["abxy", "abcx"]), Some("abcx"));
        // Both qualify; the nearer one sorts last.
        assert_eq!(closest("abcdef", ["abcdaa", "abcdeg"]), Some("abcdeg"));
    }

    #[test]
    fn distance_counts_chars() {
        // One substitution, not two bytes.
        assert_eq!(closest("é", ["e"]), Some("e"));
        assert_eq!(closest("naïve", ["naive"]), Some("naive"));
        // The cutoff counts chars too: 3 chars allow 1 edit, not the 2 a byte count would.
        assert_eq!(closest("ééé", ["aaé"]), None);
    }

    #[test]
    fn no_candidates() {
        assert_eq!(closest("total", []), None);
    }
}
