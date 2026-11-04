//! The `mut` call rule (S21): a statement that passes places to the `mut` parameters of a
//! generated function, or swaps a place with `exchange`. A `mut` parameter is stored back at
//! most the smallest value of its type (`item::mut_param`), so a place never holds more than it
//! did and no size bound moves.

use super::Gen;
use super::scope::Ty;

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
}

impl Place {
    /// Whether `exchange` may store a new value there: no write may change an array local, which
    /// would break the length that an index relies on (`stmt::assignable`).
    fn exchangeable(&self) -> bool {
        self.kind != Kind::Local || !matches!(self.ty, Ty::Array(_))
    }

    /// Whether a `mut` parameter of type `want.0`, which takes at most `want.1`, accepts it.
    fn fits(&self, want: &(Ty, u64)) -> bool {
        self.ty == want.0 && self.size <= want.1
    }
}

/// Whether each of `wants` can take a place of `places` that starts at a local no earlier one
/// did, and none of `used` did (E0170: two `mut` arguments may not name one place or overlap).
fn placeable(wants: &[(Ty, u64)], places: &[Place], used: &mut Vec<usize>) -> bool {
    let Some((want, rest)) = wants.split_first() else {
        return true;
    };
    places.iter().any(|place| {
        if !place.fits(want) || used.contains(&place.root) {
            return false;
        }
        used.push(place.root);
        let found = placeable(rest, places, used);
        used.pop();
        found
    })
}

impl Gen<'_> {
    /// The places of the point being written: the locals that a plain write may change, the
    /// elements of their arrays, and the `mut` fields of the class objects that locals hold.
    fn mut_places(&self) -> Vec<Place> {
        let mut places = Vec::new();
        for (root, var) in self.scope.vars.iter().enumerate() {
            if var.assignable {
                places.push(Place {
                    kind: Kind::Local,
                    root,
                    ty: var.ty.clone(),
                    size: var.size,
                    bound: var.base,
                });
            }
            if let Ty::Array(element) = &var.ty
                && self.settable(var)
            {
                places.push(Place {
                    kind: Kind::Element,
                    root,
                    ty: (**element).clone(),
                    size: var.elem,
                    bound: var.base,
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
                });
            }
        }
        places
    }

    /// The type and size of each `mut` parameter of the function `callee`.
    fn mut_wants(&self, callee: usize) -> Vec<(Ty, u64)> {
        let func = &self.scope.funcs[callee];
        let params = func.params.iter().zip(&func.muts);
        params
            .filter(|&(_, &mutable)| mutable)
            .map(|(param, _)| param.clone())
            .collect()
    }

    /// The functions that a `mut` call may name now: each has a place for every `mut` parameter.
    fn mut_callees(&self, places: &[Place]) -> Vec<usize> {
        let mut callees = self.reachable();
        callees.retain(|&f| {
            self.scope.funcs[f].has_mut() && placeable(&self.mut_wants(f), places, &mut Vec::new())
        });
        callees
    }

    /// Whether a `mut` call, or an `exchange`, can be written here.
    pub(crate) fn mut_call_possible(&self) -> bool {
        let places = self.mut_places();
        places.iter().any(Place::exchangeable) || !self.mut_callees(&places).is_empty()
    }

    /// A place for each of `wants`, each at a local of its own, so that no two overlap.
    fn choose_places(&mut self, wants: &[(Ty, u64)], places: &[Place]) -> Vec<Place> {
        let mut chosen: Vec<Place> = Vec::new();
        let mut used = Vec::new();
        for (at, want) in wants.iter().enumerate() {
            let fitting: Vec<&Place> = places
                .iter()
                .filter(|place| {
                    if !place.fits(want) || used.contains(&place.root) {
                        return false;
                    }
                    used.push(place.root);
                    let found = placeable(&wants[at + 1..], places, &mut used);
                    used.pop();
                    found
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

    /// `f(depth, mut place, ...)`, or `let _ = f(...)` of a function with a result; or
    /// `let _ = exchange(mut place, e)`. Now and then, the rewritten copy copies each local
    /// in and out (decision 2).
    pub(crate) fn mut_call(&mut self) {
        let places = self.mut_places();
        let callees = self.mut_callees(&places);
        let swaps: Vec<usize> = (0..places.len())
            .filter(|&p| places[p].exchangeable())
            .collect();
        let start = self.w.mark();
        let mut locals = Vec::new();
        let own;
        if !callees.is_empty() && (swaps.is_empty() || self.pick(3) != 0) {
            let callee = callees[self.pick(callees.len())];
            let chosen = self.choose_places(&self.mut_wants(callee), &places);
            own = chosen.len();
            if self.scope.funcs[callee].ret != Ty::Unit {
                self.w.text("let _ = ");
            }
            self.invoke_with(callee, false, &chosen, &mut locals);
        } else {
            let place = &places[swaps[self.pick(swaps.len())]];
            own = 1;
            self.w.text("let _ = exchange(mut ");
            self.write_place(place, &mut locals);
            self.w.text(", ");
            self.expr(&place.ty, place.bound);
            self.w.text(")");
        }
        let copy = self.rewrite_pick(2) == 0;
        self.w.mut_block(start, own, &locals, copy);
    }
}

#[cfg(test)]
mod tests {
    use super::{Kind, Place, Ty, placeable};

    fn place(root: usize, ty: Ty, size: u64) -> Place {
        Place {
            kind: Kind::Local,
            root,
            ty,
            size,
            bound: 0,
        }
    }

    /// GIVEN two `mut` parameters of `i64` and two places that start at one local
    /// WHEN each parameter takes a place
    /// THEN none can, because two arguments of a call may not overlap (E0170)
    #[test]
    fn placeable_should_refuse_two_places_of_one_local() {
        let wants = [(Ty::I64, 0), (Ty::I64, 0)];
        let one = [place(0, Ty::I64, 0), place(0, Ty::I64, 0)];
        assert!(!placeable(&wants, &one, &mut Vec::new()));
        let two = [place(0, Ty::I64, 0), place(1, Ty::I64, 0)];
        assert!(placeable(&wants, &two, &mut Vec::new()));
    }

    /// GIVEN a `mut` parameter that takes at most 16, and places of the same type
    /// WHEN it takes a place
    /// THEN only a place that holds at most 16 fits
    #[test]
    fn placeable_should_refuse_a_place_larger_than_the_parameter_takes() {
        let wants = [(Ty::String, 16)];
        assert!(!placeable(
            &wants,
            &[place(0, Ty::String, 17)],
            &mut Vec::new()
        ));
        assert!(placeable(
            &wants,
            &[place(0, Ty::String, 16)],
            &mut Vec::new()
        ));
    }
}
