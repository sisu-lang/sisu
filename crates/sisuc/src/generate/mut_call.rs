//! The `mut` call rule (S21): a statement that passes places to the `mut` parameters of a
//! generated function, or swaps a place with `exchange`. A `mut` parameter stores back a plain
//! write of at most the smallest value of its type (`item::mut_param`), and, for a local, what
//! it adds (`Adds`), which the call takes from the room and the growth of the caller's local. So
//! no bound of the value model moves. A field and an element take no addition: their bounds are
//! caps and element sizes that no call may raise.

use super::Gen;
use super::scope::{Adds, Ty};
use super::stmt::MUT_FUEL;

/// What a place is, besides its local.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// The local.
    Local,
    /// A `mut` field of the object that the local holds: the class and the field.
    Field(usize, usize),
    /// An element of the array that the local holds.
    Element,
}

/// A place that a `mut` argument may name.
#[derive(Clone)]
pub(crate) struct Place {
    kind: Kind,
    /// The local that the place starts at: an index into `Scope::vars`.
    root: usize,
    ty: Ty,
    /// The most size its value holds now.
    size: u64,
    /// The most size a plain write may store there.
    bound: u64,
    /// For a local: how many elements it holds at least, the room and the growth it has at each
    /// run of the loops since its declaration, and how many runs there are. Other places have
    /// none, and one run.
    len: u64,
    room: u64,
    grow: u64,
    times: u64,
}

/// What a `mut` parameter takes: its type, the most its place may hold, and what it adds.
#[derive(Clone)]
pub(crate) struct Want {
    pub(crate) ty: Ty,
    pub(crate) size: u64,
    pub(crate) adds: Adds,
}

impl Place {
    /// Whether `exchange` may store a new value there: no write may change an array local, which
    /// would break the length that an index relies on (`stmt::assignable`).
    fn exchangeable(&self) -> bool {
        self.kind != Kind::Local || !matches!(self.ty, Ty::Array(_))
    }

    /// Whether a `mut` parameter accepts it. A parameter that adds needs a local with the room,
    /// the growth and the length that it adds to.
    fn fits(&self, want: &Want) -> bool {
        let adds = want.adds;
        self.ty == want.ty
            && self.size <= want.size
            && (adds.is_none()
                || self.kind == Kind::Local
                    && self.len >= adds.len
                    && self.room >= adds.room
                    && self.grow >= adds.grow)
    }
}

