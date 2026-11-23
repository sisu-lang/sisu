//! The position of a read of a local.

use super::expectation::Expectation;
use super::place::Change;
use crate::tir::{Type, TypeKind};

/// How the program uses the local that a name stands for. The lookup of the name gets it, so a
/// local with no type can settle by the rule of its position.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Use {
    /// A value read under the type that its context expects.
    Read(Expectation),
    /// The target of an assignment, an element or field write, a `push` or a compound assignment.
    Write,
    /// The scrutinee of a pattern: `match`, `if let`, `while let` and `let ... else`.
    Scrutinee,
    /// The place of a `mut` argument.
    MutArgument,
    /// The receiver of a method, the base of a field or an index, or the iterable of a `for`.
    Receiver,
    /// The value of a conversion to `any I`.
    ToAny,
}

impl Use {
    /// The expectation of a read; none for any other use.
    pub(super) fn expectation(self) -> Expectation {
        match self {
            Self::Read(expected) => expected,
            _ => Expectation::None,
        }
    }

    /// The use of the root name of a place that `change` writes or passes.
    pub(super) fn of_change(change: Change) -> Self {
        match change {
            Change::Assign | Change::Element | Change::Push | Change::Field => Self::Write,
            Change::MutArg => Self::MutArgument,
            Change::Receiver(..) => Self::Receiver,
        }
    }

    /// `self`, with a read where an `any I` or `(any I)?` is required made a `ToAny`: the
    /// value converts to it after the read.
    pub(super) fn converted(self) -> Self {
        let Self::Read(expected) = self else {
            return self;
        };
        let target = expected.ty().map(|ty| match ty.kind() {
            TypeKind::Optional(payload) => payload,
            _ => ty,
        });
        match target.map(Type::kind) {
            Some(TypeKind::Any(..)) => Self::ToAny,
            _ => self,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::check::expectation::Origin;

    #[test]
    fn of_change_should_name_the_use_of_each_change() {
        let method = crate::symbol::Symbol::intern("m");
        let table = [
            (Change::Assign, Use::Write),
            (Change::Element, Use::Write),
            (Change::Push, Use::Write),
            (Change::Field, Use::Write),
            (Change::MutArg, Use::MutArgument),
            (Change::Receiver(method, false), Use::Receiver),
            (Change::Receiver(method, true), Use::Receiver),
        ];
        for (change, want) in table {
            assert_eq!(Use::of_change(change), want);
        }
    }

    #[test]
    fn converted_should_keep_a_read_that_needs_no_conversion() {
        let read = Use::Read(Expectation::Required(Type::I64, Origin::None));
        assert_eq!(read.converted(), read);
        assert_eq!(Use::Write.converted(), Use::Write);
    }
}
