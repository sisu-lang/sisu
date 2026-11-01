//! The generic rules of milestone 8. They write generic classes and enums, in shapes that give
//! each `tir::Rank`. They write generic functions, whose bodies do with a type parameter only what
//! an unconstrained `T` allows: pass it, store it, return it and match on it. And they write calls
//! with type arguments written out and with type arguments left to be inferred.
//!
//! A generic type is written once, over `GTy`. Each instance of it is also made as an ordinary
//! class or enum in `Scope::classes` and `Scope::enums`, with the type arguments put into its
//! fields. Every other rule then reads and builds an instance as it does a declared type.

use std::fmt;
use std::rc::Rc;

use super::Gen;
use super::expr::{CHARS, Info};
use super::item::MAX_FIELDS;
use super::item::payload;
use super::scope::{Applied, Class, Enum, Field, RESULT, Ty, Variant, generic_name};

/// The names of a generic declaration's type parameters, by index.
const PARAMS: [&str; 2] = ["T", "U"];
/// The most generic classes and enums a program declares.
const MAX_GENERICS: usize = 5;
/// The most generic functions.
const MAX_GFUNCS: usize = 3;
/// The room a field of an instance has beyond its floor, for an argument of a generic function.
const GCAP: u64 = 16;
/// The room an argument of a generic function has beyond its floor.
const GARG: u64 = 4;
/// The most floor a type argument has, so an instance of three fields stays near `MOST`.
const ARG_FLOOR: u64 = 16;
/// The most levels a value in a generic body nests.
const MAX_SOLVE: u32 = 3;

/// A type over the type parameters of a generic declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GTy {
    Param(usize),
    /// A scalar: it names no parameter.
    Plain(Ty),
    Array(Box<GTy>),
    Optional(Box<GTy>),
    /// The generic `Scope::generics[g]` with its type arguments.
    Applied(usize, Vec<GTy>),
}

impl fmt::Display for GTy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GTy::Param(i) => f.write_str(PARAMS[*i]),
            GTy::Plain(ty) => write!(f, "{ty}"),
            GTy::Array(element) => write!(f, "[{element}]"),
            GTy::Optional(payload) => write!(f, "{payload}?"),
            GTy::Applied(g, args) => write!(f, "{}{}", generic_name(*g), angled(args)),
        }
    }
}

/// `<a, b>` of the types in `items`.
fn angled<T: fmt::Display>(items: &[T]) -> String {
    let items: Vec<String> = items.iter().map(ToString::to_string).collect();
    format!("<{}>", items.join(", "))
}

impl GTy {
    /// `ty` with its generic instances as `Applied`.
    pub(crate) fn lift(ty: &Ty) -> GTy {
        match ty {
            Ty::Array(element) => GTy::Array(Box::new(GTy::lift(element))),
            Ty::Optional(payload) => GTy::Optional(Box::new(GTy::lift(payload))),
            Ty::Class {
                applied: Some(a), ..
            }
            | Ty::Enum {
                applied: Some(a), ..
            } => GTy::Applied(a.generic, a.args.iter().map(GTy::lift).collect()),
            _ => GTy::Plain(ty.clone()),
        }
    }

    /// Marks each parameter the type names.
    fn names(&self, used: &mut [bool]) {
        match self {
            GTy::Param(i) => used[*i] = true,
            GTy::Plain(_) => {}
            GTy::Array(inner) | GTy::Optional(inner) => inner.names(used),
            GTy::Applied(_, args) => args.iter().for_each(|arg| arg.names(used)),
        }
    }

    /// Whether the types in `types` name every one of `params` parameters.
    fn name_all<'a>(types: impl IntoIterator<Item = &'a GTy>, params: usize) -> bool {
        let mut used = vec![false; params];
        types.into_iter().for_each(|ty| ty.names(&mut used));
        used.iter().all(|&u| u)
    }

    /// The type with each parameter put as `args` says.
    fn subst(&self, args: &[GTy]) -> GTy {
        match self {
            GTy::Param(i) => args[*i].clone(),
            GTy::Plain(_) => self.clone(),
            GTy::Array(element) => GTy::Array(Box::new(element.subst(args))),
            GTy::Optional(payload) => GTy::Optional(Box::new(payload.subst(args))),
            GTy::Applied(g, inner) => {
                GTy::Applied(*g, inner.iter().map(|a| a.subst(args)).collect())
            }
        }
    }
}

/// Matches `pattern` against `target` and binds each parameter of `pattern` to the type it
/// stands for, an optional too.
fn unify(pattern: &GTy, target: &GTy, binds: &mut [Option<GTy>]) -> bool {
    match (pattern, target) {
        (GTy::Param(i), _) => {
            if let Some(bound) = &binds[*i] {
                return bound == target;
            }
            binds[*i] = Some(target.clone());
            true
        }
        (GTy::Plain(a), GTy::Plain(b)) => a == b,
        (GTy::Array(a), GTy::Array(b)) | (GTy::Optional(a), GTy::Optional(b)) => unify(a, b, binds),
        (GTy::Applied(g, a), GTy::Applied(h, b)) => {
            g == h && a.len() == b.len() && a.iter().zip(b).all(|(a, b)| unify(a, b, binds))
        }
        _ => false,
    }
}

