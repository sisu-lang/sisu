//! Monomorphization: the checked program, whose generic functions, classes and enums were each
//! checked once for all their type arguments, becomes one with no type parameter left. Each list
//! of type arguments the program reaches gets its own copy of the declaration, named with its
//! arguments as a program writes them (`first<i64>`, `Stack<[string]>`, `Stack<i64>.push`,
//! `Tree<i64>`), so `mir::build` and codegen see only concrete types, and each copy is an
//! ordinary function, class or enum to them. The enums are then laid out again on their
//! concrete payloads, since an instantiation can close a cycle that no declaration has.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::mem;
use std::rc::Rc;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::conformance::{
    Cause, Decide, Holds, Source, Stuck, StuckChoices, any_closure, ensure_stack, fields_of, find,
    holds, makes_group, normalize_noting, parts, prints_by_helper,
};
use crate::diagnostic::{Diagnostic, DiagnosticKind, UnknownForm};
use crate::lang::{self, Cx, Dispatch};
use crate::payload_limit;
use crate::source::Span;
use crate::tir::{
    Align, AnyLayout, AnyTable, Arm, Block, ChoicePlace, Class, ClassId, CompareOp, Conformance,
    Conformances, Enum, EnumId, Expr, ExprKind, Field, FuncId, Function, InterfaceId, Local, Place,
    Program, Stmt, Type, TypeArgs, TypeKind, head_of, layout_enums,
};
use crate::types::{Head, write_args, write_method, write_type};

#[cfg(test)]
thread_local! {
    /// How many programs on this thread have taken the path that instantiates nothing.
    static FAST_PATHS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many `(type, interface)` pairs the record walk has entered on this thread.
    pub(crate) static WALK_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many tables `Mono::table` has expanded on this thread, each of an object type for an
    /// `any` type, once.
    pub(crate) static TABLES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many interface entries `Mono::entries` has resolved on this thread, once for each
    /// object type and interface.
    pub(crate) static ENTRIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many interfaces `Mono::layout` has added to a closure, on this thread.
    pub(crate) static LAYOUT_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// An object type, an interface and its arguments: the key of a list of table entries.
type EntryKey = (Type, InterfaceId, TypeArgs);

/// `program` with each generic function, class and enum replaced by the instantiations it
/// reaches, and its enums laid out again.
/// The non-generic declarations come first, in their order; the instantiations follow, in the
/// order a worklist first reaches them: the non-generic classes' fields and enums' payloads, then
/// the user's non-generic functions' bodies, then each instantiation's in turn. A method is
/// reached only by a call, never with its class. A function of the prelude is reached the same
/// way, by a call from a body that is in the program, so a program holds only the prelude
/// functions it uses.
///
/// A program takes a fast path that instantiates nothing and drops the prelude's items when no
/// declaration is generic, the checker recorded no use of the prelude (`Program::instantiates`),
/// and no conformance has a method of the prelude for a type outside the prelude.
///
/// The checker has accepted the program, but its probe does not follow every associated type
/// that an instance meets (`probe_associated`): the diagnostics are the E0030 of each choice that
/// an instance needs itself, or needs more steps than the limit, and then there is no program.
pub(crate) fn instantiate(mut program: Program) -> Result<Program, Vec<Diagnostic>> {
    let prelude = program.prelude;
    let generic = program.functions[..prelude.funcs]
        .iter()
        .any(|f| !f.type_params.is_empty())
        || (program.classes[..prelude.classes].iter()).any(|c| !c.type_params.is_empty())
        || (program.enums[..prelude.enums].iter()).any(|e| !e.type_params.is_empty());
    if !generic && !program.instantiates && !prelude_method_of_outside_type(&program) {
        #[cfg(test)]
        FAST_PATHS.set(FAST_PATHS.get() + 1);
        program.fast_path = true;
        // Reaching a generic item records a type argument, and so does a use of any other item
        // of the prelude (see `tir::Program::instantiates`). An implicit call, such as `print`,
        // does not: codegen finds the method through the conformance. No conformance holds a
        // prelude method for a type outside the prelude (`prelude_method_of_outside_type`), so
        // the prelude's items are all templates or unused.
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
        return Ok(program);
    }
    let program = Mono::new(program).run()?;
    if cfg!(debug_assertions) {
        assert_concrete(&program);
    }
    Ok(program)
}

/// Whether a conformance has a method of the prelude for a target that is not an item of the
/// prelude. An implicit call of the interface (`print`, `==`) reaches such a method with no
/// written use, so the program must keep it. The cost is one step for each conformance method.
fn prelude_method_of_outside_type(program: &Program) -> bool {
    let prelude = program.prelude;
    program.conformances.iter().any(|(_, conformance)| {
        let in_prelude = match head_of(conformance.target) {
            Some(Head::Class(id)) => id.0 >= prelude.classes,
            Some(Head::Enum(id)) => id.0 >= prelude.enums,
            // `head_of` gives only a class or an enum, and `Conformances::add` admits only those
            // targets. `false` leaves the fast path.
            Some(Head::Interface(_) | Head::Assoc(..)) | None => false,
        };
        !in_prelude && conformance.methods.iter().any(|f| f.0 >= prelude.funcs)
    })
}

/// The most instances of generic items that `mono` makes for one program. An instance is one copy
/// of a generic function, class or enum for one list of type arguments. A chain of generic items
/// that each use the next one twice, with different arguments, doubles the instances at each step.
/// When the count passes the limit, `mono` stops and reports E0163 (`docs/diagnostics/E0163.md`).
const INSTANCE_LIMIT: usize = 100_000;

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
    /// The checked program's generic functions, and the prelude's functions, at their ids; a
    /// non-generic function of the user's has moved to `functions`.
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
    /// The output's type for each concrete checked type met so far, and the output enum that this
    /// type holds, through arrays and optionals, if any: the enum whose use a hit notes.
    types: FxHashMap<Type, (Type, Option<EnumId>)>,
    /// The output's functions, each `None` until the worklist instantiates it.
    functions: Vec<Option<Function>>,
    /// The output, without its functions until the end: its classes name types as they arrive.
    out: Program,
    /// The checked program's conformances, which resolve each interface call.
    conformances: Conformances,
    /// Where the choices that write an associated type or a type parameter are written
    /// (`Program::choice_ats`).
    choice_ats: FxHashMap<ChoicePlace, Span>,
    /// The loops and the chains that are too long that `subst` has met, with the first type found
    /// for each place. `subst` then gives a stand-in type, and no instance after the first is
    /// walked: `run` reports them.
    stuck: RefCell<StuckChoices>,
    /// The payload types of the checked program's enums, in its ids: `holds` reads them, as the
    /// output's enums name the output's ids.
    input_payloads: Vec<Vec<Type>>,
    /// What decides whether a written conformance holds for a type, in the checked program's ids.
    decide: Decide,
    /// The field types of the checked program's classes, in its ids. The record walk of a `dbg`
    /// reads them, as the output's classes name the output's ids.
    input_fields: Vec<Vec<Type>>,
    /// The `(type, interface, dbg)` triples the record walk has entered, in the checked program's
    /// ids.
    walked: FxHashSet<(Type, InterfaceId, bool)>,
    /// The locals of the function whose body the worklist is lowering; operators that make
    /// temporaries add to them.
    locals: Vec<Local>,
    /// The instances to lower, each with the `user_at` of the moment that it was reached.
    work: VecDeque<(Option<Span>, Work)>,
    /// The span of the expression that `expr` lowers, `None` outside one and inside the
    /// prelude's: the place that an instance is first reached from.
    at: Option<Span>,
    /// The interfaces of the table of each checked `any` type that `output` has met, with the
    /// checked arguments: see `tir::AnyLayout`.
    layouts: FxHashMap<Type, Rc<[(InterfaceId, TypeArgs)]>>,
    /// The checked types and `any` types whose table the output holds.
    tabled: FxHashSet<(Type, Type)>,
    /// The entries of each interface for each object type, resolved once and shared by every
    /// table that holds them.
    entries: FxHashMap<EntryKey, Rc<[Option<FuncId>]>>,
    /// The first use of each output enum, in its ids: where an instance first comes from an
    /// expression. The enums of `out.enums` and this table have the same length. An instance
    /// that only a local's type or a class field reaches has none until an expression meets
    /// its type. `payload_limit` reads it for the error of an instance that is too large.
    uses: Vec<Option<Span>>,
    /// The span of the last item or expression of the user's source that `mono` lowered before
    /// `failed` held: the place that E0163 points at when an item of the prelude has the most
    /// instances.
    user_at: Option<Span>,
    /// How many instances `class`, `enum_id` and `function_id` have made. The non-generic items
    /// that `new` copies do not count.
    instances: usize,
    /// The count of `instances` that `run` accepts.
    limit: usize,
}

impl Mono {
    fn new(program: Program) -> Mono {
        Mono::with_limit(program, INSTANCE_LIMIT)
    }

