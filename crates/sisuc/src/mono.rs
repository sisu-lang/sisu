//! Monomorphization: the checked program, whose generic functions, classes and enums were each
//! checked once for all their type arguments, becomes one with no type parameter left. Each list
//! of type arguments the program reaches gets its own copy of the declaration, named with its
//! arguments as a program writes them (`first<i64>`, `Stack<[string]>`, `Stack<i64>.push`,
//! `Tree<i64>`), so `mir::build` and codegen see only concrete types, and each copy is an
//! ordinary function, class or enum to them. The enums are then laid out again on their
//! concrete payloads, since an instantiation can close a cycle that no declaration has.

use std::collections::VecDeque;
use std::mem;

use rustc_hash::FxHashMap;

use crate::conformance::{find, normalize};
use crate::tir::{
    Arm, Block, Class, ClassId, Conformances, Enum, EnumId, Expr, ExprKind, Field, FuncId,
    Function, Place, Program, Stmt, Type, TypeArgs, TypeKind, layout_enums,
};
use crate::types::{Head, write_args, write_method};

#[cfg(test)]
thread_local! {
    /// How many programs on this thread have taken the path that instantiates nothing.
    static FAST_PATHS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// `program` with each generic function, class and enum replaced by the instantiations it
/// reaches, and its enums laid out again.
/// The non-generic declarations come first, in their order; the instantiations follow, in the
/// order a worklist first reaches them: the non-generic classes' fields and enums' payloads, then
/// the non-generic functions' bodies, then each instantiation's in turn. A method is reached only
/// by a call, never with its class. The checker has accepted the program, so nothing here can
/// fail.
pub(crate) fn instantiate(mut program: Program) -> Program {
    let prelude = program.prelude;
    let generic = program.functions[..prelude.funcs]
        .iter()
        .any(|f| !f.type_params.is_empty())
        || (program.classes[..prelude.classes].iter()).any(|c| !c.type_params.is_empty())
        || (program.enums[..prelude.enums].iter()).any(|e| !e.type_params.is_empty());
    if !generic && !program.instantiates {
        #[cfg(test)]
        FAST_PATHS.set(FAST_PATHS.get() + 1);
        // Reaching a generic item records a type argument, so nothing of the prelude's that a
        // program does not name is used: they are all templates or unused.
        program.functions.truncate(prelude.funcs);
        program.classes.truncate(prelude.classes);
        program.enums.truncate(prelude.enums);
        // A method of a conformance block has its interface in its symbol.
        for i in 0..program.functions.len() {
            if let Some((mut name, cut)) = program.method_name(&program.functions[i]) {
                if cut {
                    name.push('#');
                    name.push_str(&i.to_string());
                }
                let f = &mut program.functions[i];
                f.name = name;
                f.method = None;
            }
        }
        return program;
    }
    let program = Mono::new(program).run();
    if cfg!(debug_assertions) {
        assert_concrete(&program);
    }
    program
}

/// An instantiation reached and given its id, whose body waits on the worklist.
enum Work {
    Function {
        id: FuncId,
        decl: FuncId,
        args: TypeArgs,
        name: String,
    },
    Class {
        id: ClassId,
        decl: ClassId,
        args: TypeArgs,
    },
    Enum {
        id: EnumId,
        decl: EnumId,
        args: TypeArgs,
    },
}

struct Mono {
    /// The checked program's generic functions, at their ids; a non-generic one has moved to
    /// `functions`.
    decls: Vec<Option<Function>>,
    /// The checked program's generic classes, at their ids.
    class_decls: Vec<Option<Class>>,
    /// The output's id of each declaration and argument list reached, the non-generic ones with
    /// `TypeArgs::NONE`.
    function_ids: FxHashMap<(FuncId, TypeArgs), FuncId>,
    class_ids: FxHashMap<(ClassId, TypeArgs), ClassId>,
    enum_ids: FxHashMap<(EnumId, TypeArgs), EnumId>,
    /// The checked program's generic enums, at their ids.
    enum_decls: Vec<Option<Enum>>,
    /// The output's type for each concrete checked type met so far.
    types: FxHashMap<Type, Type>,
    /// The output's functions, each `None` until the worklist instantiates it.
    functions: Vec<Option<Function>>,
    /// The output, without its functions until the end: its classes name types as they arrive.
    out: Program,
    /// The checked program's conformances, which resolve each interface call.
    conformances: Conformances,
    work: VecDeque<Work>,
}

impl Mono {
    fn new(program: Program) -> Mono {
        let mut mono = Mono {
            decls: Vec::with_capacity(program.functions.len()),
            class_decls: Vec::with_capacity(program.classes.len()),
            function_ids: FxHashMap::default(),
            class_ids: FxHashMap::default(),
            enum_ids: FxHashMap::default(),
            enum_decls: Vec::with_capacity(program.enums.len()),
            types: FxHashMap::default(),
            functions: Vec::new(),
            out: Program {
                interfaces: program.interfaces,
                ..Program::default()
            },
            conformances: program.conformances,
            work: VecDeque::new(),
        };
        for (i, f) in program.functions.into_iter().enumerate() {
            if f.type_params.is_empty() {
                let id = FuncId(mono.functions.len());
                mono.function_ids.insert((FuncId(i), TypeArgs::NONE), id);
                mono.functions.push(Some(f));
                mono.decls.push(None);
            } else {
                mono.decls.push(Some(f));
            }
        }
        for (i, class) in program.classes.into_iter().enumerate() {
            if class.type_params.is_empty() {
                let id = ClassId(mono.out.classes.len());
                mono.class_ids.insert((ClassId(i), TypeArgs::NONE), id);
                mono.out.classes.push(class);
                mono.class_decls.push(None);
            } else {
                mono.class_decls.push(Some(class));
            }
        }
        for (i, e) in program.enums.into_iter().enumerate() {
            if e.type_params.is_empty() {
                let id = EnumId(mono.out.enums.len());
                mono.enum_ids.insert((EnumId(i), TypeArgs::NONE), id);
                mono.out.enums.push(e);
                mono.enum_decls.push(None);
            } else {
                mono.enum_decls.push(Some(e));
            }
        }
        mono
    }

    fn run(mut self) -> Program {
        for i in 0..self.out.classes.len() {
            let mut fields = mem::take(&mut self.out.classes[i].fields);
            for field in &mut fields {
                field.ty = self.output(field.ty);
            }
            self.out.classes[i].fields = fields;
        }
        for i in 0..self.out.enums.len() {
            let mut variants = mem::take(&mut self.out.enums[i].variants);
            for payload in variants.iter_mut().flat_map(|v| &mut v.fields) {
                payload.ty = self.output(payload.ty);
            }
            self.out.enums[i].variants = variants;
        }
        for i in 0..self.functions.len() {
            let mut f = self.functions[i]
                .take()
                .expect("every non-generic function should be in place before the worklist runs");
            if f.method.is_some() {
                f.name = self.instance_name(&f, &[], i);
            }
            self.function(&mut f, &[]);
            self.functions[i] = Some(f);
        }
        while let Some(work) = self.work.pop_front() {
            match work {
                Work::Function {
                    id,
                    decl,
                    args,
                    name,
                } => {
                    let mut f = self.decls[decl.0]
                        .clone()
                        .expect("an instantiated function should be generic");
                    f.name = name;
                    f.type_params = Vec::new();
                    f.owner_params = 0;
                    self.function(&mut f, args.get());
                    self.functions[id.0] = Some(f);
                }
                Work::Class { id, decl, args } => {
                    let fields = self.class_decls[decl.0]
                        .as_ref()
                        .expect("an instantiated class should be generic")
                        .fields
                        .iter()
                        .map(|field| (field.ty, field.name.clone(), field.mutable))
                        .collect::<Vec<_>>();
                    let fields = fields
                        .into_iter()
                        .map(|(ty, name, mutable)| Field {
                            name,
                            ty: self.ty(ty, args.get()),
                            mutable,
                        })
                        .collect();
                    self.out.classes[id.0].fields = fields;
                }
                Work::Enum { id, decl, args } => {
                    let decl = self.enum_decls[decl.0]
                        .as_ref()
                        .expect("an instantiated enum should be generic");
                    let mut variants = decl.variants.clone();
                    for p in variants.iter_mut().flat_map(|v| &mut v.fields) {
                        p.ty = self.ty(p.ty, args.get());
                        p.boxed = false;
                    }
                    self.out.enums[id.0].variants = variants;
                }
            }
        }
        layout_enums(&mut self.out.enums);
        self.out.functions = self
            .functions
            .into_iter()
            .map(|f| f.expect("the worklist should instantiate every function it reaches"))
            .collect();
        self.out
    }

    /// `ty` of a body instantiated with `args`, as the output names it.
    fn ty(&mut self, ty: Type, args: &[Type]) -> Type {
        let ty = self.subst(ty, args);
        self.output(ty)
    }

    /// `ty` with `args` for its type parameters and each associated type replaced by the type its
    /// conformance chose. The checker has accepted the program, and checked the bound that gives
    /// each base its conformance.
    fn subst(&self, ty: Type, args: &[Type]) -> Type {
        normalize(&self.conformances, ty.subst(args))
            .expect("a call that checks should find the conformance of every associated type")
    }

    /// `subst` for a list of arguments.
    fn subst_args(&self, list: TypeArgs, args: &[Type]) -> TypeArgs {
        let list: Vec<Type> = list.get().iter().map(|&ty| self.subst(ty, args)).collect();
        TypeArgs::intern(&list)
    }

    /// The output's type for the concrete checked type `ty`: each class and enum by its output
    /// id, with no arguments. Reaching a class's instantiation puts it on the worklist.
    fn output(&mut self, ty: Type) -> Type {
        if let Some(&done) = self.types.get(&ty) {
            return done;
        }
        let done = match ty.kind() {
            TypeKind::Class(id, args) => {
                Type::intern(TypeKind::Class(self.class(id, args), TypeArgs::NONE))
            }
            TypeKind::Enum(id, args) => {
                Type::intern(TypeKind::Enum(self.enum_id(id, args), TypeArgs::NONE))
            }
            TypeKind::Array(element) => Type::array(self.output(element)),
            TypeKind::Optional(payload) => Type::optional(self.output(payload)),
            TypeKind::Param(..) => {
                panic!("`subst` should replace every type parameter before `mono` maps a type")
            }
            _ => ty,
        };
        self.types.insert(ty, done);
        done
    }

    /// Reaches each of the concrete checked types `args`, before the instance that takes them
    /// gets its id, so that the instances inside them get lower ids.
    fn reach(&mut self, args: &[Type]) {
        for &ty in args {
            self.output(ty);
        }
    }

    /// The name of the instance `id` of the function `decl`, with the concrete checked `args`. A
    /// method is written from its owner type, the interface of its conformance block and its
    /// bare name (`write_method`), then its own arguments: `Stack<i64>.top`,
    /// `Pair<Circle>.Same.same`, `Stack<[i64]>.flat`. It writes the checked types, because an
    /// output type's head is a whole instance name. A name cut at `NAME_LIMIT` in any part ends
    /// in `#id`, so it stays distinct.
    fn instance_name(&self, decl: &Function, args: &[Type], id: usize) -> String {
        let head = |head| self.head(head);
        let mut out = String::new();
        let (own, mut cut) = if let (Some(method), Some(owner)) = (&decl.method, decl.owner) {
            let interface = decl.conformance.map(|(interface, args_of)| {
                (
                    self.out.interfaces[interface.0].name.as_str(),
                    self.subst_args(args_of, args),
                )
            });
            let cut = write_method(&mut out, self.subst(owner, args), interface, method, head);
            (&args[decl.owner_params..], cut)
        } else {
            out.push_str(&decl.name);
            (args, false)
        };
        cut |= write_args(&mut out, own, head);
        if cut {
            // `#` cannot occur in an identifier, so a cut name never meets a whole one.
            out.push('#');
            out.push_str(&id.to_string());
        }
        out
    }

    /// The name of the instance `id` of the class or enum `name`, with the concrete checked
    /// `args`, written as a program writes it: `Stack<i64>`. A name cut at `NAME_LIMIT` ends in
    /// `#id`.
    fn type_instance_name(&self, name: &str, args: &[Type], id: usize) -> String {
        let mut out = name.to_string();
        if write_args(&mut out, args, |head| self.head(head)) {
            out.push('#');
            out.push_str(&id.to_string());
        }
        out
    }

    /// The declared name of the checked class or enum `head`.
    fn head(&self, head: Head) -> &str {
        match head {
            Head::Assoc(interface, k) => &self.out.interfaces[interface.0].assoc[k as usize],
            Head::Class(id) => match &self.class_decls[id.0] {
                Some(decl) => &decl.name,
                None => &self.out.classes[self.class_ids[&(id, TypeArgs::NONE)].0].name,
            },
            Head::Enum(id) => match &self.enum_decls[id.0] {
                Some(decl) => &decl.name,
                None => &self.out.enums[self.enum_ids[&(id, TypeArgs::NONE)].0].name,
            },
        }
    }

    /// The output id of class `id` with `args`, given and put on the worklist when first reached.
    fn class(&mut self, id: ClassId, args: TypeArgs) -> ClassId {
        if let Some(&done) = self.class_ids.get(&(id, args)) {
            return done;
        }
        self.reach(args.get());
        let done = ClassId(self.out.classes.len());
        let decl = self.class_decls[id.0]
            .as_ref()
            .expect("a class reached with arguments should be generic");
        let class = Class {
            name: self.type_instance_name(&decl.name, args.get(), done.0),
            type_params: Vec::new(),
            fields: Vec::new(),
        };
        self.out.classes.push(class);
        self.class_ids.insert((id, args), done);
        self.work.push_back(Work::Class {
            id: done,
            decl: id,
            args,
        });
        done
    }

    /// The output id of enum `id` with `args`, given and put on the worklist when first reached.
    fn enum_id(&mut self, id: EnumId, args: TypeArgs) -> EnumId {
        if let Some(&done) = self.enum_ids.get(&(id, args)) {
            return done;
        }
        self.reach(args.get());
        let done = EnumId(self.out.enums.len());
        let decl = self.enum_decls[id.0]
            .as_ref()
            .expect("an enum reached with arguments should be generic");
        let e = Enum {
            name: self.type_instance_name(&decl.name, args.get(), done.0),
            type_params: Vec::new(),
            variants: Vec::new(),
            counted: false,
            cycle: None,
        };
        self.out.enums.push(e);
        self.enum_ids.insert((id, args), done);
        self.work.push_back(Work::Enum {
            id: done,
            decl: id,
            args,
        });
        done
    }

    /// The output id of function `id` called with the concrete `args`, given and put on the
    /// worklist when first reached.
    fn function_id(&mut self, id: FuncId, args: TypeArgs) -> FuncId {
        if let Some(&done) = self.function_ids.get(&(id, args)) {
            return done;
        }
        self.reach(args.get());
        let done = FuncId(self.functions.len());
        let decl = self.decls[id.0]
            .as_ref()
            .expect("a function reached with arguments should be generic");
        let name = self.instance_name(decl, args.get(), done.0);
        self.functions.push(None);
        self.function_ids.insert((id, args), done);
        self.work.push_back(Work::Function {
            id: done,
            decl: id,
            args,
            name,
        });
        done
    }

    /// Instantiates `f`'s types and calls with `args`, its own type parameters' arguments.
    fn function(&mut self, f: &mut Function, args: &[Type]) {
        for local in &mut f.locals {
            local.ty = self.ty(local.ty, args);
        }
        f.ret = self.ty(f.ret, args);
        // The output's ids replace the checker's in the types a method names as its own.
        f.owner = f.owner.map(|ty| self.ty(ty, args));
        f.conformance = f.conformance.map(|(interface, types)| {
            let types: Vec<Type> = (types.get().iter()).map(|&ty| self.ty(ty, args)).collect();
            (interface, TypeArgs::intern(&types))
        });
        // The name is written whole.
        f.method = None;
        self.block(&mut f.body, args);
    }

    fn block(&mut self, block: &mut Block, args: &[Type]) {
        block.ty = self.ty(block.ty, args);
        for stmt in &mut block.stmts {
            match stmt {
                Stmt::Let { init: e, .. } | Stmt::Expr(e) => self.expr(e, args),
                Stmt::Assign { place, value } | Stmt::Append { place, value } => {
                    self.place(place, args);
                    self.expr(value, args);
                }
            }
        }
        if let Some(value) = &mut block.value {
            self.expr(value, args);
        }
    }

    fn place(&mut self, place: &mut Place, args: &[Type]) {
        match place {
            Place::Local(_) => {}
            Place::Field { base, .. } => self.expr(base, args),
            Place::Index { base, index } => {
                self.place(base, args);
                self.expr(index, args);
            }
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per node kind, so the walk reads in one place"
    )]
    fn expr(&mut self, e: &mut Expr, args: &[Type]) {
        use ExprKind as K;
        e.ty = self.ty(e.ty, args);
        let ty = e.ty;
        match &mut e.kind {
            K::Int(_)
            | K::Float(_)
            | K::Bool(_)
            | K::Char(_)
            | K::Str(_)
            | K::None
            | K::Local(_)
            | K::ReadLine
            | K::Break { value: None, .. }
            | K::Continue { .. }
            | K::Return(None) => {}
            K::Call {
                func,
                types,
                args: operands,
            } => {
                *func = self.function_id(*func, self.subst_args(*types, args));
                *types = TypeArgs::NONE;
                self.exprs(operands, args);
            }
            K::InterfaceCall {
                interface,
                iargs,
                method,
                types,
                args: operands,
            } => {
                // The conformance of the receiver's type, which the checker found a bound for.
                let (receiver, rest) = (self.subst_args(*iargs, args).get().split_first())
                    .map(|(&receiver, rest)| (receiver, TypeArgs::intern(rest)))
                    .expect("an interface call should name its receiver's type");
                let (index, bindings) = find(&self.conformances, *interface, receiver, rest)
                    .expect("a call that checks should find the conformance of its receiver");
                let found = self.conformances.get(*interface, index).methods[*method as usize];
                // The conformance's parameters, then the requirement's own.
                let mut with = bindings.get().to_vec();
                with.extend_from_slice(self.subst_args(*types, args).get());
                let func = self.function_id(found, TypeArgs::intern(&with));
                let mut operands = mem::take(operands);
                self.exprs(&mut operands, args);
                e.kind = K::Call {
                    func,
                    types: TypeArgs::NONE,
                    args: operands,
                };
            }
            K::New {
                class,
                args: operands,
            } => {
                let TypeKind::Class(id, _) = ty.kind() else {
                    panic!("the checker should type a construction as its class")
                };
                *class = id;
                self.exprs(operands, args);
            }
            K::Variant {
                id, args: operands, ..
            } => {
                let TypeKind::Enum(enum_id, _) = ty.kind() else {
                    panic!("the checker should type a variant as its enum")
                };
                *id = enum_id;
                self.exprs(operands, args);
            }
            K::ArrayLit(operands)
            | K::Math { operands, .. }
            | K::Interpolate {
                parts: operands, ..
            }
            | K::Compare { operands, .. } => self.exprs(operands, args),
            K::Field { base: operand, .. }
            | K::Wrap(operand)
            | K::Convert(operand)
            | K::Print { operand, .. }
            | K::Panic(operand)
            | K::Len(operand)
            | K::Chars(operand)
            | K::Bytes(operand)
            | K::Words(operand)
            | K::FromChars(operand)
            | K::Sum(operand)
            | K::Unary { operand, .. }
            | K::Break {
                value: Some(operand),
                ..
            }
            | K::Return(Some(operand)) => self.expr(operand, args),
            K::ArrayFill { value: a, len: b }
            | K::Index { base: a, index: b }
            | K::Contains { base: a, value: b }
            | K::Binary { lhs: a, rhs: b, .. }
            | K::Concat { lhs: a, rhs: b }
            | K::Equal { lhs: a, rhs: b, .. }
            | K::Is { lhs: a, rhs: b } => {
                self.expr(a, args);
                self.expr(b, args);
            }
            K::If {
                cond: first,
                then_block,
                else_block,
            }
            | K::IfSome {
                scrutinee: first,
                then_block,
                else_block,
                ..
            } => {
                self.expr(first, args);
                self.block(then_block, args);
                if let Some(block) = else_block {
                    self.block(block, args);
                }
            }
            K::Match {
                scrutinee, arms, ..
            } => {
                self.expr(scrutinee, args);
                for arm in arms {
                    self.arm(arm, args);
                }
            }
            K::Loop(block) | K::Block(block) => self.block(block, args),
        }
    }

    fn arm(&mut self, arm: &mut Arm, args: &[Type]) {
        if let Some(guard) = &mut arm.guard {
            self.expr(guard, args);
        }
        self.block(&mut arm.body, args);
    }

    fn exprs(&mut self, exprs: &mut [Expr], args: &[Type]) {
        for e in exprs {
            self.expr(e, args);
        }
    }
}

/// Asserts that no type in `program` names a type parameter or holds type arguments: `mir` and
/// codegen read neither, so one left behind would be miscounted or mislaid silently.
fn assert_concrete(program: &Program) {
    let fields = program.classes.iter().flat_map(|c| &c.fields).map(|f| f.ty);
    let payloads = (program.enums.iter())
        .flat_map(|e| &e.variants)
        .flat_map(|v| &v.fields)
        .map(|p| p.ty);
    for ty in fields.chain(payloads) {
        assert!(
            concrete(ty),
            "mono should leave no generic field type: {ty:?}"
        );
    }
    for f in &program.functions {
        let mut exprs = Vec::new();
        f.body.exprs(&mut exprs);
        let mut blocks = vec![&f.body];
        for e in &exprs {
            match &e.kind {
                ExprKind::If {
                    then_block,
                    else_block,
                    ..
                }
                | ExprKind::IfSome {
                    then_block,
                    else_block,
                    ..
                } => blocks.extend(
                    [Some(then_block), else_block.as_ref()]
                        .into_iter()
                        .flatten(),
                ),
                ExprKind::Match { arms, .. } => blocks.extend(arms.iter().map(|arm| &arm.body)),
                ExprKind::Loop(block) | ExprKind::Block(block) => blocks.push(block),
                _ => {}
            }
        }
        let types = exprs
            .iter()
            .map(|e| e.ty)
            .chain(f.locals.iter().map(|l| l.ty));
        let types = types.chain(blocks.iter().map(|b| b.ty));
        for ty in types.chain([f.ret]) {
            assert!(
                concrete(ty),
                "mono should leave no generic type in {}: {ty:?}",
                f.name
            );
        }
        for e in exprs {
            if let ExprKind::Call { types, .. } = e.kind {
                assert_eq!(
                    types,
                    TypeArgs::NONE,
                    "mono should leave no call's arguments in {}",
                    f.name
                );
            }
            assert!(
                !matches!(e.kind, ExprKind::InterfaceCall { .. }),
                "mono should leave no interface call in {}",
                f.name
            );
        }
    }
}

/// Whether `ty` names no type parameter and holds no type arguments, at any depth.
fn concrete(ty: Type) -> bool {
    match ty.kind() {
        TypeKind::Param(..) | TypeKind::Assoc(..) => false,
        TypeKind::Class(_, args) | TypeKind::Enum(_, args) => args == TypeArgs::NONE,
        TypeKind::Array(inner) | TypeKind::Optional(inner) => concrete(inner),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use rustc_hash::FxHashSet;

    use super::*;
    use crate::check::check_alone;
    use crate::parser::parse_ok;
    use crate::types::NAME_LIMIT;

    /// The program `front_end` gives for `src`, which checks without an error.
    fn instantiated(src: &str) -> Program {
        let (program, diagnostics) = crate::front_end::front_end(src);
        program.unwrap_or_else(|| panic!("{diagnostics:?}"))
    }

    fn function_names(program: &Program) -> Vec<&str> {
        program.functions.iter().map(|f| f.name.as_str()).collect()
    }

    fn class_names(program: &Program) -> Vec<&str> {
        program.classes.iter().map(|c| c.name.as_str()).collect()
    }

    const FIRST: &str =
        "fn first<T>(xs: [T]) -> T? {\n    if xs.len() > 0 { xs[0] } else { None }\n}";

    /// The names of `program`'s functions, classes and enums.
    fn all_names(program: &Program) -> Vec<&str> {
        let enums = program.enums.iter().map(|e| e.name.as_str());
        let mut names = function_names(program);
        names.extend(class_names(program).into_iter().chain(enums));
        names
    }

    /// `f{i}` calls `f{i+1}` with `P(a: x, b: Q.V(x))`, so the argument of `f25` is a type
    /// whose text, written whole, has 2^24 leaves. The enum `Q` grows with it.
    #[test]
    fn instance_names_should_stay_bounded() {
        let mut src =
            String::from("class P<A, B> {\n    a: A\n    b: B\n}\nenum Q<A> {\n    V(A)\n}\n");
        for i in 1..=24 {
            writeln!(
                src,
                "fn f{i}<T>(x: T) -> i64 {{ f{}(P(a: x, b: Q.V(x))) }}",
                i + 1
            )
            .expect("writing to a `String` should succeed");
        }
        src += "fn f25<T>(x: T) -> i64 { 1 }\nfn main() {\n    f1(1)\n}";
        let program = instantiated(&src);
        let names = all_names(&program);
        for name in &names {
            assert!(name.len() <= NAME_LIMIT + 64, "{name}");
        }
        let distinct: FxHashSet<&str> = names.iter().copied().collect();
        assert_eq!(distinct.len(), names.len(), "{names:?}");
        assert!(names.contains(&"f1<i64>"), "{names:?}");
        assert!(names.contains(&"P<i64, Q<i64>>"), "{names:?}");
        assert!(names.contains(&"Q<i64>"), "{names:?}");
        assert!(
            names
                .iter()
                .any(|name| name.starts_with("Q<") && name.contains("..."))
        );
    }

    /// The class name is longer than `NAME_LIMIT`, so the two instances of `g`, of `P` and of
    /// `P.m` agree in their first `NAME_LIMIT` bytes and differ only in their last argument.
    #[test]
    fn cut_names_should_stay_distinct() {
        let long = format!("A{}", "a".repeat(300));
        let program = instantiated(&format!(
            "class {long} {{\n    x: i64\n}}\nclass P<A, B> {{\n    a: A\n    b: B\n    fn m(self) -> i64 {{ 1 }}\n}}\n\
             fn g<T>(x: T) -> i64 {{ 1 }}\nfn main() {{\n    let p = P(a: {long}(x: 1), b: 1)\n    \
             let q = P(a: {long}(x: 1), b: \"s\")\n    print(g(p) + g(q) + p.m() + q.m() + g(1))\n}}"
        ));
        let functions = function_names(&program);
        let classes = class_names(&program);
        for (names, count) in [(&functions, 4), (&classes, 2)] {
            let cut: Vec<(usize, &str)> = (names.iter().copied().enumerate())
                .filter(|(_, name)| name.contains("..."))
                .collect();
            assert_eq!(cut.len(), count, "{names:?}");
            let distinct: FxHashSet<&str> = cut.iter().map(|&(_, name)| name).collect();
            assert_eq!(distinct.len(), count, "{names:?}");
            for (id, name) in cut {
                assert!(name.ends_with(&format!("#{id}")), "{name} at {id}");
            }
        }
        // A name under the limit gets no `#`.
        assert!(functions.contains(&"g<i64>"), "{functions:?}");
        for name in functions.iter().chain(&classes) {
            assert_eq!(name.contains('#'), name.contains("..."), "{name}");
        }
    }

    #[test]
    fn each_argument_list_should_be_instantiated_once() {
        let program = instantiated(&format!(
            "{FIRST}\nfn main() {{\n    let a = first([1])\n    let b = first([2, 3])\n    let c = first([\"s\"])\n}}"
        ));
        assert_eq!(
            function_names(&program),
            ["main", "first<i64>", "first<string>"]
        );
    }

    /// A queue takes `a`'s body before `b`'s, so `c` is reached before `d`; a stack would take
    /// `b`'s first. Class fields come before any function body.
    #[test]
    fn instantiations_should_follow_the_worklist_order() {
        let program = instantiated(
            "fn a<T>(x: T) { c(x) }\nfn b<T>(x: T) { d(x) }\nfn c<T>(x: T) {}\nfn d<T>(x: T) {}\n\
             fn main() {\n    a(1)\n    b(2)\n}",
        );
        assert_eq!(
            function_names(&program),
            ["main", "a<i64>", "b<i64>", "c<i64>", "d<i64>"]
        );
        let program = instantiated(
            "class Box<T> {\n    v: T\n}\nclass Holder {\n    b: Box<i64>\n}\n\
             fn main() {\n    let b = Box(v: \"a\")\n}",
        );
        assert_eq!(class_names(&program), ["Holder", "Box<i64>", "Box<string>"]);
        // Enum payloads come before any function body.
        let program = instantiated(
            "enum S {\n    A(Box<i64>)\n}\nclass Box<T> {\n    v: T\n}\n\
             fn main() {\n    let b = Box(v: \"a\")\n}",
        );
        assert_eq!(class_names(&program), ["Box<i64>", "Box<string>"]);
        // An instance inside a type argument comes before the instance that takes it.
        let program = instantiated(
            "class Box<T> {\n    v: T\n}\nfn main() {\n    let b = Box(v: Box(v: 1))\n}",
        );
        assert_eq!(class_names(&program), ["Box<i64>", "Box<Box<i64>>"]);
    }

    #[test]
    fn no_type_parameter_should_survive_instantiation() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/run");
        let mut seen = 0;
        for entry in std::fs::read_dir(dir).expect("tests/run should be readable") {
            let path = entry.expect("a directory entry should be readable").path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if !(name.starts_with("generic_") && path.extension().is_some_and(|e| e == "sisu")) {
                continue;
            }
            seen += 1;
            let src = std::fs::read_to_string(&path).expect("a case should be readable");
            assert_concrete(&instantiated(&src));
        }
        assert!(seen >= 3, "only {seen} generic_ cases in {dir}");
    }

    #[test]
    fn methods_should_be_instantiated_on_demand() {
        let program = instantiated(
            "class C<T> {\n    v: T\n    fn up(self) -> C<[T]> { C(v: [self.v]) }\n}\n\
             fn main() {\n    let c = C(v: 1).up()\n}",
        );
        assert_eq!(function_names(&program), ["main", "C<i64>.up"]);
        // `main`'s local `c` is a `C<[i64]>`, and a function's locals come before its body.
        assert_eq!(class_names(&program), ["C<[i64]>", "C<i64>"]);
    }

    /// A generic body that holds every kind of statement, place and expression, a non-generic
    /// enum whose payload is an instantiation, and a generic enum declared before it, which no
    /// type reaches and which takes no id. `instantiate`'s own check finds no type left generic.
    #[test]
    fn every_node_should_be_instantiated() {
        let program = instantiated(
            "enum Unused<T> {\n    U(T)\n}\nenum Shape {\n    Boxed(Box<i64>)\n    Empty\n}\n\
             class Box<T> {\n    v: T\n}\n\
             fn fill<T>(xs: [Box<T>], x: T) -> i64 {\n    let mut ys = xs\n    ys[0] = Box(v: x)\n\
                 let mut s = [\"s\"]\n    s[0] += \"a\"\n    let words = \"a b\".words()\n\
                 let n = [1, 2].sum() + \"ab\".bytes().len() + string(\"ab\".chars()).len() + words.len()\n\
                 let m = 2.min(3) + [0; 2].len() + (i64(2.5) ?? 0)\n\
                 if [1].contains(1) && ys[0] is Box(v: x) {\n        panic(\"{n} {m}\")\n    }\n\
                 match Shape.Boxed(Box(v: 1)) {\n        Boxed(b) if [x].len() > b.v => [x].len()\n\
                     _ => [x, x].len()\n    }\n}\n\
             fn main() {\n    print(fill([Box(v: 2)], 3))\n}",
        );
        assert_eq!(function_names(&program), ["main", "fill<i64>"]);
        assert_eq!(class_names(&program), ["Box<i64>"]);
        let enums: Vec<&str> = program.enums.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(enums, ["Shape"]);
        let mut exprs = Vec::new();
        program.functions[1].body.exprs(&mut exprs);
        let variant = exprs
            .iter()
            .find(|e| matches!(e.kind, ExprKind::Variant { .. }))
            .expect("`fill` builds a `Shape`");
        assert_eq!(program.type_name(variant.ty), "Shape");
    }

    /// A generic class or enum that is the program's only generic declaration is still
    /// instantiated, or dropped when nothing reaches it.
    #[test]
    fn a_lone_generic_class_or_enum_should_be_instantiated() {
        let used =
            instantiated("class Box<T> {\n    v: T\n}\nfn main() {\n    print(Box(v: 42).v)\n}");
        assert_eq!(class_names(&used), ["Box<i64>"]);
        let unused = instantiated("class C<T> {\n    v: T\n}\nfn main() {\n    print(1)\n}");
        assert!(unused.classes.is_empty(), "{:?}", class_names(&unused));
        let unused = instantiated("enum Wrap<T> {\n    W(T)\n}\nfn main() {\n    print(1)\n}");
        assert!(unused.enums.is_empty(), "{:?}", unused.enums);
    }

    /// The box closes a cycle on the declaring enum's own field, never on a field inside the
    /// generic type: a field whose declared type is a parameter, or an optional of one, is never
    /// boxed, and a field whose declared type names a parameter only when a cycle remains
    /// without it, as `B<i64>.Y`'s does through `A<B<i64>>.X`, a parameter field, and `C<i64>.Y`'s
    /// through `D<C<i64>>.X`, an optional of one. `G<i64>.P` closes its cycle through
    /// `Wrap2<G<i64>>.W`, whose `Two<T>` names a parameter too, yet only `G<i64>.P` names its
    /// own declaration. `H<i64>.P` does too, though `Wrap3<H<i64>>.W` names `Wrap3<i64>`, which
    /// closes its own cycle and is boxed after; so does `K<i64>.P`, through an optional. An
    /// optional field's payload is named too (`Wrap4<i64>.W`), and `R<i64, Q<i64>>.Y`'s
    /// `Two<B>`, of a declaration off `R`'s cycle, is never boxed. `M<i64>.F` and `N2<i64>.K`
    /// name each other, not themselves, yet `Wrap3<N2<i64>>.W` stays inline as `Wrap3<H<i64>>.W`
    /// does. `U<i64>.Q` names `S<T>` too, yet `S<i64>.P`, which names its own declaration,
    /// breaks every cycle through it alone, directly or, in `S2<i64>.P`, through an optional.
    /// Every member of a component with a box is on that cycle and counted.
    #[test]
    fn the_box_should_go_on_the_declaring_enum() {
        let program = instantiated(
            "enum Two<T> {\n    Both(T, T)\n}\nenum Expr {\n    Lit(i64)\n    Pair(Two<Expr>)\n}\n\
             enum Opt2<T> {\n    S(T?)\n    N\n}\nenum E {\n    L\n    P(Opt2<E>)\n}\n\
             enum Wrap2<T> {\n    W(Two<T>)\n}\nenum F {\n    L\n    P(Wrap2<F>)\n}\n\
             enum Tree<T> {\n    Leaf\n    Node(Tree<T>, T, Tree<T>)\n}\n\
             enum A<T> {\n    X(T)\n}\nenum B<T> {\n    Y(A<B<T>>)\n    Z\n}\n\
             enum D<T> {\n    X(T?)\n}\nenum C<T> {\n    Y(D<C<T>>)\n    Z\n}\n\
             enum G<T> {\n    L(T)\n    P(Wrap2<G<T>>)\n}\n\
             enum P2<A, B> {\n    P(A, B)\n}\nenum Wrap3<T> {\n    W(P2<T, Wrap3<i64>>)\n    N\n}\n\
             enum H<T> {\n    L(T)\n    P(Wrap3<H<T>>)\n}\n\
             enum K<T> {\n    L(T)\n    P(Wrap3<K<T>>?)\n}\n\
             enum Wrap4<T> {\n    W(P2<T, Wrap4<i64>>?)\n    N\n}\n\
             enum R<A, B> {\n    Y(Two<B>)\n}\nenum Q<T> {\n    X(R<T, Q<i64>>)\n    N\n}\n\
             enum M<T> {\n    F(Wrap3<N2<T>>)\n    Z\n}\nenum N2<T> {\n    K(M<T>)\n    Y(T)\n}\n\
             enum S<T> {\n    P(P2<S<T>, U<T>>)\n    Z\n}\nenum U<T> {\n    Q(S<T>)\n    Z\n}\n\
             enum S2<T> {\n    P(P2<S2<T>, U2<T>>?)\n    Z\n}\nenum U2<T> {\n    Q(S2<T>)\n    Z\n}\n\
             fn main() {\n    let t: Tree<i64> = Tree.Leaf\n    let b: B<i64> = B.Z\n    let c: C<i64> = C.Z\n    let g: G<i64> = G.L(1)\n    let h: H<i64> = H.L(1)\n    let k: K<i64> = K.L(1)\n    \
             let w: Wrap4<i64> = Wrap4.N\n    let q: Q<i64> = Q.N\n    \
             let m: M<i64> = M.Z\n    let s: S<i64> = S.Z\n    let s2: S2<i64> = S2.Z\n}",
        );
        let printed = program.to_string();
        for part in [
            "(enum Expr (Lit i64) (Pair (box Two<Expr>)))",
            "(enum Two<Expr> (Both Expr Expr))",
            "(enum E L (P (box Opt2<E>)))",
            "(enum Opt2<E> (S E?) N)",
            "(enum F L (P (box Wrap2<F>)))",
            "(enum Wrap2<F> (W Two<F>))",
            "(enum Two<F> (Both F F))",
            "(enum Tree<i64> Leaf (Node (box Tree<i64>) i64 (box Tree<i64>)))",
            "(enum B<i64> (Y (box A<B<i64>>)) Z)",
            "(enum A<B<i64>> (X B<i64>))",
            "(enum C<i64> (Y (box D<C<i64>>)) Z)",
            "(enum D<C<i64>> (X C<i64>?))",
            "(enum G<i64> (L i64) (P (box Wrap2<G<i64>>)))",
            "(enum Wrap2<G<i64>> (W Two<G<i64>>))",
            "(enum Two<G<i64>> (Both G<i64> G<i64>))",
            "(enum H<i64> (L i64) (P (box Wrap3<H<i64>>)))",
            "(enum Wrap3<H<i64>> (W P2<H<i64>, Wrap3<i64>>) N)",
            "(enum Wrap3<i64> (W (box P2<i64, Wrap3<i64>>)) N)",
            "(enum K<i64> (L i64) (P (box Wrap3<K<i64>>?)))",
            "(enum Wrap3<K<i64>> (W P2<K<i64>, Wrap3<i64>>) N)",
            "(enum Wrap4<i64> (W (box P2<i64, Wrap4<i64>>?)) N)",
            "(enum Q<i64> (X (box R<i64, Q<i64>>)) N)",
            "(enum R<i64, Q<i64>> (Y Two<Q<i64>>))",
            "(enum M<i64> (F (box Wrap3<N2<i64>>)) Z)",
            "(enum N2<i64> (K (box M<i64>)) (Y i64))",
            "(enum Wrap3<N2<i64>> (W P2<N2<i64>, Wrap3<i64>>) N)",
            "(enum S<i64> (P (box P2<S<i64>, U<i64>>)) Z)",
            "(enum U<i64> (Q S<i64>) Z)",
            "(enum S2<i64> (P (box P2<S2<i64>, U2<i64>>?)) Z)",
            "(enum U2<i64> (Q S2<i64>) Z)",
        ] {
            assert!(printed.contains(part), "`{part}` in\n{printed}");
        }
        let named = |name: &str| {
            (program.enums.iter())
                .find(|e| e.name == name)
                .unwrap_or_else(|| panic!("no enum {name} in\n{printed}"))
        };
        assert!(named("Expr").cycle.is_some());
        assert_eq!(named("Two<Expr>").cycle, named("Expr").cycle);
        for name in ["Two<Expr>", "Opt2<E>", "Wrap2<F>", "Two<F>"] {
            assert!(named(name).counted, "{name}");
        }
    }

    #[test]
    fn concrete_should_look_through_arrays_and_optionals() {
        let param = Type::intern(TypeKind::Param(crate::symbol::Symbol::intern("T"), 0));
        let i64s = TypeArgs::intern(&[Type::I64]);
        let class = |args| Type::intern(TypeKind::Class(ClassId(0), args));
        let enumeration = Type::intern(TypeKind::Enum(EnumId(0), i64s));
        for (ty, want) in [
            (Type::I64, true),
            (Type::array(Type::optional(class(TypeArgs::NONE))), true),
            (param, false),
            (class(i64s), false),
            (enumeration, false),
            (Type::array(param), false),
            (Type::optional(class(i64s)), false),
        ] {
            assert_eq!(concrete(ty), want, "{ty:?}");
        }
    }

    #[test]
    #[should_panic(expected = "mono should leave no generic type in id")]
    fn a_type_parameter_left_behind_should_be_caught() {
        let src = "fn id<T>(x: T) -> T { x }\nfn main() {\n    print(id(1))\n}";
        let (program, diagnostics) = check_alone(&parse_ok(src), src, &[]);
        assert_concrete(&program.unwrap_or_else(|| panic!("{diagnostics:?}")));
    }

    /// A type parameter left only in a nested block's type is caught: in either branch of an
    /// `if`, an `if let`'s, a `match` arm's, a loop's or a block's.
    #[test]
    fn a_block_type_left_generic_should_be_caught() {
        use crate::source::Span;
        use crate::tir::{LocalId, Pattern, PatternKind};
        let span = Span::new(0, 0);
        let param = Type::intern(TypeKind::Param(crate::symbol::Symbol::intern("T"), 0));
        let int = || Expr {
            kind: ExprKind::Int(1),
            ty: Type::I64,
            span,
        };
        let block = |ty| Block {
            stmts: Vec::new(),
            value: Some(Box::new(int())),
            ty,
        };
        let cond = || {
            Box::new(Expr {
                kind: ExprKind::Bool(true),
                ty: Type::BOOL,
                span,
            })
        };
        let if_some = |then_block| ExprKind::IfSome {
            bind: LocalId(0),
            scrutinee: Box::new(Expr {
                kind: ExprKind::None,
                ty: Type::optional(Type::I64),
                span,
            }),
            then_block,
            else_block: Some(block(Type::I64)),
        };
        let arm = Arm {
            pattern: Pattern {
                kind: PatternKind::Wild,
                span,
            },
            bindings: Vec::new(),
            guard: None,
            body: block(param),
            reachable: true,
        };
        for (case, kind) in [
            (
                "then",
                ExprKind::If {
                    cond: cond(),
                    then_block: block(param),
                    else_block: Some(block(Type::I64)),
                },
            ),
            (
                "else",
                ExprKind::If {
                    cond: cond(),
                    then_block: block(Type::I64),
                    else_block: Some(block(param)),
                },
            ),
            ("if let", if_some(block(param))),
            (
                "match",
                ExprKind::Match {
                    scrutinee: Box::new(int()),
                    temp: LocalId(0),
                    arms: vec![arm],
                },
            ),
            ("loop", ExprKind::Loop(block(param))),
            ("block", ExprKind::Block(block(param))),
        ] {
            let mut program = instantiated("fn main() {}");
            let expr = Expr {
                kind,
                ty: Type::I64,
                span,
            };
            program.functions[0].body.stmts.push(Stmt::Expr(expr));
            let caught = std::panic::catch_unwind(|| assert_concrete(&program));
            let message = caught.expect_err(case);
            let message = message.downcast_ref::<String>().map_or("", String::as_str);
            assert!(
                message.contains("no generic type in main"),
                "{case}: {message}"
            );
        }
    }

    #[test]
    fn a_program_without_generics_should_stay_unchanged() {
        let src = "class P {\n    x: i64\n    fn get(self) -> i64 { self.x }\n}\n\
                   enum E {\n    A(P)\n    B\n}\n\
                   fn f(e: E) -> i64 {\n    match e {\n        E.A(p) => p.get()\n        E.B => 0\n    }\n}\n\
                   fn main() {\n    print(f(E.A(P(x: 1))))\n}";
        let (program, diagnostics) = check_alone(&parse_ok(src), src, &[]);
        let program = program.unwrap_or_else(|| panic!("{diagnostics:?}"));
        let (text, count) = (program.to_string(), program.functions.len());
        let program = instantiate(program);
        assert_eq!(program.to_string(), text);
        assert_eq!(program.functions.len(), count);
    }

    /// The arguments of a conformance's interface name output classes: `G<i64>` in the symbol is
    /// the output's class.
    #[test]
    fn a_conformance_method_should_name_its_interface_arguments_by_output_ids() {
        let program = instantiated(
            "interface Label<T> {\n    fn name(self) -> T\n}\nclass G<T> {\n    value: T\n}\nclass C {}\nextend C: Label<G<i64>> {\n    fn name(self) -> G<i64> { G(value: 1) }\n}\nfn main() {\n    print(C().name().value)\n}",
        );
        let method = (program.functions.iter())
            .find(|f| f.name == "C.Label<G<i64>>.name")
            .expect("the method should be kept");
        let (_, args) = method.conformance.expect("the conformance should be kept");
        let [TypeKind::Class(id, _)] = args.get().iter().map(|t| t.kind()).collect::<Vec<_>>()[..]
        else {
            panic!("the argument should be a class: {args:?}");
        };
        assert_eq!(program.classes[id.0].name, "G<i64>");
    }

    /// Two conformances of one generic interface give two symbols, with the interface's arguments
    /// in them, on the path that instantiates nothing and on the one that does.
    #[test]
    fn two_conformances_of_a_generic_interface_should_get_two_symbols() {
        let blocks = "interface Convert<T> {\n    fn convert(self) -> T\n}\nclass Fahrenheit {\n    d: f64\n}\nclass Kelvin {\n    d: f64\n}\nclass Celsius {\n    d: f64\n}\nextend Celsius: Convert<Fahrenheit> {\n    fn convert(self) -> Fahrenheit { Fahrenheit(d: self.d) }\n}\nextend Celsius: Convert<Kelvin> {\n    fn convert(self) -> Kelvin { Kelvin(d: self.d) }\n}\n";
        let main = "fn main() {\n    let c = Celsius(d: 1.0)\n    let f: Fahrenheit = c.convert()\n    let k: Kelvin = c.convert()\n    print(f.d + k.d)\n}";
        // Nothing generic: the fast path, which writes the symbols from the checker's fields.
        let before = FAST_PATHS.get();
        let program = instantiated(&format!("{blocks}{main}"));
        assert_eq!(FAST_PATHS.get(), before + 1);
        // A generic function that nothing calls changes nothing but the path.
        let generic = format!("{blocks}fn id<T>(x: T) -> T {{ x }}\n{main}");
        for program in [program, instantiated(&generic)] {
            let names = function_names(&program);
            for want in [
                "Celsius.Convert<Fahrenheit>.convert",
                "Celsius.Convert<Kelvin>.convert",
            ] {
                assert!(names.contains(&want), "{want} in {names:?}");
            }
        }
    }

    /// Two conformances whose symbols are alike up to the cut take `#id`, each its own, on the
    /// path that instantiates nothing: the program has no generic class, enum or function, and
    /// an interface is none of those.
    #[test]
    fn conformances_with_long_interface_arguments_should_get_distinct_symbols() {
        let long = "L".repeat(100);
        let src = format!(
            "interface Conv4<A, B, C, D> {{\n    fn convert(self) -> D\n}}\nclass {long} {{}}\nclass C {{}}\nextend C: Conv4<{long}, {long}, {long}, i64> {{\n    fn convert(self) -> i64 {{ 1 }}\n}}\nextend C: Conv4<{long}, {long}, {long}, bool> {{\n    fn convert(self) -> bool {{ true }}\n}}\nfn main() {{\n    let c = C()\n    let a: i64 = c.convert()\n    let b: bool = c.convert()\n    print(a)\n    print(b)\n}}"
        );
        let before = FAST_PATHS.get();
        let program = instantiated(&src);
        assert_eq!(FAST_PATHS.get(), before + 1, "the fast path should run");
        let cut: Vec<&str> = (function_names(&program).into_iter())
            .filter(|name| name.starts_with("C.Conv4<"))
            .collect();
        assert_eq!(cut.len(), 2, "{cut:?}");
        assert_ne!(cut[0], cut[1]);
        for name in cut {
            assert!(name.contains("...") && name.contains('#'), "{name}");
        }
    }

    /// Two conformances of one generic interface to one instantiation of a generic class, whose
    /// arguments the occurs check keeps apart, name the interface's arguments in the symbols.
    #[test]
    fn conformances_of_one_instantiation_should_name_the_interface_arguments() {
        let program = instantiated(
            "interface Convert<T> {\n    fn convert(self) -> T\n}\nclass Box<T> {\n    value: T\n}\nextend<T> Box<T>: Convert<T> {\n    fn convert(self) -> T { self.value }\n}\nextend<T> Box<[T]>: Convert<T> {\n    fn convert(self) -> T { self.value[0] }\n}\nfn main() {\n    let b = Box(value: [1])\n    let a: [i64] = b.convert()\n    let n: i64 = b.convert()\n    print(a.len() + n)\n}",
        );
        let names = function_names(&program);
        for want in [
            "Box<[i64]>.Convert<[i64]>.convert",
            "Box<[i64]>.Convert<i64>.convert",
        ] {
            assert!(names.contains(&want), "{want} in {names:?}");
        }
    }

    /// A method is named from the type it is declared on, as the block writes it, whichever
    /// parameters the block has: `Stack<i64>.top` for `extend<T> Stack<T>`, `Stack<[i64]>.flat`
    /// for `extend<T> Stack<[T]>`, and the interface of a conformance block between the type and
    /// the method.
    #[test]
    fn a_method_of_an_extend_block_should_be_named_from_its_owner() {
        let program = instantiated(
            "interface Same {\n    fn same(self, other: Self) -> bool\n}\n\
             class Circle: Same {\n    r: f64\n    fn same(self, other: Circle) -> bool { self.r == other.r }\n}\n\
             class Stack<T> {\n    items: [T]\n}\n\
             class Pair<T> {\n    first: T\n    second: T\n}\n\
             extend<T> Stack<T> {\n    fn top(self) -> T? { self.items[0] }\n}\n\
             extend<T> Stack<[T]> {\n    fn flat(self) -> [T] { self.items[0] }\n}\n\
             extend Stack<f64> {\n    fn sum2<U>(self, u: U) -> U { u }\n}\n\
             extend<T: Same> Pair<T>: Same {\n    fn same(self, other: Self) -> bool { self.first.same(other.first) }\n}\n\
             fn agree<T: Same>(a: T, b: T) -> bool { a.same(b) }\n\
             fn main() {\n    let c = Circle(r: 1.0)\n    print(Stack(items: [1]).top() ?? 0)\n    print(Stack(items: [[1]]).flat().len())\n    print(Stack(items: [1.5]).sum2(\"s\"))\n    print(agree(Pair(first: c, second: c), Pair(first: c, second: c)))\n}",
        );
        let names = function_names(&program);
        for want in [
            "Stack<i64>.top",
            "Stack<[i64]>.flat",
            "Stack<f64>.sum2<string>",
            "Pair<Circle>.Same.same",
        ] {
            assert!(names.contains(&want), "{want} in {names:?}");
        }
    }

    /// A method name that is cut in any component ends in the id of the function, so that two cut
    /// names stay two, on the path that names no instance too.
    #[test]
    fn a_cut_method_name_should_end_in_the_id_of_the_function_when_nothing_is_instantiated() {
        let src = "interface Label {\n    fn name(self) -> string\n}\nclass C {}\nextend C: Label {\n    fn name(self) -> string { \"c\" }\n}\nfn main() {\n    print(C().name())\n}";
        let (program, diagnostics) = check_alone(&parse_ok(src), src, &[]);
        let mut program = program.unwrap_or_else(|| panic!("{diagnostics:?}"));
        // The interface's arguments name a type longer than a name may be.
        let mut deep = Type::intern(TypeKind::Class(ClassId(0), TypeArgs::NONE));
        for _ in 0..NAME_LIMIT {
            deep = Type::array(deep);
        }
        let method = (program.functions.iter_mut())
            .find(|f| f.name == "C.name")
            .expect("the method should be declared");
        let (interface, _) = method.conformance.expect("a conformance's method");
        method.conformance = Some((interface, TypeArgs::intern(&[deep])));
        let program = instantiate(program);
        let method = (program.functions.iter())
            .find(|f| f.name.starts_with("C.Label<"))
            .expect("the method should be named");
        let name = method.name.as_str();
        assert!(name.contains("...") && name.ends_with("#0"), "{name}");
        // The name is whole: the output prints it, `#0` included, not the parts it was cut from.
        assert!(method.method.is_none());
        assert!(
            program.to_string().contains(&format!("(fn {name} (")),
            "{program}"
        );
    }

    /// The unused generic class comes before `C`, so `C` has another id in the output than in the
    /// checked program, and the printed owner of its conformance method reads the output's.
    #[test]
    fn a_conformance_method_should_name_its_owner_by_the_output_id() {
        let src = "interface Label {\n    fn name(self) -> string\n}\nclass G<T> {\n    value: T\n}\nclass C {}\nextend C: Label {\n    fn name(self) -> string { \"c\" }\n}\nfn main() {\n    print(C().name())\n}";
        let program = instantiated(src);
        assert!(
            program.to_string().contains("(fn C.Label.name ("),
            "{program}"
        );
    }

    /// An interface call becomes a call of the method of the conformance that the receiver's type
    /// has, in each instantiation: no interface call is left in the output.
    #[test]
    fn an_interface_call_should_become_a_call_of_the_conformance_method() {
        let program = instantiated(
            "interface Shape {\n    fn area(self) -> f64\n}\n\
             class Circle {\n    r: f64\n}\n\
             extend Circle: Shape {\n    fn area(self) -> f64 { self.r }\n}\n\
             class Square: Shape {\n    side: f64\n    fn area(self) -> f64 { self.side }\n}\n\
             fn total<T: Shape>(x: T) -> f64 {\n    x.area()\n}\n\
             fn main() {\n    print(total(Circle(r: 1.0)))\n    print(total(Square(side: 2.0)))\n}",
        );
        let printed = program.to_string();
        for part in [
            "(fn total<Circle> ((x#0 Circle)) f64 (block (call Circle.Shape.area x#0)))",
            "(fn total<Square> ((x#0 Square)) f64 (block (call Square.area x#0)))",
        ] {
            assert!(printed.contains(part), "{part} in {printed}");
        }
        assert!(!printed.contains("(call Shape.area"), "{printed}");
    }

    /// A call through a bound of a generic interface finds the conformance whose interface
    /// arguments are the bound's: the same type conforms twice, and each bound reaches its own.
    #[test]
    fn an_interface_call_should_find_the_conformance_by_the_arguments_of_its_bound() {
        let program = instantiated(
            "interface Convert<T> {\n    fn convert(self) -> T\n}\n\
             class Fahrenheit {\n    d: f64\n}\n\
             class Kelvin {\n    d: f64\n}\n\
             class Celsius {\n    d: f64\n}\n\
             extend Celsius: Convert<Fahrenheit> {\n    fn convert(self) -> Fahrenheit { Fahrenheit(d: self.d) }\n}\n\
             extend Celsius: Convert<Kelvin> {\n    fn convert(self) -> Kelvin { Kelvin(d: self.d) }\n}\n\
             fn to_f<T: Convert<Fahrenheit>>(x: T) -> Fahrenheit {\n    x.convert()\n}\n\
             fn to_k<T: Convert<Kelvin>>(x: T) -> Kelvin {\n    x.convert()\n}\n\
             fn main() {\n    let c = Celsius(d: 1.0)\n    print(to_f(c).d + to_k(c).d)\n}",
        );
        let printed = program.to_string();
        for part in [
            "(fn to_f<Celsius> ((x#0 Celsius)) Fahrenheit (block (call Celsius.Convert<Fahrenheit>.convert x#0)))",
            "(fn to_k<Celsius> ((x#0 Celsius)) Kelvin (block (call Celsius.Convert<Kelvin>.convert x#0)))",
        ] {
            assert!(printed.contains(part), "{part} in {printed}");
        }
    }

    /// The conformance's parameters come before the method's own type arguments in the
    /// instantiation: the interface call of a requirement with a type parameter of its own names
    /// the method with the argument the call infers.
    #[test]
    fn an_interface_call_should_instantiate_the_method_with_its_own_type_arguments() {
        let program = instantiated(
            "interface Keep {\n    fn keep<U>(self, x: U) -> U\n}\n\
             class C {}\n\
             extend C: Keep {\n    fn keep<V>(self, x: V) -> V { x }\n}\n\
             fn go<T: Keep>(t: T) -> string {\n    let n: i64 = t.keep(1)\n    t.keep(\"a\")\n}\n\
             fn main() {\n    print(go(C()))\n}",
        );
        let names = function_names(&program);
        assert!(names.contains(&"C.Keep.keep<i64>"), "{names:?}");
        assert!(names.contains(&"C.Keep.keep<string>"), "{names:?}");
    }
}