#[cfg(test)]
thread_local! {
    /// How many steps the matching of places to `mut` parameters has taken on this thread.
    static PLACE_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn tick() {
    #[cfg(test)]
    PLACE_STEPS.set(PLACE_STEPS.get() + 1);
}

/// The most locals that `mut_epilogue` declares for a parameter that adds.
const MAX_LOCALS: usize = 3;

/// The most `mut` parameters a function has, and so the slots of a match: its sets of slots are
/// the subsets of `0..SLOTS`.
const SLOTS: usize = 3;

/// For each local, which slots (`wants`) it can serve with some place, as bits; 0 for a local of
/// `used`. A call never gives two `mut` arguments places at one local (E0171).
fn slot_masks(wants: &[Want], places: &[Place], used: &[usize]) -> Vec<u8> {
    debug_assert!(
        wants.len() <= SLOTS,
        "a function has at most {SLOTS} parameters"
    );
    let roots = places.iter().map(|place| place.root + 1).max().unwrap_or(0);
    let mut masks = vec![0; roots];
    for place in places {
        for (slot, want) in wants.iter().enumerate() {
            tick();
            if !used.contains(&place.root) && place.fits(want) {
                masks[place.root] |= 1 << slot;
            }
        }
    }
    masks
}

/// The sets of slots that locals of `masks` can fill, one slot each, as a bit set of the sets:
/// bit `s` is set when the slots of `s` can be filled. Adds one local, which `mask` gives the
/// slots of, to `reach`.
fn add_local(reach: u8, mask: u8) -> u8 {
    let mut next = reach;
    for set in 0..1u8 << SLOTS {
        for slot in 0..SLOTS {
            tick();
            if reach >> set & 1 == 1 && mask >> slot & 1 == 1 && set >> slot & 1 == 0 {
                next |= 1 << (set | 1 << slot);
            }
        }
    }
    next
}

/// Whether `before` and `after`, sets of sets of slots, hold two sets that are apart and fill
/// `rest` together.
fn complete(before: u8, after: u8, rest: u8) -> bool {
    (0..1u8 << SLOTS).any(|a| {
        (0..1u8 << SLOTS).any(|b| {
            tick();
            before >> a & 1 == 1 && after >> b & 1 == 1 && a & b == 0 && a | b == rest
        })
    })
}

/// Whether each of `wants` can take a place of `places` that starts at a local of its own, none
/// of them one of `used` (E0171: two `mut` arguments may not name one place or overlap). It adds
/// one local at a time to the sets of slots that are filled, so its work is in proportion to
/// the places.
fn placeable(wants: &[Want], places: &[Place], used: &[usize]) -> bool {
    let masks = slot_masks(wants, places, used);
    let full = (1u8 << wants.len()) - 1;
    let reach = masks.iter().fold(1, |reach, &mask| add_local(reach, mask));
    reach >> full & 1 == 1
}

impl Gen<'_> {
    /// The places of the point being written: the locals that a plain write may change, the
    /// elements of their arrays, and the `mut` fields of the class objects that locals hold.
    fn mut_places(&self) -> Vec<Place> {
        let mut places = Vec::new();
        for (root, var) in self.scope.vars.iter().enumerate() {
            let times = self.cost.times_since(var.loops).max(1);
            let whole = Place {
                kind: Kind::Local,
                root,
                ty: var.ty.clone(),
                size: var.size,
                bound: var.base,
                len: var.len,
                room: var.room / times,
                grow: var.grow / times,
                times,
            };
            let other = Place {
                len: 0,
                room: 0,
                grow: 0,
                times: 1,
                ..whole.clone()
            };
            if var.assignable {
                places.push(whole);
            }
            if let Ty::Array(element) = &var.ty
                && self.settable(var)
            {
                places.push(Place {
                    kind: Kind::Element,
                    ty: (**element).clone(),
                    size: var.elem,
                    ..other
                });
            }
        }
        for (root, class, f) in self.field_targets() {
            // `?.` is no place of a `mut` argument.
            if matches!(self.scope.vars[root].ty, Ty::Class { .. }) {
                let field = &self.scope.classes[class].fields[f];
                places.push(Place {
                    kind: Kind::Field(class, f),
                    root,
                    ty: field.ty.clone(),
                    size: field.cap,
                    bound: field.cap,
                    len: 0,
                    room: 0,
                    grow: 0,
                    times: 1,
                });
            }
        }
        places
    }

    /// What each `mut` parameter of the function `callee` takes.
    fn mut_wants(&self, callee: usize) -> Vec<Want> {
        let func = &self.scope.funcs[callee];
        let params = func.params.iter().zip(&func.muts);
        params
            .filter_map(|((ty, size), adds)| {
                adds.map(|adds| Want {
                    ty: ty.clone(),
                    size: *size,
                    adds,
                })
            })
            .collect()
    }

    /// The functions that a `mut` call may name now: each has a place for every `mut` parameter.
    fn mut_callees(&self, places: &[Place]) -> Vec<usize> {
        let mut callees = self.reachable();
        callees.retain(|&f| {
            self.scope.funcs[f].has_mut() && placeable(&self.mut_wants(f), places, &[])
        });
        callees
    }

    /// Whether a `mut` call, or an `exchange`, can be written here.
    pub(crate) fn mut_call_possible(&self) -> bool {
        let places = self.mut_places();
        places.iter().any(Place::exchangeable) || !self.mut_callees(&places).is_empty()
    }

    /// A place for each of `wants`, each at a local of its own, so that no two overlap and each
    /// of the later ones still has one. For each slot it matches the locals that remain, once
    /// from the front and once from the back, so that the places it considers cost work in
    /// proportion to their number.
    fn choose_places(&mut self, wants: &[Want], places: &[Place]) -> Vec<Place> {
        let mut chosen: Vec<Place> = Vec::new();
        let mut used = Vec::new();
        for (slot, want) in wants.iter().enumerate() {
            // The slots after this one.
            let rest = ((1u8 << wants.len()) - 1) & !((2u8 << slot) - 1);
            let masks: Vec<u8> = slot_masks(wants, places, &used)
                .into_iter()
                .map(|mask| mask & rest)
                .collect();
            let mut before = vec![1u8];
            for &mask in &masks {
                before.push(add_local(before[before.len() - 1], mask));
            }
            let mut after = vec![1u8];
            for &mask in masks.iter().rev() {
                after.push(add_local(after[after.len() - 1], mask));
            }
            let fitting: Vec<&Place> = places
                .iter()
                .filter(|place| {
                    // The rest, without the locals before and after this one.
                    !used.contains(&place.root)
                        && place.fits(want)
                        && complete(
                            before[place.root],
                            after[masks.len() - 1 - place.root],
                            rest,
                        )
                })
                .collect();
            let place = fitting[self.pick(fitting.len())].clone();
            used.push(place.root);
            chosen.push(place);
        }
        chosen
    }

    /// Writes `place`. A local goes into `locals`, with where the rewritten copy holds it.
    pub(crate) fn write_place(&mut self, place: &Place, locals: &mut Vec<(String, usize)>) {
        let var = &self.scope.vars[place.root];
        let (name, len) = (var.name.clone(), var.len);
        match place.kind {
            Kind::Local => {
                locals.push((name.clone(), self.w.at()));
                self.w.text(&name);
            }
            Kind::Field(class, f) => {
                let field = &self.scope.classes[class].fields[f].name;
                let text = format!("{name}.{field}");
                self.w.text(&text);
            }
            Kind::Element => {
                let at = self.upto(len - 1);
                self.w.text(&format!("{name}[{at}]"));
            }
        }
    }

    /// Takes from each local that a `mut` argument names what its parameter adds, at each run:
    /// the room of a `string`, the growth of an array. This comes before the other arguments,
    /// so that a statement in one of them takes no more than what is left.
    fn take_adds(&mut self, wants: &[Want], chosen: &[Place]) {
        for (want, place) in wants.iter().zip(chosen) {
            if place.kind == Kind::Local {
                let var = &mut self.scope.vars[place.root];
                var.room -= want.adds.room * place.times;
                var.grow -= want.adds.grow * place.times;
            }
        }
    }

    /// After the statements of `main`, now and then, a `mut` call of each function with a `mut`
    /// parameter that has places to take, so that every such function is called. It first
    /// declares a local for each parameter that adds something. It is written as `mut_statement`
    /// writes a call.
    pub(crate) fn mut_epilogue(&mut self) {
        for callee in 0..self.scope.funcs.len() {
            if !self.scope.funcs[callee].has_mut() || self.mut_pick(4) == 0 {
                continue;
            }
            let fuel = std::mem::replace(&mut self.fuel, MUT_FUEL);
            self.swap_streams();
            // A local for each parameter that adds something, so that a place for it is there.
            for want in self.mut_wants(callee) {
                // A local may not have the room that the parameter adds to, so up to 3 are tried.
                for _ in 0..MAX_LOCALS {
                    let wanted = [want.clone()];
                    if want.adds.is_none() || placeable(&wanted, &self.mut_places(), &[]) {
                        break;
                    }
                    self.w.line();
                    self.cost.statement();
                    self.statement(|g| g.let_typed(want.ty.clone(), true));
                }
            }
            if placeable(&self.mut_wants(callee), &self.mut_places(), &[]) {
                self.w.line();
                self.cost.statement();
                self.statement(|g| g.mut_call_of(Some(callee)));
            }
            self.swap_streams();
            self.fuel = fuel;
        }
    }

    /// `f(depth, mut place, ...)`, or `let _ = f(...)` of a function with a result; or
    /// `let _ = exchange(mut place, e)`. Now and then, the rewritten copy copies each local
    /// in and out (decision 2).
    pub(crate) fn mut_call(&mut self) {
        self.mut_call_of(None);
    }

    /// `mut_call`, of the function `forced` if it is one.
    fn mut_call_of(&mut self, forced: Option<usize>) {
        let places = self.mut_places();
        let callees = match forced {
            Some(callee) => vec![callee],
            None => self.mut_callees(&places),
        };
        let swaps: Vec<usize> = (0..places.len())
            .filter(|&p| places[p].exchangeable())
            .collect();
        let start = self.w.mark();
        let statements = self.statements;
        let mut locals = Vec::new();
        if !callees.is_empty() && (swaps.is_empty() || self.pick(3) != 0) {
            let callee = callees[self.pick(callees.len())];
            let wants = self.mut_wants(callee);
            let chosen = self.choose_places(&wants, &places);
            self.take_adds(&wants, &chosen);
            if self.scope.funcs[callee].ret != Ty::Unit {
                self.w.text("let _ = ");
            }
            self.invoke_with(callee, false, &chosen, &mut locals);
        } else {
            let place = &places[swaps[self.pick(swaps.len())]];
            self.w.text("let _ = exchange(mut ");
            self.write_place(place, &mut locals);
            self.w.text(", ");
            self.expr(&place.ty, place.bound);
            self.w.text(")");
        }
        let copy = self.rewrite_pick(2) == 0;
        // The other arguments leave the locals as they are, unless a statement ran in one.
        let clean = self.statements == statements;
        self.w.mut_block(start, &locals, copy, clean);
    }
}

