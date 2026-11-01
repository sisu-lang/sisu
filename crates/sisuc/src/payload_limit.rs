//! The size limit of an enum payload.
//!
//! Codegen lays out an enum as `{ tag, [n x iA] }`: `n` units of the largest payload alignment
//! `A`. `n` is a `u32`, and it is at most the payload size in bytes. A payload under
//! `PAYLOAD_LIMIT` bytes thus always fits, and one of that size or more may not. `oversized`
//! finds the enums that may not fit, so that the front end reports them and codegen never sees
//! one.
//!
//! The sizes here are upper bounds that follow the layout rules of `codegen::enums` and
//! `codegen::layout`. An optional adds one alignment unit, as when it keeps a flag beside its
//! payload. An optional with a niche adds nothing, and an `Option`-shaped enum has no tag, so
//! the bound is exact except for these. A test in `codegen/tests.rs` checks the bound against
//! the LLVM size of each enum.

use std::collections::VecDeque;

use rustc_hash::FxHashSet;

use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::source::Span;
use crate::tir::Enum;
use crate::types::{Type, TypeArgs, TypeKind};

/// The limit in GiB, as the diagnostic writes it.
pub(crate) const PAYLOAD_LIMIT_GIB: u64 = 4;

/// The least payload size in bytes that an enum may not have.
pub(crate) const PAYLOAD_LIMIT: u64 = PAYLOAD_LIMIT_GIB << 30;

const _: () = assert!(
    PAYLOAD_LIMIT <= 1 << 32,
    "a payload under the limit should have fewer than 2^32 units of any alignment"
);

/// The size of a pointer, and the alignment of every type with a pointer or an `i64`: the most
/// that any type has on a target Sisu compiles for.
const WORD: u64 = 8;

/// The size and alignment of a type in bytes. The alignment is a power of two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    pub(crate) size: u64,
    pub(crate) align: u64,
}

impl Layout {
    const EMPTY: Layout = Layout { size: 0, align: 1 };

    const fn new(size: u64, align: u64) -> Layout {
        Layout { size, align }
    }
}

/// `size` rounded up to a multiple of the power of two `align`. A size past `u64` stays near the
/// largest `u64`, which is over the limit.
fn round_up(size: u64, align: u64) -> u64 {
    size.saturating_add(align - 1) & !(align - 1)
}

/// The layout of a struct with `fields` in order, as LLVM lays out one that is not packed;
/// `None` when a field has no layout.
fn struct_of(fields: impl IntoIterator<Item = Option<Layout>>) -> Option<Layout> {
    let (offset, align) = fields
        .into_iter()
        .try_fold((0, 1), |(offset, align), field| {
            let field = field?;
            let start = round_up(offset, field.align);
            Some((start.saturating_add(field.size), align.max(field.align)))
        })?;
    Some(Layout::new(round_up(offset, align), align))
}

/// The size of the tag of an enum with `count` variants, as `codegen::enums::tag_bits` picks it.
fn tag_bytes(count: usize) -> u64 {
    if count <= 1 << 8 {
        1
    } else if count <= 1 << 16 {
        2
    } else {
        4
    }
}

/// An upper bound of what an enum takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Measured {
    /// The size of its largest variant.
    pub(crate) payload: u64,
    /// The size and alignment of a value.
    pub(crate) value: Layout,
}

