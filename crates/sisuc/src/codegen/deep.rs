//! Deep drops (ADR 0038): which drop functions count the drop depth, and the drop weight of
//! each. A drop function that counts adds its weight to a global depth while it runs.
//!
//! The walks here follow the nesting of the source types: a chain of enums that each hold the
//! next, or nested arrays. They call themselves once for each level, so each recursive step
//! goes through `ensure_stack`, as the other walks over the depth of a type do.
// Nothing outside the tests calls these functions yet.
#![expect(
    dead_code,
    reason = "the depth check in the drop functions calls them in the next change of #191"
)]

use rustc_hash::FxHashMap;

use super::Codegen;
use crate::conformance::ensure_stack;
use crate::tir::{ClassId, EnumId, Type, TypeKind};

/// The drop depth at which a counting drop function stops recursing and defers its block. The
/// depth counts frames. The value keeps the stack of a drop at the limit, plus the largest
/// drop weight, under a quarter of the 256 KiB stack of the small-stack tests (ADR 0038).
pub(super) const DEPTH_LIMIT: u64 = 256;

/// The drop decisions that `Codegen` has made so far. Each is made once for each type, so the
/// walk over the fields of all types is linear in the program.
#[derive(Default)]
pub(super) struct DropMemo {
    /// Whether the drop function of a class or an array counts. A cycle of classes is one entry,
    /// under its first class.
    counts: FxHashMap<Type, bool>,
    /// The drop weight of the drop function of a class or an array, under the same keys.
    weights: FxHashMap<Type, u32>,
    /// The frames that the release of an enum opens, for each component under its first member.
    enum_frames: FxHashMap<EnumId, u32>,
}

impl Codegen<'_, '_> {
    /// Whether a counted field, element or payload of type `ty` is a recursing part: its release
    /// can call a drop function without a bound that the source fixes. `holder` is the class
    /// whose field this is, and `None` for an array element. Counting is the default: a part is
    /// bounded only when its type, with each `?` removed, is on this list:
    /// - not counted, or a `string`: its drop calls nothing;
    /// - a class in the cycle of `holder`: `define_drop` puts it on a list and frees it in a
    ///   loop, in one frame;
    /// - an enum with a clear reach flag: the source fixes how deep the enums and strings in it
    ///   nest (see `tir::Enum::reaches`);
    /// - an array whose element type, with each `?` removed, is not counted or is bounded as the
    ///   element of an array: `[string]`, `[[i64]]` and `[[Kind]]` are bounded. The type fixes
    ///   how many arrays nest, and each level is a frame of the drop weight. A class element is
    ///   never bounded, because no array is in a class cycle: `[A]` and `[[A]]` recurse.
    ///
    /// Any other type is a recursing part, so a new kind of counted reference needs no rule.
    pub(super) fn recursing(&self, holder: Option<ClassId>, ty: Type) -> bool {
        let enums = &self.program.enums;
        let ty = ty.without_optionals();
        if !ty.is_counted(enums) {
            return false;
        }
        match ty.kind() {
            TypeKind::String => false,
            TypeKind::Class(..) => {
                holder.is_none_or(|class| self.class_cycles.link(class, ty).is_none())
            }
            // The elements are dropped in the frame of the array, so the array is as deep as one
            // element.
            TypeKind::Array(element) => ensure_stack(|| self.recursing(None, element)),
            TypeKind::Enum(id, _) => enums[id.0].reaches,
            _ => true,
        }
    }

    /// Whether the drop function of the class or array `ty` counts. An array counts when its
    /// element is a recursing part. A class counts when a field of it, or of a class in its
    /// cycle, is one: the drop of a cycle frees every object of it in one drain, so a chain can
    /// leave the cycle at a field of any member and come back through a class that has no such
    /// field.
    pub(super) fn counts(&self, ty: Type) -> bool {
        let key = self.drop_key(ty);
        if let Some(&counts) = self.deep.borrow().counts.get(&key) {
            return counts;
        }
        let counts = match key.kind() {
            TypeKind::Class(id, _) => self
                .cycle_fields(id)
                .any(|(class, field)| self.recursing(Some(class), field)),
            TypeKind::Array(element) => self.recursing(None, element),
            _ => unreachable!("only a class or an array should have a drop function"),
        };
        self.deep.borrow_mut().counts.insert(key, counts);
        counts
    }