#[cfg(test)]
mod tests {
    use super::{Adds, Gen, Kind, PLACE_STEPS, Place, Ty, Want, placeable};
    use crate::generate::DEPTHS;
    use crate::generate::coverage::seed;
    use crate::generate::scope::{Func, Var};

    fn place(root: usize, ty: Ty, size: u64) -> Place {
        Place {
            kind: Kind::Local,
            root,
            ty,
            size,
            bound: 0,
            len: 0,
            room: 0,
            grow: 0,
            times: 1,
        }
    }

    fn want(ty: Ty, size: u64) -> Want {
        Want {
            ty,
            size,
            adds: Adds::default(),
        }
    }

    /// GIVEN two `mut` parameters of `i64` and two places that start at one local
    /// WHEN each parameter takes a place
    /// THEN none can, because two arguments of a call may not overlap (E0171)
    #[test]
    fn placeable_should_refuse_two_places_of_one_local() {
        let wants = [want(Ty::I64, 0), want(Ty::I64, 0)];
        let one = [place(0, Ty::I64, 0), place(0, Ty::I64, 0)];
        assert!(!placeable(&wants, &one, &[]));
        let two = [place(0, Ty::I64, 0), place(1, Ty::I64, 0)];
        assert!(placeable(&wants, &two, &[]));
    }

