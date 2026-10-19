use super::*;
use crate::check::check;
use crate::parser::parse_ok;

mod lower;
mod pattern;

/// The typed tree of `src`, which checks without an error, with an empty `main` added when it
/// has none, and with its last uses marked, as the build passes it on.
fn checked(src: &str) -> tir::Program {
    let src = if src.contains("fn main") {
        src.to_owned()
    } else {
        format!("{src}\nfn main() {{}}")
    };
    let (program, diagnostics) = check(&parse_ok(&src), &src);
    let mut program = program.unwrap_or_else(|| panic!("{diagnostics:?}"));
    crate::last_use::mark(&mut program);
    program
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
        stack.extend(successors(&f.blocks[block.0].term));
    }
    (0..f.blocks.len())
        .filter(|&i| !seen[i])
        .map(BlockId)
        .collect()
}

/// The blocks `term` can jump to.
fn successors(term: &Terminator) -> Vec<BlockId> {
    match term {
        Terminator::Goto(to) => vec![*to],
        Terminator::Branch { then, else_, .. } => vec![*then, *else_],
        Terminator::IfPresent { some, none, .. } => vec![*some, *none],
        Terminator::SwitchTag {
            arms, otherwise, ..
        } => arms.iter().map(|(_, to)| *to).chain(*otherwise).collect(),
        Terminator::SwitchValue {
            arms, otherwise, ..
        } => arms.iter().map(|(_, to)| *to).chain(*otherwise).collect(),
        Terminator::Return(_) | Terminator::Panic { .. } | Terminator::Unreachable => vec![],
    }
}