    /// `new`, with `run` stopping when the instances pass `limit`.
    fn with_limit(program: Program, limit: usize) -> Mono {
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
                lang: program.lang,
                ..Program::default()
            },
            conformances: program.conformances,
            choice_ats: program.choice_ats,
            stuck: RefCell::default(),
            input_payloads: program
                .enums
                .iter()
                .map(|e| (e.variants.iter().flat_map(|v| &v.fields).map(|p| p.ty)).collect())
                .collect(),
            input_fields: program
                .classes
                .iter()
                .map(|c| c.fields.iter().map(|f| f.ty).collect())
                .collect(),
            decide: Decide::default(),
            walked: FxHashSet::default(),
            locals: Vec::new(),
            work: VecDeque::new(),
            at: None,
            layouts: FxHashMap::default(),
            tabled: FxHashSet::default(),
            entries: FxHashMap::default(),
            user_at: None,
            uses: Vec::new(),
            instances: 0,
            limit,
        };
        let user_funcs = program.prelude.funcs;
        for (i, f) in program.functions.into_iter().enumerate() {
            // A function of the prelude joins the program when a body that is in it calls it.
            if f.type_params.is_empty() && i < user_funcs {
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
                mono.uses.push(None);
                mono.enum_decls.push(None);
            } else {
                mono.enum_decls.push(Some(e));
            }
        }
        mono
    }

    fn run(mut self) -> Result<Program, Vec<Diagnostic>> {
        self.expand();
        if self.failed() {
            let found = self.stuck_diagnostics();
            if !found.is_empty() {
                return Err(found);
            }
            return Err(vec![self.too_many_instances()]);
        }
        layout_enums(&mut self.out.enums);
        debug_assert!(
            payload_limit::measure(&self.out.enums)
                .iter()
                .all(Option::is_some),
            "every enum should have a size bound after the layout: its types are final and the layout boxes each cycle"
        );
        let oversized = payload_limit::diagnostics(&self.out.enums, &self.uses);
        if !oversized.is_empty() {
            return Err(oversized);
        }
        self.out.functions = self
            .functions
            .into_iter()
            .map(|f| f.expect("the worklist should instantiate every function it reaches"))
            .collect();
        Ok(self.out)
    }

    /// Lowers the non-generic items, then each instance on the worklist, until the worklist is
    /// empty or `failed` holds.
    fn expand(&mut self) {
        for i in 0..self.out.classes.len() {
            self.note_item(self.out.classes[i].span);
            let mut fields = mem::take(&mut self.out.classes[i].fields);
            for field in &mut fields {
                field.ty = self.output(field.ty);
            }
            self.out.classes[i].fields = fields;
        }
        for i in 0..self.out.enums.len() {
            self.note_item(self.out.enums[i].span);
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
            self.note_item(f.span);
            if f.method.is_some() {
                f.name = self.instance_name(&f, &[], i);
            }
            self.function(&mut f, &[]);
            self.functions[i] = Some(f);
        }
        while let Some((from, work)) = self.work.pop_front() {
            if self.failed() {
                break;
            }
            // An instance of the prelude's item has no span of the user's: it keeps the place that
            // reached it.
            self.user_at = from;
            match work {
                Work::Function {
                    id,
                    decl,
                    args,
                    name,
                } => {
                    let mut f = self.decls[decl.0]
                        .clone()
                        .expect("an instantiated function should be generic or the prelude's");
                    self.note_item(f.span);
                    f.name = name;
                    f.type_params = Vec::new();
                    f.owner_params = 0;
                    self.function(&mut f, args.get());
                    self.functions[id.0] = Some(f);
                }
                Work::Class { id, decl, args } => {
                    self.note_item(self.out.classes[id.0].span);
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
                    self.note_item(self.out.enums[id.0].span);
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
    }

    /// Notes `span`, the name of the item that `mono` starts to lower, as the place of the user's
    /// source that `mono` is at, unless the item is the prelude's. After `failed` holds, the place
    /// stays where the failure came from.
    fn note_item(&mut self, span: Span) {
        if !span.in_prelude() && !self.failed() {
            self.user_at = Some(span);
        }
    }

    /// Whether `subst` has met a type that it cannot normalize, or the instances have passed the
    /// limit.
    fn failed(&self) -> bool {
        !self.stuck.borrow().is_empty() || self.instances > self.limit
    }

    /// The generic item with the most instances, as its name, the span of its name and its count
    /// of instances. The items of the prelude count too: an alias of the user can make all the
    /// instances of the prelude's `Result`. One walk over the maps of instances.
    fn busiest(&self) -> (String, Span, usize) {
        let mut classes = vec![0; self.class_decls.len()];
        for &(decl, _) in self.class_ids.keys() {
            classes[decl.0] += 1;
        }
        let mut enums = vec![0; self.enum_decls.len()];
        for &(decl, _) in self.enum_ids.keys() {
            enums[decl.0] += 1;
        }
        let mut functions = vec![0; self.decls.len()];
        for &(decl, _) in self.function_ids.keys() {
            functions[decl.0] += 1;
        }
        let classes = (self.class_decls.iter().zip(classes))
            .filter_map(|(decl, n)| decl.as_ref().map(|d| (d.name.as_str(), d.span, n)));
        let enums = (self.enum_decls.iter().zip(enums))
            .filter_map(|(decl, n)| decl.as_ref().map(|d| (d.name.as_str(), d.span, n)));
        let functions = (self.decls.iter().zip(functions))
            .filter_map(|(decl, n)| decl.as_ref().map(|d| (d.name.as_str(), d.span, n)));
        let (name, at, count) = classes
            .chain(enums)
            .chain(functions)
            .filter(|&(_, _, n)| n > 0)
            .max_by_key(|&(_, _, n)| n)
            .expect("a program over the instance limit should have an item with an instance");
        (name.to_string(), at, count)
    }

    /// The E0163 for the instances that `busiest` finds. It is at the name of that item when the
    /// user declares it, and else at the user's source that `mono` last lowered.
    fn too_many_instances(&self) -> Diagnostic {
        let (name, decl, count) = self.busiest();
        // `user_at` is set before the first instance: the prelude's own items make none (a test
        // checks it), and every other instance comes from an item or an expression of the user's.
        let at = if decl.in_prelude() {
            self.user_at
                .expect("an instance should come from the user's source")
        } else {
            decl
        };
        DiagnosticKind::TooManyInstances {
            name,
            at,
            count,
            limit: self.limit,
        }
        .into()
    }

    /// The E0030 of each choice that `subst` met, ordered by place in the source, each at the
    /// type that the choice writes: the span of the choice is the best place `mono` has, and the
    /// same one the checker uses. A call may need the choice in many instances, so a use is no
    /// better place.
    fn stuck_diagnostics(&self) -> Vec<Diagnostic> {
        let head = |head| match head {
            Head::Assoc(interface, k) => {
                self.out.interfaces[interface.0].assoc[k as usize].as_str()
            }
            other => self.head(other),
        };
        let mut found: Vec<_> = (self.stuck.borrow().iter())
            .map(|(place, &(assoc, cause))| {
                let mut name = String::new();
                write_type(&mut name, assoc, head);
                let form = match cause {
                    Cause::Loops => UnknownForm::ChoiceLoops,
                    Cause::TooDeep => UnknownForm::ChoiceTooDeep,
                };
                let at = self.choice_ats[place];
                DiagnosticKind::UnknownType { name, at, form }.into()
            })
            .collect();
        found.sort_by_key(|d: &Diagnostic| d.span().start());
        found
    }

    /// `ty` of a body instantiated with `args`, as the output names it.
    fn ty(&mut self, ty: Type, args: &[Type]) -> Type {
        let ty = self.subst(ty, args);
        self.output(ty)
    }

    /// `ty` with `args` for its type parameters and each associated type replaced by the type its
    /// conformance chose. The checker has accepted the program, and checked the bound that gives
    /// each base its conformance.
    ///
    /// A loop, or a chain of more steps than the limit, that the checker's probe did not follow
    /// is noted in `stuck`, and the type is the stand-in `i64`.
    fn subst(&self, ty: Type, args: &[Type]) -> Type {
        let mut stuck = Vec::new();
        let done = normalize_noting(&self.conformances, ty.subst(args), &mut stuck);
        done.unwrap_or_else(|| {
            assert!(
                !stuck.is_empty(),
                "a call that checks should find the conformance of every associated type"
            );
            let mut known = self.stuck.borrow_mut();
            for Stuck {
                assoc,
                interface,
                index,
                k,
                cause,
            } in stuck
            {
                known.entry((interface, index, k)).or_insert((assoc, cause));
            }
            Type::I64
        })
    }

    /// `subst` for a list of arguments.
    fn subst_args(&self, list: TypeArgs, args: &[Type]) -> TypeArgs {
        let list: Vec<Type> = list.get().iter().map(|&ty| self.subst(ty, args)).collect();
        TypeArgs::intern(&list)
    }

    /// The output's type for the concrete checked type `ty`: each class and enum by its output
    /// id, with no arguments. Reaching a class's instantiation puts it on the worklist.
    fn output(&mut self, ty: Type) -> Type {
        if let Some(&(done, held)) = self.types.get(&ty) {
            if let Some(id) = held {
                self.note_use(id);
            }
            return done;
        }
        // An optional or an array of an instance has the instance in its type. The entry of the
        // inner type, which `output` has just made, says which: no walk down the type.
        let (done, held) = ensure_stack(|| match ty.kind() {
            TypeKind::Class(id, args) => (
                Type::intern(TypeKind::Class(self.class(id, args), TypeArgs::NONE)),
                None,
            ),
            TypeKind::Enum(id, args) => {
                let id = self.enum_id(id, args);
                (Type::intern(TypeKind::Enum(id, TypeArgs::NONE)), Some(id))
            }
            TypeKind::Any(interface, args) => {
                // Meeting an `any` type is how its layout reaches the output.
                self.layout(ty);
                let args: Vec<Type> = args.get().iter().map(|&arg| self.output(arg)).collect();
                (
                    Type::intern(TypeKind::Any(interface, TypeArgs::intern(&args))),
                    None,
                )
            }
            TypeKind::Array(element) => {
                let inner = self.output(element);
                (Type::array(inner), self.held_by(element))
            }
            TypeKind::Optional(payload) => {
                let inner = self.output(payload);
                (Type::optional(inner), self.held_by(payload))
            }
            TypeKind::Param(..) => {
                panic!("`subst` should replace every type parameter before `mono` maps a type")
            }
            TypeKind::I64
            | TypeKind::U8
            | TypeKind::F64
            | TypeKind::Bool
            | TypeKind::Char
            | TypeKind::String
            | TypeKind::Unit
            | TypeKind::Never
            | TypeKind::Assoc(..) => (ty, None),
        });
        self.types.insert(ty, (done, held));
        done
    }

    /// The interfaces of the table of the checked `any` type `any`: see `tir::AnyLayout`. The
    /// first time, the output gets the layout, with the arguments as the output names them.
    fn layout(&mut self, any: Type) -> Rc<[(InterfaceId, TypeArgs)]> {
        if let Some(layout) = self.layouts.get(&any) {
            return Rc::clone(layout);
        }
        let interfaces = &self.out.interfaces;
        let mut closure = any_closure(
            |inherited: InterfaceId| interfaces[inherited.0].inherits.iter().copied(),
            any,
            || {
                #[cfg(test)]
                LAYOUT_STEPS.set(LAYOUT_STEPS.get() + 1);
            },
        );
        let display = self.out.lang.display;
        if !closure.iter().any(|&(interface, _)| interface == display) {
            closure.push((display, TypeArgs::NONE));
        }
        let closure: Rc<[(InterfaceId, TypeArgs)]> = closure.into();
        self.layouts.insert(any, Rc::clone(&closure));
        let shown = closure
            .iter()
            .map(|&(interface, args)| {
                if args.get().is_empty() {
                    return (interface, args);
                }
                let args: Vec<Type> = args.get().iter().map(|&arg| self.output(arg)).collect();
                (interface, TypeArgs::intern(&args))
            })
            .collect();
        let any = self.output(any);
        self.out.any_layouts.push(AnyLayout {
            any,
            interfaces: shown,
        });
        closure
    }

    /// Reaches the table of the checked type `source`, a class or another value, for the checked
    /// `any` type `target`, and of each interface in its layout: the methods that meet their
    /// requirements are instantiated, and so are the written `to_string` that each show helper may
    /// call.
    fn table(&mut self, source: Type, target: Type) {
        if !self.tabled.insert((source, target)) {
            return;
        }
        #[cfg(test)]
        TABLES.set(TABLES.get() + 1);
        let layout = self.layout(target);
        let any_object = self.out.lang.any_object;
        let mut tables = Vec::new();
        for &(interface, args) in layout.iter() {
            let inner = Type::intern(TypeKind::Any(interface, args));
            if inner != target {
                self.table(source, inner);
            }
            tables.push(if interface == any_object {
                Vec::new()
            } else {
                self.entries(source, interface, args).to_vec()
            });
        }
        let (ty, any) = (self.output(source), self.output(target));
        self.out.any_tables.push(AnyTable {
            ty,
            any,
            methods: tables,
        });
    }

    /// The entries of the interface `interface` with `args` in the table of the object type
    /// `source`: the instance of each method that meets a requirement, or for `Display` the
    /// written `to_string`, if there is one. A table of a descendant holds the entries of each
    /// interface it inherits, so each is resolved once and shared.
    fn entries(
        &mut self,
        source: Type,
        interface: InterfaceId,
        args: TypeArgs,
    ) -> Rc<[Option<FuncId>]> {
        if let Some(entries) = self.entries.get(&(source, interface, args)) {
            return Rc::clone(entries);
        }
        #[cfg(test)]
        ENTRIES.set(ENTRIES.get() + 1);
        let display = self.out.lang.display;
        let entries: Rc<[Option<FuncId>]> = if interface == display {
            // The helpers show the value in both modes, through the written `to_string`.
            self.walk(source, display, false);
            self.walk(source, display, true);
            match self.holds(source, display) {
                Holds::Written(k, stands) => {
                    let method = self.conformances.get(display, k).methods[0];
                    Rc::from([Some(self.function_id(method, stands))])
                }
                Holds::Generated | Holds::No => Rc::from([None]),
            }
        } else {
            let (k, stands) = find(&self.conformances, interface, source, args)
                .expect("the checker should have found the conformance of a converted type");
            let methods = self.conformances.get(interface, k).methods.clone();
            (methods.into_iter())
                .map(|method| Some(self.function_id(method, stands)))
                .collect()
        };
        self.entries
            .insert((source, interface, args), Rc::clone(&entries));
        entries
    }

    /// The output enum that the type `ty` holds, through arrays and optionals, for a type that
    /// `output` has made.
    fn held_by(&self, ty: Type) -> Option<EnumId> {
        self.types.get(&ty).and_then(|&(_, held)| held)
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
            Head::Assoc(..) => unreachable!("mono should replace every associated type"),
            Head::Interface(id) => &self.out.interfaces[id.0].name,
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
            span: decl.span,
            type_params: Vec::new(),
            fields: Vec::new(),
        };
        self.out.classes.push(class);
        self.instances += 1;
        self.class_ids.insert((id, args), done);
        self.work.push_back((
            self.user_at,
            Work::Class {
                id: done,
                decl: id,
                args,
            },
        ));
        done
    }

    /// Records the expression that `expr` lowers as the first use of the output enum `id`.
    fn note_use(&mut self, id: EnumId) {
        let first = &mut self.uses[id.0];
        if first.is_none() {
            *first = self.at;
        }
    }

    /// The output id of enum `id` with `args`, given and put on the worklist when first reached.
    fn enum_id(&mut self, id: EnumId, args: TypeArgs) -> EnumId {
        if let Some(&done) = self.enum_ids.get(&(id, args)) {
            self.note_use(done);
            return done;
        }
        self.reach(args.get());
        let done = EnumId(self.out.enums.len());
        let decl = self.enum_decls[id.0]
            .as_ref()
            .expect("an enum reached with arguments should be generic");
        let e = Enum {
            name: self.type_instance_name(&decl.name, args.get(), done.0),
            span: decl.span,
            instance: true,
            type_params: Vec::new(),
            variants: Vec::new(),
            counted: false,
            cycle: None,
            reaches: false,
        };
        self.out.enums.push(e);
        self.uses.push(self.at);
        self.instances += 1;
        self.enum_ids.insert((id, args), done);
        self.work.push_back((
            self.user_at,
            Work::Enum {
                id: done,
                decl: id,
                args,
            },
        ));
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
            .expect("a function reached with arguments should be generic or the prelude's");
        let name = self.instance_name(decl, args.get(), done.0);
        self.functions.push(None);
        self.instances += 1;
        self.function_ids.insert((id, args), done);
        self.work.push_back((
            self.user_at,
            Work::Function {
                id: done,
                decl: id,
                args,
                name,
            },
        ));
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
        self.locals = mem::take(&mut f.locals);
        self.block(&mut f.body, args);
        f.locals = mem::take(&mut self.locals);
    }

    fn block(&mut self, block: &mut Block, args: &[Type]) {
        block.ty = self.ty(block.ty, args);
        for stmt in &mut block.stmts {
            match stmt {
                Stmt::Let { init: e, .. } | Stmt::Expr(e) => self.expr(e, args),
                Stmt::Assign { place, value }
                | Stmt::Append { place, value }
                | Stmt::Push { place, value, .. } => {
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
            Place::Some(base) => self.place(base, args),
        }
    }

    /// `expr_kind`, with `at` at `e`: an instance that the types of `e` reach first has `e` for
    /// its first use. Each early return of `expr_kind` leaves `at` as `expr` found it.
    fn expr(&mut self, e: &mut Expr, args: &[Type]) {
        let outer = self.at;
        if !e.span.in_prelude() {
            self.at = Some(e.span);
            if !self.failed() {
                self.user_at = self.at;
            }
        }
        self.expr_kind(e, args);
        self.at = outer;
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per node kind, so the walk reads in one place"
    )]
    fn expr_kind(&mut self, e: &mut Expr, args: &[Type]) {
        use ExprKind as K;
        // A stand-in type may not fit its node, so no node is walked after the first failure.
        if self.failed() {
            return;
        }
        // An operator is lowered again before its types change to the output's ids: its operand
        // type decides, in the checked program's ids, whether a written conformance runs.
        match &mut e.kind {
            K::Equal { negated, lhs, rhs } => {
                let (negated, ty, eq) = (*negated, self.subst(lhs.ty, args), self.out.lang.eq);
                if let Some(dispatch) = self.dispatch(ty, eq) {
                    let (lhs, rhs) = (Box::new(hole(lhs)), Box::new(hole(rhs)));
                    e.kind = lang::equal(dispatch, negated, lhs, rhs, e.span);
                } else if !matches!(lhs.kind, K::None) && !matches!(rhs.kind, K::None) {
                    // `x == None` tests presence, and runs no `eq`.
                    self.walk(ty, eq, false);
                }
            }
            K::Compare { operands, ops } => {
                let (operands, ops) = (mem::take(operands), mem::take(ops));
                e.kind = self.compare(operands, &ops, e.span, args);
            }
            K::ToAny(operand) => {
                // The table of an object or a boxed value reaches the methods that meet the
                // interface.
                let (source, target) = (self.subst(operand.ty, args), self.subst(e.ty, args));
                if source == target {
                    // A type parameter that stands for the `any` type is already one.
                    let inner = hole(operand);
                    *e = inner;
                    return self.expr_kind(e, args);
                }
                if !matches!(source.kind(), TypeKind::Any(..)) {
                    self.table(source, target);
                }
            }
            K::Print { operand, .. } => {
                // The helper for a compound operand calls a written `to_string` at each part
                // that has one.
                let ty = self.subst(operand.ty, args);
                if prints_by_helper(ty) {
                    self.walk(ty, self.out.lang.display, false);
                }
            }
            K::Dbg { operand, .. } => {
                // The `dbg` helper also writes the fields of a class with no written `to_string`,
                // each by the `print` helper.
                let ty = self.subst(operand.ty, args);
                if prints_by_helper(ty) {
                    self.walk(ty, self.out.lang.display, true);
                }
            }
            K::Interpolate { parts, formats } => {
                for format in &mut *formats {
                    // A part of a type parameter's type pads as its type argument's does.
                    if let Some(pad) = format.pad.as_mut().filter(|pad| pad.by_type) {
                        let ty = self.subst(parts[format.part].ty, args);
                        pad.align = if matches!(ty, Type::I64 | Type::U8 | Type::F64) {
                            Align::Right
                        } else {
                            Align::Left
                        };
                        pad.by_type = false;
                    }
                    // A part of a type parameter's type is laid out only if its type argument
                    // makes a group.
                    if format.pretty {
                        format.pretty = makes_group(self.subst(parts[format.part].ty, args));
                    }
                }
                formats.retain(|format| !format.is_plain());
                // The helper for a compound part calls a written `to_string` at each part that
                // has one.
                for part in parts.iter() {
                    let ty = self.subst(part.ty, args);
                    if prints_by_helper(ty) {
                        self.walk(ty, self.out.lang.display, false);
                    }
                }
            }
            _ => {}
        }
        e.ty = self.ty(e.ty, args);
        if self.failed() {
            return;
        }
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
                if self.failed() {
                    return;
                }
                // An `any` receiver calls through its table.
                if matches!(receiver.kind(), TypeKind::Any(..)) {
                    let checked = self.subst_args(*iargs, args);
                    let called: Vec<Type> =
                        checked.get().iter().map(|&ty| self.output(ty)).collect();
                    let mut operands = mem::take(operands);
                    self.exprs(&mut operands, args);
                    e.kind = K::DynCall {
                        interface: *interface,
                        iargs: TypeArgs::intern(&called),
                        method: *method,
                        args: operands,
                    };
                    return;
                }
                // A `Display` that the compiler writes: also for a type whose written record fails
                // its bounds, which `find` would match.
                if *interface == self.out.lang.display
                    && self.dispatch(receiver, *interface).is_none()
                {
                    let [operand]: [Expr; 1] = mem::take(operands)
                        .try_into()
                        .expect("a call of `to_string` should have one operand");
                    e.kind = lang::to_string(operand);
                    return self.expr(e, args);
                }
                let Some((index, bindings)) = find(&self.conformances, *interface, receiver, rest)
                else {
                    // No written conformance covers the receiver: the checker proved that the
                    // compiler writes it, and the operator itself is the call.
                    let lang = self.out.lang;
                    let mut operands = mem::take(operands).into_iter();
                    let (Some(lhs), Some(rhs)) = (operands.next(), operands.next()) else {
                        panic!("a call of `eq` or `less` should have two operands");
                    };
                    e.kind = if *interface == lang.eq {
                        lang::equal(
                            Dispatch::Generated,
                            false,
                            Box::new(lhs),
                            Box::new(rhs),
                            e.span,
                        )
                    } else if *interface == lang.ord {
                        K::Compare {
                            operands: vec![lhs, rhs],
                            ops: vec![CompareOp::Lt],
                        }
                    } else {
                        panic!("only `eq` and `less` are lowered from a generated conformance")
                    };
                    return self.expr(e, args);
                };
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
            K::DynCall { .. } => unreachable!("the checker should make no call through a table"),
            K::Field { base: operand, .. }
            | K::ToAny(operand)
            | K::Wrap(operand)
            | K::Convert(operand)
            | K::Print { operand, .. }
            | K::Dbg { operand, .. }
            | K::Panic(operand)
            | K::Len(operand)
            | K::Chars(operand)
            | K::Bytes(operand)
            | K::Words(operand)
            | K::FromChars(operand)
            | K::Unary { operand, .. }
            | K::Break {
                value: Some(operand),
                ..
            }
            | K::Return(Some(operand)) => self.expr(operand, args),
            K::ArrayFill { value: a, len: b }
            | K::Index { base: a, index: b }
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

    /// `holds` over the checked program's ids.
    fn holds(&mut self, ty: Type, interface: InterfaceId) -> Holds {
        let src = Source {
            conformances: &self.conformances,
            enums: self.input_payloads.as_slice(),
            lang: self.out.lang,
            interfaces: self.out.interfaces.as_slice(),
        };
        holds(&mut self.decide, &src, ty, interface, TypeArgs::NONE)
    }

    /// The method that a written conformance of `interface` runs for `ty`, as the call that the
    /// operator makes of it.
    fn dispatch(&mut self, ty: Type, interface: InterfaceId) -> Option<Dispatch> {
        if !matches!(ty.kind(), TypeKind::Class(..) | TypeKind::Enum(..)) {
            return None;
        }
        match self.holds(ty, interface) {
            Holds::Written(k, stands) => Some(Dispatch::Method {
                func: self.conformances.get(interface, k).methods[0],
                types: stands,
            }),
            Holds::Generated | Holds::No => None,
        }
    }

    /// A comparison on a type that `mono` knows: a class or an enum with a written `Ord` is a call
    /// of its `less` for each link, and a chain of `string`s binds each operand once. A number
    /// keeps its node.
    fn compare(
        &mut self,
        operands: Vec<Expr>,
        ops: &[CompareOp],
        span: Span,
        args: &[Type],
    ) -> ExprKind {
        let ty = self.subst(operands[0].ty, args);
        let dispatch = (self.dispatch(ty, self.out.lang.ord)).unwrap_or(Dispatch::Generated);
        let before = self.locals.len();
        let mut cx = Cx {
            locals: &mut self.locals,
        };
        let kind = lang::ordered(&mut cx, dispatch, ty, operands, ops, span);
        // The locals that the lowering made have the checked program's types.
        for i in before..self.locals.len() {
            let local_ty = self.locals[i].ty;
            self.locals[i].ty = self.ty(local_ty, args);
        }
        kind
    }

    /// Reaches the written conformances of `interface` that the helper for an operand of the
    /// checked type `ty` may call: the output gets one record for each type in `ty`, through its
    /// parts, that a written conformance holds for, and the methods are instantiated. With `dbg`,
    /// the helper is the one that `dbg` calls. A class with no written conformance also has its
    /// field types walked, each as `print` writes it.
    fn walk(&mut self, ty: Type, interface: InterfaceId, dbg: bool) {
        let mut stack = vec![(ty, dbg)];
        while let Some((ty, dbg)) = stack.pop() {
            if !self.walked.insert((ty, interface, dbg)) {
                continue;
            }
            #[cfg(test)]
            WALK_VISITS.set(WALK_VISITS.get() + 1);
            if matches!(ty.kind(), TypeKind::Class(..) | TypeKind::Enum(..))
                && let Holds::Written(k, stands) = self.holds(ty, interface)
            {
                self.record(ty, interface, k, stands);
                continue;
            }
            if dbg && matches!(ty.kind(), TypeKind::Class(..)) {
                fields_of(
                    &self.conformances,
                    self.input_fields.as_slice(),
                    ty,
                    |part| stack.push((part, false)),
                );
                continue;
            }
            parts(
                &self.conformances,
                self.input_payloads.as_slice(),
                ty,
                |part| {
                    stack.push((part, dbg));
                },
            );
        }
    }

    /// Adds to the output the record of the written conformance `k` of `interface` for the
    /// checked type `ty`, in the output's ids, with its methods instantiated.
    fn record(&mut self, ty: Type, interface: InterfaceId, k: usize, stands: TypeArgs) {
        let methods = self.conformances.get(interface, k).methods.clone();
        let methods = methods
            .into_iter()
            .map(|method| self.function_id(method, stands))
            .collect();
        let target = self.output(ty);
        self.out.conformances.add(
            interface,
            Conformance {
                type_params: 0,
                target,
                args: TypeArgs::NONE,
                methods,
                assoc: Vec::new(),
                bounds: Vec::new(),
            },
        );
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

/// The expression `expr` holds, which moves out; a placeholder stays, which `mono` overwrites.
fn hole(expr: &mut Expr) -> Expr {
    let span = expr.span;
    mem::replace(
        expr,
        Expr {
            kind: ExprKind::None,
            ty: Type::UNIT,
            span,
        },
    )
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
        TypeKind::Any(_, args) => args.get().iter().all(|&arg| concrete(arg)),
        TypeKind::Array(inner) | TypeKind::Optional(inner) => concrete(inner),
        TypeKind::I64
        | TypeKind::U8
        | TypeKind::F64
        | TypeKind::Bool
        | TypeKind::Char
        | TypeKind::String
        | TypeKind::Unit
        | TypeKind::Never => true,
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use rustc_hash::FxHashSet;

    use super::*;
    use crate::check::check_alone;
    use crate::parser::parse_ok;
    use crate::tir::InterfaceId;
    use crate::types::NAME_LIMIT;

    /// The program of the wrapper chain: `a{k}` is `a{k-1}` wrapped in one more array, so its type
    /// is `k` arrays deep and the `n` types of the chain are all distinct.
    fn wrapper_chain(n: usize) -> String {
        let mut src =
            String::from("fn wrap<T>(t: T) -> [T] {\n    [t]\n}\nfn main() {\n    let a0 = 1\n");
        for k in 1..=n {
            writeln!(src, "    let a{k} = wrap(a{})", k - 1)
                .expect("writing to a `String` should succeed");
        }
        src + "}\n"
    }

    /// `mono` reads each type of the wrapper chain a bounded number of times, whatever its depth:
    /// a type `k` arrays deep is met again, and a walk down it on each meeting costs `k`. The
    /// count is `types::READS`, each read of a kind, so a walk anywhere in `mono` shows in it. It
    /// starts at `Mono::run`, which the debug check of the output (`assert_concrete`) does not
    /// enter. The name of an instance is cut at `NAME_LIMIT`, so the reads for each wrapper grow
    /// with its depth only up to that limit. The sizes are 400, 800 and 1200 wrappers, past it.
    /// Names stop growing at about 126 wrappers (`NAME_LIMIT` is 256, and each level takes two
    /// bytes), and 400 is about three times that.
    #[test]
    fn mono_should_read_each_type_of_a_wrapper_chain_a_bounded_number_of_times() {
        use crate::types::READS;

        let reads = |n| {
            let src = wrapper_chain(n);
            let (program, diagnostics) = check_alone(&parse_ok(&src), &src, &[]);
            let program = program.unwrap_or_else(|| panic!("{diagnostics:?}"));
            READS.set(0);
            Mono::new(program)
                .run()
                .unwrap_or_else(|found| panic!("{found:?}"));
            READS.get()
        };
        let (a, b, c) = (reads(400), reads(800), reads(1200));
        crate::mir::tests::assert_near_linear(a, b, c);
    }

    /// A chain of interfaces `I1` to `In`, each of which inherits the two before it, a class `C`
    /// that conforms to the last, and a `main` that converts a `C` to an `any In` and calls its
    /// last requirement: `2^n` paths lead from `In` to `I1`, and `n` interfaces are in the
    /// chain.
    fn diamond(n: usize) -> String {
        let mut src = String::from("interface I1 {\n    fn f1(self)\n}\n");
        src.push_str("interface I2: I1 {\n    fn f2(self)\n}\n");
        for k in 3..=n {
            let _ = writeln!(
                src,
                "interface I{k}: I{}, I{} {{\n    fn f{k}(self)\n}}",
                k - 1,
                k - 2
            );
        }
        let _ = writeln!(src, "class C: I{n} {{");
        for k in 1..=n {
            let _ = writeln!(src, "    fn f{k}(self) {{}}");
        }
        let _ = write!(
            src,
            "}}\nfn main() {{\n    let x: any I{n} = C()\n    x.f{n}()\n}}"
        );
        src
    }

    /// A conversion to `any I` makes a table for `I`, one for each interface that it inherits, and
    /// one for `Display`, each once: a chain of thirty interfaces, each inheriting the two before
    /// it, makes thirty-one tables, not a table for each of its `2^30` paths.
    #[test]
    fn a_conversion_should_make_one_table_for_each_inherited_interface() {
        TABLES.set(0);
        let program = instantiated(&diamond(30));
        assert_eq!(TABLES.get(), 31);
        assert_eq!(program.any_tables.len(), 31);
        // The layout of `any I30` holds each interface once, and `Display` after them.
        let widest = program.any_layouts.iter().map(|l| l.interfaces.len()).max();
        assert_eq!(widest, Some(31));
    }

    /// A chain of `n` interfaces: `I0` has a method, each `Ik` inherits `I(k-1)` and has none, and
    /// a class conforms to the last one, which a conversion and a call use.
    fn chain(n: usize) -> String {
        let mut src = String::from("interface I0 {\n    fn f(self)\n}\n");
        for k in 1..n {
            let _ = writeln!(src, "interface I{k}: I{} {{}}", k - 1);
        }
        let _ = write!(
            src,
            "class C: I{} {{\n    fn f(self) {{}}\n}}\nfn main() {{\n    let x: any I{} = C()\n    x.f()\n}}",
            n - 1,
            n - 1
        );
        src
    }

    /// The work of the tables for a chain of `n` interfaces: the entries resolved, the interfaces
    /// added to a layout, and the slots that the output holds, which are the entries and upcast
    /// pointers of every table.
    fn table_work(n: usize) -> (usize, usize, usize) {
        ENTRIES.set(0);
        LAYOUT_STEPS.set(0);
        let program = instantiated(&chain(n));
        let slots = program
            .any_layouts
            .iter()
            .map(|l| l.interfaces.len())
            .sum::<usize>()
            + (program.any_tables.iter())
                .map(|table| table.methods.iter().map(Vec::len).sum::<usize>())
                .sum::<usize>();
        (ENTRIES.get(), LAYOUT_STEPS.get(), slots)
    }

    /// A table holds the slots of every interface that it inherits, so a chain of `n` interfaces
    /// has `n` squared slots in all. The work stays linear in them: each interface's entries are
    /// resolved once for the class, and each layout is a walk of its own closure, no more.
    #[test]
    fn the_tables_of_a_chain_should_cost_in_proportion_to_their_slots() {
        let [(e1, l1, s1), (e2, l2, s2), (e3, l3, s3)] = [16, 32, 48].map(table_work);
        // One entry list for each interface and `Display`, however many tables hold it.
        assert_eq!([e1, e2, e3], [17, 33, 49]);
        // The layouts add `k` interfaces each, for each `k` up to `n`, and one for `Display`.
        assert_eq!([l1, l2, l3], [137, 529, 1177]);
        // The output is the slots, and the layouts are part of them.
        for (layout, slots) in [(l1, s1), (l2, s2), (l3, s3)] {
            assert!(layout <= slots, "{layout} layout steps for {slots} slots");
        }
        // The slots grow with the square of the chain; the work, with the slots.
        assert!(s2 < 5 * s1 && s3 < 9 * s1, "{s1}, {s2}, {s3} slots");
    }

    /// The methods of the table of a converted class are instantiated, though nothing calls them.
    #[test]
    fn a_conversion_should_reach_the_methods_of_the_table() {
        let program = instantiated(
            "interface Run {\n    fn run(self)\n    fn stop(self)\n}\nclass C: Run {\n    fn run(self) {}\n    fn stop(self) {}\n}\nfn main() {\n    let r: any Run = C()\n    r.run()\n}",
        );
        let names = function_names(&program);
        assert!(
            names.contains(&"C.run") && names.contains(&"C.stop"),
            "{names:?}"
        );
        // The table for `any Run` holds both methods, then the `to_string` that the compiler
        // writes; the table for `any Display` holds that one alone.
        let lengths: Vec<Vec<usize>> = (program.any_tables.iter())
            .map(|table| table.methods.iter().map(Vec::len).collect())
            .collect();
        assert!(
            lengths.contains(&vec![2, 1]) && lengths.contains(&vec![1]),
            "{lengths:?}"
        );
    }

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

    /// A `#` part of a type parameter keeps its row at a type argument that may make a group and
    /// loses it at one that writes one piece, so the instance has the `tir` of a part without `#`.
    #[test]
    fn a_hash_part_of_a_type_parameter_should_keep_its_row_only_in_an_instance_with_a_group() {
        let program = instantiated(
            "fn g<T>(x: T) -> string {\n    \"{x:#}\"\n}\nfn main() {\n    \
             print(g(1) + g(\"s\") + g([1]))\n}",
        );
        let text = program.to_string();
        for (instance, row) in [("g<i64>", false), ("g<string>", false), ("g<[i64]>", true)] {
            let rest = text
                .split_once(&format!("(fn {instance} "))
                .unwrap_or_else(|| panic!("{instance} in {text}"))
                .1;
            let function = rest.split("(fn ").next().unwrap_or(rest);
            assert_eq!(function.contains("(fmt x#0 \"#\")"), row, "{function}");
            assert_eq!(function.contains("(fmt"), row, "{function}");
        }
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

    /// The printed function `name` of the program `instantiated` gives for `src`.
    fn printed_function(src: &str, name: &str) -> String {
        let printed = instantiated(src).to_string();
        let head = format!("(fn {name} ");
        printed
            .lines()
            .find(|line| line.starts_with(&head))
            .unwrap_or_else(|| panic!("no `{name}` in\n{printed}"))
            .to_string()
    }

    /// An operator in a generic body is lowered again for each type argument: a number keeps its
    /// `Compare` (so a generic `>` on `f64` is IEEE), a class with a written `Ord` gets a call of
    /// its `less`, and the same for `==`.
    #[test]
    fn generic_operators_should_be_lowered_again_for_their_type_argument() {
        let src = "class Circle: Eq, Ord {\n    r: i64\n    fn eq(self, other: Self) -> bool { self.r == other.r }\n    fn less(self, other: Self) -> bool { self.r < other.r }\n}\n\
                   fn bigger<T: Ord>(a: T, b: T) -> bool {\n    a > b\n}\n\
                   fn same<T: Eq>(a: T, b: T) -> bool {\n    a == b\n}\n\
                   fn main() {\n    let c = Circle(r: 1)\n    print(bigger(1.5, 2.5))\n    print(bigger(c, c))\n    print(same(1, 2))\n    print(same(c, c))\n}";
        let number = printed_function(src, "bigger<f64>");
        assert!(number.contains("(> a#0 b#1)"), "{number}");
        let class = printed_function(src, "bigger<Circle>");
        assert!(class.contains("(call Circle.less b#1 a#0)"), "{class}");
        let scalar = printed_function(src, "same<i64>");
        assert!(scalar.contains("(== a#0 b#1)"), "{scalar}");
        let object = printed_function(src, "same<Circle>");
        assert!(object.contains("(call Circle.eq a#0 b#1)"), "{object}");
    }

    /// A chain of three keeps one node for numbers, and binds each `string` once, as the checker
    /// does for a `string` chain; two operands keep one node for either.
    #[test]
    fn generic_chains_should_keep_their_shape_for_each_type_argument() {
        let src = "fn between<T: Ord>(a: T, b: T, c: T) -> bool {\n    a < b < c\n}\n\
                   fn below<T: Ord>(a: T, b: T) -> bool {\n    a < b\n}\n\
                   fn main() {\n    print(between(1, 2, 3))\n    print(between(\"a\", \"b\", \"c\"))\n    print(below(\"a\", \"b\"))\n    print(below(1, 2))\n}";
        let number = printed_function(src, "between<i64>");
        assert!(number.contains("(< a#0 b#1 < c#2)"), "{number}");
        let text = printed_function(src, "between<string>");
        assert!(text.contains("(&& (< #"), "{text}");
        for name in ["below<string>", "below<i64>"] {
            let two = printed_function(src, name);
            assert!(two.contains("(< a#0 b#1)"), "{two}");
        }
    }

    /// `x == None` tests presence and calls no `eq`, so it reaches no written conformance: the
    /// output holds no record for the class that `x` holds, and `x == x` makes one.
    #[test]
    fn equality_with_none_should_make_no_record() {
        let class = "class Circle: Eq {\n    r: i64\n    fn eq(self, other: Self) -> bool { self.r == other.r }\n}\n\
                     fn same<T: Eq>(x: T?, y: T?) -> bool {\n    x == y\n}\n\
                     fn missing<T: Eq>(x: T?) -> bool {\n    x == None\n}\n";
        let records = |main: &str| {
            let program = instantiated(&format!("{class}fn main() {{\n    {main}\n}}"));
            program.conformances.iter().count()
        };
        assert_eq!(records("print(missing(Circle(r: 1)))"), 0);
        assert_eq!(records("print(same(Circle(r: 1), Circle(r: 2)))"), 1);
    }

    /// `dbg` of an `Outer<i64>` shows its field `inner`, an `Inner<i64>` whose written
    /// `to_string` nothing else calls. The output holds the record of that `to_string`, whether
    /// `dbg` takes the object, an array of it or an optional of it. `print` of the object writes
    /// only its name, so it makes no record.
    #[test]
    fn dbg_should_make_a_record_for_a_written_to_string_in_a_field_but_print_should_not() {
        let decls = "class Inner<T> {\n    v: T\n}\nextend<T> Inner<T>: Display {\n    fn to_string(self) -> string {\n        \"inner\"\n    }\n}\nclass Outer<T> {\n    inner: Inner<T>\n}\n";
        for (shown, records) in [
            ("print(o)", 0),
            ("print([o])", 0),
            ("dbg(o)", 1),
            ("dbg([o])", 1),
            ("dbg(Some(o))", 1),
        ] {
            let program = instantiated(&format!(
                "{decls}fn main() {{\n    let o = Outer(inner: Inner(v: 1))\n    {shown}\n    print(o.inner.v)\n}}"
            ));
            assert_eq!(program.conformances.iter().count(), records, "{shown}");
        }
    }

    /// An enum `Slot` with a written `to_string` shows that text, so `dbg` of it, of an array of
    /// it or of an enum that holds it makes the record of that `to_string` alone, and none of the
    /// `to_string` of `Inner`, the class in its payload.
    #[test]
    fn dbg_should_make_no_record_below_an_enum_with_a_written_to_string() {
        let decls = "class Inner<T> {\n    v: T\n}\nextend<T> Inner<T>: Display {\n    fn to_string(self) -> string {\n        \"inner\"\n    }\n}\nenum Slot<T> {\n    Full(Inner<T>)\n}\nextend<T> Slot<T>: Display {\n    fn to_string(self) -> string {\n        \"slot\"\n    }\n}\nenum Pick<T> {\n    One(Slot<T>)\n}\n";
        for shown in ["dbg(s)", "dbg([s])", "dbg(Some(s))", "dbg(Pick.One(s))"] {
            let program = instantiated(&format!(
                "{decls}fn main() {{\n    let s = Slot.Full(Inner(v: 1))\n    {shown}\n}}"
            ));
            assert_eq!(program.conformances.iter().count(), 1, "{shown}");
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
        // `[1, 2].sum()`, `2.min(3)` and `[1].contains(1)` reach the prelude's `sum`, `min` and
        // `contains`, and no other function of the prelude.
        assert_eq!(
            function_names(&program),
            [
                "main",
                "fill<i64>",
                "[i64].sum",
                "i64.min",
                "[i64].contains"
            ]
        );
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
        let program = instantiate(program).unwrap_or_else(|found| panic!("{found:?}"));
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

    /// The checker sets `instantiates` at a call from the user's file into the prelude, and not at
    /// the prelude's own calls (`clamp` calls `max`): a program that calls no prelude function
    /// takes the path that instantiates nothing, and one that calls `min` does not.
    #[test]
    fn a_call_into_the_prelude_should_leave_the_path_that_instantiates_nothing() {
        let before = FAST_PATHS.get();
        instantiated("fn main() {\n    print(1)\n}");
        assert_eq!(FAST_PATHS.get(), before + 1);
        let before = FAST_PATHS.get();
        let program = instantiated("fn main() {\n    print(5.min(3))\n}");
        assert_eq!(FAST_PATHS.get(), before);
        assert_eq!(function_names(&program), ["main", "i64.min"]);
    }

    /// An implicit call, such as `print` or `==`, reaches a method of the prelude through a
    /// conformance, with no written use, so the checker does not set `instantiates`. The program
    /// then keeps the method when its target is outside the prelude.
    #[test]
    fn a_prelude_method_of_a_user_type_should_leave_the_fast_path() {
        let show = "extend C {\n    fn to_string(self) -> string { \"custom\" }\n}\n";
        let show_enum = "extend E {\n    fn to_string(self) -> string { \"custom\" }\n}\n";
        let eq = "extend C: Eq {\n    fn eq(self, _other: C) -> bool { true }\n}\n";
        let main = |body: &str| format!("class C {{}}\nfn main() {{\n    {body}\n}}");
        for (row, prelude, src, method) in [
            (
                "display on a class",
                show,
                main("print(C())"),
                "C.to_string",
            ),
            (
                "display on an enum",
                show_enum,
                "enum E {\n    A\n}\nfn main() {\n    print(E.A)\n}".to_string(),
                "E.to_string",
            ),
            ("eq on a class", eq, main("print(C() == C())"), "C.Eq.eq"),
        ] {
            let before = FAST_PATHS.get();
            let (program, diagnostics) = crate::front_end::front_end_with(prelude, &src);
            let program = program.unwrap_or_else(|| panic!("{row}: {diagnostics:?}"));
            assert_eq!(FAST_PATHS.get(), before, "{row}");
            let names = function_names(&program);
            assert!(names.contains(&method), "{row}: {method} in {names:?}");
        }
    }

    /// A target in the prelude is an item of the prelude: nothing reaches it, so it goes with the
    /// rest of the prelude.
    #[test]
    fn a_prelude_method_of_a_prelude_type_should_keep_the_fast_path() {
        for (row, prelude) in [
            (
                "a class in the prelude",
                "class P {}\nextend P {\n    fn to_string(self) -> string { \"p\" }\n}\n",
            ),
            (
                "an enum in the prelude",
                "enum Q {\n    A\n}\nextend Q {\n    fn to_string(self) -> string { \"q\" }\n}\n",
            ),
        ] {
            let before = FAST_PATHS.get();
            let (program, diagnostics) =
                crate::front_end::front_end_with(prelude, "fn main() {\n    print(1)\n}");
            let program = program.unwrap_or_else(|| panic!("{row}: {diagnostics:?}"));
            assert_eq!(FAST_PATHS.get(), before + 1, "{row}");
            assert_eq!(function_names(&program), ["main"], "{row}");
        }
    }

    /// The prelude of the tests below: a class and an enum that are not generic, so `mono` roots
    /// them only when the user's file reaches them.
    const ITEMS_PRELUDE: &str = "class Probe {\n    n: i64\n}\nenum Light {\n    Red\n    Green\n}\nfn probe() -> i64 { 1 }\n";

    /// The program `front_end_with` gives for `src` over `ITEMS_PRELUDE`, with the fast paths
    /// that `mono` took while it ran.
    fn instantiated_over_items(src: &str) -> (Program, usize) {
        let before = FAST_PATHS.get();
        let (program, diagnostics) = crate::front_end::front_end_with(ITEMS_PRELUDE, src);
        let program = program.unwrap_or_else(|| panic!("{diagnostics:?}"));
        (program, FAST_PATHS.get() - before)
    }

    /// The user's file reaches a class, an enum variant, a function or a type name of the prelude
    /// that is not generic, and so leaves the path that instantiates nothing. A program that
    /// reaches none takes it, and drops all of the prelude's items.
    #[test]
    fn a_use_of_a_prelude_item_that_is_not_generic_should_leave_the_path_that_instantiates_nothing()
    {
        for (src, kept) in [
            ("fn main() {\n    print(Probe(n: 1).n)\n}", "Probe"),
            (
                "fn main() {\n    match Light.Red {\n        Light.Red => print(1)\n        Light.Green => print(2)\n    }\n}",
                "Light",
            ),
            ("fn main() {\n    print(probe())\n}", "probe"),
            (
                "fn main() {\n    let p: Probe? = None\n    print(p == None)\n}",
                "Probe",
            ),
            (
                "fn main() {\n    let l: [Light] = []\n    print(l.len())\n}",
                "Light",
            ),
        ] {
            let (program, fast) = instantiated_over_items(src);
            assert_eq!(fast, 0, "{src}");
            let names: Vec<&str> = (program.classes.iter().map(|c| c.name.as_str()))
                .chain(program.enums.iter().map(|e| e.name.as_str()))
                .chain(function_names(&program))
                .collect();
            assert!(names.contains(&kept), "{kept} in {names:?} for {src}");
        }
        let (program, fast) = instantiated_over_items("fn main() {\n    print(1)\n}");
        assert_eq!(fast, 1);
        assert!(program.classes.is_empty() && program.enums.is_empty());
        assert_eq!(function_names(&program), ["main"]);
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
        let program = instantiate(program).unwrap_or_else(|found| panic!("{found:?}"));
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
    /// The program that checks, and then `src` with each choice of `Leaf` to `Source` (the first
    /// conformance of interface 1, whose associated types `A` and `B` are numbers 0 and 1) changed
    /// to name its own associated type, as no checker lets a program do. The choices of `A` and `B`
    /// are written at the bytes 40 and 10.
    fn looping(src: &str) -> (Program, Program) {
        let program = || {
            let (program, diagnostics) = check_alone(&parse_ok(src), src, &[]);
            program.unwrap_or_else(|| panic!("{diagnostics:?}"))
        };
        let source = InterfaceId(1);
        let mut broken = program();
        let leaf = broken.conformances.get(source, 0).target;
        for k in 0..2 {
            let own = Type::intern(TypeKind::Assoc(leaf, source, TypeArgs::NONE, k));
            broken.conformances.get_mut(source, 0).assoc[k as usize] = Some(own);
        }
        broken.choice_ats.insert((source, 0, 0), Span::new(40, 41));
        broken.choice_ats.insert((source, 0, 1), Span::new(10, 11));
        (program(), broken)
    }

    const LOOPING: &str = "interface Sized: Eq {\n    fn size(self) -> i64\n}\n\
        interface Source {\n    type A: Sized\n    type B: Sized\n    fn first(self) -> A\n    fn second(self) -> B\n}\n\
        class Thing: Sized {\n    fn size(self) -> i64 { 1 }\n}\n\
        extend Thing: Eq {\n    fn eq(self, other: Self) -> bool { true }\n}\n\
        class Leaf: Source {\n    type A = Thing\n    type B = Thing\n    fn first(self) -> A { Thing() }\n    fn second(self) -> B { Thing() }\n}\n\
        class Holder<T> {\n    value: T\n}\n\
        enum Maybe<T> {\n    Just(T)\n    Nothing\n}\n";

    /// The starts of the diagnostics that `instantiate` gives for `program`, each an E0030.
    fn stuck_places(program: Program) -> Vec<usize> {
        let found =
            instantiate(program).expect_err("a choice that needs itself should be reported");
        for d in &found {
            assert_eq!(d.code(), crate::diagnostic::Code::UnknownType);
        }
        found.iter().map(|d| d.span().start()).collect()
    }

    /// Two choices that need themselves are both reported, in the order of the source: a function
    /// whose two parameters each name one, so `mono` meets both before it stops.
    #[test]
    fn mono_should_report_each_choice_that_needs_itself_in_the_order_of_the_source() {
        let src = format!(
            "{LOOPING}fn run<S: Source>(_s: S, _a: S.A, _b: S.B) {{}}\nfn main() {{\n    run(Leaf(), Thing(), Thing())\n}}"
        );
        let (whole, broken) = looping(&src);
        assert!(instantiate(whole).is_ok(), "the program without the loops");
        assert_eq!(stuck_places(broken), [10, 40]);
    }

    /// An expression whose own type names a choice that needs itself, and an interface call whose
    /// receiver does, stop the walk of the body: no panic on a stand-in type, and one report. Each
    /// row is a statement that drops its value (a warning), before a `print` so that the value of
    /// the block is no part of it. A construction and a variant read their class or enum from the
    /// type, which a stand-in is not.
    #[test]
    fn mono_should_stop_a_body_whose_expression_names_a_choice_that_needs_itself() {
        for (row, body) in [
            ("a call statement", "s.first()"),
            ("a call", "s.first().size()"),
            ("a construction", "Holder(value: s.first())"),
            ("a variant", "Maybe.Just(s.first())"),
        ] {
            let src = format!(
                "{LOOPING}fn run<S: Source>(s: S) {{\n    {body}\n    print(1)\n}}\nfn main() {{\n    run(Leaf())\n}}"
            );
            let (whole, broken) = looping(&src);
            assert!(
                instantiate(whole).is_ok(),
                "{row}: the program without the loops"
            );
            assert_eq!(stuck_places(broken), [40], "{row}");
        }
    }

    /// #242: a type that nests deeper than a 256 KiB stack allows recursing into is decided for
    /// `Eq` by the checker and by `mono` on a stack that grows. The chain `E0<T>` ... `E159<T>`
    /// holds `E(i + 1)<W<W<W<T>>>>`, so `E159` holds `W` nested 159 * 3 = 477 times, with a written
    /// `Eq` on `W<T: Eq>` that asks the bound of each level.
    #[test]
    fn deep_wrapped_types_should_be_decided_without_overflowing_a_small_stack() {
        // The checker needs more than 64 KiB for a body. 256 KiB holds about 200 levels of the
        // walk.
        const STACK: usize = 256 * 1024;
        let (n, k) = (160, 3);
        let mut src = String::from(
            "class W<T> {\n    value: T\n}\nextend<T: Eq> W<T>: Eq {\n    fn eq(self, other: Self) -> bool {\n        self.value == other.value\n    }\n}\n",
        );
        for i in 0..n - 1 {
            let (open, close) = ("W<".repeat(k), ">".repeat(k));
            writeln!(
                src,
                "enum E{i}<T> {{\n    End\n    A(E{}<{open}T{close}>)\n}}",
                i + 1
            )
            .expect("writing to a `String` should succeed");
        }
        writeln!(src, "enum E{}<T> {{\n    End\n    A(T)\n}}", n - 1)
            .expect("writing to a `String` should succeed");
        src.push_str("fn main() {\n    let e: E0<i64> = E0.End\n    print(e == e)\n}\n");
        // The parser needs more than 64 KiB for a statement, so it runs here, not on the thread.
        let parsed = parse_ok(&src);
        std::thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || {
                let (program, diagnostics) = check_alone(&parsed, &src, &[]);
                let program = program.unwrap_or_else(|| panic!("{diagnostics:?}"));
                instantiate(program).unwrap_or_else(|found| panic!("{found:?}"));
            })
            .expect("the thread should start")
            .join()
            .expect("the walks should not overflow the stack");
    }

    /// The codes and starts of the diagnostics of the whole front end for `src`.
    fn front_end_reports(src: &str) -> Vec<(crate::diagnostic::Code, usize)> {
        let found = crate::front_end::front_end(src).1;
        found.iter().map(|d| (d.code(), d.span().start())).collect()
    }

    /// `enum P<T> { End; A(T, T) }` nested `levels` deep in the type of a `let`.
    fn nested_pairs(levels: usize) -> String {
        let nested = format!("{}i64{}", "P<".repeat(levels), ">".repeat(levels));
        format!(
            "enum P<T> {{\n    End\n    A(T, T)\n}}\nfn main() {{\n    let x: {nested} = P.End\n    print(x)\n}}\n"
        )
    }

    /// A payload that no declaration shows: each level of `P<P<..P<i64>..>>` doubles it. The
    /// small program has a warning, and no error.
    #[test]
    fn instantiate_should_report_a_generic_enum_that_an_instance_makes_too_large() {
        // `P` is at byte 5 of line 1.
        assert_eq!(
            front_end_reports(&nested_pairs(32)),
            [(crate::diagnostic::Code::PayloadTooLarge, 5)]
        );
        let small = front_end_reports(&nested_pairs(10));
        assert!(
            small
                .iter()
                .all(|&(code, _)| code != crate::diagnostic::Code::PayloadTooLarge),
            "{small:?}"
        );
    }

    /// The start of each secondary span of the E0160 that `front_end` reports for `src`.
    fn payload_uses(src: &str) -> Vec<usize> {
        let found = crate::front_end::front_end(src).1;
        (found.iter())
            .filter(|d| d.code() == crate::diagnostic::Code::PayloadTooLarge)
            .flat_map(|d| d.secondary().iter().map(|(span, _)| span.start()))
            .collect()
    }

    /// The `let` types the instance first, as a local, so no expression reaches it first. The
    /// expression `P.End` then has the type of the outer instance, and the instance that is over
    /// the limit is nested inside it.
    #[test]
    fn instantiate_should_point_e0160_at_the_expression_that_has_the_type_of_the_outer_instance() {
        let src = nested_pairs(32);
        assert_eq!(
            payload_uses(&src),
            [src.find("P.End").expect("the source should hold the use")]
        );
    }

    /// An instance nested in another enum: `W.Box(P.End)` has the type of `W`, and `P.End`, an
    /// operand, has the type of the outer instance of `P`. The use is the nearest holder.
    #[test]
    fn instantiate_should_point_e0160_at_the_nearest_expression_that_holds_the_instance() {
        let nested = format!("{}i64{}", "P<".repeat(32), ">".repeat(32));
        let src = format!(
            "enum P<T> {{\n    End\n    A(T, T)\n}}\nenum W<T> {{\n    Box(T)\n}}\nfn main() {{\n    let x: W<{nested}> = W.Box(P.End)\n    print(x)\n}}\n"
        );
        assert_eq!(
            payload_uses(&src),
            [src.find("P.End").expect("the source should hold the use")]
        );
    }

    /// No local, parameter or field has the type: the first use is the expression that makes
    /// the instance, and the instances inside it come from the same expression.
    #[test]
    fn instantiate_should_point_e0160_at_the_expression_that_first_makes_the_instance() {
        let nested = format!("{}i64{}", "P<".repeat(32), ">".repeat(32));
        let src = format!(
            "enum P<T> {{\n    End\n    A(T, T)\n}}\nfn main() {{\n    print({nested}.End)\n}}\n"
        );
        assert_eq!(
            payload_uses(&src),
            [src.find(&nested).expect("the source should hold the use")]
        );
    }

    /// The `Result` is the instance that the use makes, not `F`, which the user wrote: a use
    /// of `F` alone is no use of the instance.
    #[test]
    fn instantiate_should_point_e0160_at_the_use_of_the_result_that_holds_the_enum() {
        let mut src = String::new();
        for n in 5..31 {
            writeln!(
                src,
                "enum E{n} {{\n    End\n    A(E{}, E{})\n}}",
                n + 1,
                n + 1
            )
            .expect("writing to a `String` should succeed");
        }
        src.push_str("enum E31 {\n    End\n    A(i64, i64)\n}\n");
        src.push_str("enum F {\n    End\n    A(E5, E5, i64)\n}\n");
        src.push_str("fn main() {\n    let r: Result<F, F> = Result.Ok(F.End)\n    print(r)\n}\n");
        assert_eq!(
            payload_uses(&src),
            [src.find("Result.Ok")
                .expect("the source should hold the use")]
        );
    }

    /// The source of a program with `P`, then `decls`, then `main` with the body `body`. `P^32` and
    /// `P^28` in `decls` and `body` stand for `P` nested 32 and 28 levels deep.
    fn with_pairs(decls: &str, body: &str) -> String {
        let nested = |levels: usize| format!("{}i64{}", "P<".repeat(levels), ">".repeat(levels));
        let program = format!(
            "enum P<T> {{\n    End\n    A(T, T)\n}}\n{decls}fn main() {{\n{body}    print(1)\n}}\n"
        );
        program
            .replace("P^32", &nested(32))
            .replace("P^28", &nested(28))
    }

    /// How many E0160 the front end reports for `src`.
    fn payload_errors(src: &str) -> usize {
        let found = crate::front_end::front_end(src).1;
        let too_large = |d: &&Diagnostic| d.code() == crate::diagnostic::Code::PayloadTooLarge;
        found.iter().filter(too_large).count()
    }

    /// An earlier function ends in an expression, so `at` has that expression's span when `main`
    /// starts. `expr` puts `at` back, so the locals of `main` do not take that span.
    #[test]
    fn instantiate_should_not_leak_the_span_of_an_earlier_function_into_the_label() {
        let src = with_pairs(
            "fn a() -> i64 {\n    return 1\n}\n",
            "    let x: P^32 = P.End\n",
        );
        assert_eq!(
            payload_uses(&src),
            [src.find("P.End").expect("the source should hold the use")]
        );
    }

    /// `W` is no instance: two fields of `P^28` make it too large, and `W.End` only uses `W`. Each
    /// `P^28` alone is under the limit, so the error is on `W`.
    #[test]
    fn instantiate_should_give_no_e0160_label_for_an_enum_that_is_no_instance() {
        let src = with_pairs(
            "enum W {\n    End\n    A(P^28, P^28)\n}\n",
            "    let w = W.End\n",
        );
        assert_eq!(payload_errors(&src), 1);
        assert_eq!(payload_uses(&src), Vec::<usize>::new());
    }

    /// `G` is no instance: its declaration makes the instance of `P`, and `G.B` only uses `G`.
    #[test]
    fn instantiate_should_give_no_e0160_label_when_a_non_generic_enum_holds_the_instance() {
        let src = with_pairs("enum G {\n    A(P^32)\n    B\n}\n", "    let g = G.B\n");
        assert_eq!(payload_errors(&src), 1);
        assert_eq!(payload_uses(&src), Vec::<usize>::new());
    }

    /// `G` is no instance, so the instance `H<i64>` that holds it does not pass its use to `P`.
    #[test]
    fn instantiate_should_give_no_e0160_label_when_an_instance_holds_a_non_generic_enum() {
        let decls = "enum G {\n    A(P^32)\n    B\n}\nenum H<T> {\n    A(G)\n    B(T)\n}\n";
        let src = with_pairs(decls, "    let h: H<i64> = H.B(1)\n");
        assert_eq!(payload_errors(&src), 1);
        assert_eq!(payload_uses(&src), Vec::<usize>::new());
    }

    /// An instance holds an instance: `W.B` uses `W<P^32>`, which holds the instance of `P` that is
    /// over the limit. The local made both before any expression, so the label comes from the
    /// spread of the use of `W<P^32>`.
    #[test]
    fn instantiate_should_point_e0160_at_the_use_of_an_instance_that_holds_the_instance() {
        let src = with_pairs(
            "enum W<T> {\n    A(T)\n    B\n}\n",
            "    let w: W<P^32> = W.B\n",
        );
        assert_eq!(
            payload_uses(&src),
            [src.find("W.B").expect("the source should hold the use")]
        );
    }

    /// The type of `None` is an optional of the instance, and the type of `[]` an array of it.
    #[test]
    fn instantiate_should_point_e0160_at_an_expression_whose_type_is_an_optional_or_array() {
        for (ty, value) in [("P^32?", "None"), ("[P^32]", "[]")] {
            let src = with_pairs("", &format!("    let x: {ty} = {value}\n"));
            let at = src.rfind(value).expect("the source should hold the use");
            assert_eq!(payload_uses(&src), [at], "{ty}");
        }
    }

    /// A `Result` that holds an instance of `P`, through another `Result`: the error is at the
    /// declaration of `P`, as for a `Result` that holds the instance directly. The wording is
    /// in `build_should_name_the_result_that_holds_an_instance_of_a_generic_enum`.
    #[test]
    fn instantiate_should_report_e0160_at_the_declaration_of_an_instance_that_a_result_holds() {
        let pairs = format!("{}i64{}", "P<".repeat(28), ">".repeat(28));
        let src = format!(
            "enum P<T> {{\n    End\n    A(T, T)\n}}\nfn main() {{\n    let r: Result<Result<{pairs}, i64>, i64> = Result.Err(1)\n    print(r)\n}}\n"
        );
        // `P` is at byte 5 of line 1.
        assert_eq!(
            front_end_reports(&src),
            [(crate::diagnostic::Code::PayloadTooLarge, 5)]
        );
    }

    /// Reached by `check` only through `front_end`: the checker sees no instance.
    #[test]
    fn checked_should_not_find_the_payload_of_an_instance() {
        let (program, diagnostics) = crate::front_end::checked(&nested_pairs(32));
        assert!(program.is_some(), "{diagnostics:?}");
    }

    /// `Result<F, F>` holds an `F` of 2^32 bytes, though the payload of `F` is 8 bytes less:
    /// the error names `F`, which the user wrote, and not the prelude's `Result`. `F` holds two
    /// `E5` and an `i64`, where `E5` starts a chain of 27 enums that each hold two of the next.
    #[test]
    fn instantiate_should_name_the_user_enum_that_a_result_holds() {
        let mut src = String::new();
        for n in 5..31 {
            writeln!(
                src,
                "enum E{n} {{\n    End\n    A(E{}, E{})\n}}",
                n + 1,
                n + 1
            )
            .expect("writing to a `String` should succeed");
        }
        src.push_str("enum E31 {\n    End\n    A(i64, i64)\n}\n");
        let at = src.len() + "enum ".len();
        src.push_str("enum F {\n    End\n    A(E5, E5, i64)\n}\n");
        src.push_str("fn main() {\n    let r: Result<F, F> = Result.Ok(F.End)\n    print(r)\n}\n");
        assert_eq!(
            front_end_reports(&src),
            [(crate::diagnostic::Code::PayloadTooLarge, at)]
        );
    }

    /// `n` classes in a chain: `C{k}` holds `C{k-1}` twice, with `[T]` and `T?`, so `C{n}<i64>`
    /// makes 2^n - 1 instances in all.
    fn class_chain(n: usize) -> String {
        let mut src = String::from("class C1<T> {\n    a: [T]\n    b: T?\n}\n");
        for k in 2..=n {
            let prev = k - 1;
            writeln!(
                src,
                "class C{k}<T> {{\n    a: C{prev}<[T]>\n    b: C{prev}<T?>\n}}"
            )
            .expect("writing to a `String` should succeed");
        }
        write!(src, "fn main() {{\n    let x: C{n}<i64>? = None\n}}\n")
            .expect("writing to a `String` should succeed");
        src
    }

    /// The same chain of generic enums.
    fn enum_chain(n: usize) -> String {
        let mut src = String::from("enum E1<T> {\n    A([T])\n    B(T?)\n}\n");
        for k in 2..=n {
            let prev = k - 1;
            writeln!(
                src,
                "enum E{k}<T> {{\n    A(E{prev}<[T]>)\n    B(E{prev}<T?>)\n}}"
            )
            .expect("writing to a `String` should succeed");
        }
        write!(src, "fn main() {{\n    let x: E{n}<i64>? = None\n}}\n")
            .expect("writing to a `String` should succeed");
        src
    }

    /// The same chain of generic functions: `g{k}` calls `g{k-1}` twice.
    fn function_chain(n: usize) -> String {
        let mut src = String::from("fn g1<T>(x: i64) -> i64 {\n    x + 1\n}\n");
        for k in 2..=n {
            let prev = k - 1;
            writeln!(
                src,
                "fn g{k}<T>(x: i64) -> i64 {{\n    g{prev}<[T]>(x) + g{prev}<T?>(x)\n}}"
            )
            .expect("writing to a `String` should succeed");
        }
        write!(src, "fn main() {{\n    let y = g{n}<i64>(0)\n}}\n")
            .expect("writing to a `String` should succeed");
        src
    }

    /// `src` checked with the prelude, then monomorphized with `limit`, not `INSTANCE_LIMIT`.
    fn mono_with_limit(src: &str, limit: usize) -> Mono {
        let (program, diagnostics) = crate::front_end::checked(src);
        let program = program.unwrap_or_else(|| panic!("{diagnostics:?}"));
        Mono::with_limit(program, limit)
    }

    /// The name, the span start and the instance count that `busiest` gives for `src` when the
    /// worklist has stopped at `limit`.
    fn busiest_at(src: &str, limit: usize) -> (String, usize, usize) {
        let mut mono = mono_with_limit(src, limit);
        mono.expand();
        let (name, at, count) = mono.busiest();
        (name, at.start(), count)
    }

    /// A chain of 6 items makes 2^6 - 1 = 63 instances, so the limit 63 holds and the limit 62
    /// does not. At 62, the worklist stops while it makes the 32nd of the 32 instances of the
    /// item `first`, which is the first in the chain. The instances come level by level.
    fn assert_chain_stops_at_its_first_item(row: &str, src: &str, first: &str) {
        let at = src.find(first).expect("the source should declare the item");
        let found = mono_with_limit(src, 62).run().err();
        let found = found.unwrap_or_else(|| panic!("{row}: 63 instances should pass the limit 62"));
        let reports: Vec<_> = found.iter().map(|d| (d.code(), d.span().start())).collect();
        assert_eq!(
            reports,
            [(crate::diagnostic::Code::TooManyInstances, at)],
            "{row}"
        );
        assert_eq!(busiest_at(src, 62), (first.to_string(), at, 32), "{row}");
        let at_limit = mono_with_limit(src, 63).run().err();
        assert!(
            at_limit.is_none(),
            "{row}: 63 instances should hold the limit 63"
        );
    }

    #[test]
    fn mono_should_report_e0163_at_the_first_class_of_a_chain_over_the_limit() {
        assert_chain_stops_at_its_first_item("class", &class_chain(6), "C1");
    }

    #[test]
    fn mono_should_report_e0163_at_the_first_enum_of_a_chain_over_the_limit() {
        assert_chain_stops_at_its_first_item("enum", &enum_chain(6), "E1");
    }

    #[test]
    fn mono_should_report_e0163_at_the_first_function_of_a_chain_over_the_limit() {
        assert_chain_stops_at_its_first_item("function", &function_chain(6), "g1");
    }

    /// The user's `W` has 2 instances and the prelude's `Result` has more, so `Result` is the
    /// busiest item of the program. The error is not at `Result`, which is in the prelude, but at
    /// the user's source.
    #[test]
    fn mono_should_name_the_prelude_item_that_has_the_most_instances_and_point_into_user_source() {
        let src = "class W<T> {\n    v: T\n}\nfn main() {\n    let a: W<i64> = W(v: 1)\n    let b: W<bool> = W(v: true)\n    let c: Result<i64, i64> = Result.Ok(1)\n    let d: Result<i64, bool> = Result.Ok(1)\n    let e: Result<bool, i64> = Result.Ok(true)\n    let f: Result<bool, bool> = Result.Ok(true)\n    let g: Result<string, i64> = Result.Ok(\"a\")\n}\n";
        let program = (mono_with_limit(src, INSTANCE_LIMIT).run())
            .unwrap_or_else(|found| panic!("{found:?}"));
        let results = program
            .enums
            .iter()
            .filter(|e| e.name.starts_with("Result<"))
            .count();
        assert_eq!(
            results, 5,
            "the prelude's enum should have the most instances"
        );
        let found = mono_with_limit(src, 4)
            .run()
            .expect_err("7 instances pass 4");
        assert_eq!(found.len(), 1);
        assert!(!found[0].span().in_prelude(), "{found:?}");
        assert_eq!(
            found[0].span().start(),
            // The 5th instance comes from the types of the locals of `main`, before any expression
            // of it: the place is the name of `main`.
            src.find("main").expect("the source should have `main`"),
            "{found:?}"
        );
        let mut mono = mono_with_limit(src, 4);
        mono.expand();
        let (name, decl, _) = mono.busiest();
        assert_eq!(name, "Result");
        assert!(decl.in_prelude());
    }

    /// The aliases make all the instances in the prelude's `Result`, each written type is small,
    /// and the user has no generic item. The error is one E0163 at the user's source.
    #[test]
    fn mono_should_report_e0163_for_a_chain_of_aliases_over_a_prelude_enum() {
        let mut src = "type B1<T> = Result<[T], T?>\n".to_string();
        for k in 2..=6 {
            let prev = k - 1;
            writeln!(src, "type B{k}<T> = Result<B{prev}<[T]>, B{prev}<T?>>")
                .expect("writing to a `String` should succeed");
        }
        src.push_str("fn main() {\n    let x: B6<i64>? = None\n    print(x == None)\n}\n");
        let found = mono_with_limit(&src, 20).run().err();
        let found = found.expect("the instances of `Result` should pass the limit 20");
        let reports: Vec<_> = found
            .iter()
            .map(|d| (d.code(), d.span().in_prelude()))
            .collect();
        assert_eq!(
            reports,
            [(crate::diagnostic::Code::TooManyInstances, false)]
        );
        let mut mono = mono_with_limit(&src, 20);
        mono.expand();
        assert_eq!(mono.busiest().0, "Result");
    }

    /// The instances of the prelude's `Result` come from the field types of a class that no
    /// expression uses. The count passes the limit while `mono` lowers `K`, and `main`, which
    /// comes later, is not the place.
    #[test]
    fn mono_should_point_e0163_at_the_item_whose_types_make_the_instances_of_the_prelude() {
        let mut src = "type B1<T> = Result<[T], T?>\n".to_string();
        for k in 2..=6 {
            let prev = k - 1;
            writeln!(src, "type B{k}<T> = Result<B{prev}<[T]>, B{prev}<T?>>")
                .expect("writing to a `String` should succeed");
        }
        src.push_str(
            "class K {\n    a: B6<i64>?\n    b: B6<bool>?\n}\nfn main() {\n    print(1)\n}\n",
        );
        let found = mono_with_limit(&src, 40).run().err();
        let found = found.expect("the instances of `Result` should pass the limit 40");
        let at: Vec<_> = found.iter().map(|d| d.span().start()).collect();
        assert_eq!(at, [src.find('K').expect("the source should declare K")]);
    }

    /// `main` makes 5 instances: 3 of `Result`, `W<i64>` and the `contains` of `[W<i64>]`. The
    /// worklist then lowers the body of that `contains`, a function of the prelude, and its `==`
    /// makes the 6th instance, `W<i64>.eq`, which passes the limit 5. The `Result` of the prelude
    /// has the most instances, so the error is at the user's source that reached the prelude's
    /// `contains`: the call, which is not the last expression of `main`, and not the place of an
    /// item that the worklist lowered before.
    #[test]
    fn mono_should_point_e0163_at_the_call_that_reached_an_item_of_the_prelude() {
        let src = "class W<T>: Eq {\n    v: T\n    fn eq(self, other: Self) -> bool {\n        true\n    }\n}\nfn main() {\n    let c: Result<i64, i64> = Result.Ok(1)\n    let d: Result<i64, bool> = Result.Ok(1)\n    let e: Result<bool, i64> = Result.Ok(true)\n    let ws = [W(v: 1)]\n    print(ws.contains(W(v: 1)))\n    print(2)\n}\n";
        let found = mono_with_limit(src, 5)
            .run()
            .expect_err("6 instances pass 5");
        let at: Vec<_> = found.iter().map(|d| d.span().start()).collect();
        assert_eq!(
            at,
            [src.find("ws.contains")
                .expect("the source should have the call")]
        );
        let mut mono = mono_with_limit(src, 5);
        mono.expand();
        assert_eq!(mono.busiest().0, "Result");
    }

    /// The error of an instance that comes from the user's source relies on this: no item of the
    /// prelude makes an instance until the user's source reaches it.
    #[test]
    fn mono_should_make_no_instance_for_the_items_of_the_prelude_alone() {
        let mut mono = mono_with_limit("fn main() {}\n", 0);
        mono.expand();
        assert_eq!(mono.instances, 0);
    }
}