    /// GIVEN a `mut` parameter that takes at most 16, and places of the same type
    /// WHEN it takes a place
    /// THEN only a place that holds at most 16 fits
    #[test]
    fn placeable_should_refuse_a_place_larger_than_the_parameter_takes() {
        let wants = [want(Ty::String, 16)];
        assert!(!placeable(&wants, &[place(0, Ty::String, 17)], &[]));
        assert!(placeable(&wants, &[place(0, Ty::String, 16)], &[]));
    }

    /// GIVEN parameters of `i64` and `string`, a local that has a place of each type, and another
    /// local that has one of `i64`
    /// WHEN each parameter takes a place at a local of its own
    /// THEN they can, the `string` at the first local and the `i64` at the other, and cannot when
    /// the first local is used
    #[test]
    fn placeable_should_match_each_parameter_to_a_local_of_its_own() {
        let wants = [want(Ty::I64, 0), want(Ty::String, 0)];
        let places = [
            place(0, Ty::I64, 0),
            place(0, Ty::String, 0),
            place(1, Ty::I64, 0),
        ];
        assert!(placeable(&wants, &places, &[]));
        assert!(!placeable(&wants, &places, &[0]));
    }

    /// GIVEN a `mut` parameter that adds 16 chars to a `string`, and a local with room for 15
    /// WHEN the parameter takes a place
    /// THEN a local with room for 16, or the same room at the loops it runs in, takes it: not a
    /// field, which has no room to give
    #[test]
    fn placeable_should_refuse_a_place_without_the_room_a_parameter_adds() {
        let adds = Adds {
            room: 16,
            ..Adds::default()
        };
        let wants = [Want {
            adds,
            ..want(Ty::String, 100)
        }];
        let local = |room| Place {
            room,
            ..place(0, Ty::String, 0)
        };
        assert!(!placeable(&wants, &[local(15)], &[]));
        assert!(placeable(&wants, &[local(16)], &[]));
        let field = Place {
            kind: Kind::Field(0, 0),
            room: 16,
            ..place(0, Ty::String, 0)
        };
        assert!(!placeable(&wants, &[field], &[]));
    }

