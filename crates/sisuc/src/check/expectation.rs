//! The type that the context of a value expects of it.

use super::Checker;
use crate::diagnostic::MismatchOrigin;
use crate::source::Span;
use crate::tir::Type;

/// Where the type that a context requires or hints comes from: a declaration, an earlier value or
/// a language rule. A report that names the type points at it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Origin {
    /// No origin that a report names.
    None,
    /// The `break` at `at`, the first of its loop with a value, gave the loop the type `ty`.
    Break { at: Span, ty: Type },
}

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
    /// The context expects this type, which comes from the origin.
    Required(Type, Origin),
    /// The siblings that the value joins give it `want`, the type of the siblings (`siblings`).
    /// Siblings are the branches before it and the earlier values of its loop. They are also the
    /// other side of its `==`, the left side of its `??`, and an element beside an `any` element
    /// (`value_beside`). An operator can also hint each operand except its leader, the first
    /// operand of a concrete type, or each operand when it has no leader. The hint is the type
    /// that the operator takes by default (`i64`), or takes from a leader of a type that it does
    /// not take. The context did not write it. `want` steers inference as a required type does.
    /// But the value does not convert to an `any` type for it. A join in the value takes
    /// `siblings` as its first anchor, not as its expected type (see `split_hint`). A value part
    /// of the value keeps the hint. Such a part is the tail of a block, a branch of a hinted `if`
    /// or `match`, or the argument of a hinted `Some`. It is also an argument that a hinted
    /// call's context bound, or a `break` value of a hinted loop. An operand does not take the
    /// hint of the value: it takes `plain`. `origin` is where `want` and `siblings` come from.
    Hint {
        want: Type,
        siblings: Type,
        origin: Origin,
    },
}

const _: () = assert!(
    size_of::<Expectation>() <= 32,
    "an `Expectation` goes by value through each check, so it must stay small"
);

impl Expectation {
    /// The type that the value should have; none when nothing is expected or the context failed.
    pub(super) fn ty(self) -> Option<Type> {
        match self {
            Self::Required(ty, _) | Self::Hint { want: ty, .. } => Some(ty),
            Self::None | Self::Poisoned => None,
        }
    }

    /// Where the type comes from; `Origin::None` when nothing is expected or the context failed.
    pub(super) fn origin(self) -> Origin {
        match self {
            Self::Required(_, origin) | Self::Hint { origin, .. } => origin,
            Self::None | Self::Poisoned => Origin::None,
        }
    }

    /// `Required`, or `Poisoned` when `ty` is none: there `None` means that the type failed.
    pub(super) fn or_poisoned(ty: Option<Type>, origin: Origin) -> Self {
        ty.map_or(Self::Poisoned, |ty| Self::Required(ty, origin))
    }

    /// `Required(want)` for a hint, with its origin, so that an operand does not see the hint;
    /// else `self`.
    pub(super) fn plain(self) -> Self {
        match self {
            Self::Hint { want, origin, .. } => Self::Required(want, origin),
            other => other,
        }
    }

    /// For a hint: `None`, and the type of its siblings with its origin. Else `self` and none.
    pub(super) fn split_hint(self) -> (Self, Option<(Type, Origin)>) {
        match self {
            Self::Hint {
                siblings, origin, ..
            } => (Self::None, Some((siblings, origin))),
            other => (other, None),
        }
    }

    /// The type of the siblings of a hint; else none.
    pub(super) fn siblings(self) -> Option<Type> {
        self.split_hint().1.map(|(ty, _)| ty)
    }
}

impl Checker<'_> {
    /// The fact that a report gives for `origin`, with its types named; none for `Origin::None`.
    pub(super) fn origin_fact(&self, origin: Origin) -> Option<MismatchOrigin> {
        match origin {
            Origin::None => None,
            Origin::Break { at, ty } => Some(MismatchOrigin::Break {
                at,
                ty: self.type_name(ty),
            }),
        }
    }
}