/// A variant of a generic enum.
#[derive(Clone)]
pub(crate) struct GVariant {
    name: String,
    fields: Vec<GTy>,
    /// Whether a value of it holds a value of the enum, or of one that holds the enum again.
    recursive: bool,
}

/// A field of a generic class.
#[derive(Clone)]
pub(crate) struct GField {
    name: String,
    ty: GTy,
    mutable: bool,
}

#[derive(Clone)]
pub(crate) enum Body {
    Class(Vec<GField>),
    Enum(Vec<GVariant>),
}

/// A generic class or enum: `G<n>`.
#[derive(Clone)]
pub(crate) struct Generic {
    params: usize,
    body: Body,
}

impl Generic {
    /// The prelude's `enum Result<T, E> { Ok(T)  Err(E) }`, whose second parameter is `U` here.
    pub(crate) fn result() -> Generic {
        let variant = |name: &str, param| GVariant {
            name: name.to_string(),
            fields: vec![GTy::Param(param)],
            recursive: false,
        };
        Generic {
            params: 2,
            body: Body::Enum(vec![variant("Ok", 0), variant("Err", 1)]),
        }
    }

    fn is_enum(&self) -> bool {
        matches!(self.body, Body::Enum(_))
    }
}

/// A generic function.
#[derive(Clone)]
pub(crate) struct GFunc {
    name: String,
    /// How many type parameters it has.
    params: usize,
    args: Vec<GTy>,
    ret: GTy,
    /// What a call costs: the statements of the body, and the calls in it.
    cost: u64,
    /// The size that a result has in addition to the size of its arguments: its nodes.
    extra: u64,
}

impl GFunc {
    /// Whether the arguments name every type parameter, so a call may leave them out.
    fn inferable(&self) -> bool {
        GTy::name_all(&self.args, self.params)
    }
}

/// A value in a generic body: its text, its size in addition to its arguments, and the cost of
/// the calls in it.
struct Solved {
    text: String,
    extra: u64,
    cost: u64,
}

/// A parameter of the generic function that is being written: its name, its type, and whether the
/// body uses it already. A body uses each parameter once at most, so no argument is in a result
/// twice.
type Env = Vec<(String, GTy, bool)>;