#[cfg(test)]
thread_local! {
    /// How many entries of its stack `measure` has popped on this thread.
    static MEASURE_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
thread_local! {
    /// How many calls of `spread_uses`, passes of its early return, enums and `held_inline` edges
    /// it has visited on this thread.
    static SPREAD_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// What `measure` knows of each enum: `None` before it is measured, then its bound, which is
/// `None` too when a field has a type that is not final or an inline cycle.
type Known = [Option<Option<Measured>>];

/// The bound of the layout of `ty`, which is the type of an unboxed field, once the enums that it
/// holds are measured; `None` for a type that is not final.
fn layout_of(ty: Type, known: &Known) -> Option<Layout> {
    match ty.kind() {
        TypeKind::I64 | TypeKind::F64 => Some(Layout::new(8, 8)),
        TypeKind::U8 | TypeKind::Bool => Some(Layout::new(1, 1)),
        TypeKind::Char => Some(Layout::new(4, 4)),
        TypeKind::Unit | TypeKind::Never => Some(Layout::EMPTY),
        // An object, an array or a string is a pointer to its block.
        TypeKind::String | TypeKind::Class(..) | TypeKind::Array(_) => {
            Some(Layout::new(WORD, WORD))
        }
        // An enum with arguments is a generic one that `mono` has not instantiated.
        TypeKind::Enum(_, args) if args != TypeArgs::NONE => None,
        TypeKind::Enum(id, _) => Some(known[id.0].flatten()?.value),
        // A flag beside the payload, which a niche makes smaller.
        TypeKind::Optional(payload) => {
            let payload = layout_of(payload, known)?;
            Some(Layout::new(
                payload.size.saturating_add(payload.align),
                payload.align,
            ))
        }
        // The pointer to the data and the pointer to the table.
        TypeKind::Any(..) => Some(Layout::new(2 * WORD, WORD)),
        TypeKind::Param(..) | TypeKind::Assoc(..) => None,
    }
}

/// The bound of the layout of a boxed field of type `ty`: a pointer, inside one flag for each
/// optional of an optional (`codegen::enums::box_storage`).
fn boxed_layout(mut ty: Type) -> Layout {
    let mut flags = 0;
    while let TypeKind::Optional(payload) = ty.kind() {
        if !matches!(payload.kind(), TypeKind::Optional(_)) {
            break;
        }
        flags += 1;
        ty = payload;
    }
    Layout::new(WORD * (1 + flags), WORD)
}

/// The bound of enum `e`, whose inline-held enums are measured.
fn measure_enum(e: &Enum, known: &Known) -> Option<Measured> {
    let mut payload = Layout::EMPTY;
    for variant in &e.variants {
        let variant = struct_of(variant.fields.iter().map(|field| {
            if field.boxed {
                Some(boxed_layout(field.ty))
            } else {
                layout_of(field.ty, known)
            }
        }))?;
        payload = Layout::new(
            payload.size.max(variant.size),
            payload.align.max(variant.align),
        );
    }
    let tag = tag_bytes(e.variants.len());
    let value = if payload.size == 0 {
        Layout::new(tag, tag)
    } else {
        // `{ tag, [n x iA] }`: the array starts at the next multiple of `A`.
        let units = payload.size.div_ceil(payload.align);
        let start = round_up(tag, payload.align);
        let align = tag.max(payload.align);
        let end = start.saturating_add(units.saturating_mul(payload.align));
        Layout::new(round_up(end, align), align)
    };
    Some(Measured {
        payload: payload.size,
        value,
    })
}

/// The enums that `e` holds inline: each unboxed field of an enum type or its optional.
fn held_inline(e: &Enum) -> impl Iterator<Item = usize> + '_ {
    (e.variants.iter().flat_map(|v| &v.fields))
        .filter(|p| !p.boxed)
        .filter_map(|p| p.ty.inline_enum())
        .map(|id| id.0)
}

/// The bound of each enum of `enums`, in one pass. An enum is measured after the enums it holds
/// inline, in post-order with an explicit stack, as `codegen` lays them out. `tir::layout_enums`
/// boxes a field on every cycle, so the walk ends; an inline cycle that remains gives no bound.
pub(crate) fn measure(enums: &[Enum]) -> Vec<Option<Measured>> {
    let mut known: Vec<Option<Option<Measured>>> = vec![None; enums.len()];
    let mut waiting = vec![false; enums.len()];
    // `(id, true)` once the enums `id` holds inline are queued before it. Empty after each root.
    let mut stack = Vec::new();
    for root in 0..enums.len() {
        stack.push((root, false));
        while let Some((id, held_queued)) = stack.pop() {
            #[cfg(test)]
            MEASURE_STEPS.set(MEASURE_STEPS.get() + 1);
            if known[id].is_some() {
                continue;
            }
            if held_queued {
                known[id] = Some(measure_enum(&enums[id], &known));
            } else if waiting[id] {
                // An inline cycle: the enum is its own part.
                known[id] = Some(None);
            } else {
                waiting[id] = true;
                stack.push((id, true));
                stack.extend(held_inline(&enums[id]).map(|held| (held, false)));
            }
        }
    }
    known.into_iter().map(Option::flatten).collect()
}

/// An enum whose payload may be `PAYLOAD_LIMIT` bytes or more.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Oversized {
    /// The name of the enum that the user can change.
    pub(crate) name: String,
    /// Where the declaration of that enum names it.
    pub(crate) at: Span,
    /// Whether `name` is an instance of a generic enum: `at` is then the declaration.
    pub(crate) instance: bool,
    /// The prelude enum that holds it, when the payload over the limit is that enum's: a
    /// `Result<E, E>` is as large as `E` and a tag.
    pub(crate) holder: Option<String>,
    /// The first expression that uses the instance that is over the limit, when `mono` knows
    /// one. An enum that is no instance has none.
    pub(crate) used: Option<Span>,
}

