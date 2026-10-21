//! The program and its items: `const`s, functions, and `main`.

use super::expr::CHARS;
use super::scope::{Const, Func, Ty, Var};
use super::stmt::Jumps;
use super::writer::Piece;
use super::{Cost, DEPTHS, Gen, MAX_NESTING, MAX_SIZE};

/// The most items before `main`.
const MAX_ITEMS: usize = 8;
/// The most parameters a function takes after `depth`.
const MAX_PARAMS: usize = 3;
/// The most lines of stdin.
const MAX_LINES: usize = 4;
/// The sizes a function's `string` parameter or result may have.
const SIZES: [u64; 3] = [0, 16, 100];

/// What a constant expression is known to be: an integer's or a `char`'s value, and a
/// `string`'s length.
#[derive(Clone, Copy)]
struct Folded {
    value: i64,
    size: u64,
}

impl Gen<'_> {
    /// Writes the program, and returns its cost (decision 5).
    pub(crate) fn program(&mut self) -> u64 {
        let mut items = 0;
        while items < MAX_ITEMS && self.pick(4) != 0 && self.spend(1) {
            if self.pick(2) == 0 {
                self.constant();
            } else {
                self.function();
            }
            items += 1;
        }
        self.w.text("fn main() ");
        self.open_body();
        self.stmts(false);
        self.w.close();
        self.w.line();
        // The stdin comes last, so the first bytes choose the program's shape.
        for _ in 0..self.pick(MAX_LINES + 1) {
            let line = self.stdin_line();
            self.stdin += &line;
            self.stdin.push('\n');
        }
        let costs: Vec<[u64; DEPTHS]> = self.scope.funcs.iter().map(|f| f.cost).collect();
        self.cost.of_main(&costs)
    }

    /// Opens the body of a function or `main`, which counts its own cost.
    fn open_body(&mut self) {
        self.cost = Cost::default();
        self.jumps = Jumps {
            ret: true,
            in_loop: false,
        };
        self.w.open();
    }

    /// `fn f(depth: i64, params) -> R { if depth <= 0 { return default } ... value }`.
    fn function(&mut self) {
        let name = self.scope.fresh("f");
        let mut params = Vec::new();
        for _ in 0..self.pick(MAX_PARAMS + 1) {
            let ty = self.value_type();
            let size = if ty == Ty::String {
                SIZES[self.pick(SIZES.len())]
            } else {
                0
            };
            params.push((self.scope.fresh("v"), ty, size));
        }
        let ret = match self.pick(Ty::VALUES.len() + 1) {
            0 => Ty::Unit,
            n => Ty::VALUES[n - 1].clone(),
        };
        let ret_size = if ret == Ty::String {
            SIZES[self.pick(SIZES.len())]
        } else {
            0
        };
        self.w.text(&format!("fn {name}(depth: i64"));
        for (param, ty, _) in &params {
            self.w.text(&format!(", {param}: {}", ty.name()));
        }
        self.w.text(")");
        if ret != Ty::Unit {
            self.w.text(&format!(" -> {}", ret.name()));
        }
        self.w.text(" ");
        self.scope.funcs.push(Func {
            name,
            params: params
                .iter()
                .map(|(_, ty, size)| (ty.clone(), *size))
                .collect(),
            ret: ret.clone(),
            ret_size,
            cost: [1; DEPTHS],
        });
        self.scope.func = Some(self.scope.funcs.len() - 1);
        self.scope.open();
        self.scope.vars.push(param("depth".to_string(), Ty::I64, 0));
        for (name, ty, size) in params {
            self.scope.vars.push(param(name, ty, size));
        }
        self.open_body();
        self.w.line();
        self.w.text("if depth <= 0 ");
        self.w.open();
        self.w.line();
        self.w.text(match ret {
            Ty::I64 | Ty::U8 => "return 0",
            Ty::F64 => "return 0.0",
            Ty::Bool => "return false",
            Ty::Char => "return 'a'",
            Ty::String => "return \"\"",
            Ty::Unit => "return",
        });
        self.w.close();
        self.stmts(false);
        if ret != Ty::Unit {
            self.w.line();
            self.expr(&ret, ret_size);
        }
        self.w.close();
        self.w.line();
        self.scope.close();
        self.scope.func = None;
        let costs: Vec<[u64; DEPTHS]> = self.scope.funcs.iter().map(|f| f.cost).collect();
        let cost = self.cost.of_function(&costs[..costs.len() - 1]);
        self.scope
            .funcs
            .last_mut()
            .expect("the function should have been pushed")
            .cost = cost;
    }

    /// `const c[: T] = e`, where `e` folds without a panic (`check/constant.rs`).
    fn constant(&mut self) {
        let ty = self.value_type();
        let name = self.scope.fresh("c");
        self.w.text(&format!("const {name}"));
        if ty == Ty::U8 || self.pick(2) == 1 {
            self.w.text(&format!(": {}", ty.name()));
        }
        self.w.text(" = ");
        let folded = self.const_expr(&ty, MAX_SIZE);
        self.w.line();
        self.scope.consts.push(Const {
            name,
            ty,
            value: folded.value,
            size: folded.size,
        });
    }

    /// A constant expression of type `ty`, at most `max` chars if a `string`: literals, earlier
    /// constants, operators and the conversions that cannot fail.
    fn const_expr(&mut self, ty: &Ty, max: u64) -> Folded {
        let names: Vec<usize> = (0..self.scope.consts.len())
            .filter(|&c| self.scope.consts[c].ty == *ty && self.scope.consts[c].size <= max)
            .collect();
        let compound = self.nesting < MAX_NESTING && self.spend(1);
        let choices = 1 + usize::from(!names.is_empty()) + if compound { 3 } else { 0 };
        let choice = self.pick(choices);
        self.nesting += 1;
        let folded = match choice {
            0 => self.const_literal(ty, max),
            1 if !names.is_empty() => {
                let pick = self.pick(names.len());
                let c = &self.scope.consts[names[pick]];
                let folded = Folded {
                    value: c.value,
                    size: c.size,
                };
                let name = c.name.clone();
                self.w.text(&name);
                folded
            }
            n => self.const_compound(ty, max, n % 3),
        };
        self.nesting -= 1;
        folded
    }

    fn const_literal(&mut self, ty: &Ty, max: u64) -> Folded {
        let (text, value, size) = match ty {
            Ty::I64 => {
                let (text, value) = self.int_literal();
                (text, value, 0)
            }
            Ty::U8 => {
                let (text, value) = self.u8_literal();
                (text, value, 0)
            }
            Ty::Char => {
                let (text, c) = CHARS[self.pick(CHARS.len())];
                (text.to_string(), i64::from(u32::from(c)), 0)
            }
            Ty::String => {
                let len = self.upto(max.min(8));
                (format!("\"{}\"", self.string_text(len, 26)), 0, len)
            }
            Ty::F64 | Ty::Bool | Ty::Unit => {
                self.literal(ty, max);
                return Folded { value: 0, size: 0 };
            }
        };
        self.w.text(&text);
        Folded { value, size }
    }

    /// One of three compound forms of a constant of type `ty`, chosen by `form`.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per type and form, so the constant forms read in one place"
    )]
    fn const_compound(&mut self, ty: &Ty, max: u64, form: usize) -> Folded {
        match (ty, form) {
            (Ty::I64 | Ty::U8, 0) => self.const_arithmetic(ty),
            (Ty::I64, 1) => {
                // `-(e)`, which overflows only on the smallest `i64`.
                let (piece, operand) = self.capture(|g| g.const_expr(ty, 0));
                if let Some(value) = operand.value.checked_neg() {
                    self.w.text("(-(");
                    self.w.paste(&piece);
                    self.w.text("))");
                    Folded { value, size: 0 }
                } else {
                    self.w.paste(&piece);
                    operand
                }
            }
            (Ty::I64, _) => {
                let u8_names = self.scope.consts.iter().any(|c| c.ty == Ty::U8);
                self.w.text("i64(");
                let operand = if u8_names && self.pick(2) == 1 {
                    let names: Vec<usize> = (0..self.scope.consts.len())
                        .filter(|&c| self.scope.consts[c].ty == Ty::U8)
                        .collect();
                    let pick = self.pick(names.len());
                    let c = &self.scope.consts[names[pick]];
                    let folded = Folded {
                        value: c.value,
                        size: 0,
                    };
                    let name = c.name.clone();
                    self.w.text(&name);
                    folded
                } else {
                    self.const_expr(&Ty::Char, 0)
                };
                self.w.text(")");
                operand
            }
            (Ty::F64, 0) => {
                const OPS: [&str; 5] = [" + ", " - ", " * ", " / ", " % "];
                self.w.text("(");
                self.const_expr(ty, 0);
                self.one_of(&OPS);
                self.const_expr(ty, 0);
                self.w.text(")");
                Folded { value: 0, size: 0 }
            }
            (Ty::F64, 1) => {
                self.w.text("(-(");
                self.const_expr(ty, 0);
                self.w.text("))");
                Folded { value: 0, size: 0 }
            }
            (Ty::F64, _) => {
                self.w.text("f64(");
                self.const_expr(&Ty::I64, 0);
                self.w.text(")");
                Folded { value: 0, size: 0 }
            }
            (Ty::Bool, 0) if self.pick(2) == 0 => {
                self.w.text("(!(");
                self.const_expr(ty, 0);
                self.w.text("))");
                Folded { value: 0, size: 0 }
            }
            (Ty::Bool, 0) => {
                self.w.text("(");
                self.const_expr(ty, 0);
                self.one_of(&[" && ", " || "]);
                self.const_expr(ty, 0);
                self.w.text(")");
                Folded { value: 0, size: 0 }
            }
            (Ty::Bool, 1) => {
                let operand = self.value_type();
                self.w.text("(");
                self.const_expr(&operand, MAX_SIZE);
                self.one_of(&[" == ", " != "]);
                self.const_expr(&operand, MAX_SIZE);
                self.w.text(")");
                Folded { value: 0, size: 0 }
            }
            (Ty::Bool, _) => {
                const ORDERED: [Ty; 5] = [Ty::I64, Ty::U8, Ty::F64, Ty::Char, Ty::String];
                let operand = ORDERED[self.pick(ORDERED.len())].clone();
                let ops = if self.pick(2) == 0 {
                    [" < ", " <= "]
                } else {
                    [" > ", " >= "]
                };
                self.w.text("(");
                self.const_expr(&operand, MAX_SIZE);
                for _ in 0..=self.pick(2) {
                    self.one_of(&ops);
                    self.const_expr(&operand, MAX_SIZE);
                }
                self.w.text(")");
                Folded { value: 0, size: 0 }
            }
            (Ty::String, _) => {
                self.w.text("(");
                let lhs = self.const_expr(ty, max);
                self.w.text(" + ");
                let rhs = self.const_expr(ty, max - lhs.size);
                self.w.text(")");
                Folded {
                    value: 0,
                    size: lhs.size + rhs.size,
                }
            }
            // A `char` constant has no compound form.
            (Ty::U8 | Ty::Char | Ty::Unit, _) => self.const_literal(ty, max),
        }
    }

    /// `(a op b)` of integers, with an operator under which it neither overflows its type nor
    /// divides by zero; `+` or `/` always holds one.
    fn const_arithmetic(&mut self, ty: &Ty) -> Folded {
        let (lhs_piece, lhs) = self.capture(|g| g.const_expr(ty, 0));
        let (rhs_piece, rhs) = self.capture(|g| g.const_expr(ty, 0));
        let (l, r) = (lhs.value, rhs.value);
        let results = [
            (" + ", l.checked_add(r)),
            (" - ", l.checked_sub(r)),
            (" * ", l.checked_mul(r)),
            (" / ", l.checked_div(r)),
            (" % ", l.checked_rem(r)),
        ];
        let fits = |n: &i64| *ty == Ty::I64 || (0..=255).contains(n);
        let valid: Vec<(&str, i64)> = results
            .iter()
            .filter_map(|(op, n)| n.filter(fits).map(|n| (*op, n)))
            .collect();
        let (op, value) = valid[self.pick(valid.len())];
        self.w.text("(");
        self.w.paste(&lhs_piece);
        self.w.text(op);
        self.w.paste(&rhs_piece);
        self.w.text(")");
        Folded { value, size: 0 }
    }

    /// Runs `write`, and returns what it wrote instead of writing it.
    fn capture<T>(&mut self, write: impl FnOnce(&mut Self) -> T) -> (Piece, T) {
        let outer = self.w.swap(Piece::default());
        let result = write(self);
        (self.w.swap(outer), result)
    }
}

/// A parameter: in scope, never assigned.
fn param(name: String, ty: Ty, size: u64) -> Var {
    Var {
        name,
        ty,
        assignable: false,
        base: size,
        room: 0,
        loops: 0,
    }
}
