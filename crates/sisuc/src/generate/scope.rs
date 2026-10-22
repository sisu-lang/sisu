//! What the generator knows at a point of the program: the names in scope, their types, the
//! sizes their values can reach, and the values it knows.

use super::DEPTHS;

/// A type the generator writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Ty {
    I64,
    U8,
    F64,
    Bool,
    Char,
    String,
    Unit,
    Array(Box<Ty>),
    /// Never of an optional: `optional_of` does not nest them.
    Optional(Box<Ty>),
}

impl Ty {
    /// The types that have values: every type but `unit`.
    pub(crate) const VALUES: [Ty; 6] = [Ty::I64, Ty::U8, Ty::F64, Ty::Bool, Ty::Char, Ty::String];

    /// The type as Sisu writes it.
    pub(crate) fn name(&self) -> String {
        match self {
            Ty::I64 => "i64".to_string(),
            Ty::U8 => "u8".to_string(),
            Ty::F64 => "f64".to_string(),
            Ty::Bool => "bool".to_string(),
            Ty::Char => "char".to_string(),
            Ty::String => "string".to_string(),
            Ty::Unit => "unit".to_string(),
            Ty::Array(element) => format!("[{}]", element.name()),
            Ty::Optional(payload) => format!("{}?", payload.name()),
        }
    }

    /// Whether a value of it written with no expected type may take another type: an integer
    /// literal is an `i64`, and `None` and `[]` cannot be inferred.
    pub(crate) fn inferred(&self) -> bool {
        matches!(self, Ty::U8 | Ty::Array(_) | Ty::Optional(_))
    }

    /// The size of its smallest value written with no expected type: an array holds an element.
    pub(crate) fn least(&self) -> u64 {
        match self {
            Ty::Array(element) => 1 + element.least(),
            Ty::Optional(payload) => payload.least(),
            Ty::I64 | Ty::U8 | Ty::F64 | Ty::Bool | Ty::Char | Ty::String | Ty::Unit => 0,
        }
    }

    /// Whether its values have a size: a `string`, an array, or an optional of one.
    pub(crate) fn sized(&self) -> bool {
        match self {
            Ty::String | Ty::Array(_) => true,
            Ty::Optional(payload) => payload.sized(),
            Ty::I64 | Ty::U8 | Ty::F64 | Ty::Bool | Ty::Char | Ty::Unit => false,
        }
    }

    /// The most chars `print` or an interpolation writes for a value of this type that holds at
    /// most `size` chars, with `precision` decimals for an `f64`: `-1.2345678901234568e-300` is
    /// the longest `f64` without one, and `1e308` with one has 309 digits before the point.
    pub(crate) fn printed(&self, size: u64, precision: Option<u64>) -> u64 {
        match self {
            Ty::I64 => 20,
            Ty::U8 => 3,
            Ty::F64 => precision.map_or(24, |p| 311 + p),
            Ty::Bool => 5,
            Ty::Char => 1,
            Ty::String => size,
            Ty::Unit | Ty::Array(_) | Ty::Optional(_) => {
                unreachable!("only a value of `Ty::VALUES` is printed")
            }
        }
    }
}

/// A local or a parameter.
pub(crate) struct Var {
    pub(crate) name: String,
    pub(crate) ty: Ty,
    /// A `let mut` that no loop counts with: the body may assign to it.
    pub(crate) assignable: bool,
    /// For a `string`: the most chars its first value or a plain assignment may hold. For a
    /// mutable array: the most size each element may take that way.
    pub(crate) base: u64,
    /// The most size its value can ever reach (decision 5): for a `string`, its base and its
    /// room at declaration.
    pub(crate) size: u64,
    /// For a `string`, or a mutable array of them: the chars that `+=` may still add, summed
    /// over every run of each one.
    pub(crate) room: u64,
    /// How many loops are around its declaration.
    pub(crate) loops: usize,
    /// For an array: how many elements it holds at least, which an index stays under.
    pub(crate) len: u64,
    /// For an array: the most size an element can reach.
    pub(crate) elem: u64,
}

impl Var {
    /// A local or a parameter, with `base` and `room` as a `string` has them, 0 otherwise.
    pub(crate) fn new(
        name: String,
        ty: Ty,
        assignable: bool,
        base: u64,
        room: u64,
        loops: usize,
    ) -> Var {
        Var {
            name,
            ty,
            assignable,
            base,
            size: base + room,
            room,
            loops,
            len: 0,
            elem: base + room,
        }
    }

    /// The most chars one run of a `+=` may add, when it runs `times` times.
    pub(crate) fn append_max(&self, times: u64) -> u64 {
        self.room / times.max(1)
    }

    /// Takes the room of a `+=` that adds at most `added` chars, `times` times.
    pub(crate) fn append(&mut self, added: u64, times: u64) {
        self.room -= added * times;
    }
}

/// A top-level `const`.
pub(crate) struct Const {
    pub(crate) name: String,
    pub(crate) ty: Ty,
    /// The value of an `i64` or a `u8`, or the code point of a `char`.
    pub(crate) value: i64,
    /// For a `string`: its length.
    pub(crate) size: u64,
}

/// A generated function: `depth: i64` first, then its parameters.
pub(crate) struct Func {
    pub(crate) name: String,
    /// Each parameter's type, and for a `string` the most chars an argument may hold.
    pub(crate) params: Vec<(Ty, u64)>,
    pub(crate) ret: Ty,
    /// For a `string` result: the most chars it holds.
    pub(crate) ret_size: u64,
    /// What a call costs at each `depth` it can pass (decision 5); set once the body is written.
    pub(crate) cost: [u64; DEPTHS],
}

#[derive(Default)]
pub(crate) struct Scope {
    pub(crate) consts: Vec<Const>,
    pub(crate) funcs: Vec<Func>,
    /// The locals in scope, innermost last.
    pub(crate) vars: Vec<Var>,
    /// Where each open block's locals start in `vars`.
    frames: Vec<usize>,
    /// The function being written, an index into `funcs`; `None` in `main` and in constants.
    pub(crate) func: Option<usize>,
    /// How many branches, loops and right operands of `&&`, `||` and `??` are around the point
    /// being written: code there may not run (decision 2).
    pub(crate) conditional: usize,
    /// The last number a name took.
    names: usize,
}

impl Scope {
    /// A name that no other name of the program has: `prefix` and a number.
    pub(crate) fn fresh(&mut self, prefix: &str) -> String {
        self.names += 1;
        format!("{prefix}{}", self.names)
    }

    pub(crate) fn open(&mut self) {
        self.frames.push(self.vars.len());
    }

    pub(crate) fn close(&mut self) {
        let start = self
            .frames
            .pop()
            .expect("each closed block should have been opened");
        self.vars.truncate(start);
    }
}