    /// GIVEN a `mut` parameter that adds to an array 2 elements' length, 64 of growth and 16 of
    /// room, and a local array that has each
    /// WHEN the parameter takes a place
    /// THEN a local that lacks the length, the growth or the room is refused, each by its own
    /// clause
    #[test]
    fn placeable_should_refuse_an_array_without_the_length_or_growth_a_parameter_adds() {
        let ty = Ty::Array(Box::new(Ty::String));
        let adds = Adds {
            room: 16,
            grow: 64,
            len: 2,
        };
        let wants = [Want {
            adds,
            ..want(ty.clone(), 100)
        }];
        let array = |len, room, grow| Place {
            len,
            room,
            grow,
            ..place(0, ty.clone(), 0)
        };
        assert!(placeable(&wants, &[array(2, 16, 64)], &[]));
        assert!(!placeable(&wants, &[array(1, 16, 64)], &[]), "length");
        assert!(!placeable(&wants, &[array(2, 16, 63)], &[]), "growth");
        assert!(!placeable(&wants, &[array(2, 15, 64)], &[]), "room");
    }

    /// A `Gen` in `main` that knows the local `v1`, `ty` with the room and growth `room` and
    /// `grow`, and `f1`, which has one `mut` parameter of `ty` that adds `adds`.
    fn gen_with_local(data: &[u8], ty: Ty, (room, grow): (u64, u64), adds: Adds) -> Gen<'_> {
        let mut g = Gen::new(data);
        let mut var = Var::new("v1".to_string(), ty.clone(), true, 0, room, 0);
        var.size += grow;
        var.grow = grow;
        var.len = adds.len;
        g.scope.vars.push(var);
        g.scope.funcs.push(Func {
            name: "f1".to_string(),
            owner: None,
            params: vec![(ty, 100)],
            muts: vec![Some(adds)],
            ret: Ty::Unit,
            ret_size: 0,
            cost: [1; DEPTHS],
        });
        g
    }

    /// GIVEN a local `string` with room for 16 chars, and a function whose `mut` parameter adds 16
    /// WHEN a `mut` call passes the local
    /// THEN the call takes the room, so no block in another argument can spend it again
    /// AND the same holds for the growth of an array
    #[test]
    fn a_mut_call_should_take_what_its_parameter_adds_from_the_local() {
        let array = Ty::Array(Box::new(Ty::I64));
        let rows = [
            (
                Ty::String,
                (16, 0),
                Adds {
                    room: 16,
                    ..Adds::default()
                },
            ),
            (
                array,
                (0, 64),
                Adds {
                    grow: 64,
                    len: 1,
                    ..Adds::default()
                },
            ),
        ];
        for (ty, (room, grow), adds) in rows {
            let mut g = gen_with_local(&[], ty, (room, grow), adds);
            g.statement(|g| g.mut_call_of(Some(0)));
            assert_eq!(g.w.out.program, "f1(0, mut v1)");
            assert_eq!((g.scope.vars[0].room, g.scope.vars[0].grow), (0, 0));
        }
    }

    /// GIVEN a `mut` call of `f1`, whose other argument is an `i64` that the generator writes
    /// WHEN it is written from many seeds
    /// THEN a call whose other argument holds a statement, so a line break, has no copy of the
    /// local in the rewritten program, as the seed 404 of the review had one that printed 3 and
    /// not 7
    /// AND some call without one has a copy, and some with one is written
    #[test]
    fn a_mut_call_should_not_copy_a_local_that_a_statement_in_an_argument_can_change() {
        let (mut with, mut without) = (0, 0);
        for n in 0..256 {
            let data = seed(n);
            let mut g = gen_with_local(&data, Ty::I64, (0, 0), Adds::default());
            g.scope.funcs[0].params.push((Ty::I64, 0));
            g.scope.funcs[0].muts.push(None);
            g.statement(|g| g.mut_call_of(Some(0)));
            let copied = g.w.out.rewritten.contains("let mut w");
            if g.w.out.program.contains('\n') {
                assert!(!copied, "{}", g.w.out.rewritten);
                with += 1;
            } else if copied {
                without += 1;
            }
        }
        assert!(
            with > 0 && without > 0,
            "{with} with a statement, {without} copied"
        );
    }

    /// GIVEN `mut` parameters of `i64`, `i64` and `string`, and `n` places of `i64`, each at a
    /// local of its own, which no parameter of `string` can take
    /// WHEN `placeable` looks for a place for each parameter
    /// THEN its work grows in proportion to `n`, not to its cube: it has no backtracking
    /// over the places. The sizes are 10 times the 2 places that show the search.
    #[test]
    fn placeable_should_read_the_places_linearly() {
        let wants = [want(Ty::I64, 0), want(Ty::I64, 0), want(Ty::String, 0)];
        let reads = |n: usize| {
            let places: Vec<Place> = (0..n).map(|root| place(root, Ty::I64, 0)).collect();
            PLACE_STEPS.set(0);
            assert!(!placeable(&wants, &places, &[]));
            PLACE_STEPS.get()
        };
        crate::mir::tests::assert_near_linear(reads(20), reads(40), reads(60));
    }

    /// GIVEN the same parameters, and `n` places of `i64`, each at a local of its own, and one
    /// place of `string` at another
    /// WHEN `choose_places` picks a place for each parameter
    /// THEN its work grows in proportion to `n` too: it does not ask `placeable` about each place
    #[test]
    fn choose_places_should_read_the_places_linearly() {
        let wants = [want(Ty::I64, 0), want(Ty::I64, 0), want(Ty::String, 0)];
        let reads = |n: usize| {
            let mut places: Vec<Place> = (0..n).map(|root| place(root, Ty::I64, 0)).collect();
            places.push(place(n, Ty::String, 0));
            let mut g = Gen::new(&[]);
            PLACE_STEPS.set(0);
            assert_eq!(g.choose_places(&wants, &places).len(), 3);
            PLACE_STEPS.get()
        };
        crate::mir::tests::assert_near_linear(reads(20), reads(40), reads(60));
    }
}
