//! The `struct` keyword and its stopgap: until structs lower, each one is rejected.

use super::*;
use Code::{Expected, ReservedWord};

/// The word `record` is a name, the word `struct` is a keyword that no name can be, and the word
/// `move` is reserved.
#[test]
fn words_should_report_their_code() {
    let rows = [
        (
            "record",
            "fn main() {\n    let record = 1\n}".to_string(),
            vec![],
        ),
        (
            "struct",
            "fn main() {\n    let struct = 1\n}".to_string(),
            vec![(Expected, (2, 9))],
        ),
        (
            "move",
            "fn main() {\n    let move = 1\n}".to_string(),
            vec![(ReservedWord, (2, 9))],
        ),
    ];
    error_rows(&rows);
}

/// GIVEN `struct P { x: i64 }` THEN the error is E0083 at the keyword. A use of the name adds no
/// error, because the rejected name is poisoned.
#[test]
fn struct_should_be_rejected_until_it_lowers() {
    let rejected = vec![(ReservedWord, (1, 1))];
    let rows = [
        (
            "a struct",
            "struct P { x: i64 }\nfn main() {}".to_string(),
            rejected.clone(),
        ),
        (
            "a mut field",
            "struct P { mut x: i64 }\nfn main() {}".to_string(),
            rejected.clone(),
        ),
        (
            "spaces after the keyword",
            "struct   P { x: i64 }\nfn main() {}".to_string(),
            rejected.clone(),
        ),
        (
            "a use of the name",
            "struct P { x: i64 }\nfn main() { let p: P = P(x: 1) }".to_string(),
            rejected,
        ),
    ];
    error_rows(&rows);
}
