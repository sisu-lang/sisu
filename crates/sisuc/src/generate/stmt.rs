//! The statement rules. Each statement starts a line, spends fuel, and counts toward the cost of
//! the body it is in (decision 5).

use super::expr::{Info, array_size};
use super::scope::{Ty, Var};
use super::writer::{Rewrite, Site, allowed};
use super::{Gen, MAX_BOUND, MAX_LOOPS, MAX_SIZE};

/// What each `break` of a `loop` value gives: a value of `ty` of at most `max`, written with
/// `synth` (`Gen::expr`).
struct Value {
    ty: Ty,
    max: u64,
    synth: bool,
}

/// A loop whose body holds the point being written, which a `break` or `continue` there may
/// leave.
struct Target {
    /// Its loop label, if it has one.
    label: Option<String>,
    /// Whether a jump names the label, so it is used (W0008).
    used: bool,
    /// For a `loop` value, what its `break`s give.
    value: Option<Value>,
    /// The most size a `break` gave.
    size: u64,
}

/// The most statements in a block, before its jump.
const MAX_STATEMENTS: usize = 8;
/// The most blocks nest, statements' and values' alike.
pub(crate) const MAX_BLOCKS: u32 = 4;

/// The jumps a statement may make at the point being written.
#[derive(Default)]
pub(crate) struct Jumps {
    /// `return`: in a statement block of a function or `main`, outside every value block.
    pub(crate) ret: bool,
    /// `break` and `continue`: the loops whose body holds the point, innermost last, outside
    /// every value block in them.
    loops: Vec<Target>,
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
    /// `v.push(e)`, `v[i].push(e)`, or a guarded push onto a `mut` field.
    Push,
    /// `v.g = e`, or `v.g op= e` of a number.
    SetField,
    Compound,
    If,
    While,
    WhileLet,
    Loop,
    For,
    ForArray,
    Call,
}