impl Gen<'_> {
    /// Writes `<T, U>` of `params` type parameters for the declaration numbered `id`. An odd `id`
    /// breaks the line after `<`, after each `,` and before `>`. The choice takes no byte, so
    /// every other choice stays as it is.
    fn tparams(&mut self, params: usize, id: usize) {
        if id.is_multiple_of(2) {
            self.w.text(&format!("<{}>", PARAMS[..params].join(", ")));
            return;
        }
        self.w.text("<");
        for (i, param) in PARAMS[..params].iter().enumerate() {
            if i > 0 {
                self.w.text(",");
            }
            self.w.line();
            self.w.text(param);
        }
        self.w.line();
        self.w.text(">");
    }

    /// Declares the generic classes and enums, makes instances of them, and declares the generic
    /// functions. When the bytes run out, it declares nothing.
    pub(crate) fn generics(&mut self) {
        for _ in 0..self.pick(MAX_GENERICS + 1) {
            if !self.spend(1) {
                break;
            }
            self.generic_type();
        }
        // The instances come before the functions, so that a function can name them.
        // The prelude's `Result` is a generic like the declared ones.
        let declared = 0..self.scope.generics.len();
        for g in declared.chain([RESULT]) {
            for _ in 0..=self.pick(2) {
                let args: Vec<Ty> = (0..self.scope.generic(g).params)
                    .map(|_| self.type_arg())
                    .collect();
                let ty = self.instance(g, &args);
                if !self.scope.generic_types.contains(&ty) {
                    self.scope.generic_types.push(ty);
                }
            }
        }
        for _ in 0..self.pick(MAX_GFUNCS + 1) {
            if !self.spend(1) {
                break;
            }
            self.generic_function();
        }
    }

    /// The generic enums declared so far with `params` parameters.
    fn generic_enums(&self, params: usize) -> Vec<usize> {
        let generics = self.scope.generics.iter().enumerate();
        generics
            .filter(|(_, g)| g.is_enum() && g.params == params)
            .map(|(i, _)| i)
            .collect()
    }

    /// Declares a generic class, or a generic enum of one of five shapes. Together they give a
    /// field of each `tir::Rank` that names a type parameter. A field `T` is `Never`. A field of
    /// the enum itself, bare or in another enum, is `Own`. Two enums that name each other give
    /// `Open`. A pair that holds the enum at a type argument gives `Late`.
    fn generic_type(&mut self) {
        let wrappers = self.generic_enums(1);
        let pairs = self.generic_enums(2);
        let mut shapes = vec![Shape::Plain, Shape::Class, Shape::Recursive];
        if !wrappers.is_empty() {
            shapes.extend([Shape::Wrapped, Shape::Open]);
        }
        if !pairs.is_empty() {
            shapes.push(Shape::Late);
        }
        match shapes[self.pick(shapes.len())] {
            Shape::Class => self.generic_class(),
            Shape::Plain => self.generic_enum(Shape::Plain, &wrappers),
            Shape::Recursive => self.generic_enum(Shape::Recursive, &wrappers),
            Shape::Wrapped => self.generic_enum(Shape::Wrapped, &wrappers),
            Shape::Late => self.generic_enum(Shape::Late, &pairs),
            Shape::Open => self.open_pair(&wrappers),
        }
    }

    /// `class G<n><T, U> { g: T ... }`: fields that name each parameter, and then others. A
    /// field can be `mut`. An assignment to a field of an instance obeys decision 4, as one to a
    /// field of a declared class does: it never stores a class object.
    fn generic_class(&mut self) {
        let id = self.scope.generics.len();
        let params = 1 + self.pick(2);
        let enums = self.all_generic_enums();
        let types = self.gfields(params, &enums, MAX_FIELDS);
        let mut fields = Vec::new();
        for ty in types {
            let mutable = self.pick(2) == 1;
            let name = self.scope.fresh("g");
            fields.push(GField { name, ty, mutable });
        }
        self.w.text(&format!("class G{id}"));
        self.tparams(params, id);
        self.w.text(" ");
        self.w.open();
        for GField { name, ty, mutable } in &fields {
            self.w.line();
            let keyword = if *mutable { "mut " } else { "" };
            self.w.text(&format!("{keyword}{name}: {ty}"));
        }
        self.w.close();
        self.w.line();
        self.scope.generics.push(Generic {
            params,
            body: Body::Class(fields),
        });
    }

    /// Every generic enum declared so far.
    fn all_generic_enums(&self) -> Vec<usize> {
        (0..self.scope.generics.len())
            .filter(|&g| self.scope.generics[g].is_enum())
            .collect()
    }

    /// `enum G<n><T> { V ... }` of `shape`. The first variant holds no value of the enum, so a
    /// finite value ends in it. `others` are the generic enums that the shape uses: for `Wrapped`
    /// one with one parameter, for `Late` one with two.
    fn generic_enum(&mut self, shape: Shape, others: &[usize]) {
        let id = self.scope.generics.len();
        let params = match shape {
            Shape::Late => 1,
            _ => 1 + self.pick(2),
        };
        let enums = self.all_generic_enums();
        let own = GTy::Applied(id, (0..params).map(GTy::Param).collect());
        let mut variants = vec![self.base_variant(params)];
        let more = 1 + usize::from(shape == Shape::Plain && self.pick(2) == 1);
        for _ in 0..more {
            let mut fields = Vec::new();
            match shape {
                Shape::Recursive => fields.push(own.clone()),
                Shape::Wrapped => {
                    let w = others[self.pick(others.len())];
                    fields.push(GTy::Applied(w, vec![own.clone()]));
                }
                Shape::Late => {
                    let p = others[self.pick(others.len())];
                    let own_at = GTy::Applied(id, vec![GTy::Plain(Ty::I64)]);
                    fields.push(GTy::Applied(p, vec![GTy::Param(0), own_at]));
                }
                _ => {}
            }
            let named = !matches!(shape, Shape::Late);
            if named {
                fields.extend(self.gfields(params, &enums, MAX_FIELDS - fields.len()));
            }
            variants.push(GVariant {
                name: self.scope.fresh("V"),
                fields,
                recursive: shape != Shape::Plain,
            });
        }
        self.declare_enum(id, params, variants);
    }

    /// Two generic enums that name each other. `M<T>` holds a wrapper of `N<T>`, and `N<T>` holds
    /// `M<T>`, so each declaration is on a cycle of the other.
    fn open_pair(&mut self, wrappers: &[usize]) {
        let (m, n) = (self.scope.generics.len(), self.scope.generics.len() + 1);
        let w = wrappers[self.pick(wrappers.len())];
        let applied = |g: usize| GTy::Applied(g, vec![GTy::Param(0)]);
        let mut one = vec![self.base_variant(1)];
        one.push(GVariant {
            name: self.scope.fresh("V"),
            fields: vec![GTy::Applied(w, vec![applied(n)])],
            recursive: true,
        });
        self.declare_enum(m, 1, one);
        let mut two = vec![GVariant {
            name: self.scope.fresh("V"),
            fields: vec![GTy::Param(0)],
            recursive: false,
        }];
        two.push(GVariant {
            name: self.scope.fresh("V"),
            fields: vec![applied(m)],
            recursive: true,
        });
        self.declare_enum(n, 1, two);
    }

    /// A variant that holds no generic enum: no field, a type parameter, or a scalar.
    fn base_variant(&mut self, params: usize) -> GVariant {
        let fields = match self.pick(3) {
            0 => Vec::new(),
            1 => vec![GTy::Param(self.pick(params))],
            _ => vec![GTy::Plain(self.value_type())],
        };
        GVariant {
            name: self.scope.fresh("V"),
            fields,
            recursive: false,
        }
    }

    /// Writes the enum `G<id>` and records it.
    fn declare_enum(&mut self, id: usize, params: usize, variants: Vec<GVariant>) {
        self.w.text(&format!("enum G{id}"));
        self.tparams(params, id);
        self.w.text(" ");
        self.w.open();
        for variant in &variants {
            let types: Vec<String> = variant.fields.iter().map(ToString::to_string).collect();
            self.w.line();
            self.w
                .text(&format!("{}{}", variant.name, payload(&[], &types)));
        }
        self.w.close();
        self.w.line();
        debug_assert_eq!(
            id,
            self.scope.generics.len(),
            "a generic is pushed in order"
        );
        self.scope.generics.push(Generic {
            params,
            body: Body::Enum(variants),
        });
    }

    /// The fields of a generic class or variant. Each of the first `params` names one parameter.
    /// At most `most` others follow, of any shape.
    fn gfields(&mut self, params: usize, enums: &[usize], most: usize) -> Vec<GTy> {
        let count = params.max(self.pick(most + 1));
        (0..count)
            .map(|i| {
                if i < params {
                    self.param_field(i)
                } else {
                    self.gfield(params, enums)
                }
            })
            .collect()
    }

    /// `T`, `[T]` or `T?` of the parameter `i`.
    fn param_field(&mut self, i: usize) -> GTy {
        match self.pick(3) {
            0 => GTy::Param(i),
            1 => GTy::Array(Box::new(GTy::Param(i))),
            _ => GTy::Optional(Box::new(GTy::Param(i))),
        }
    }

    /// A field of any shape: a parameter, an array or optional of one, a scalar, or a generic
    /// enum with these as type arguments.
    fn gfield(&mut self, params: usize, enums: &[usize]) -> GTy {
        match self.pick(4 + usize::from(!enums.is_empty())) {
            0 => GTy::Param(self.pick(params)),
            1 => GTy::Array(Box::new(GTy::Param(self.pick(params)))),
            2 => GTy::Optional(Box::new(GTy::Param(self.pick(params)))),
            3 => GTy::Plain(self.value_type()),
            _ => {
                let g = enums[self.pick(enums.len())];
                self.applied(g, params)
            }
        }
    }

    /// The generic `g` applied to arguments over `params` parameters.
    fn applied(&mut self, g: usize, params: usize) -> GTy {
        let args = (0..self.scope.generics[g].params)
            .map(|_| match self.pick(3) {
                0 => GTy::Param(self.pick(params)),
                1 => GTy::Array(Box::new(GTy::Param(self.pick(params)))),
                _ => GTy::Plain(self.value_type()),
            })
            .collect();
        GTy::Applied(g, args)
    }

    /// A type in the signature of a generic function, nested `nesting` levels at most.
    fn gsig(&mut self, params: usize, nesting: u32) -> GTy {
        // A parameter comes twice and a scalar once. The smallest is first.
        let mut options = vec![0, 0, 1];
        if nesting > 0 {
            options.extend([2, 3]);
        }
        if !self.scope.generics.is_empty() {
            options.push(4);
        }
        match options[self.pick(options.len())] {
            0 => GTy::Param(self.pick(params)),
            1 => GTy::Plain(self.value_type()),
            2 => GTy::Array(Box::new(self.gsig_inner(params, nesting - 1))),
            3 => GTy::Optional(Box::new(self.gsig_inner(params, nesting - 1))),
            _ => {
                let g = self.pick(self.scope.generics.len());
                self.applied(g, params)
            }
        }
    }

    /// The type inside an array or an optional, which is no optional itself.
    fn gsig_inner(&mut self, params: usize, nesting: u32) -> GTy {
        match self.gsig(params, nesting) {
            GTy::Optional(payload) => *payload,
            ty => ty,
        }
    }

    /// A type argument: a scalar, a declared type or an instance, or an array of a scalar. Its
    /// floor is at most `ARG_FLOOR`.
    pub(crate) fn type_arg(&mut self) -> Ty {
        let mut pool: Vec<Ty> = Ty::VALUES.to_vec();
        let declared = self.scope.types.iter().chain(&self.scope.generic_types);
        pool.extend(declared.filter(|t| t.floor() <= ARG_FLOOR).cloned());
        pool.extend(Ty::VALUES.iter().map(|t| Ty::Array(Box::new(t.clone()))));
        pool[self.pick(pool.len())].clone()
    }

    /// The type that `gty` is when each parameter is the type that `args` gives.
    pub(crate) fn lower(&mut self, gty: &GTy, args: &[Ty]) -> Ty {
        match gty {
            GTy::Param(i) => args[*i].clone(),
            GTy::Plain(ty) => ty.clone(),
            GTy::Array(element) => Ty::Array(Box::new(self.lower(element, args))),
            GTy::Optional(payload) => Ty::Optional(Box::new(self.lower(payload, args))),
            GTy::Applied(g, inner) => {
                let inner: Vec<Ty> = inner.iter().map(|arg| self.lower(arg, args)).collect();
                self.instance(*g, &inner)
            }
        }
    }

    /// The instance of the generic `generic` with the type arguments `args`. The first use makes
    /// it: an ordinary class or enum, with the type arguments put into its fields.
    pub(crate) fn instance(&mut self, generic: usize, args: &[Ty]) -> Ty {
        let key = Rc::new(Applied {
            generic,
            args: args.to_vec(),
        });
        if let Some((_, ty)) = self.scope.instances.iter().find(|(k, _)| **k == *key) {
            return ty.clone();
        }
        let (params, body) = {
            let generic = self.scope.generic(generic);
            (generic.params, generic.body.clone())
        };
        let ty = match body {
            Body::Class(fields) => {
                let inferable = GTy::name_all(fields.iter().map(|f| &f.ty), params);
                let mut made = Vec::new();
                let mut size = 1;
                for field in &fields {
                    let ty = self.lower(&field.ty, args);
                    let cap = self.size_for(&ty, &[GCAP]);
                    size += cap;
                    made.push(Field {
                        name: field.name.clone(),
                        ty,
                        mutable: field.mutable,
                        cap,
                    });
                }
                let id = self.scope.classes.len();
                let applied = Some(Rc::clone(&key));
                self.scope.classes.push(Class {
                    fields: made,
                    size,
                    applied: applied.clone(),
                    inferable,
                });
                Ty::Class { id, size, applied }
            }
            Body::Enum(variants) => {
                let first = self.instance_variant(&variants[0], args, params);
                let least = 1 + first.fields.iter().map(Ty::floor).sum::<u64>();
                let id = self.scope.enums.len();
                let has_class = args.iter().any(|arg| self.scope.contains_class(arg));
                let applied = Some(Rc::clone(&key));
                self.scope.enums.push(Enum {
                    variants: vec![first],
                    least,
                    has_class,
                    applied: applied.clone(),
                });
                let ty = Ty::Enum { id, least, applied };
                // Recorded before the other variants, which can name this instance again.
                self.scope.instances.push((Rc::clone(&key), ty.clone()));
                for variant in &variants[1..] {
                    let made = self.instance_variant(variant, args, params);
                    self.scope.enums[id].variants.push(made);
                }
                return ty;
            }
        };
        self.scope.instances.push((key, ty.clone()));
        ty
    }

    fn instance_variant(&mut self, variant: &GVariant, args: &[Ty], params: usize) -> Variant {
        let made = Variant {
            name: variant.name.clone(),
            fields: variant.fields.iter().map(|f| self.lower(f, args)).collect(),
            labels: Vec::new(),
            recursive: variant.recursive,
            inferable: GTy::name_all(&variant.fields, params),
        };
        self.scope.note(&made);
        made
    }

    /// A generic function `fn h<T>(v: T, ...) -> R { e }`. The value `e` comes from the arguments,
    /// each used once at most, and from scalars, with arrays, optionals, instances of generics,
    /// calls of earlier generic functions, and a `match` on an argument that is a generic enum.
    fn generic_function(&mut self) {
        let params = 1 + self.pick(2);
        let name = self.scope.fresh("h");
        let enums = self.all_generic_enums();
        // Sometimes `(o: G<n><T>, d: T) -> T`, which matches `o` for a `T` and gives `d` for the
        // other variants.
        let unwrap = !enums.is_empty() && self.pick(4) == 0;
        let mut args: Vec<GTy> = if unwrap {
            let e = enums[self.pick(enums.len())];
            let second = if params == 2 {
                GTy::Param(1)
            } else {
                GTy::Plain(Ty::I64)
            };
            let targs = [GTy::Param(0), second];
            let n = self.scope.generics[e].params;
            let mut args = vec![GTy::Applied(e, targs[..n].to_vec()), GTy::Param(0)];
            if params == 2 && n == 1 {
                args.push(GTy::Param(1));
            }
            args
        } else {
            (0..params).map(|i| self.param_field(i)).collect()
        };
        for _ in 0..self.pick(2) {
            args.push(self.gsig(params, 1));
        }
        let names: Vec<String> = args.iter().map(|_| self.scope.fresh("v")).collect();
        let mut chosen = None;
        for attempt in 0..3 {
            // The last try returns an argument. It can always do that.
            let ret = match attempt {
                0 if unwrap => GTy::Param(0),
                0 | 1 => self.gsig(params, 2),
                _ => args[0].clone(),
            };
            let mut env: Env = names
                .iter()
                .zip(&args)
                .map(|(n, t)| (n.clone(), t.clone(), false))
                .collect();
            if let Some(solved) = self.solve(&ret, &mut env, 0) {
                chosen = Some((ret, solved));
                break;
            }
        }
        let (ret, solved) = chosen.expect("the argument itself should solve the last try");
        self.w.text(&format!("fn {name}"));
        self.tparams(params, self.scope.gfuncs.len());
        self.w.text("(");
        for (i, (n, t)) in names.iter().zip(&args).enumerate() {
            if i > 0 {
                self.w.text(", ");
            }
            self.w.text(&format!("{n}: {t}"));
        }
        self.w.text(&format!(") -> {ret} "));
        self.w.open();
        self.w.line();
        self.w.text(&solved.text);
        self.w.close();
        self.w.line();
        self.scope.gfuncs.push(GFunc {
            name,
            params,
            args,
            ret,
            cost: 1 + solved.cost,
            extra: solved.extra,
        });
    }

    /// A value of `target`, a type over the parameters of the function that is being written,
    /// from the unused arguments in `env`. When no rule can make one, it gives `None` and leaves
    /// `env` as it was. The rules, the smallest first, are: an argument, a scalar, an array or
    /// optional of a value, an instance, a call, and a `match`. A byte picks among the rules that
    /// can make the value.
    fn solve(&mut self, target: &GTy, env: &mut Env, nesting: u32) -> Option<Solved> {
        let unused = |env: &Env, keep: &dyn Fn(&GTy) -> bool| {
            env.iter().any(|(_, ty, used)| !used && keep(ty))
        };
        let mut options = Vec::new();
        if unused(env, &|ty| ty == target) {
            options.push(Source::Argument);
        }
        if matches!(target, GTy::Plain(_)) {
            options.push(Source::Scalar);
        }
        if matches!(target, GTy::Array(_) | GTy::Optional(_) | GTy::Applied(..)) {
            options.push(Source::Build);
        }
        if nesting < MAX_SOLVE {
            if !self.scope.gfuncs.is_empty() {
                options.push(Source::Call);
            }
            if unused(
                env,
                &|ty| matches!(ty, GTy::Applied(g, _) if self.scope.generic(*g).is_enum()),
            ) {
                options.push(Source::Match);
            }
        }
        while !options.is_empty() {
            let option = options.remove(self.pick(options.len()));
            let used: Vec<bool> = env.iter().map(|e| e.2).collect();
            let solved = match option {
                Source::Argument => Self::argument(target, env),
                Source::Scalar => self.scalar(target),
                Source::Build => self.build(target, env, nesting),
                Source::Call => self.gcall_in(target, env, nesting),
                Source::Match => self.destructure(target, env, nesting),
            };
            if solved.is_some() {
                return solved;
            }
            for (e, u) in env.iter_mut().zip(used) {
                e.2 = u;
            }
        }
        None
    }

    /// An unused argument of type `target`.
    fn argument(target: &GTy, env: &mut Env) -> Option<Solved> {
        let entry = env.iter_mut().find(|(_, ty, used)| ty == target && !used)?;
        entry.2 = true;
        Some(Solved {
            text: entry.0.clone(),
            extra: 0,
            cost: 0,
        })
    }

    /// A scalar literal. It has its own type in every position: a `u8` is `(u8(n) ?? n)`.
    fn scalar(&mut self, target: &GTy) -> Option<Solved> {
        let GTy::Plain(ty) = target else { return None };
        let text = match ty {
            Ty::I64 => self.int_literal().0,
            Ty::U8 => {
                let n = self.int(0, 255);
                format!("(u8({n}) ?? {n})")
            }
            Ty::F64 => self.float_literal(),
            Ty::Bool => if self.pick(2) == 0 { "false" } else { "true" }.to_string(),
            Ty::Char => CHARS[self.pick(CHARS.len())].0.to_string(),
            Ty::String => {
                let len = self.upto(GARG);
                format!("\"{}\"", self.string_text(len, 6))
            }
            _ => return None,
        };
        Some(Solved {
            text,
            extra: 0,
            cost: 0,
        })
    }

    /// `[a, b]`, `(if c { a } else { None })`, or an instance of a generic class or enum.
    fn build(&mut self, target: &GTy, env: &mut Env, nesting: u32) -> Option<Solved> {
        match target {
            GTy::Array(element) => {
                let mut parts = Vec::new();
                for _ in 0..=self.pick(2) {
                    parts.push(self.solve(element, env, nesting + 1)?);
                }
                Some(Solved {
                    text: format!(
                        "[{}]",
                        parts
                            .iter()
                            .map(|p| p.text.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    extra: parts.iter().map(|p| 1 + p.extra).sum(),
                    cost: parts.iter().map(|p| p.cost).sum(),
                })
            }
            GTy::Optional(payload) => {
                let value = self.solve(payload, env, nesting + 1)?;
                // An `if` over an optional takes one `?` layer, whatever its context expects.
                if matches!(**payload, GTy::Optional(_)) {
                    return Some(Solved {
                        text: format!("Some({})", value.text),
                        ..value
                    });
                }
                let cond = if self.pick(2) == 0 { "true" } else { "false" };
                Some(Solved {
                    text: format!("(if {cond} {{ {} }} else {{ None }})", value.text),
                    ..value
                })
            }
            GTy::Applied(g, args) => self.construct(*g, args, env, nesting),
            GTy::Param(_) | GTy::Plain(_) => None,
        }
    }

    /// `G<n><args>(g: a, ...)` or `G<n><args>.V(a, ...)`. Where the fields name every type
    /// parameter, it can also leave out the type arguments.
    fn construct(&mut self, g: usize, args: &[GTy], env: &mut Env, nesting: u32) -> Option<Solved> {
        let generic = self.scope.generic(g).clone();
        let (bare, typed) = (
            generic_name(g),
            format!("{}{}", generic_name(g), angled(args)),
        );
        let mut parts = Vec::new();
        let (variant, inferable) = match &generic.body {
            Body::Class(fields) => {
                for field in fields {
                    let part = self.solve(&field.ty.subst(args), env, nesting + 1)?;
                    parts.push((Some(field.name.as_str()), part));
                }
                (
                    None,
                    GTy::name_all(fields.iter().map(|f| &f.ty), generic.params),
                )
            }
            Body::Enum(variants) => {
                // A recursive variant comes only while the value is shallow, so the value ends.
                let mut viable: Vec<&GVariant> = variants
                    .iter()
                    .filter(|v| !v.recursive || nesting + 1 < MAX_SOLVE)
                    .collect();
                let mut found = None;
                while !viable.is_empty() {
                    let at = self.pick(viable.len());
                    let variant = viable.remove(at);
                    let used: Vec<bool> = env.iter().map(|e| e.2).collect();
                    let mut made = Vec::new();
                    let solved = variant.fields.iter().try_for_each(|f| {
                        made.push(self.solve(&f.subst(args), env, nesting + 1)?);
                        Some(())
                    });
                    if solved.is_some() {
                        found = Some((variant, made));
                        break;
                    }
                    for (e, u) in env.iter_mut().zip(used) {
                        e.2 = u;
                    }
                }
                let (variant, made) = found?;
                parts.extend(made.into_iter().map(|p| (None, p)));
                let fields = &variant.fields;
                (
                    Some(variant.name.as_str()),
                    !fields.is_empty() && GTy::name_all(fields, generic.params),
                )
            }
        };
        let leave_out = inferable && self.pick(2) == 0;
        let head = format!(
            "{}{}",
            if leave_out { &bare } else { &typed },
            variant.map_or(String::new(), |v| format!(".{v}"))
        );
        let text = if parts.is_empty() {
            head
        } else {
            let inner: Vec<String> = parts
                .iter()
                .map(|(name, p)| name.map_or(p.text.clone(), |n| format!("{n}: {}", p.text)))
                .collect();
            format!("{head}({})", inner.join(", "))
        };
        Some(Solved {
            text,
            extra: u64::from(generic.is_enum()) + parts.iter().map(|(_, p)| p.extra).sum::<u64>(),
            cost: parts.iter().map(|(_, p)| p.cost).sum(),
        })
    }

    /// A call of an earlier generic function that gives `target`.
    fn gcall_in(&mut self, target: &GTy, env: &mut Env, nesting: u32) -> Option<Solved> {
        let mut callees: Vec<usize> = (0..self.scope.gfuncs.len()).collect();
        while !callees.is_empty() {
            let f = callees.remove(self.pick(callees.len()));
            let func = self.scope.gfuncs[f].clone();
            let mut binds = vec![None; func.params];
            if !unify(&func.ret, target, &mut binds) {
                continue;
            }
            // A parameter that the result does not name is an `i64`.
            let targs: Vec<GTy> = binds
                .into_iter()
                .map(|b| b.unwrap_or(GTy::Plain(Ty::I64)))
                .collect();
            let used: Vec<bool> = env.iter().map(|e| e.2).collect();
            let mut parts = Vec::new();
            let all = func.args.iter().try_for_each(|a| {
                parts.push(self.solve(&a.subst(&targs), env, nesting + 1)?);
                Some(())
            });
            if all.is_none() {
                for (e, u) in env.iter_mut().zip(used) {
                    e.2 = u;
                }
                continue;
            }
            let written = if func.inferable() && self.pick(2) == 0 {
                String::new()
            } else {
                angled(&targs)
            };
            let inner: Vec<&str> = parts.iter().map(|p| p.text.as_str()).collect();
            return Some(Solved {
                text: format!("{}{written}({})", func.name, inner.join(", ")),
                extra: func.extra + parts.iter().map(|p| p.extra).sum::<u64>(),
                cost: func.cost + parts.iter().map(|p| p.cost).sum::<u64>(),
            });
        }
        None
    }

    /// `(match v { G<n>.V(_, p) => p; _ => d })`: a field of a variant of an unused argument `v`
    /// that is a generic enum. The value `d` for the other variants is made without `v`.
    fn destructure(&mut self, target: &GTy, env: &mut Env, nesting: u32) -> Option<Solved> {
        let mut candidates = Vec::new();
        for (i, (_, ty, used)) in env.iter().enumerate() {
            let (GTy::Applied(g, args), false) = (ty, *used) else {
                continue;
            };
            let generic = self.scope.generic(*g);
            let Body::Enum(variants) = &generic.body else {
                continue;
            };
            for variant in variants {
                for (k, field) in variant.fields.iter().enumerate() {
                    if field.subst(args) == *target {
                        candidates.push((i, *g, variant.clone(), k));
                    }
                }
            }
        }
        if candidates.is_empty() {
            return None;
        }
        let (i, g, variant, k) = candidates.swap_remove(self.pick(candidates.len()));
        env[i].2 = true;
        let rest = self.solve(target, env, nesting + 1)?;
        let fields: Vec<String> = (0..variant.fields.len())
            .map(|f| {
                if f == k {
                    "p".to_string()
                } else {
                    "_".to_string()
                }
            })
            .collect();
        Some(Solved {
            text: format!(
                "(match {} {{ {}.{}({}) => p; _ => {} }})",
                env[i].0,
                generic_name(g),
                variant.name,
                fields.join(", "),
                rest.text
            ),
            extra: rest.extra,
            cost: 1 + rest.cost,
        })
    }

    /// The generic functions that a call here can name, to give `ty` within `max`.
    pub(crate) fn gcallees(&self, ty: &Ty, max: u64) -> Vec<usize> {
        if *ty == Ty::Unit {
            return Vec::new();
        }
        let target = GTy::lift(ty);
        (0..self.scope.gfuncs.len())
            .filter(|&f| {
                let func = &self.scope.gfuncs[f];
                let mut binds = vec![None; func.params];
                unify(&func.ret, &target, &mut binds)
                    && binds.iter().flatten().all(|b| self.small(b))
                    && (self.scope.func.is_some()
                        || self.cost.times().saturating_mul(func.cost) <= self.budget)
                    && (!ty.sized() || max >= func.extra + func.args.len() as u64 * GARG)
            })
            .collect()
    }

    /// Whether the floor of a type argument is at most `ARG_FLOOR`.
    fn small(&self, gty: &GTy) -> bool {
        match self.closed(gty) {
            Some(ty) => ty.floor() <= ARG_FLOOR,
            None => false,
        }
    }

    /// The type of `gty`, which names no parameter, if its instances exist already.
    fn closed(&self, gty: &GTy) -> Option<Ty> {
        match gty {
            GTy::Param(_) => None,
            GTy::Plain(ty) => Some(ty.clone()),
            GTy::Array(element) => Some(Ty::Array(Box::new(self.closed(element)?))),
            GTy::Optional(payload) => Some(Ty::Optional(Box::new(self.closed(payload)?))),
            GTy::Applied(g, args) => {
                let args: Option<Vec<Ty>> = args.iter().map(|a| self.closed(a)).collect();
                let key = Applied {
                    generic: *g,
                    args: args?,
                };
                let found = self.scope.instances.iter().find(|(k, _)| **k == key);
                found.map(|(_, ty)| ty.clone())
            }
        }
    }

    /// `h(a, b)` or `h<A>(a, b)`: a call of a generic function that gives `ty`. The type
    /// arguments are written out, or left to be inferred where the arguments name them all. An
    /// argument then has its own type (`Gen::expr`).
    pub(crate) fn gcall(&mut self, ty: &Ty, max: u64) -> Info {
        let callees = self.gcallees(ty, max);
        let callee = callees[self.pick(callees.len())];
        let func = self.scope.gfuncs[callee].clone();
        let mut binds = vec![None; func.params];
        let matched = unify(&func.ret, &GTy::lift(ty), &mut binds);
        debug_assert!(matched, "the callee should have matched the type");
        let mut targs = Vec::new();
        for bound in binds {
            let gty = bound.unwrap_or_else(|| GTy::lift(&self.type_arg()));
            targs.push(self.lower(&gty, &[]));
        }
        if self.scope.func.is_none() {
            self.budget -= self.cost.times() * func.cost;
        }
        self.cost.generic_call(func.cost);
        // An argument of an optional of an optional has no value of its own type.
        let lowered: Vec<Ty> = func
            .args
            .iter()
            .map(|arg| self.lower(arg, &targs))
            .collect();
        let infer = func.inferable()
            && lowered.iter().all(|arg| arg.least() != u64::MAX)
            && self.pick(2) == 0;
        self.w.text(&func.name);
        if !infer {
            self.w.text(&angled(&targs));
        }
        self.w.text("(");
        let mut parts = Vec::new();
        for (j, arg) in lowered.into_iter().enumerate() {
            if j > 0 {
                self.w.text(", ");
            }
            // Without inference, an optional of an optional takes its floor.
            let least = Some(arg.least()).filter(|&least| least != u64::MAX);
            let room = if arg.sized() {
                least.unwrap_or(0).max(arg.floor()) + GARG
            } else {
                0
            };
            self.synth = infer;
            parts.push(self.expr(&arg, room));
        }
        self.w.text(")");
        let size = match ty {
            Ty::Class { .. } => ty.floor(),
            _ => (parts.iter().map(|p| p.size).sum::<u64>() + func.extra).min(max),
        };
        Info::of(&parts, size, false)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Plain,
    Class,
    Recursive,
    Wrapped,
    Late,
    Open,
}

#[derive(Clone, Copy)]
enum Source {
    Argument,
    Scalar,
    Build,
    Call,
    Match,
}

#[cfg(test)]
mod tests;