/// The diagnostics of `oversized`, one for each report. `first_uses` is the first use of each enum
/// that `mono` found, or `&[]` when there is none.
pub(crate) fn diagnostics(enums: &[Enum], first_uses: &[Option<Span>]) -> Vec<Diagnostic> {
    (oversized(enums, first_uses).into_iter())
        .map(|o| {
            DiagnosticKind::PayloadTooLarge {
                name: o.name,
                instance: o.instance,
                holder: o.holder,
                at: o.at,
                used: o.used,
            }
            .into()
        })
        .collect()
}

/// The enum that `id` holds inline with the largest value, which is the cause of its size.
fn largest_held(enums: &[Enum], measured: &[Option<Measured>], id: usize) -> Option<usize> {
    held_inline(&enums[id]).max_by_key(|&held| measured[held].map(|m| m.value.size))
}

/// Each innermost enum of `enums` whose payload may be 4 GiB or more: none of the enums that
/// it holds inline has such a payload. The enums that hold it have one too, but they are not
/// another mistake. One report for each declaration: instances of a generic enum share one. An
/// enum of the prelude is no place for the user to change, so the report names the user's enum
/// that its largest field holds. When that chain reaches no user's enum, the report names the
/// prelude's enum. On the checker path, `checked_with` treats a report in the prelude as an
/// internal error. The reports are in the order of the enums. `first_uses` is the first use of each
/// enum, which a report gives as the place that uses an instance.
pub(crate) fn oversized(enums: &[Enum], first_uses: &[Option<Span>]) -> Vec<Oversized> {
    oversized_with(enums, first_uses, PAYLOAD_LIMIT)
}

/// The use of each instance in `enums`: its own in `first_uses`, else the one of its nearest
/// holder that is an instance. The pass is breadth first from every instance that has a use, so
/// it takes time linear in the enums and the `held_inline` edges, however many reports need it.
/// An enum that is no instance has no use, and a use does not pass through it: a field of a
/// non-generic enum or class is the declaration's, not the instance's. When no instance has a use
/// the result is empty.
fn spread_uses(enums: &[Enum], first_uses: &[Option<Span>]) -> Vec<Option<Span>> {
    #[cfg(test)]
    SPREAD_STEPS.set(SPREAD_STEPS.get() + 1);
    let own = |id: usize| first_uses.get(id).copied().flatten();
    let mut queue: VecDeque<usize> = (0..enums.len())
        .filter(|&id| enums[id].instance && own(id).is_some())
        .collect();
    if queue.is_empty() {
        return Vec::new();
    }
    #[cfg(test)]
    SPREAD_STEPS.set(SPREAD_STEPS.get() + 1);
    let mut uses = vec![None; enums.len()];
    for &id in &queue {
        uses[id] = own(id);
    }
    while let Some(holder) = queue.pop_front() {
        #[cfg(test)]
        SPREAD_STEPS.set(SPREAD_STEPS.get() + 1);
        for held in held_inline(&enums[holder]) {
            #[cfg(test)]
            SPREAD_STEPS.set(SPREAD_STEPS.get() + 1);
            if enums[held].instance && uses[held].is_none() {
                uses[held] = uses[holder];
                queue.push_back(held);
            }
        }
    }
    // shortcut: an instance that only a field of a non-generic class or enum reaches has no use,
    // as `mono` lowers the classes before any expression, and a non-generic enum is no instance;
    // no second label then. Upgrade by giving a class or enum field a span.
    uses
}

