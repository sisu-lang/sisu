//! The statement rules. Each statement starts a line, spends fuel, and counts toward the cost of
//! the body it is in (decision 5).

use super::expr::Info;
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
    Compound,
    If,
    While,
    For,
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
        if self.scope.vars.iter().any(|var| var.assignable) {
            rules.push(Rule::Assign);
        }
        if self.scope.vars.iter().any(compoundable) {
            rules.push(Rule::Compound);
        }
        if self.blocks < MAX_BLOCKS {
            rules.push(Rule::If);
            if self.cost.depth() < MAX_LOOPS {
                rules.push(Rule::For);
                // A `while` is three statements: its counter, itself and the step.
                if self.fuel >= 2 {
                    rules.push(Rule::While);
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
                let ty = self.value_type();
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
                    .filter(|&v| self.scope.vars[v].assignable)
                    .collect();
                let pick = self.pick(vars.len());
                let var = &self.scope.vars[vars[pick]];
                let (ty, base) = (var.ty.clone(), var.base);
                self.w.text(&format!("{} = ", var.name));
                self.expr(&ty, base);
            }
            Rule::Compound => self.compound(),
            Rule::If => self.if_stmt(false),
            Rule::While => self.while_stmt(),
            Rule::For => self.for_stmt(),
            Rule::Call => {
                self.call(&Ty::Unit, 0);
            }
        }
    }

    /// `let [mut] v[: T] = e`. A mutable `string` gets a size for its first value and plain
    /// assignments, and room for what `+=` adds.
    fn let_stmt(&mut self) {
        const SIZES: [u64; 4] = [0, 16, 64, MAX_SIZE / 2];
        let ty = self.value_type();
        let mutable = self.pick(2) == 1;
        let name = self.scope.fresh("v");
        let keyword = if mutable { "let mut" } else { "let" };
        self.w.text(&format!("{keyword} {name}"));
        if ty == Ty::U8 || self.pick(2) == 1 {
            self.w.text(&format!(": {}", ty.name()));
        }
        self.w.text(" = ");
        let (base, room) = match (&ty, mutable) {
            (Ty::String, true) => (SIZES[self.pick(SIZES.len())], SIZES[self.pick(SIZES.len())]),
            (Ty::String, false) => (MAX_SIZE, 0),
            _ => (0, 0),
        };
        let init = self.expr(&ty, base);
        let base = if mutable { base } else { init.size };
        let var = Var::new(name, ty, mutable, base, room, self.cost.depth());
        debug_assert!(var.size <= MAX_SIZE, "a string may hold {} chars", var.size);
        self.scope.vars.push(var);
    }

    /// `v op= e` on a number, or `v += e` on a `string`, whose room is what each run of it,
    /// in every loop since `v`'s declaration, takes.
    pub(crate) fn compound(&mut self) {
        const OPS: [&str; 5] = [" += ", " -= ", " *= ", " /= ", " %= "];
        let vars: Vec<usize> = (0..self.scope.vars.len())
            .filter(|&v| compoundable(&self.scope.vars[v]))
            .collect();
        let v = vars[self.pick(vars.len())];
        let (name, ty, loops) = {
            let var = &self.scope.vars[v];
            (var.name.clone(), var.ty.clone(), var.loops)
        };
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
        self.expr(&Ty::Bool, 0);
        self.conditionally(|g| {
            g.w.text(" ");
            branch(g);
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
        self.conditionally(|g| {
            g.cost.enter_loop(count);
            g.loop_body("", Some(name));
            g.cost.exit_loop();
        });
    }

    /// A loop's body: `first`, if any, then statements, which may `break` and `continue`.
    fn loop_body(&mut self, first: &str, name: Option<String>) {
        let jumps = self.jumps;
        self.jumps.in_loop = true;
        self.open_block();
        if let Some(name) = name {
            let var = Var::new(name, Ty::I64, false, 0, 0, self.cost.depth());
            self.scope.vars.push(var);
        }
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

/// Whether `var` can take a compound assignment: a number, or a `string` with room.
fn compoundable(var: &Var) -> bool {
    var.assignable && matches!(var.ty, Ty::I64 | Ty::U8 | Ty::F64 | Ty::String)
}
