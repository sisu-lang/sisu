//! The position of a read of a local.

use super::Checker;
use super::any::any_target;
use super::expectation::Expectation;
use super::place::Change;

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
    /// A read under `any I` or `(any I)?`, which expects `Expectation`. The value converts there
    /// after the read, unless its type is an open `Any`: that root unifies, so a reader treats it
    /// as `Required`. A conversion in a join, a generic argument or an `==` is no `ToAny`. There
    /// the lookup sees `Read(Member)` or `Read(None)`, and `conversion` settles the value.
    ToAny(Expectation),
}

impl Use {
    /// The expectation of a read or a conversion; none for any other use.
    pub(super) fn expectation(self) -> Expectation {
        match self {
            Self::Read(expected) | Self::ToAny(expected) => expected,
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

    /// `self`, with a read where an `any I` or `(any I)?` is required made a `ToAny`.
    pub(super) fn converted(self) -> Self {
        match self {
            Self::Read(expected) if expected.ty().and_then(any_target).is_some() => {
                Self::ToAny(expected)
            }
            _ => self,
        }
    }
}

impl Checker<'_> {
    /// The use of the root name of the base of a field or an index: the use of the place that
    /// the chain writes or passes, at any depth, else `Receiver`.
    pub(super) fn chain_use(&self) -> Use {
        (self.body.write_chain).map_or(Use::Receiver, |(_, change)| Use::of_change(change))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::check::expectation::Origin;
    use crate::tir::{InterfaceId, Type, TypeArgs, TypeKind};

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
        for (i, (change, want)) in table.into_iter().enumerate() {
            assert_eq!(Use::of_change(change), want, "row {i}");
        }
    }

    #[test]
    fn converted_should_keep_a_read_that_needs_no_conversion() {
        let read = Use::Read(Expectation::Required(Type::I64, Origin::None));
        assert_eq!(read.converted(), read);
        assert_eq!(Use::Write.converted(), Use::Write);
    }

    #[test]
    fn converted_should_make_a_read_under_an_any_type_a_conversion() {
        let any = Type::intern(TypeKind::Any(InterfaceId(0), TypeArgs::NONE));
        let required = |ty| Use::Read(Expectation::Required(ty, Origin::None));
        let table = [
            (
                required(any),
                Use::ToAny(Expectation::Required(any, Origin::None)),
            ),
            (
                required(Type::optional(any)),
                Use::ToAny(Expectation::Required(Type::optional(any), Origin::None)),
            ),
            (required(Type::array(any)), required(Type::array(any))),
        ];
        for (i, (read, want)) in table.into_iter().enumerate() {
            assert_eq!(read.converted(), want, "row {i}");
        }
    }
}
