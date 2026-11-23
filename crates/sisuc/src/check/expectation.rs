//! The type that the context of a value expects of it.

use super::Checker;
use super::unused_names::Owner;
use crate::diagnostic::MismatchOrigin;
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{Type, TypeKind};
use std::rc::Rc;

/// Where the type that a context requires comes from: a declaration, an earlier value or
/// a language rule. A report that names the type points at it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Origin {
    /// No origin that a report names.
    None,
    /// Parameter `index` of the function or method `function` (a `FuncId`).
    Parameter { function: u32, index: u32 },
    /// The return type of the function or method `function`.
    Returns { function: u32 },
    /// The type `ty` that a `let` or a constant writes at `at`.
    Declared { at: Span, ty: Type },
    /// Field `index` of the class `class` (a `ClassId`).
    Field { class: u32, index: u32 },
    /// Field `index` of the struct `id` (a `StructId`).
    StructField { id: u32, index: u32 },
    /// Payload field `field` of the variant `variant` of the enum `enum_` (an `EnumId`).
    Payload {
        enum_: u32,
        variant: u32,
        field: u32,
    },
    /// The local of the type `ty` that the name at `at` declares, which an assignment writes.
    Assigned { at: Span, ty: Type },
    /// The rule that a condition is a `bool`.
    Condition,
    /// The rule that an index of an array value is an `i64` or a range.
    Index,
    /// The rule that an index of a place is an `i64`: a place takes no range.
    PlaceIndex,
    /// The argument at `at`, which bound the type parameter `param` of a call to `ty`.
    Inferred { at: Span, param: Symbol, ty: Type },
    /// The first bare `break` of a loop, at `at`, which gives the loop the value `unit`. A report
    /// of a `break` that has a value points at it.
    BareBreak { at: Span },
}

impl Origin {
    /// `index` as an `Origin` stores it.
    pub(super) fn id(index: usize) -> u32 {
        u32::try_from(index).expect("an index of the program should fit in a `u32`")
    }
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
    /// The value is a member of a join, or at a value position inside one: its inference
    /// variables stay open. The join closes them when it applies its own expectation.
    Member,
}

const _: () = assert!(
    size_of::<Expectation>() <= 32,
    "an `Expectation` goes by value through each check, so it must stay small"
);

impl Expectation {
    /// The type that the value should have; none when nothing is expected or the context failed.
    pub(super) fn ty(self) -> Option<Type> {
        match self {
            Self::Required(ty, _) => Some(ty),
            Self::None | Self::Poisoned | Self::Member => None,
        }
    }

    /// What the argument of a `Some` expects. A required optional gives its payload, with no
    /// origin, so that `let x: u8? = Some("s")` names no declaration. Any other expectation goes
    /// to the argument as it is.
    pub(super) fn payload(self) -> Self {
        match self {
            Self::Required(ty, _) => match ty.kind() {
                TypeKind::Optional(payload) => Self::Required(payload, Origin::None),
                _ => self,
            },
            _ => self,
        }
    }

    /// Where the type comes from; `Origin::None` when nothing is expected or the context failed.
    pub(super) fn origin(self) -> Origin {
        match self {
            Self::Required(_, origin) => origin,
            Self::None | Self::Poisoned | Self::Member => Origin::None,
        }
    }

    /// `Required`, or `Poisoned` when `ty` is none: there `None` means that the type failed.
    pub(super) fn or_poisoned(ty: Option<Type>, origin: Origin) -> Self {
        ty.map_or(Self::Poisoned, |ty| Self::Required(ty, origin))
    }

    /// The origin of an operand that is held to the type `want`: the origin of this context when
    /// `want` is the type that it expects. Else none.
    pub(super) fn origin_for(self, want: Option<Type>) -> Origin {
        if want.is_some() && want == self.ty() {
            self.origin()
        } else {
            Origin::None
        }
    }
}

impl Checker<'_> {
    /// The origin of the return type of the function that the body is in; `Origin::None` in a
    /// constant.
    pub(super) fn returns(&self) -> Origin {
        match self.body.owner {
            Some(Owner::Func(f)) => Origin::Returns {
                function: Origin::id(f.0),
            },
            _ => Origin::None,
        }
    }

    /// The fact that a report gives for `origin`, with its types named; none for `Origin::None`,
    /// a type that failed, and a parameter, a field or a payload whose declared type names a type
    /// parameter: a call or an object can give that parameter another type.
    pub(super) fn origin_fact(&self, origin: Origin) -> Option<MismatchOrigin> {
        let named = |ty: Type, shown: Option<&Rc<str>>| {
            shown.map_or_else(|| self.type_name(ty), ToString::to_string)
        };
        match origin {
            Origin::None | Origin::BareBreak { .. } => None,
            Origin::Parameter { function, index } => {
                let sig = &self.signatures[function as usize];
                let i = index as usize;
                let ty = sig.params[i].filter(|ty| !ty.has_param())?;
                Some(MismatchOrigin::Parameter {
                    at: sig.param_spans[i],
                    function: own_name(&sig.name).to_string(),
                    param: sig.param_names[i].name.to_string(),
                    ty: named(ty, sig.param_shown[i].as_ref()),
                })
            }
            Origin::Returns { function } => {
                let sig = &self.signatures[function as usize];
                Some(MismatchOrigin::Returns {
                    at: sig.ret_span?,
                    function: own_name(&sig.name).to_string(),
                    ty: named(sig.ret?, sig.ret_shown.as_ref()),
                })
            }
            Origin::Declared { at, ty } => Some(MismatchOrigin::Declared {
                at,
                ty: self.want_name(at, ty),
            }),
            Origin::Field {
                class: owner,
                index,
            }
            | Origin::StructField { id: owner, index } => {
                let table = if matches!(origin, Origin::Field { .. }) {
                    &self.classes
                } else {
                    &self.structs
                };
                let field = &table[owner as usize].fields[index as usize];
                let ty = field.ty.filter(|ty| !ty.has_param())?;
                Some(MismatchOrigin::Field {
                    at: field.ty_span,
                    field: field.name.name.to_string(),
                    ty: named(ty, field.shown.as_ref()),
                })
            }
            Origin::Payload {
                enum_,
                variant,
                field,
            } => {
                let variant = &self.enums[enum_ as usize].variants[variant as usize];
                let ty = variant.fields[field as usize].filter(|ty| !ty.has_param())?;
                Some(MismatchOrigin::Payload {
                    at: variant.spans[field as usize],
                    variant: variant.name.name.to_string(),
                    ty: named(ty, variant.shown[field as usize].as_ref()),
                })
            }
            Origin::Assigned { at, ty } => Some(MismatchOrigin::Assigned {
                at,
                name: self.source.text(at).to_string(),
                ty: self.want_name(at, ty),
            }),
            Origin::Condition => Some(MismatchOrigin::Condition),
            Origin::Index => Some(MismatchOrigin::Index),
            Origin::PlaceIndex => Some(MismatchOrigin::PlaceIndex),
            Origin::Inferred { at, param, ty } => Some(MismatchOrigin::Inferred {
                at,
                param: param.to_string(),
                ty: self.type_name(ty),
            }),
        }
    }
}

/// The function's or method's own name: `m` of `P.m`.
pub(super) fn own_name(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}