/// `oversized` with `limit` in place of `PAYLOAD_LIMIT`, for the tests, which cannot build a
/// payload of 4 GiB cheaply.
fn oversized_with(enums: &[Enum], first_uses: &[Option<Span>], limit: u64) -> Vec<Oversized> {
    let measured = measure(enums);
    let over = |id: usize| measured[id].is_some_and(|m| m.payload >= limit);
    let mut seen = FxHashSet::default();
    let mut found = Vec::new();
    let mut spread = None;
    for (id, e) in enums.iter().enumerate() {
        if !over(id) || held_inline(e).any(over) {
            continue;
        }
        let user = e.span.in_prelude().then(|| {
            let mut cause = id;
            loop {
                cause = largest_held(enums, &measured, cause)?;
                if !enums[cause].span.in_prelude() {
                    return Some(cause);
                }
            }
        });
        let (target, holder) = match user.flatten() {
            Some(user) => (user, Some(e.name.clone())),
            None => (id, None),
        };
        let target = &enums[target];
        if seen.insert(target.span) {
            // The use is of `id`, not of `target`: with a holder, a use makes the holder's
            // instance, and `target` may be a user's enum that is no instance.
            let first = if !enums[id].instance {
                None
            } else if let Some(own) = first_uses.get(id).copied().flatten() {
                Some(own)
            } else {
                let spread = spread.get_or_insert_with(|| spread_uses(enums, first_uses));
                spread.get(id).copied().flatten()
            };
            found.push(Oversized {
                name: target.name.clone(),
                at: target.span,
                instance: target.instance,
                holder,
                used: first,
            });
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::PRELUDE_BASE;
    use crate::tir::{EnumId, Payload, Rank, Variant};

    fn enum_type(id: usize) -> Type {
        Type::intern(TypeKind::Enum(EnumId(id), TypeArgs::NONE))
    }

    /// An enum named `name` that is declared at `at`, with one variant for each entry of
    /// `variants`: the field types of that variant, each boxed when the flag is set.
    fn declared(name: &str, at: usize, variants: Vec<Vec<(Type, bool)>>) -> Enum {
        Enum {
            name: name.to_string(),
            span: Span::new(at, at + 1),
            instance: false,
            type_params: Vec::new(),
            variants: (variants.into_iter().enumerate())
                .map(|(i, fields)| Variant {
                    name: format!("V{i}"),
                    fields: (fields.into_iter())
                        .map(|(ty, boxed)| Payload {
                            ty,
                            label: None,
                            boxed,
                            rank: Rank::Plain,
                        })
                        .collect(),
                })
                .collect(),
            counted: false,
            cycle: None,
        }
    }

    /// An enum with one variant that holds `fields` inline.
    fn holding(name: &str, at: usize, fields: Vec<Type>) -> Enum {
        declared(
            name,
            at,
            vec![fields.into_iter().map(|ty| (ty, false)).collect()],
        )
    }

    fn pair(name: &str, at: usize, held: usize) -> Enum {
        holding(name, at, vec![enum_type(held), enum_type(held)])
    }

    #[test]
    fn measure_should_pad_fields_to_their_alignment() {
        // `u8` then `i64`: the `i64` starts at 8, so the struct is 16 bytes and the tag adds a
        // word before the array of two `i64`s.
        let padded = holding("A", 0, vec![Type::U8, Type::I64]);
        let bytes = holding("B", 1, vec![Type::U8]);
        let measured = measure(&[padded, bytes]);
        let layout = |size, align| Layout::new(size, align);
        assert_eq!(
            measured[0],
            Some(Measured {
                payload: 16,
                value: layout(24, 8)
            })
        );
        assert_eq!(
            measured[1],
            Some(Measured {
                payload: 1,
                value: layout(2, 1)
            })
        );
    }

    #[test]
    fn measure_should_give_a_tag_alone_when_no_variant_has_a_field() {
        let measured = measure(&[declared("A", 0, vec![vec![], vec![]])]);
        assert_eq!(
            measured[0].map(|m| (m.payload, m.value)),
            Some((0, Layout::new(1, 1)))
        );
    }

    #[test]
    fn measure_should_add_one_alignment_unit_for_an_optional() {
        let enums = [
            holding("A", 0, vec![Type::optional(Type::I64)]),
            holding("B", 1, vec![Type::optional(Type::U8)]),
        ];
        let measured = measure(&enums);
        assert_eq!(measured[0].map(|m| m.payload), Some(16));
        assert_eq!(measured[1].map(|m| m.payload), Some(2));
    }

    #[test]
    fn measure_should_count_an_array_a_string_and_a_class_as_a_pointer() {
        let class = Type::intern(TypeKind::Class(crate::tir::ClassId(0), TypeArgs::NONE));
        let array = Type::array(enum_type(0));
        let measured = measure(&[holding("A", 0, vec![array, Type::STRING, class])]);
        assert_eq!(measured[0].map(|m| m.payload), Some(24));
    }

    #[test]
    fn measure_should_count_a_boxed_field_as_a_pointer_and_a_flag_for_each_nested_optional() {
        let nested = Type::optional(Type::optional(enum_type(0)));
        let measured = measure(&[declared(
            "A",
            0,
            vec![vec![(enum_type(0), true), (nested, true)]],
        )]);
        // One pointer, then a flag beside a pointer.
        assert_eq!(measured[0].map(|m| m.payload), Some(8 + 16));
    }

    #[test]
    fn measure_should_follow_the_enums_that_a_field_holds_inline() {
        // Held after the holder in the list: the walk does not depend on the order.
        let enums = [
            pair("Outer", 0, 1),
            pair("Inner", 1, 2),
            holding("Leaf", 2, vec![Type::I64]),
        ];
        let measured = measure(&enums);
        let payloads: Vec<_> = measured.iter().map(|m| m.map(|m| m.payload)).collect();
        // Leaf: tag then `i64` = 16. Inner: two of Leaf = 32, with the tag 40. Outer: 80.
        assert_eq!(payloads, [Some(80), Some(32), Some(8)]);
    }

    #[test]
    fn measure_should_give_no_bound_for_a_type_that_is_not_final() {
        let param = Type::intern(TypeKind::Param(crate::symbol::Symbol::intern("T"), 0));
        // The declaration of `Plain` has a bound; a use of it with arguments, as in a generic
        // declaration that `mono` has not instantiated, has none.
        let generic = Type::intern(TypeKind::Enum(EnumId(3), TypeArgs::intern(&[Type::I64])));
        let enums = [
            holding("Param", 0, vec![param]),
            holding("Generic", 1, vec![generic]),
            holding("Holder", 2, vec![enum_type(0)]),
            holding("Plain", 3, vec![Type::I64]),
        ];
        let bounded: Vec<_> = measure(&enums).iter().map(Option::is_some).collect();
        assert_eq!(bounded, [false, false, false, true]);
    }

    /// Enum `k` holds enums `k + 1` and `k + 2`: each enum is held by two others, so a walk
    /// that measures an enum again for each holder takes time exponential in `n`, and one that
    /// forgets the bounds between roots takes quadratic time.
    #[test]
    fn measure_should_pop_a_stack_entry_a_bounded_number_of_times_for_each_enum() {
        let steps = |n: usize| {
            let mut enums: Vec<_> = (0..n - 1)
                .map(|k| {
                    let held = (k + 1..=(k + 2).min(n - 1)).map(enum_type).collect();
                    holding("E", k, held)
                })
                .collect();
            enums.push(holding("Last", n - 1, vec![Type::I64]));
            MEASURE_STEPS.set(0);
            assert!(measure(&enums).iter().all(Option::is_some));
            MEASURE_STEPS.get()
        };
        crate::mir::tests::assert_near_linear(steps(100), steps(200), steps(300));
    }

    #[test]
    fn measure_should_give_no_bound_for_a_cycle_of_inline_fields() {
        let enums = [pair("A", 0, 1), pair("B", 1, 0)];
        assert!(measure(&enums).iter().all(Option::is_none));
    }

    #[test]
    fn measure_should_stay_over_the_limit_when_the_size_overflows() {
        // 70 enums that each hold two of the next: 2^70 bytes cannot be a `u64`.
        let mut enums: Vec<_> = (0..70).map(|n| pair("E", n, n + 1)).collect();
        enums.push(holding("Last", 70, vec![Type::I64]));
        let measured = measure(&enums);
        assert!(measured[0].is_some_and(|m| m.payload >= PAYLOAD_LIMIT));
    }

    #[test]
    fn tag_bytes_should_follow_the_variant_count() {
        let sizes: Vec<_> = [1, 256, 257, 65_536, 65_537].map(tag_bytes).into();
        assert_eq!(sizes, [1, 1, 2, 2, 4]);
    }

    #[test]
    fn oversized_should_flag_a_payload_that_equals_the_limit() {
        // The payload of `A` is two `i64`s: 16 bytes.
        let enums = [holding("A", 0, vec![Type::I64, Type::I64])];
        assert_eq!(oversized_with(&enums, &[], 16).len(), 1);
        assert_eq!(oversized_with(&enums, &[], 17), []);
    }

    #[test]
    fn oversized_should_report_only_the_innermost_enum_of_a_chain() {
        // `Top` and `Mid` are over a limit of 100 because `Low` is; `Low` is the mistake.
        let enums = [
            pair("Top", 0, 1),
            pair("Mid", 1, 2),
            pair("Low", 2, 3),
            holding("Leaf", 3, vec![Type::I64]),
        ];
        let found = oversized_with(&enums, &[], 30);
        let names: Vec<_> = found
            .iter()
            .map(|o| (o.name.as_str(), o.holder.as_deref()))
            .collect();
        assert_eq!(names, [("Low", None)]);
        assert_eq!(found[0].at, Span::new(2, 3));
    }

    #[test]
    fn oversized_should_report_each_independent_chain() {
        let enums = [
            pair("A", 0, 1),
            holding("Leaf", 1, vec![Type::I64]),
            pair("B", 2, 1),
        ];
        let names: Vec<_> = oversized_with(&enums, &[], 16)
            .into_iter()
            .map(|o| o.name)
            .collect();
        assert_eq!(names, ["A", "B"]);
    }

    #[test]
    fn oversized_should_report_the_instances_of_one_declaration_once() {
        // Two instances of one generic enum have the span of its declaration.
        let enums = [
            pair("P<i64>", 5, 2),
            pair("P<u8>", 5, 2),
            holding("Leaf", 6, vec![Type::I64]),
        ];
        let found = oversized_with(&enums, &[], 16);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "P<i64>");
    }

    #[test]
    fn oversized_should_not_count_a_boxed_field() {
        // Held inline, `Boxed` would have a payload of 32 bytes. Boxed, it has two pointers.
        let boxed = declared(
            "Boxed",
            0,
            vec![vec![(enum_type(1), true), (enum_type(1), true)]],
        );
        let leaf = holding("Leaf", 1, vec![Type::I64]);
        assert_eq!(oversized_with(&[boxed, leaf], &[], 20), []);
    }

    #[test]
    fn oversized_should_not_report_an_enum_with_an_unknown_part() {
        let param = Type::intern(TypeKind::Param(crate::symbol::Symbol::intern("T"), 0));
        let enums = [holding("A", 0, vec![param, Type::I64, Type::I64])];
        assert_eq!(oversized_with(&enums, &[], 1), []);
    }

    #[test]
    fn oversized_should_name_the_user_enum_that_a_prelude_enum_holds() {
        // The payload of `User` is 16 bytes, under the limit of 20. A value of `User` is 24
        // bytes, so the payload of `Wrap` of the prelude is over.
        let base = PRELUDE_BASE as usize;
        let enums = [
            holding("User", 3, vec![Type::I64, Type::I64]),
            holding("Wrap", base, vec![enum_type(0)]),
        ];
        let found = oversized_with(&enums, &[], 20);
        assert_eq!(
            found,
            [Oversized {
                name: "User".to_string(),
                at: Span::new(3, 4),
                instance: false,
                holder: Some("Wrap".to_string()),
                used: None,
            }]
        );
    }

    #[test]
    fn oversized_should_follow_prelude_enums_to_the_user_enum_at_the_end() {
        // The payloads are 16 for `User`, 24 for `Inner` and 32 for `Outer`.
        let base = PRELUDE_BASE as usize;
        let enums = [
            holding("Outer", base, vec![enum_type(1)]),
            holding("Inner", base + 1, vec![enum_type(2)]),
            holding("User", 3, vec![Type::I64, Type::I64]),
        ];
        let found = oversized_with(&enums, &[], 32);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "User");
        assert_eq!(found[0].holder.as_deref(), Some("Outer"));
    }

    #[test]
    fn oversized_should_name_the_prelude_enum_when_the_chain_ends_in_no_user_enum() {
        // The payloads are 8 for `Inner`, 16 for `Middle` and 24 for `Outer`. A value is 8 bytes
        // more than its payload. Only `Outer` is over the limit of 20, so the walk steps into
        // `Middle` and `Inner`, which are under it, and then holds no enum.
        let base = PRELUDE_BASE as usize;
        let enums = [
            holding("Outer", base, vec![enum_type(1)]),
            holding("Middle", base + 1, vec![enum_type(2)]),
            holding("Inner", base + 2, vec![Type::I64]),
        ];
        let found = oversized_with(&enums, &[], 20);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Outer");
        assert_eq!(found[0].holder, None);
    }

    #[test]
    fn oversized_should_name_a_prelude_enum_that_is_over_the_limit_by_its_own_fields() {
        let base = PRELUDE_BASE as usize;
        let enums = [holding("Big", base, vec![Type::I64, Type::I64])];
        let found = oversized_with(&enums, &[], 16);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Big");
        assert_eq!(found[0].holder, None);
    }

    /// `enums` as instances of a generic enum, which is what `mono` makes.
    fn instances<const N: usize>(enums: [Enum; N]) -> [Enum; N] {
        enums.map(|e| Enum {
            instance: true,
            ..e
        })
    }

    /// `Top` holds `Mid`, `Mid` holds `Low`, and `Low`, over a limit of 30, is the mistake. All
    /// four are instances.
    fn chain() -> [Enum; 4] {
        instances([
            pair("Top", 0, 1),
            pair("Mid", 1, 2),
            pair("Low", 2, 3),
            holding("Leaf", 3, vec![Type::I64]),
        ])
    }

    #[test]
    fn oversized_should_use_the_first_use_of_the_enum_over_the_limit() {
        let uses = [Some(Span::new(10, 11)), None, Some(Span::new(30, 31)), None];
        let found = oversized_with(&chain(), &uses, 30);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].used, Some(Span::new(30, 31)));
    }

    #[test]
    fn oversized_should_use_the_first_use_of_an_enum_that_holds_it_at_depth_two() {
        // `Low` and `Mid` have no use; `Top` holds `Low` through `Mid`.
        let uses = [Some(Span::new(10, 11)), None, None, None];
        let found = oversized_with(&chain(), &uses, 30);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].used, Some(Span::new(10, 11)));
    }

    #[test]
    fn oversized_should_use_the_use_of_a_holder_that_is_nearer_than_the_use_of_another() {
        // `Low` is held by `Near`, which has a use, and by `Far`, which `Outer` holds, and
        // `Outer` has a use too. `Far` comes first in the order of the enums, so a depth first
        // walk would find the use of `Outer`. The use of `Near` is one step from `Low`, and the
        // one of `Outer` is two. When the distances tie, either use may win: this test has none.
        let enums = instances([
            pair("Outer", 0, 1),
            pair("Far", 1, 3),
            pair("Near", 2, 3),
            pair("Low", 3, 4),
            holding("Leaf", 4, vec![Type::I64]),
        ]);
        let uses = [
            Some(Span::new(10, 11)),
            None,
            Some(Span::new(20, 21)),
            None,
            None,
        ];
        let found = oversized_with(&enums, &uses, 30);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Low");
        assert_eq!(found[0].used, Some(Span::new(20, 21)));
    }

    #[test]
    fn oversized_should_give_no_use_when_no_enum_that_holds_it_has_one() {
        let uses = [None, None, None, Some(Span::new(40, 41))];
        // The use of `Leaf`, which `Low` holds, is no use of `Low`.
        let found = oversized_with(&chain(), &uses, 30);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].used, None);
    }

    #[test]
    fn oversized_should_give_no_use_for_an_enum_that_is_no_instance() {
        // `Low` has a use, but no generic enum makes it: no instance, no use.
        let mut enums = chain();
        enums[2].instance = false;
        let uses = [None, None, Some(Span::new(30, 31)), None];
        let found = oversized_with(&enums, &uses, 30);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].used, None);
    }

    #[test]
    fn oversized_should_not_pass_the_use_of_an_enum_that_is_no_instance_to_the_one_it_holds() {
        // `Top` is no instance and has a use, and `Mid` holds nothing with a use: the field of a
        // non-generic enum is no use of the instance `Low`.
        let mut enums = chain();
        enums[0].instance = false;
        let uses = [Some(Span::new(10, 11)), None, None, None];
        let found = oversized_with(&enums, &uses, 30);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].used, None);
    }

    /// The enums `C0` to `C{n-1}` form a chain, and the last one holds `n` enums `R` that are over
    /// the limit, so there are `n` reports. Only `C0` has a use. A search for each report would
    /// walk the whole chain again, and the work would grow with the square of `n`.
    #[test]
    fn oversized_should_visit_each_enum_and_edge_a_bounded_number_of_times_for_many_reports() {
        let steps = |n: usize| {
            let mut enums: Vec<_> = (0..n - 1)
                .map(|k| holding("C", k, vec![enum_type(k + 1)]))
                .collect();
            // The enum `R` for report `k` has the id `n + k`.
            let reports = n..2 * n;
            enums.push(holding(
                "C",
                n - 1,
                reports.clone().map(enum_type).collect(),
            ));
            enums.extend(reports.map(|k| holding("R", k, vec![Type::I64, Type::I64])));
            for e in &mut enums {
                e.instance = true;
            }
            let mut uses = vec![None; enums.len()];
            uses[0] = Some(Span::new(5, 6));
            SPREAD_STEPS.set(0);
            let found = oversized_with(&enums, &uses, 16);
            assert_eq!(found.len(), n);
            assert!(found.iter().all(|o| o.used == Some(Span::new(5, 6))));
            SPREAD_STEPS.get()
        };
        crate::mir::tests::assert_near_linear(steps(100), steps(200), steps(300));
    }

    #[test]
    fn oversized_should_not_spread_uses_when_no_enum_has_one() {
        SPREAD_STEPS.set(0);
        let found = oversized_with(&chain(), &[], 30);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].used, None);
        // One step for the call, and none for the early return, the enums or the edges.
        assert_eq!(SPREAD_STEPS.get(), 1);
    }

    #[test]
    fn oversized_should_use_the_first_use_of_the_prelude_enum_that_holds_the_user_enum() {
        // A use of `User` is no use of `Wrap`, the instance that makes the payload too large.
        let base = PRELUDE_BASE as usize;
        let enums = instances([
            holding("User", 3, vec![Type::I64, Type::I64]),
            holding("Wrap", base, vec![enum_type(0)]),
        ]);
        let uses = [Some(Span::new(60, 61)), Some(Span::new(70, 71))];
        let found = oversized_with(&enums, &uses, 20);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].used, Some(Span::new(70, 71)));
    }
}
