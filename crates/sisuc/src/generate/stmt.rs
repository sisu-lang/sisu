//! The statement rules. Each statement starts a line, spends fuel, and counts toward the cost of
//! the body it is in (decision 5).

use super::expr::{Info, array_size};
use super::scope::{Ty, Var};
use super::{Gen, MAX_BOUND, MAX_LOOPS, MAX_SIZE};

/// The most statements in a block, before its jump.
const MAX_STATEMENTS: usize = 8;
/// The most blocks nest, statements' and values' alike.
pub(crate) const MAX_BLOCKS: u32 = 4;

/// The jumps a statement may make at the point being written.
#[derive(Clone, Copy, Default)]
pub(crate) struct Jumps {
    /// `return`: in a statement block of a function or `main`, outside every value block.
    pub(crate) ret: bool,
    /// `break` and `continue`: in a loop's body, outside every value block in it.
    pub(crate) in_loop: bool,
}

#[derive(Clone, Copy)]
enum Rule {
    Print,
    Let,
    Discard,
    Inexact,
    Assign,
    /// `v[i] = e`.
    Element,
    Compound,
    If,
    While,
    WhileLet,
    For,
    ForArray,
    Call,
}

impl Gen<'_> {
    /// The statements of a block, each on a line of its own; then, if `jump`, now and then a
    /// jump, which ends the block so no code after it is unreachable (W0001).
    pub(crate) fn stmts(&mut self, jump: bool) {
        let mut count = 0;
        while count < MAX_STATEMENTS && self.pick(4) != 0 && self.spend(1) {
            self.stmt();
            count += 1;
        }
        if jump && self.pick(4) == 3 && self.spend(1) {
            self.w.line();
            if self.blocks < MAX_BLOCKS && self.pick(2) == 1 {
                self.cost.statement();
                self.if_stmt(true);
            } else {
                self.jump();
            }
        }
    }

    fn stmt(&mut self) {
        let mut rules = vec![Rule::Print, Rule::Let, Rule::Discard, Rule::Inexact];
        if self.scope.vars.iter().any(assignable) {
            rules.push(Rule::Assign);
        }
        if self.scope.vars.iter().any(has_elements) {
            rules.push(Rule::Element);
        }
        if !self.compound_targets().is_empty() {
            rules.push(Rule::Compound);
        }
        if self.blocks < MAX_BLOCKS {
            rules.push(Rule::If);
            if self.cost.depth() < MAX_LOOPS {
                rules.extend([Rule::For, Rule::ForArray]);
                // A `while` is three statements: its counter, itself and the step.
                if self.fuel >= 2 {
                    rules.extend([Rule::While, Rule::WhileLet]);
                }
            }
        }
        if !self.callees(&Ty::Unit, 0).is_empty() {
            rules.push(Rule::Call);
        }
        self.w.line();
        self.cost.statement();
        match rules[self.pick(rules.len())] {
            Rule::Print => {
                self.one_of(&["print(", "eprint("]);
                let ty = self.value_type();
                self.synth = true;
                self.expr(&ty, MAX_SIZE);
                self.w.text(")");
            }
            Rule::Let => self.let_stmt(),
            Rule::Discard => {
                self.w.text("let _ = ");
                let ty = self.any_type();
                self.synth = true;
                self.expr(&ty, MAX_SIZE);
            }
            Rule::Inexact => {
                self.w.text("let _ = ");
                if self.pick(2) == 0 {
                    self.inexact();
                } else {
                    const CLASSIFY: [&str; 3] = ["is_nan", "is_finite", "is_infinite"];
                    self.w.text("(");
                    self.inexact();
                    self.w.text(").");
                    self.one_of(&CLASSIFY);
                    self.w.text("()");
                }
            }
            Rule::Assign => {
                let vars: Vec<usize> = (0..self.scope.vars.len())
                    .filter(|&v| assignable(&self.scope.vars[v]))
                    .collect();
                let pick = self.pick(vars.len());
                let var = &self.scope.vars[vars[pick]];
                let (ty, base) = (var.ty.clone(), var.base);
                self.w.text(&format!("{} = ", var.name));
                self.expr(&ty, base);
            }
            Rule::Element => {
                let vars: Vec<usize> = (0..self.scope.vars.len())
                    .filter(|&v| has_elements(&self.scope.vars[v]))
                    .collect();
                let pick = self.pick(vars.len());
                let var = &self.scope.vars[vars[pick]];
                let (name, len, base) = (var.name.clone(), var.len, var.base);
                let Ty::Array(element) = var.ty.clone() else {
                    unreachable!("only an array has elements")
                };
                let at = self.upto(len - 1);
                self.w.text(&format!("{name}[{at}] = "));
                self.expr(&element, base);
            }
            Rule::Compound => self.compound(),
            Rule::If => self.if_stmt(false),
            Rule::While => self.while_stmt(),
            Rule::WhileLet => self.while_let(),
            Rule::For => self.for_stmt(),
            Rule::ForArray => self.for_array(),
            Rule::Call => {
                self.call(&Ty::Unit, 0);
            }
        }
    }

    /// `let [mut] v[: T] = e`. A mutable `string` gets a size for its first value and plain
    /// assignments, and room for what `+=` adds; a mutable optional of a size, a size.
    fn let_stmt(&mut self) {
        let ty = self.any_type();
        let mutable = self.pick(2) == 1;
        let name = self.scope.fresh("v");
        let keyword = if mutable { "let mut" } else { "let" };
        self.w.text(&format!("{keyword} {name}"));
        // A value of a type the checker infers needs one where it is not written.
        if ty.inferred() || self.pick(2) == 1 {
            self.w.text(&format!(": {}", ty.name()));
        }
        self.w.text(" = ");
        if mutable && let Ty::Array(element) = &ty {
            let element = (**element).clone();
            return self.array_let(name, ty, &element);
        }
        let (base, room) = match (&ty, mutable) {
            (Ty::String, true) => (SIZES[self.pick(SIZES.len())], SIZES[self.pick(SIZES.len())]),
            (_, true) if ty.sized() => (SIZES[self.pick(SIZES.len())], 0),
            (_, false) if ty.sized() => (MAX_SIZE, 0),
            _ => (0, 0),
        };
        let init = self.expr(&ty, base);
        let base = if mutable { base } else { init.size };
        let mut var = Var::new(name, ty, mutable, base, room, self.cost.depth());
        var.len = init.len;
        debug_assert!(var.size <= MAX_SIZE, "a value may reach {}", var.size);
        self.scope.vars.push(var);
    }

    /// The literal or fill of a mutable array of `len` elements, each at most `base` but for
    /// what `+=` adds to them, all of them within the `string` elements' room.
    fn array_let(&mut self, name: String, ty: Ty, element: &Ty) {
        const BASES: [u64; 3] = [0, 16, 64];
        let base = if element.sized() {
            BASES[self.pick(BASES.len())]
        } else {
            0
        };
        let fill = self.pick(2) == 1;
        let len = self.upto(if fill { MAX_BOUND } else { 4 });
        let size = array_size(len, base);
        let rooms: Vec<u64> = SIZES
            .into_iter()
            .filter(|room| *element == Ty::String && size + room <= MAX_SIZE || *room == 0)
            .collect();
        let room = rooms[self.pick(rooms.len())];
        self.w.text("[");
        if fill {
            self.expr(element, base);
            self.w.text(&format!("; {len}"));
        } else {
            for i in 0..len {
                if i > 0 {
                    self.w.text(", ");
                }
                self.expr(element, base);
            }
        }
        self.w.text("]");
        let mut var = Var::new(name, ty, true, base, room, self.cost.depth());
        var.size = size + room;
        var.len = len;
        self.scope.vars.push(var);
    }

    /// The places a compound assignment may target: a local, `false`, or one of its elements,
    /// `true`.
    fn compound_targets(&self) -> Vec<(usize, bool)> {
        let vars = &self.scope.vars;
        (0..vars.len())
            .filter_map(|v| {
                let var = &vars[v];
                match &var.ty {
                    ty if !var.assignable || !compoundable(ty) => None,
                    Ty::Array(_) if var.len == 0 => None,
                    Ty::Array(_) => Some((v, true)),
                    _ => Some((v, false)),
                }
            })
            .collect()
    }

    /// `v op= e` on a number, or `v += e` on a `string`, whose room is what each run of it,
    /// in every loop since `v`'s declaration, takes; or the same on an element of an array.
    pub(crate) fn compound(&mut self) {
        const OPS: [&str; 5] = [" += ", " -= ", " *= ", " /= ", " %= "];
        let targets = self.compound_targets();
        let (v, element) = targets[self.pick(targets.len())];
        let (mut name, mut ty, loops, len) = {
            let var = &self.scope.vars[v];
            (var.name.clone(), var.ty.clone(), var.loops, var.len)
        };
        if element {
            name = format!("{name}[{}]", self.upto(len - 1));
            if let Ty::Array(inner) = ty {
                ty = *inner;
            }
        }
        if ty == Ty::String {
            let times = self.cost.times_since(loops);
            self.w.text(&format!("{name} += "));
            let max = self.scope.vars[v].append_max(times);
            // While `e` is written, `v` reads at its full size, and a `+=` in `e` adds nothing.
            let room = std::mem::take(&mut self.scope.vars[v].room);
            let added: Info = self.expr(&Ty::String, max);
            let var = &mut self.scope.vars[v];
            var.room = room;
            var.append(added.size, times);
        } else {
            self.w.text(&name);
            self.one_of(&OPS);
            self.expr(&ty, 0);
        }
    }

    /// `if c { ... } [else if d { ... }] [else { ... }]`. Every branch but an `else` may end in
    /// a jump: an `if` that cannot finish leaves a value block after it no value (E0043). If
    /// `last`, the block's last statement, every branch ends in a jump and the `else` is there.
    fn if_stmt(&mut self, last: bool) {
        let branch = |g: &mut Self| {
            if last {
                g.jumping_block();
            } else {
                g.statement_block(true);
            }
        };
        self.w.text("if ");
        let (_, binding) = self.condition();
        self.conditionally(|g| {
            g.w.text(" ");
            let vars = g.scope.vars.len();
            g.scope.vars.extend(binding);
            branch(g);
            g.scope.vars.truncate(vars);
            while g.pick(3) == 2 && g.spend(1) {
                g.w.text(" else if ");
                g.expr(&Ty::Bool, 0);
                g.w.text(" ");
                branch(g);
            }
            if last || g.pick(2) == 1 {
                g.w.text(" else ");
                if last {
                    g.jumping_block();
                } else {
                    g.statement_block(false);
                }
            }
        });
    }

    /// `let mut c = n` and `while c > 0 [&& cond] { c -= 1 ... }`: at most `MAX_BOUND` runs of
    /// the body, and one more of the condition (decision 5).
    fn while_stmt(&mut self) {
        self.spend(2);
        let bound = self.upto(MAX_BOUND);
        let counter = self.scope.fresh("v");
        self.w.text(&format!("let mut {counter} = {bound}"));
        self.w.line();
        self.cost.statement();
        self.w.text(&format!("while {counter} > 0"));
        let var = Var::new(counter.clone(), Ty::I64, false, 0, 0, self.cost.depth());
        self.scope.vars.push(var);
        self.conditionally(|g| {
            if g.pick(2) == 1 {
                g.w.text(" && ");
                g.cost.enter_condition(bound);
                g.expr(&Ty::Bool, 0);
                g.cost.exit_loop();
            }
            g.w.text(" ");
            g.cost.enter_loop(bound);
            g.loop_body(&format!("{counter} -= 1"), None);
            g.cost.exit_loop();
        });
    }

    /// `let mut c = n` and `while let x = (if c > 0 { e } else { None }) { c -= 1 ... }`:
    /// `while_stmt`'s bounds, and `x` bound to each `e`.
    fn while_let(&mut self) {
        self.spend(2);
        let bound = self.upto(MAX_BOUND);
        let counter = self.scope.fresh("v");
        self.w.text(&format!("let mut {counter} = {bound}"));
        self.w.line();
        self.cost.statement();
        let payload = match self.any_type() {
            Ty::Optional(payload) => *payload,
            ty => ty,
        };
        let name = if self.pick(4) == 0 {
            None
        } else {
            Some(self.scope.fresh("v"))
        };
        let pattern = name.as_deref().unwrap_or("_");
        self.w
            .text(&format!("while let {pattern} = (if {counter} > 0 {{ "));
        let var = Var::new(counter.clone(), Ty::I64, false, 0, 0, self.cost.depth());
        self.scope.vars.push(var);
        self.conditionally(|g| {
            g.cost.enter_condition(bound);
            // The value has no expected type, and an `if` takes the type of its first branch.
            g.synth = true;
            let value = g.expr(&payload, MAX_SIZE);
            g.cost.exit_loop();
            g.w.text(" } else { None }) ");
            let loops = g.cost.depth() + 1;
            let binding = name.map(|name| Var::new(name, payload, false, value.size, 0, loops));
            g.cost.enter_loop(bound);
            g.loop_body(&format!("{counter} -= 1"), binding);
            g.cost.exit_loop();
        });
    }

    /// `for v in a..b` or `for v in a..=b`, with literal ends at most `MAX_BOUND` apart, now
    /// and then at an end of `i64`'s range.
    fn for_stmt(&mut self) {
        let inclusive = self.pick(2) == 1;
        let count = self.upto(MAX_BOUND);
        // An inclusive range of `count` values ends at `start + count - 1`.
        let span = if inclusive {
            i128::from(count) - 1
        } else {
            i128::from(count)
        };
        let start = match self.pick(4) {
            0 => 0,
            1 => i128::from(self.int(-5, 5)),
            2 => i128::from(i64::MAX) - span.max(0),
            _ => i128::from(i64::MIN) - span.min(0),
        };
        let name = self.scope.fresh("v");
        let dots = if inclusive { "..=" } else { ".." };
        self.w
            .text(&format!("for {name} in {start}{dots}{} ", start + span));
        let var = Var::new(name, Ty::I64, false, 0, 0, self.cost.depth() + 1);
        self.conditionally(|g| {
            g.cost.enter_loop(count);
            g.loop_body("", Some(var));
            g.cost.exit_loop();
        });
    }

    /// `for v in [a, b, ...]` or `for v in [e; n]`, of at most `MAX_BOUND` elements.
    fn for_array(&mut self) {
        let element = self.any_type();
        let name = self.scope.fresh("v");
        self.w.text(&format!("for {name} in ["));
        // The source has no expected type.
        let (count, size) = if self.pick(2) == 0 {
            let most = (MAX_SIZE / (1 + element.least())).min(MAX_BOUND);
            let count = 1 + self.upto(most - 1);
            let each = (MAX_SIZE - count) / count;
            let mut size = 0;
            for i in 0..count {
                if i > 0 {
                    self.w.text(", ");
                }
                self.synth = true;
                size = size.max(self.expr(&element, each).size);
            }
            (count, size)
        } else {
            let count = self.upto(MAX_BOUND);
            self.synth = true;
            let each = (MAX_SIZE - count).checked_div(count).unwrap_or(MAX_SIZE);
            let size = self.expr(&element, each).size;
            self.w.text(&format!("; {count}"));
            (count, size)
        };
        self.w.text("] ");
        let var = Var::new(name, element, false, size, 0, self.cost.depth() + 1);
        self.conditionally(|g| {
            g.cost.enter_loop(count);
            g.loop_body("", Some(var));
            g.cost.exit_loop();
        });
    }

    /// A loop's body: `first`, if any, then statements, which may `break` and `continue`, with
    /// `var` in scope.
    fn loop_body(&mut self, first: &str, var: Option<Var>) {
        let jumps = self.jumps;
        self.jumps.in_loop = true;
        self.open_block();
        self.scope.vars.extend(var);
        if !first.is_empty() {
            self.w.line();
            self.cost.statement();
            self.w.text(first);
        }
        self.stmts(true);
        self.close_block();
        self.jumps = jumps;
    }

    /// `{ statements }`, which may end in a jump if `jump`.
    fn statement_block(&mut self, jump: bool) {
        self.open_block();
        self.stmts(jump);
        self.close_block();
    }

    /// `{ statements jump }`.
    fn jumping_block(&mut self) {
        self.open_block();
        self.stmts(false);
        self.w.line();
        self.jump();
        self.close_block();
    }

    /// `{ statements value }`: no jump leaves it.
    /// `synth` passes on to the value.
    pub(crate) fn value_block(&mut self, ty: &Ty, max: u64, synth: bool) -> Info {
        let jumps = std::mem::take(&mut self.jumps);
        self.open_block();
        self.stmts(false);
        self.w.line();
        self.synth = synth;
        let info = self.expr(ty, max);
        self.close_block();
        self.jumps = jumps;
        Info {
            pure: false,
            ..info
        }
    }

    fn open_block(&mut self) {
        self.blocks += 1;
        self.w.open();
        self.scope.open();
    }

    fn close_block(&mut self) {
        self.scope.close();
        self.w.close();
        self.blocks -= 1;
    }

    /// A jump, the last statement of its block: `panic`, `todo`, `return`, `break` or
    /// `continue`.
    fn jump(&mut self) {
        self.cost.statement();
        let mut jumps = vec!["panic", "todo"];
        if self.jumps.ret {
            jumps.push("return");
        }
        if self.jumps.in_loop {
            jumps.extend(["break", "continue"]);
        }
        match jumps[self.pick(jumps.len())] {
            "panic" => {
                let message = self.message();
                self.w.text(&format!("panic({message})"));
            }
            "todo" => {
                if self.pick(2) == 0 {
                    self.w.text("todo");
                } else {
                    let message = self.message();
                    self.w.text(&format!("todo({message})"));
                }
            }
            "return" => {
                self.w.text("return");
                let func = self.scope.func.map(|f| &self.scope.funcs[f]);
                if let Some(func) = func.filter(|func| func.ret != Ty::Unit) {
                    let (ty, max) = (func.ret.clone(), func.ret_size);
                    self.w.text(" ");
                    self.expr(&ty, max);
                }
            }
            jump => self.w.text(jump),
        }
    }
}

/// The sizes a mutable `string` or optional may have, and the room of a `string`.
const SIZES: [u64; 4] = [0, 16, 64, MAX_SIZE / 2];

/// Whether `var` can take a plain assignment: an array's elements take them instead.
fn assignable(var: &Var) -> bool {
    var.assignable && !matches!(var.ty, Ty::Array(_))
}

/// Whether `var` is a mutable array with an element to assign to.
fn has_elements(var: &Var) -> bool {
    var.assignable && matches!(var.ty, Ty::Array(_)) && var.len > 0
}

/// Whether a compound assignment can target a value of `ty`, or the elements of an array of
/// it: a number, or a `string`.
fn compoundable(ty: &Ty) -> bool {
    match ty {
        Ty::Array(element) => matches!(**element, Ty::I64 | Ty::U8 | Ty::F64 | Ty::String),
        _ => matches!(ty, Ty::I64 | Ty::U8 | Ty::F64 | Ty::String),
    }
}