    /// The drop weight of the drop function of the class or array `ty`: 1 for the function,
    /// plus the largest number of frames that a part can open before the next counting drop
    /// function starts. The depth counts frames, not objects, so the depth limit bounds the
    /// stack. The weight is the same for each class of a cycle, because one drain frees them.
    /// A link of the cycle opens no frame: `define_drop` lists it. A drain adds one frame.
    pub(super) fn drop_weight(&self, ty: Type) -> u32 {
        let key = self.drop_key(ty);
        if let Some(&weight) = self.deep.borrow().weights.get(&key) {
            return weight;
        }
        let parts = match key.kind() {
            TypeKind::Class(id, _) => {
                let drain = self
                    .class_cycles
                    .members(id)
                    .map_or(0, |m| u32::from(m.len() > 1));
                let fields = self.cycle_fields(id);
                let linked = |class, field| self.class_cycles.link(class, field).is_some();
                let fields = fields.filter(|&(class, field)| !linked(class, field));
                drain
                    + fields
                        .map(|(_, field)| self.part_frames(field))
                        .max()
                        .unwrap_or(0)
            }
            TypeKind::Array(element) => self.part_frames(element),
            _ => unreachable!("only a class or an array should have a drop function"),
        };
        let weight = 1 + parts;
        self.deep.borrow_mut().weights.insert(key, weight);
        weight
    }

    /// The type that the memos use for the drop function of `ty`, a class or an array: a class
    /// of a cycle is the first class of the cycle.
    fn drop_key(&self, ty: Type) -> Type {
        let TypeKind::Class(id, args) = ty.kind() else {
            return ty;
        };
        let first = self
            .class_cycles
            .members(id)
            .map_or(id, |members| members[0]);
        Type::intern(TypeKind::Class(first, args))
    }

    /// Each field type of the class `id`, or of each class of its cycle, with the class that
    /// holds it.
    fn cycle_fields(&self, id: ClassId) -> impl Iterator<Item = (ClassId, Type)> + '_ {
        let cycle = self.class_cycles.members(id);
        let classes = cycle.into_iter().flatten().copied();
        let classes = classes.chain(cycle.is_none().then_some(id));
        classes.flat_map(|class| {
            let fields = &self.program.classes[class.0].fields;
            fields.iter().map(move |field| (class, field.ty))
        })
    }

    /// The number of frames that the release of a value of type `ty` opens before the next
    /// counting drop function starts, or the whole drop when that function does not count.
    /// A `?` adds none: the caller tests the flag in its own frame. A frame we are not sure of
    /// counts as one. Too many frames only make a drop defer sooner.
    fn part_frames(&self, ty: Type) -> u32 {
        let ty = ty.without_optionals();
        if !ty.is_counted(&self.program.enums) {
            return 0;
        }
        // Each walk of a chain of types passes here, so this is the place for the guard.
        ensure_stack(|| match ty.kind() {
            // `release`, then `drop`: the drop of a string has no part.
            TypeKind::String => 2,
            // `release` calls the drop function, which counts the next frames if it counts.
            // A drop function that does not count has bounded parts (see `recursing`), so its
            // weight is a constant that the source fixes.
            TypeKind::Class(..) | TypeKind::Array(_) => {
                1 + if self.counts(ty) {
                    0
                } else {
                    self.drop_weight(ty)
                }
            }
            TypeKind::Enum(id, _) => self.enum_frames(id),
            _ => 1,
        })
    }

    /// The frames that `@sisu_rc.release.<E>` opens before the next counting drop function
    /// starts. It opens `release`. When the component of `E` has boxes, `release` calls `drain`,
    /// which calls `walk`, and `walk` calls itself in place for an inline link, one frame for
    /// each level of inline links. The number of members bounds those levels. A part in the
    /// component adds none: `walk` lists a box and `drain` frees it in the same frame. Every
    /// other payload field of a member adds its own frames. The release of a part in an earlier
    /// component is an enum release again, and the holds-inline graph has no cycle between
    /// components, so the count ends.
    fn enum_frames(&self, id: EnumId) -> u32 {
        let enums = &self.program.enums;
        let members = self.component(id);
        let key = members.as_ref().map_or(id, |members| members[0]);
        if let Some(&frames) = self.deep.borrow().enum_frames.get(&key) {
            return frames;
        }
        let boxes = members.as_ref().map_or(0, |members| {
            let levels = u32::try_from(members.len())
                .expect("a component should have fewer than 2^32 members");
            1 + levels
        });
        let cycle = enums[id.0].cycle;
        let in_component = |ty: Type| {
            cycle.is_some()
                && ty
                    .inline_enum()
                    .is_some_and(|held| enums[held.0].cycle == cycle)
        };
        let group = members
            .as_deref()
            .unwrap_or_else(|| std::slice::from_ref(&id));
        let parts = (group.iter())
            .flat_map(|member| &enums[member.0].variants)
            .flat_map(|variant| &variant.fields)
            .filter(|field| !field.boxed && !in_component(field.ty))
            .map(|field| self.part_frames(field.ty))
            .max()
            .unwrap_or(0);
        let frames = 1 + boxes + parts;
        self.deep.borrow_mut().enum_frames.insert(key, frames);
        frames
    }
}
