//! The type that the context of a value expects of it.

use crate::tir::Type;

/// What a value is checked against: the type that the context expects of it, if there is one.
///
/// A reader that wants a type calls [`Expectation::ty`]. A reader that must tell a failed context
/// from a context with no wish compares with [`Expectation::Poisoned`] or [`Expectation::None`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Expectation {
    /// The context expects no type.
    None,
    /// The context failed, so its type is not known: a mistake that follows from it waits.
    Poisoned,
    /// The context expects this type.
    Required(Type),
    /// The siblings that the value joins give it `want`, the type of the siblings (`siblings`).
    /// Siblings are the branches before it and the earlier values of its loop. They are also the
    /// other side of its `==`, the left side of its `??`, and an element beside an `any` element
    /// (`value_beside`). An operator also hints the operands that wait for context. The hint is
    /// the type that the operator takes by default (`i64`), or takes from a leader of a type that
    /// it does not take. The context did not write it. `want` steers inference as a required type
    /// does. But the value does not convert to an `any` type for it. A join in the value takes
    /// `siblings` as its first anchor, not as its expected type (see `split_hint`). A value part
    /// of the value keeps the hint. Such a part is the tail of a block, a branch of a hinted `if`
    /// or `match`, or the argument of a hinted `Some`. It is also an argument that a hinted
    /// call's context bound, or a `break` value of a hinted loop. An operand does not take the
    /// hint of the value: it takes `plain`.
    Hint { want: Type, siblings: Type },
}

impl Expectation {
    /// The type that the value should have; none when nothing is expected or the context failed.
    pub(super) fn ty(self) -> Option<Type> {
        match self {
            Self::Required(ty) | Self::Hint { want: ty, .. } => Some(ty),
            Self::None | Self::Poisoned => None,
        }
    }

    /// `Required`, or `Poisoned` when `ty` is none: there `None` means that the type failed.
    pub(super) fn or_poisoned(ty: Option<Type>) -> Self {
        ty.map_or(Self::Poisoned, Self::Required)
    }

    /// `Required(want)` for a hint, so that an operand does not see it; else `self`.
    pub(super) fn plain(self) -> Self {
        match self {
            Self::Hint { want, .. } => Self::Required(want),
            other => other,
        }
    }

    /// For a hint: `None` and the type of its siblings. Else `self` and none.
    pub(super) fn split_hint(self) -> (Self, Option<Type>) {
        match self {
            Self::Hint { siblings, .. } => (Self::None, Some(siblings)),
            other => (other, None),
        }
    }

    /// The type of the siblings of a hint; else none.
    pub(super) fn siblings(self) -> Option<Type> {
        self.split_hint().1
    }
}
