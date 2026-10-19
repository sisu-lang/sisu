//! `--emit mir`: one function after another, each block's statements indented under its label.

use std::fmt::Write as _;

use super::{
    Block, BlockId, Const, Function, LocalId, Operand, Place, Program, Projection, Rvalue,
    Statement, Terminator,
};
use crate::tir;

/// The text of `mir`, whose callee and type names `tir`, the program it was lowered from,
/// resolves.
pub(crate) fn print(mir: &Program, tir: &tir::Program) -> String {
    let mut out = String::new();
    for function in &mir.functions {
        Printer {
            out: &mut out,
            tir,
            function,
        }
        .function();
    }
    out
}

#[cfg(test)]
thread_local! {
    /// How many format entries the printer has examined on this thread.
    pub(super) static EXAMINED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// `x#0` for a named local, `_7` for a temporary or a local the checker made up.
pub(super) fn local_name(function: &Function, id: LocalId) -> String {
    match &function.locals[id.0].name {
        Some(name) => format!("{name}#{}", id.0),
        None => format!("_{}", id.0),
    }
}

struct Printer<'a> {
    out: &'a mut String,
    tir: &'a tir::Program,
    function: &'a Function,
}

impl Printer<'_> {
    /// `fn name(x#0: string) -> i64 {`, the blocks, and `}`.
    fn function(&mut self) {
        let f = self.function;
        self.push(&format!("fn {}(", f.name));
        for (i, local) in f.locals[..f.params].iter().enumerate() {
            if i > 0 {
                self.push(", ");
            }
            let name = self.local(LocalId(i));
            let ty = self.tir.type_name(local.ty);
            self.push(&format!("{name}: {ty}"));
        }
        let ret = self.tir.type_name(f.ret);
        self.push(&format!(") -> {ret} {{\n"));
        for (i, block) in f.blocks.iter().enumerate() {
            self.block(BlockId(i), block);
        }
        self.push("}\n");
    }

    fn block(&mut self, id: BlockId, block: &Block) {
        self.push(&format!("  bb{}:\n", id.0));
        for stmt in &block.stmts {
            let text = self.statement(stmt);
            self.push(&format!("    {text}\n"));
        }
        let text = self.terminator(&block.term);
        self.push(&format!("    {text}\n"));
    }

    fn push(&mut self, text: &str) {
        self.out.push_str(text);
    }

    /// `x#0` for a named local, `_7` for a temporary or a local the checker made up.
    fn local(&self, id: LocalId) -> String {
        local_name(self.function, id)
    }

    /// `o#2.1`, `a#1[i#2]`, `(e#3 as 1).0`.
    fn place(&self, place: &Place) -> String {
        let mut text = self.local(place.local);
        for projection in &place.projections {
            match projection {
                Projection::Field(field) => {
                    write!(text, ".{field}").expect("writing to a string should not fail");
                }
                Projection::Payload { variant, field } => {
                    text = format!("({text} as {variant}).{field}");
                }
                Projection::Index { index, .. } => {
                    let index = self.operand(index);
                    write!(text, "[{index}]").expect("writing to a string should not fail");
                }
            }
        }
        text
    }

    fn operand(&self, operand: &Operand) -> String {
        match operand {
            Operand::Const(c) => constant(c),
            Operand::Read(place) => self.place(place),
            Operand::Move(local) => format!("move {}", self.local(*local)),
            // The guard mark does not print, as the borrow-site flag does not.
            Operand::Use(local) | Operand::Guarded(local) => format!("use {}", self.local(*local)),
        }
    }

    /// The operands, separated by commas.
    fn list(&self, operands: &[Operand]) -> String {
        operands
            .iter()
            .map(|operand| self.operand(operand))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn rvalue(&self, rvalue: &Rvalue) -> String {
        match rvalue {
            Rvalue::Use(operand) => self.operand(operand),
            Rvalue::Copy(place) => format!("copy {}", self.place(place)),
            Rvalue::CopyStr(text) => format!("copy {text:?}"),
            Rvalue::Borrow(place) => format!("borrow {}", self.place(place)),
            Rvalue::Unwrap(operand) => format!("unwrap {}", self.operand(operand)),
            Rvalue::Call { func, args } => {
                let name = &self.tir.functions[func.0].name;
                format!("call {name}({})", self.list(args))
            }
            Rvalue::New { class, args } => {
                let name = &self.tir.classes[class.0].name;
                format!("new {name}({})", self.list(args))
            }
            Rvalue::Variant { id, variant, args } => {
                let e = &self.tir.enums[id.0];
                let name = &e.variants[*variant].name;
                format!("variant {}.{name}({})", e.name, self.list(args))
            }
            Rvalue::ArrayLit(elements) => format!("array[{}]", self.list(elements)),
            Rvalue::ArrayFill { value, len, .. } => {
                format!("array[{}; {}]", self.operand(value), self.operand(len))
            }
            Rvalue::Wrap(operand) => format!("wrap {}", self.operand(operand)),
            Rvalue::Concat { lhs, rhs } => {
                format!("concat({}, {})", self.operand(lhs), self.operand(rhs))
            }
            Rvalue::Interpolate { parts, formats } => {
                // The formats are in part order, so one pass over them meets each in turn.
                let mut formats = formats.iter().peekable();
                let parts: Vec<_> = parts
                    .iter()
                    .enumerate()
                    .map(|(i, part)| {
                        let part = self.operand(part);
                        match formats.next_if(|format| {
                            #[cfg(test)]
                            EXAMINED.set(EXAMINED.get() + 1);
                            format.part == i
                        }) {
                            Some(format) => format!("fmt({part}, \"{format}\")"),
                            None => part,
                        }
                    })
                    .collect();
                format!("interp({})", parts.join(", "))
            }
            Rvalue::ReadLine => "call read_line()".to_owned(),
            Rvalue::Chars(operand) => format!("chars {}", self.operand(operand)),
            Rvalue::Bytes(operand) => format!("bytes {}", self.operand(operand)),
            Rvalue::Words(operand) => format!("words {}", self.operand(operand)),
            Rvalue::FromChars(operand) => format!("string {}", self.operand(operand)),
            Rvalue::Len(operand) => format!("len {}", self.operand(operand)),
            Rvalue::LenWords(operand) => format!("len_words {}", self.operand(operand)),
            Rvalue::Sum { base, .. } => format!("sum {}", self.operand(base)),
            Rvalue::Contains { base, value } => {
                format!("contains({}, {})", self.operand(base), self.operand(value))
            }
            Rvalue::Convert(operand) => format!("convert {}", self.operand(operand)),
            Rvalue::Math { op, operands } => format!("math {}({})", op.name(), self.list(operands)),
            Rvalue::Unary { op, operand, .. } => format!("{op}{}", self.operand(operand)),
            Rvalue::Binary { op, lhs, rhs, .. } => {
                format!("{} {op} {}", self.operand(lhs), self.operand(rhs))
            }
            Rvalue::Compare { op, lhs, rhs } => {
                format!("{} {op} {}", self.operand(lhs), self.operand(rhs))
            }
            Rvalue::Equal { negated, lhs, rhs } => {
                let op = if *negated { "!=" } else { "==" };
                format!("{} {op} {}", self.operand(lhs), self.operand(rhs))
            }
            Rvalue::Is { lhs, rhs } => {
                format!("{} is {}", self.operand(lhs), self.operand(rhs))
            }
            Rvalue::Print { stream, operand } => {
                format!("call {}({})", stream.name(), self.operand(operand))
            }
        }
    }

    fn statement(&self, stmt: &Statement) -> String {
        match stmt {
            Statement::Assign {
                dest: Some(dest),
                value,
            } => format!("{} = {}", self.local(*dest), self.rvalue(value)),
            Statement::Assign { dest: None, value } => self.rvalue(value),
            Statement::Take { dest, place } => {
                format!("{} = take {}", self.local(*dest), self.place(place))
            }
            Statement::Store { place, value } => {
                format!("{} = {}", self.place(place), self.operand(value))
            }
            Statement::Release(local) => format!("release {}", self.local(*local)),
            Statement::ScopeRelease(local) => format!("release? {}", self.local(*local)),
            Statement::MakeUnique(place) => format!("make_unique {}", self.place(place)),
            Statement::Check(place) => format!("check {}", self.place(place)),
            Statement::End(local) => format!("end {}", self.local(*local)),
        }
    }

    fn terminator(&self, term: &Terminator) -> String {
        match term {
            Terminator::Goto(to) => format!("goto bb{}", to.0),
            Terminator::Branch { cond, then, else_ } => {
                format!("branch {} bb{} bb{}", self.operand(cond), then.0, else_.0)
            }
            Terminator::IfPresent {
                optional,
                some,
                none,
            } => format!(
                "if_present {} bb{} bb{}",
                self.local(*optional),
                some.0,
                none.0
            ),
            Terminator::SwitchTag {
                place,
                arms,
                otherwise,
            } => {
                let arms: Vec<_> = arms
                    .iter()
                    .map(|(tag, to)| format!("{tag}: bb{}", to.0))
                    .collect();
                let text = format!("switch tag {} [{}]", self.place(place), arms.join(", "));
                with_otherwise(text, *otherwise)
            }
            Terminator::SwitchValue {
                operand,
                arms,
                otherwise,
            } => {
                let arms: Vec<_> = arms
                    .iter()
                    .map(|(value, to)| format!("{}: bb{}", constant(value), to.0))
                    .collect();
                let text = format!("switch {} [{}]", self.operand(operand), arms.join(", "));
                with_otherwise(text, *otherwise)
            }
            Terminator::Return(Some(value)) => format!("return {}", self.operand(value)),
            Terminator::Return(None) => "return".to_owned(),
            Terminator::Panic { message, .. } => format!("panic {}", self.operand(message)),
            Terminator::Unreachable => "unreachable".to_owned(),
        }
    }
}

/// `text`, then ` otherwise bbN` when a switch has a default.
fn with_otherwise(text: String, otherwise: Option<BlockId>) -> String {
    match otherwise {
        Some(to) => format!("{text} otherwise bb{}", to.0),
        None => text,
    }
}

/// A constant as `--emit tir` prints it.
fn constant(c: &Const) -> String {
    match c {
        Const::Int(n, ty) if *ty == tir::Type::U8 => format!("{n}u8"),
        Const::Int(n, _) => format!("{n}"),
        // Rust's shortest text, which spells NaN `NaN`; Sisu spells it `nan`.
        Const::Float(x) if x.is_nan() => "nan".to_owned(),
        Const::Float(x) => format!("{x:?}"),
        Const::Bool(b) => format!("{b}"),
        Const::Char(c) => format!("{c:?}"),
        Const::None(_) => "None".to_owned(),
        Const::Str(text) => format!("{text:?}"),
    }
}