impl Gen<'_> {
    /// The statements of a block, each on a line of its own; then, if `jump`, now and then a
    /// jump, which ends the block so no code after it is unreachable (W0001).
    pub(crate) fn stmts(&mut self, jump: bool) {
        let mut count = 0;
        while count < MAX_STATEMENTS && self.pick(8) != 0 && self.spend(1) {
            self.stmt();
            count += 1;
        }
        // A loop label that no jump names yet gets one here, so it is used (W0008).
        let innermost = self.jumps.loops.len().checked_sub(1);
        if jump && let Some(at) = innermost.filter(|&at| self.unused(at)) {
            self.w.line();
            self.cost.statement();
            let kind = ["continue", "break"][self.pick(2)];
            return self.loop_jump(kind, at, true);
        }
        // Where only a panic can jump, as in a value block, a block ends in one only now and then.
        let other = self.jumps.ret || innermost.is_some();
        if jump && self.pick(4) == 3 && (other || self.pick(32) == 31) && self.spend(1) {
            self.w.line();
            if self.blocks < MAX_BLOCKS && self.pick(2) == 1 {
                self.cost.statement();
                self.if_stmt(true);
            } else {
                self.jump();
            }
        }
    }

    /// `main`'s statements. Each takes a share of the fuel left, so the first cannot spend it
    /// all, and half of them call a function or a method.
    pub(crate) fn main_stmts(&mut self) {
        for count in 0..MAX_STATEMENTS {
            if self.pick(32) == 0 || !self.spend(1) {
                break;
            }
            let share = self.fuel / (MAX_STATEMENTS - count) as u64;
            let rest = self.fuel - share;
            self.fuel = share;
            if self.callees_any() > 0 && self.pick(2) == 1 {
                self.w.line();
                self.cost.statement();
                self.statement(Self::call_stmt);
            } else {
                self.stmt();
            }
            self.fuel += rest;
        }
    }

    /// A call of a function or a method as a statement, printing what it returns where it can.
    fn call_stmt(&mut self) {
        let callee = self.pick(self.callees_any());
        let ret = &self.scope.funcs[callee].ret;
        // Every type prints but `unit`; an optional warns (W0015), then prints.
        let print = *ret != Ty::Unit;
        if print {
            self.w.text("print(");
        }
        self.invoke(callee, false);
        if print {
            self.w.text(")");
        }
    }

    pub(crate) fn stmt(&mut self) {
        let mut rules = vec![Rule::Print, Rule::Let, Rule::Discard, Rule::Inexact];
        if self.scope.vars.iter().any(assignable) {
            rules.push(Rule::Assign);
        }
        if self.scope.vars.iter().any(|var| self.settable(var)) {
            rules.push(Rule::Element);
        }
        if !self.push_targets().is_empty() {
            rules.push(Rule::Push);
        }
        if !self.field_targets().is_empty() {
            // Twice: a `mut` field is rare in scope, and it takes both `=` and `op=`.
            rules.extend([Rule::SetField, Rule::SetField]);
        }
        if !self.compound_targets().is_empty() {
            rules.push(Rule::Compound);
        }
        if self.blocks < MAX_BLOCKS {
            rules.push(Rule::If);
            if self.cost.depth() < MAX_LOOPS {
                rules.extend([Rule::For, Rule::ForArray]);
                // A `while` is three statements: its counter, itself and the step; a `loop`, four,
                // with its guard.
                if self.fuel >= 2 {
                    rules.extend([Rule::While, Rule::WhileLet, Rule::Loop]);
                }
            }
        }
        if self.callees_any() > 0 {
            rules.push(Rule::Call);
        }
        self.w.line();
        self.cost.statement();
        self.statement(|g| {
            let rule = rules[g.pick(rules.len())];
            g.stmt_rule(rule);
        });
    }

    /// Writes a statement by `write`. Before it, the rewritten copy then holds the `let`s its
    /// expressions moved there, and now and then an `if false` with a copy of it (decision 2).
    /// Its expressions run unconditionally and after no effect, as far as the statement goes.
    fn statement(&mut self, write: impl FnOnce(&mut Self)) {
        let conditional = std::mem::take(&mut self.scope.conditional);
        let effect = std::mem::take(&mut self.effect);
        let start = self.w.begin();
        write(self);
        // A statement is never after a block's last expression.
        let dead = allowed(Rewrite::DeadIf, &Site::default()) && self.rewrite_pick(8) == 7;
        self.w.end(start, dead);
        self.scope.conditional = conditional;
        self.effect = effect;
    }

    /// A value for `print`, in a `form` from 0 to 3: as it is, interpolated alone, interpolated
    /// with a width, or through `to_string`. Each writes the same text for any type. An optional
    /// in the first three forms warns (W0015), and `to_string` of it does not.
    fn printed_value(&mut self, form: usize) {
        if form == 3 {
            let ty = self.bare_type();
            self.w.text("(");
            self.synth = true;
            self.expr(&ty, MAX_SIZE);
            self.w.text(").to_string()");
            return;
        }
        let ty = self.bare_type();
        if form == 0 {
            self.synth = true;
            self.expr(&ty, MAX_SIZE);
            return;
        }
        let one_line = std::mem::replace(&mut self.one_line, true);
        self.w.text("\"{");
        self.synth = true;
        self.expr(&ty, MAX_SIZE);
        if form == 2 {
            let align = ["", "<", ">", "^"][self.pick(4)];
            let width = self.upto(39) + 1;
            self.w.text(&format!(":{align}{width}"));
        }
        self.w.text("}\"");
        self.one_line = one_line;
    }

    fn stmt_rule(&mut self, rule: Rule) {
        match rule {
            Rule::Print => {
                // One pick for both, so that the bytes after it stay where they were: the stream
                // is `print` or `eprint`, and the form is the value's.
                let choice = self.pick(8);
                self.w.text(["print(", "eprint("][choice % 2]);
                self.printed_value(choice / 2);
                self.w.text(")");
            }
            Rule::Let => self.let_stmt(),
            Rule::Discard => {
                self.w.text("let _ = ");
                let ty = self.bare_type();
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
                    .filter(|&v| self.settable(&self.scope.vars[v]))
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
            Rule::SetField => self.set_field(),
            Rule::Push => self.push_stmt(),
            Rule::Compound => self.compound(),
            Rule::If => self.if_stmt(false),
            Rule::While => self.while_stmt(),
            Rule::WhileLet => self.while_let(),
            Rule::Loop => {
                self.loop_stmt(None);
            }
            Rule::For => self.for_stmt(),
            Rule::ForArray => self.for_array(),
            Rule::Call => self.call_stmt(),
        }
    }

    /// `let [mut] v[: T] = e`. A mutable `string` gets a size for its first value and plain
    /// assignments, and room for what `+=` adds; a mutable optional of a size, a size.
    fn let_stmt(&mut self) {
        let ty = self.any_type();
        let mutable = self.pick(2) == 1;
        let name = self.scope.local();
        let keyword = if mutable { "let mut" } else { "let" };
        self.w.text(&format!("{keyword} {name}"));
        // A value of a type the checker infers needs one where it is not written.
        let written = ty.inferred() || self.pick(2) == 1;
        if written {
            let written = self.spell(&ty);
            self.w.text(&format!(": {written}"));
        }
        self.w.text(" = ");
        if mutable && let Ty::Array(element) = &ty {
            let element = (**element).clone();
            return self.array_let(name, ty, &element);
        }
        let (base, room) = match (&ty, mutable) {
            (Ty::String, true) => (SIZES[self.pick(SIZES.len())], SIZES[self.pick(SIZES.len())]),
            (_, true) if ty.sized() => (self.size_for(&ty, &SIZES), 0),
            (_, false) if ty.sized() => (MAX_SIZE, 0),
            _ => (0, 0),
        };
        self.typed = written;
        let init = self.expr(&ty, base);
        let base = if mutable { base } else { init.size };
        let mut var = Var::new(name, ty, mutable, base, room, self.cost.depth());
        var.len = init.len;
        if let Ty::Array(_) = var.ty {
            // An element holds at most what the array does but for its slots.
            var.elem = init.size - init.len;
        }
        debug_assert!(var.size <= MAX_SIZE, "a value may reach {}", var.size);
        self.scope.vars.push(var);
    }

    /// The literal or fill of a mutable array of `len` elements, each at most `base` but for
    /// what `+=` adds to them, all of them within the `string` elements' room.
    pub(crate) fn array_let(&mut self, name: String, ty: Ty, element: &Ty) {
        const BASES: [u64; 3] = [0, 16, 64];
        let base = if element.sized() {
            element.floor() + BASES[self.pick(BASES.len())]
        } else {
            0
        };
        let fill = self.pick(2) == 1 && !element.shared();
        let len = self.upto((MAX_SIZE / (1 + base)).min(if fill { MAX_BOUND } else { 4 }));
        let size = array_size(len, base);
        let rooms: Vec<u64> = SIZES
            .into_iter()
            .filter(|room| *element == Ty::String && size + room <= MAX_SIZE || *room == 0)
            .collect();
        let room = rooms[self.pick(rooms.len())];
        // Only an array of a type with no class object takes a `push`, and it has room for a few.
        let grow = if self.scope.contains_class(element) || size + room + GROW > MAX_SIZE {
            0
        } else {
            GROW
        };
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
        var.size = size + room + grow;
        var.elem += grow;
        var.grow = grow;
        var.len = len;
        self.scope.vars.push(var);
    }

    /// Whether an element of `var` may take an assignment: a mutable array with an element,
    /// of a type that holds no class object (decision 4).
    fn settable(&self, var: &Var) -> bool {
        has_elements(var)
            && matches!(&var.ty, Ty::Array(element) if !self.scope.contains_class(element))
    }

    /// The places a `push` may append to. A mutable array takes a `push` when its type holds no
    /// class object (decision 4) and its growth covers a slot and the smallest element, at each
    /// run. The same holds for an element of a mutable array of such arrays. A `mut` array field
    /// takes a guarded `push` when its elements have no size. One more element is then one more
    /// size.
    pub(crate) fn push_targets(&self) -> Vec<Push> {
        let mut targets = Vec::new();
        for (v, var) in self.scope.vars.iter().enumerate() {
            // What one run of a `push` may add, and whether it covers a slot and `element`.
            let covers = |element: &Ty| {
                let room = var.grow / self.cost.times_since(var.loops).max(1);
                room > element.floor()
            };
            match &var.ty {
                Ty::Array(element) if var.assignable && !self.scope.contains_class(element) => {
                    if covers(element) {
                        targets.push(Push::Local(v));
                    }
                    if has_elements(var) && matches!(&**element, Ty::Array(inner) if covers(inner))
                    {
                        targets.push(Push::Element(v));
                    }
                }
                Ty::Class { id, .. } => {
                    for (f, field) in self.scope.classes[*id].fields.iter().enumerate() {
                        if matches!(&field.ty, Ty::Array(element) if !element.sized())
                            && field.mutable
                            && !self.scope.contains_class(&field.ty)
                        {
                            targets.push(Push::Field(v, *id, f));
                        }
                    }
                }
                _ => {}
            }
        }
        targets
    }

    /// `v.push(e)`, `v[i].push(e)` or `if v.g.len() < cap { v.g.push(e) }`. A push adds `e` and
    /// its slot to the size of the array. That size comes out of the growth of the local that
    /// holds the array, at each run of every loop since its declaration. A field has no growth.
    /// Its guard keeps its length, which is its size, within its cap.
    pub(crate) fn push_stmt(&mut self) {
        let targets = self.push_targets();
        match targets[self.pick(targets.len())] {
            Push::Local(v) => {
                let name = self.scope.vars[v].name.clone();
                let Ty::Array(element) = self.scope.vars[v].ty.clone() else {
                    unreachable!("only an array takes a push")
                };
                self.w.text(&format!("{name}.push("));
                self.pushed(v, &element);
            }
            Push::Element(v) => {
                let (name, len) = {
                    let var = &self.scope.vars[v];
                    (var.name.clone(), var.len)
                };
                let Ty::Array(inner) = self.scope.vars[v].ty.clone() else {
                    unreachable!("only an array has elements")
                };
                let Ty::Array(element) = *inner else {
                    unreachable!("only an array of arrays takes a push onto an element")
                };
                let at = self.upto(len - 1);
                self.w.text(&format!("{name}[{at}].push("));
                self.pushed(v, &element);
            }
            Push::Field(v, class, f) => {
                let field = &self.scope.classes[class].fields[f];
                let Ty::Array(element) = field.ty.clone() else {
                    unreachable!("only an array field takes a push")
                };
                let place = format!("{}.{}", self.scope.vars[v].name, field.name);
                self.w.text(&format!("if {place}.len() < {} ", field.cap));
                self.conditionally(|g| {
                    g.open_block();
                    g.w.line();
                    g.cost.statement();
                    g.w.text(&format!("{place}.push("));
                    g.expr(&element, 0);
                    g.w.text(")");
                    g.close_block();
                });
            }
        }
    }

    /// The argument of a `push` onto `v`, an element of type `element`, and the `)`. It is as
    /// large as the growth allows at each run, and takes the growth it uses; while it is written,
    /// the growth and the room are empty, so that a `push` or a `+=` inside it adds nothing.
    fn pushed(&mut self, v: usize, element: &Ty) {
        const MOST: u64 = 64;
        let times = self.cost.times_since(self.scope.vars[v].loops);
        // The growth covers the slot and the smallest value (`push_targets`).
        let most = self.scope.vars[v].grow / times.max(1) - 1;
        let max = most.min(MOST).max(element.floor());
        let (room, grow) = {
            let var = &mut self.scope.vars[v];
            (std::mem::take(&mut var.room), std::mem::take(&mut var.grow))
        };
        let added = self.expr(element, max);
        self.w.text(")");
        let var = &mut self.scope.vars[v];
        (var.room, var.grow) = (room, grow);
        var.push(1 + added.size, times);
    }

    /// `v.g = e`, or `v.g op= e` of a number: a `string` takes only `=`, as its cap leaves it no
    /// room to grow.
    fn set_field(&mut self) {
        let targets = self.field_targets();
        let (v, class, f) = targets[self.pick(targets.len())];
        let field = &self.scope.classes[class].fields[f];
        let (ty, cap) = (field.ty.clone(), field.cap);
        self.w
            .text(&format!("{}.{}", self.scope.vars[v].name, field.name));
        if matches!(ty, Ty::I64 | Ty::U8 | Ty::F64) && self.pick(3) != 0 {
            self.compound_by(&ty);
        } else {
            self.w.text(" = ");
            self.expr(&ty, cap);
        }
    }

    /// The fields an assignment may target, each as its local, class and index there: a
    /// `mut` field of a type that holds no class object (decision 4).
    fn field_targets(&self) -> Vec<(usize, usize, usize)> {
        let mut targets = Vec::new();
        for (v, var) in self.scope.vars.iter().enumerate() {
            if let Ty::Class { id, .. } = var.ty {
                for (f, field) in self.scope.classes[id].fields.iter().enumerate() {
                    if field.mutable && !self.scope.contains_class(&field.ty) {
                        targets.push((v, id, f));
                    }
                }
            }
        }
        targets
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
            // The checker reads the element before the right operand, unless it is an append of
            // an operand that cannot exit, and that read may panic.
            self.effect = true;
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
            self.compound_by(&ty);
        }
    }

    /// ` op= e` of a number of type `ty`. A `*=` in a loop soon overflows, so it comes 1 in 8.
    fn compound_by(&mut self, ty: &Ty) {
        if self.pick(8) == 7 {
            self.w.text(" *= ");
        } else {
            self.one_of(&OPS);
        }
        self.right_operand(ty);
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
        let cond_start = self.w.at();
        let (_, binding) = self.condition();
        let cond = cond_start..self.w.at();
        let mut arms = Vec::new();
        self.conditionally(|g| {
            g.w.text(" ");
            let vars = g.scope.vars.len();
            g.scope.vars.extend(binding);
            let arm = g.w.at();
            branch(g);
            arms.push(arm..g.w.at());
            g.scope.vars.truncate(vars);
            while g.pick(3) == 2 && g.spend(1) {
                g.w.text(" else if ");
                g.expr(&Ty::Bool, 0);
                g.w.text(" ");
                branch(g);
                // Only an `if` with one `else` and no `else if` swaps.
                arms.clear();
            }
            if last || g.pick(2) == 1 {
                g.w.text(" else ");
                let arm = g.w.at();
                if last {
                    g.jumping_block();
                } else {
                    g.statement_block(false);
                }
                arms.push(arm..g.w.at());
            }
        });
        self.swap(cond, &arms);
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
        let label = self.label();
        self.w.text(&format!("while {counter} > 0"));
        let var = counter_var(counter.clone(), Ty::I64, 0, self.cost.depth());
        self.scope.vars.push(var);
        self.conditionally(|g| {
            // Now and then, as another condition may end the loop before its body runs.
            if g.pick(4) == 3 {
                g.w.text(" && ");
                g.cost.enter_condition(bound);
                g.expr(&Ty::Bool, 0);
                g.cost.exit_loop();
            }
            g.w.text(" ");
            g.cost.enter_loop(bound);
            g.loop_body(label, &format!("{counter} -= 1"), None);
            g.cost.exit_loop();
        });
    }

    /// `let mut c = n` and `loop { if c == 0 { break [v] } c -= 1 ... }`: `while_stmt`'s
    /// bounds, with the guard as the `while`'s condition. With `value`, the `loop` is a value
    /// whose `break`s each give one; gives the most size they gave.
    fn loop_stmt(&mut self, value: Option<Value>) -> u64 {
        self.spend(2);
        let bound = self.upto(MAX_BOUND);
        let counter = self.scope.fresh("v");
        self.w.text(&format!("let mut {counter} = {bound}"));
        self.w.line();
        self.cost.statement();
        let label = self.label();
        self.w.text("loop ");
        let var = counter_var(counter.clone(), Ty::I64, 0, self.cost.depth());
        self.scope.vars.push(var);
        self.conditionally(|g| {
            g.open_loop(label, value);
            g.w.line();
            g.cost.enter_condition(bound);
            g.cost.statement();
            g.w.text(&format!("if {counter} == 0 {{ "));
            let at = g.jumps.loops.len() - 1;
            g.loop_jump("break", at, false);
            g.w.text(" }");
            g.cost.exit_loop();
            g.cost.enter_loop(bound);
            g.w.line();
            g.cost.statement();
            g.w.text(&format!("{counter} -= 1"));
            g.stmts(true);
            g.cost.exit_loop();
            g.close_loop()
        })
    }

    /// `({ let mut c = n loop { ... } })`, a `loop` value of type `ty` (`loop_stmt`), at most
    /// `max`: no jump leaves it but its own `break`s.
    pub(crate) fn loop_value(&mut self, ty: &Ty, max: u64, synth: bool) -> Info {
        let jumps = std::mem::take(&mut self.jumps);
        self.w.text("(");
        self.open_block();
        self.w.line();
        let size = self.loop_stmt(Some(Value {
            ty: ty.clone(),
            max,
            synth,
        }));
        self.close_block();
        self.w.text(")");
        self.jumps = jumps;
        Info {
            size,
            pure: false,
            len: 0,
            folds: false,
            value: None,
        }
    }

    /// Now and then a fresh loop label, written before the loop's keyword.
    fn label(&mut self) -> Option<String> {
        if self.pick(4) != 3 {
            return None;
        }
        let label = self.scope.fresh("l");
        self.w.text(&format!("{label}: "));
        Some(label)
    }

    /// `let mut c = n` and `while let p = (if c > 0 { e } else { None }) { c -= 1 ... }`:
    /// `while_stmt`'s bounds, and `p` a `let_pattern`, which may bind each `e`; or, half the
    /// time an enum holds itself, `while_chain`.
    fn while_let(&mut self) {
        self.spend(2);
        let chains = self.chains();
        if !chains.is_empty() && self.pick(2) == 1 {
            let (id, variant) = chains[self.pick(chains.len())];
            return self.while_chain(id, variant);
        }
        let bound = self.upto(MAX_BOUND);
        let counter = self.scope.fresh("v");
        self.w.text(&format!("let mut {counter} = {bound}"));
        self.w.line();
        self.cost.statement();
        let payload = match self.bare_type() {
            Ty::Optional(payload) => *payload,
            ty => ty,
        };
        // A `None` pattern would match only once the counter stops the loop, so it never ends.
        let (pattern, name) = self.let_pattern(&payload, false);
        let label = self.label();
        self.w
            .text(&format!("while let {pattern} = (if {counter} > 0 {{ "));
        let var = counter_var(counter.clone(), Ty::I64, 0, self.cost.depth());
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
            g.loop_body(label, &format!("{counter} -= 1"), binding);
            g.cost.exit_loop();
        });
    }

    /// The variants a `while let` can walk a chain of: each enum's and variant's index, where
    /// the variant's first field holds the enum itself.
    fn chains(&self) -> Vec<(usize, usize)> {
        let mut chains = Vec::new();
        for (id, e) in self.scope.enums.iter().enumerate() {
            for (v, variant) in e.variants.iter().enumerate() {
                if matches!(variant.fields.first(), Some(Ty::Enum { id: own, .. }) if *own == id) {
                    chains.push((id, v));
                }
            }
        }
        chains
    }

    /// `let mut c = E.V(E.V(... end ...), ...)`, a chain of at most `MAX_BOUND` nodes of the
    /// variant `variant` of the enum `id` that ends in another variant, and
    /// `while let V(t, ...) = c { c = t ... }`, which takes one node a run, so runs once a node
    /// (decision 5).
    pub(crate) fn while_chain(&mut self, id: usize, variant: usize) {
        let ty = self.scope.enum_ty(id);
        let nodes = self.upto(MAX_BOUND);
        let chain = self.scope.fresh("v");
        self.w.text(&format!("let mut {chain} = "));
        let size = self.chain(&ty, variant, nodes);
        self.w.line();
        self.cost.statement();
        let (name, fields) = {
            let variant = &self.scope.enums[id].variants[variant];
            (variant.name.clone(), variant.fields.clone())
        };
        let loops = self.cost.depth() + 1;
        let rest = self.scope.fresh("v");
        let mut parts = vec![rest.clone()];
        let mut vars = vec![Var::new(rest.clone(), ty.clone(), false, size, 0, loops)];
        for field in &fields[1..] {
            if self.pick(2) == 0 {
                parts.push("_".to_string());
            } else {
                let bound = self.scope.fresh("v");
                let most = field.within(size);
                vars.push(Var::new(
                    bound.clone(),
                    field.clone(),
                    false,
                    most,
                    0,
                    loops,
                ));
                parts.push(bound);
            }
        }
        let qualifier = if self.pick(2) == 0 {
            String::new()
        } else {
            format!("{}.", ty.bare())
        };
        let label = self.label();
        self.w.text(&format!(
            "while let {qualifier}{name}({}) = {chain} ",
            parts.join(", ")
        ));
        let var = counter_var(chain.clone(), ty, size, self.cost.depth());
        self.scope.vars.push(var);
        self.conditionally(|g| {
            g.cost.enter_loop(nodes);
            g.loop_body(label, &format!("{chain} = {rest}"), vars);
            g.cost.exit_loop();
        });
    }

    /// A chain of `nodes` nodes of the variant `variant` of the enum `ty`, each holding the
    /// next in its first field, then a value of another variant; each of them within a ninth
    /// of `MAX_SIZE`. Gives its size.
    fn chain(&mut self, ty: &Ty, variant: usize, nodes: u64) -> u64 {
        const ROOM: u64 = MAX_SIZE / (MAX_BOUND + 1);
        if nodes == 0 {
            return self.variant(ty, ROOM, Some(variant)).size;
        }
        let Ty::Enum { id, .. } = *ty else {
            unreachable!("only an enum has variants")
        };
        let (name, fields, labels) = {
            let variant = &self.scope.enums[id].variants[variant];
            (
                variant.name.clone(),
                variant.fields.clone(),
                variant.labels.clone(),
            )
        };
        self.w.text(&format!("{ty}.{name}("));
        let label = |f: usize| labels.get(f).map(|label| format!("{label}: "));
        self.w.text(&label(0).unwrap_or_default());
        let next = self.chain(ty, variant, nodes - 1);
        let mut left = ROOM - 1;
        for f in 1..fields.len() {
            self.w.text(", ");
            self.w.text(&label(f).unwrap_or_default());
            let later: u64 = fields[f + 1..].iter().map(Ty::floor).sum();
            left -= self.expr(&fields[f], left - later).size;
        }
        self.w.text(")");
        next + ROOM - left
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
        let label = self.label();
        self.w
            .text(&format!("for {name} in {start}{dots}{} ", start + span));
        let var = Var::new(name, Ty::I64, false, 0, 0, self.cost.depth() + 1);
        self.conditionally(|g| {
            g.cost.enter_loop(count);
            g.loop_body(label, "", Some(var));
            g.cost.exit_loop();
        });
    }

    /// `for v in [a, b, ...]` or `for v in [e; n]`, of at most `MAX_BOUND` elements.
    fn for_array(&mut self) {
        let element = self.bare_type();
        let name = self.scope.fresh("v");
        let label = self.label();
        self.w.text(&format!("for {name} in "));
        // The source has no expected type.
        let array = if self.pick(2) == 0 || element.shared() {
            self.array_literal(&element, MAX_SIZE, true)
        } else {
            self.fill(&Ty::Array(Box::new(element.clone())), MAX_SIZE, true)
        };
        self.w.text(" ");
        // Each element holds at most what they all hold.
        let size = array.size - array.len;
        let var = Var::new(name, element, false, size, 0, self.cost.depth() + 1);
        self.conditionally(|g| {
            g.cost.enter_loop(array.len);
            g.loop_body(label, "", Some(var));
            g.cost.exit_loop();
        });
    }

    /// The body of a loop labeled `label`, if any: `first`, if any, then statements, which may
    /// `break` and `continue`, with `var` in scope.
    fn loop_body(
        &mut self,
        label: Option<String>,
        first: &str,
        var: impl IntoIterator<Item = Var>,
    ) {
        self.open_loop(label, None);
        self.scope.vars.extend(var);
        if !first.is_empty() {
            self.w.line();
            self.cost.statement();
            self.w.text(first);
        }
        self.stmts(true);
        self.close_loop();
    }

    /// Opens the body of a loop labeled `label`, if any, whose `break`s give `value`, if any.
    fn open_loop(&mut self, label: Option<String>, value: Option<Value>) {
        self.jumps.loops.push(Target {
            label,
            used: false,
            value,
            size: 0,
        });
        self.open_block();
    }

    /// Closes the innermost loop's body, and gives the most size its `break`s gave.
    fn close_loop(&mut self) -> u64 {
        self.close_block();
        let target = self
            .jumps
            .loops
            .pop()
            .expect("a loop's body should close as it opened");
        debug_assert!(
            target.label.is_none() || target.used,
            "a loop's body should name its loop label"
        );
        target.size
    }

    /// Whether the loop `at` has a loop label that no jump names yet.
    fn unused(&self, at: usize) -> bool {
        let target = &self.jumps.loops[at];
        target.label.is_some() && !target.used
    }

    /// `kind`, `break` or `continue`, of the loop `at`, naming its label if `named`; a `break`
    /// of a `loop` value gives a value.
    fn loop_jump(&mut self, kind: &str, at: usize, named: bool) {
        self.w.text(kind);
        let target = &mut self.jumps.loops[at];
        if named {
            target.used = true;
            let label = target
                .label
                .as_deref()
                .expect("a named loop should have a label");
            self.w.text(&format!(" {label}"));
        }
        // No jump in the value reaches this loop, so the value is out of it while it is written.
        if kind == "break"
            && let Some(value) = self.jumps.loops[at].value.take()
        {
            self.w.text(" ");
            // The two readings of `break x -1`, without the parentheses `expr` writes: a label
            // and the value `-1`, or, with no label, the value `x - 1`.
            let size = if value.ty == Ty::I64
                && self.pick(3) == 2
                && (named || self.has_name(&Ty::I64, 0))
            {
                if !named {
                    self.name(&Ty::I64, 0);
                    self.w.text(" ");
                }
                self.w.text("-1");
                0
            } else {
                self.synth = value.synth;
                self.expr(&value.ty, value.max).size
            };
            let target = &mut self.jumps.loops[at];
            target.size = target.size.max(size);
            target.value = Some(value);
        }
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
        // A block in a constant does not fold (E0086).
        Info {
            pure: false,
            folds: false,
            ..info
        }
    }

    fn open_block(&mut self) {
        self.blocks += 1;
        self.w.open();
        self.w.open_block();
        self.scope.open();
    }

    fn close_block(&mut self) {
        self.scope.close();
        self.w.close_block();
        self.w.close();
        self.blocks -= 1;
    }

    /// A jump, the last statement of its block: `panic`, `todo`, `return`, `break` or
    /// `continue`.
    fn jump(&mut self) {
        self.cost.statement();
        let mut jumps = Vec::new();
        if self.jumps.ret {
            jumps.push("return");
        }
        if !self.jumps.loops.is_empty() {
            jumps.extend(["break", "continue"]);
        }
        // A panic ends the run, so it comes about once in 32 where another jump can.
        let jump = if jumps.is_empty() || self.pick(32) == 31 {
            ["panic", "todo"][self.pick(2)]
        } else {
            jumps[self.pick(jumps.len())]
        };
        match jump {
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
                // A known panic site after it would not be reached (decision 3).
                self.returns |= self.scope.func.is_none();
                let func = self.scope.func.map(|f| &self.scope.funcs[f]);
                if let Some(func) = func.filter(|func| func.ret != Ty::Unit) {
                    let (ty, max) = (func.ret.clone(), func.ret_size);
                    self.w.text(" ");
                    self.expr(&ty, max);
                }
            }
            kind => {
                // The innermost loop, plain, or a loop with a label, named.
                let labeled: Vec<usize> = (0..self.jumps.loops.len())
                    .filter(|&at| self.jumps.loops[at].label.is_some())
                    .collect();
                match self.pick(labeled.len() + 1) {
                    0 => self.loop_jump(kind, self.jumps.loops.len() - 1, false),
                    pick => self.loop_jump(kind, labeled[pick - 1], true),
                }
            }
        }
    }
}

/// A place a `push` may append to (`Gen::push_targets`). It is a local, an element of one, or a
/// `mut` field of the class object a local holds, as its local, class and field.
#[derive(Clone, Copy)]
pub(crate) enum Push {
    Local(usize),
    Element(usize),
    Field(usize, usize, usize),
}

/// The compound assignments of a number but `*=`.
const OPS: [&str; 4] = [" += ", " -= ", " /= ", " %= "];

/// The size that `push` may add to a mutable array (`Var::grow`), at each run of each loop.
pub(crate) const GROW: u64 = 64;

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

/// A `let mut` a loop counts with, which the body never assigns.
fn counter_var(name: String, ty: Ty, size: u64, loops: usize) -> Var {
    Var {
        mutable: true,
        ..Var::new(name, ty, false, size, 0, loops)
    }
}
