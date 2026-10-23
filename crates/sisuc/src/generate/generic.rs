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
use super::scope::{Applied, Class, Enum, Field, Ty, Variant};

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
    /// The generic `Scope::generics[g]` with its type arguments. None is an optional (E0138).
    Applied(usize, Vec<GTy>),
}

impl fmt::Display for GTy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GTy::Param(i) => f.write_str(PARAMS[*i]),
            GTy::Plain(ty) => write!(f, "{ty}"),
            GTy::Array(element) => write!(f, "[{element}]"),
            GTy::Optional(payload) => write!(f, "{payload}?"),
            GTy::Applied(g, args) => write!(f, "G{g}{}", angled(args)),
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
/// stands for. An optional is no type argument (E0138).
fn unify(pattern: &GTy, target: &GTy, binds: &mut [Option<GTy>]) -> bool {
    match (pattern, target) {
        (GTy::Param(_), GTy::Optional(_)) => false,
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
        for g in 0..self.scope.generics.len() {
            for _ in 0..=self.pick(2) {
                let args: Vec<Ty> = (0..self.scope.generics[g].params)
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
        self.w.text(&format!("class G{id}{} ", tparams(params)));
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
        self.w.text(&format!("enum G{id}{} ", tparams(params)));
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

    /// A type argument: a scalar, a declared type or an instance, or an array of a scalar. It is
    /// never an optional (E0138). Its floor is at most `ARG_FLOOR`.
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
        let params = self.scope.generics[generic].params;
        let ty = match self.scope.generics[generic].body.clone() {
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
        Variant {
            name: variant.name.clone(),
            fields: variant.fields.iter().map(|f| self.lower(f, args)).collect(),
            labels: Vec::new(),
            recursive: variant.recursive,
            inferable: GTy::name_all(&variant.fields, params),
        }
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
        self.w.text(&format!("fn {name}{}(", tparams(params)));
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
                &|ty| matches!(ty, GTy::Applied(g, _) if self.scope.generics[*g].is_enum()),
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
        let generic = self.scope.generics[g].clone();
        let (bare, typed) = (format!("G{g}"), format!("G{g}{}", angled(args)));
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
            let Body::Enum(variants) = &self.scope.generics[*g].body else {
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
                "(match {} {{ G{g}.{}({}) => p; _ => {} }})",
                env[i].0,
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
        let infer = func.inferable() && self.pick(2) == 0;
        self.w.text(&func.name);
        if !infer {
            self.w.text(&angled(&targs));
        }
        self.w.text("(");
        let mut parts = Vec::new();
        for (j, arg) in func.args.iter().enumerate() {
            if j > 0 {
                self.w.text(", ");
            }
            let arg = self.lower(arg, &targs);
            let room = if arg.sized() {
                arg.least().max(arg.floor()) + GARG
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

/// `<T, U>` of `params` type parameters.
fn tparams(params: usize) -> String {
    format!("<{}>", PARAMS[..params].join(", "))
}

#[cfg(test)]
mod tests {
    use super::{Body, GField, GFunc, GTy, GVariant, Generic, Shape, unify};
    use crate::generate::scope::Ty;
    use crate::generate::{BUDGET, FUEL, Gen, MAX_NESTING, MAX_SIZE, STATEMENT_MOST, generate};
    use crate::tir::Rank;

    /// No generic function.
    const NONE: [usize; 0] = [];

    fn param(i: usize) -> GTy {
        GTy::Param(i)
    }

    fn plain(ty: Ty) -> GTy {
        GTy::Plain(ty)
    }

    fn array(inner: GTy) -> GTy {
        GTy::Array(Box::new(inner))
    }

    fn optional(inner: GTy) -> GTy {
        GTy::Optional(Box::new(inner))
    }

    fn field(name: &str, ty: GTy, mutable: bool) -> GField {
        GField {
            name: name.to_string(),
            ty,
            mutable,
        }
    }

    fn variant(name: &str, fields: Vec<GTy>, recursive: bool) -> GVariant {
        GVariant {
            name: name.to_string(),
            fields,
            recursive,
        }
    }

    /// `class G0<T, U> { g1: T  g2: [U]  g3: T? }`.
    fn pair_class() -> Generic {
        Generic {
            params: 2,
            body: Body::Class(vec![
                field("g1", param(0), false),
                field("g2", array(param(1)), true),
                field("g3", optional(param(0)), false),
            ]),
        }
    }

    /// `enum G<n><T> { V1  V2(T) }`.
    fn wrapper() -> Generic {
        Generic {
            params: 1,
            body: Body::Enum(vec![
                variant("V1", Vec::new(), false),
                variant("V2", vec![param(0)], false),
            ]),
        }
    }

    /// `fn h<T>(x: T) -> T`.
    fn identity(cost: u64) -> GFunc {
        GFunc {
            name: "h1".to_string(),
            params: 1,
            args: vec![param(0)],
            ret: param(0),
            cost,
            extra: 0,
        }
    }

    /// Declares the generics `generics` in `g`, in order, as the program text and the scope.
    fn declare(g: &mut Gen, generics: Vec<Generic>) {
        for (id, generic) in generics.into_iter().enumerate() {
            match generic.body {
                Body::Enum(variants) => g.declare_enum(id, generic.params, variants),
                Body::Class(_) => g.scope.generics.push(generic),
            }
        }
    }

    #[test]
    fn unify_should_bind_a_parameter_to_the_type_it_stands_for() {
        let pattern = GTy::Applied(0, vec![param(0), array(param(1))]);
        let target = GTy::Applied(0, vec![plain(Ty::I64), array(plain(Ty::String))]);
        let mut binds = vec![None, None];
        assert!(unify(&pattern, &target, &mut binds));
        assert_eq!(binds, [Some(plain(Ty::I64)), Some(plain(Ty::String))]);
    }

    #[test]
    fn unify_should_refuse_a_parameter_bound_to_two_types() {
        let pattern = GTy::Applied(0, vec![param(0), param(0)]);
        let same = GTy::Applied(0, vec![plain(Ty::I64), plain(Ty::I64)]);
        let other = GTy::Applied(0, vec![plain(Ty::I64), plain(Ty::U8)]);
        assert!(unify(&pattern, &same, &mut [None]));
        assert!(!unify(&pattern, &other, &mut [None]));
    }

    #[test]
    fn unify_should_refuse_an_optional_for_a_parameter() {
        // E0138: an optional is no type argument.
        assert!(!unify(&param(0), &optional(plain(Ty::I64)), &mut [None]));
        assert!(unify(
            &optional(param(0)),
            &optional(plain(Ty::I64)),
            &mut [None]
        ));
    }

    #[test]
    fn unify_should_refuse_another_generic_or_another_shape() {
        let target = GTy::Applied(0, vec![plain(Ty::I64)]);
        let mut binds = [None];
        assert!(!unify(
            &GTy::Applied(1, vec![param(0)]),
            &target,
            &mut binds
        ));
        assert!(!unify(&array(param(0)), &target, &mut binds));
        assert!(!unify(&plain(Ty::I64), &target, &mut binds));
        assert!(!unify(&plain(Ty::I64), &plain(Ty::U8), &mut binds));
        assert_eq!(binds, [None]);
    }

    #[test]
    fn lift_should_give_an_instance_its_generic_and_arguments() {
        let mut g = Gen::new(&[]);
        g.scope.generics.push(pair_class());
        let ty = g.instance(0, &[Ty::I64, Ty::String]);
        let lifted = GTy::lift(&Ty::Array(Box::new(Ty::Optional(Box::new(ty)))));
        let applied = GTy::Applied(0, vec![plain(Ty::I64), plain(Ty::String)]);
        assert_eq!(lifted, array(optional(applied)));
        assert_eq!(GTy::lift(&Ty::class(0, 9)), plain(Ty::class(0, 9)));
    }

    #[test]
    fn subst_should_put_the_arguments_in_every_position() {
        let ty = GTy::Applied(1, vec![param(1), array(param(0))]);
        let args = [plain(Ty::I64), plain(Ty::Bool)];
        let want = GTy::Applied(1, vec![plain(Ty::Bool), array(plain(Ty::I64))]);
        assert_eq!(optional(ty).subst(&args), optional(want));
        assert_eq!(plain(Ty::U8).subst(&args), plain(Ty::U8));
    }

    #[test]
    fn name_all_should_find_a_parameter_in_every_position() {
        let nested = GTy::Applied(0, vec![array(param(1))]);
        assert!(GTy::name_all([&param(0), &nested], 2));
        assert!(GTy::name_all([&optional(param(0))], 1));
        assert!(!GTy::name_all([&param(0), &plain(Ty::I64)], 2));
        assert!(!GTy::name_all([&array(param(1))], 2));
    }

    #[test]
    fn an_instance_should_put_its_arguments_into_its_fields() {
        let mut g = Gen::new(&[]);
        declare(&mut g, vec![pair_class()]);
        let ty = g.instance(0, &[Ty::I64, Ty::String]);
        assert_eq!(ty.to_string(), "G0<i64, string>");
        let class = &g.scope.classes[0];
        let types: Vec<String> = class.fields.iter().map(|f| f.ty.to_string()).collect();
        assert_eq!(types, ["i64", "[string]", "i64?"]);
        // The object, and the `[string]` field's room: `GCAP` beyond its floor.
        let caps: Vec<u64> = class.fields.iter().map(|f| f.cap).collect();
        assert_eq!(caps, [0, super::GCAP, 0]);
        assert_eq!(class.size, 1 + super::GCAP);
        let mutable: Vec<bool> = class.fields.iter().map(|f| f.mutable).collect();
        assert_eq!(mutable, [false, true, false]);
        assert_eq!(ty, g.scope.class_ty(0));
        // The same arguments name the same class; others, another.
        assert_eq!(g.instance(0, &[Ty::I64, Ty::String]), ty);
        assert_eq!(g.scope.classes.len(), 1);
        assert_ne!(g.instance(0, &[Ty::U8, Ty::String]), ty);
        assert_eq!(g.scope.classes.len(), 2);
    }

    #[test]
    fn an_enum_instance_should_name_itself_in_a_recursive_variant() {
        let mut g = Gen::new(&[]);
        declare(
            &mut g,
            vec![Generic {
                params: 1,
                body: Body::Enum(vec![
                    variant("V1", Vec::new(), false),
                    variant("V2", vec![GTy::Applied(0, vec![param(0)]), param(0)], true),
                ]),
            }],
        );
        let ty = g.instance(0, &[Ty::String]);
        let e = &g.scope.enums[0];
        assert_eq!(e.least, 1);
        assert_eq!(g.scope.enum_ty(0), ty);
        let [first, second] = &e.variants[..] else {
            panic!("two variants")
        };
        assert!(!first.recursive && first.fields.is_empty());
        assert!(second.recursive);
        assert_eq!(second.fields, [ty, Ty::String]);
        assert_eq!(g.scope.enums.len(), 1);
    }

    #[test]
    fn an_enum_instance_should_know_whether_it_may_hold_a_class() {
        let mut g = Gen::new(&[]);
        declare(&mut g, vec![wrapper()]);
        g.scope.classes.push(crate::generate::scope::Class {
            fields: Vec::new(),
            size: 1,
            applied: None,
            inferable: false,
        });
        let _ = g.instance(0, &[Ty::class(0, 1)]);
        let _ = g.instance(0, &[Ty::I64]);
        assert!(g.scope.enums[0].has_class);
        assert!(!g.scope.enums[1].has_class);
    }

    #[test]
    fn the_instances_of_an_open_pair_should_name_each_other() {
        let mut g = Gen::new(&[]);
        declare(&mut g, vec![wrapper()]);
        g.open_pair(&[0]);
        let m = g.instance(1, &[Ty::I64]);
        // `M<i64>` holds `G0<G2<i64>>`, which holds `N<i64>`, which holds `M<i64>` again.
        let names: Vec<String> = g
            .scope
            .instances
            .iter()
            .map(|(_, ty)| ty.to_string())
            .collect();
        assert_eq!(names.len(), 3, "{names:?}");
        assert!(names.contains(&"G0<G2<i64>>".to_string()), "{names:?}");
        let n = g
            .scope
            .instances
            .iter()
            .find(|(k, _)| k.generic == 2)
            .unwrap()
            .1
            .clone();
        let holds = |e: usize| g.scope.enums[e].variants[1].fields.clone();
        let (Ty::Enum { id: m_id, .. }, Ty::Enum { id: n_id, .. }) = (&m, &n) else {
            panic!("two enums")
        };
        assert_eq!(holds(*n_id), std::slice::from_ref(&m));
        assert!(g.scope.enums[*m_id].variants[1].recursive);
        assert!(g.scope.enums[*n_id].variants[1].recursive);
    }

    #[test]
    fn a_variant_should_be_inferable_when_its_fields_name_every_parameter() {
        let mut g = Gen::new(&[]);
        declare(&mut g, vec![wrapper()]);
        let _ = g.instance(0, &[Ty::I64]);
        let variants = &g.scope.enums[0].variants;
        assert!(!variants[0].inferable, "`V1` has no field");
        assert!(variants[1].inferable);
    }

    #[test]
    fn a_class_should_be_inferable_when_its_fields_name_every_parameter() {
        let mut g = Gen::new(&[]);
        g.scope.generics.push(pair_class());
        g.scope.generics.push(Generic {
            params: 2,
            body: Body::Class(vec![field("g1", param(0), false)]),
        });
        let first = g.instance(0, &[Ty::I64, Ty::I64]);
        let second = g.instance(1, &[Ty::I64, Ty::I64]);
        assert!(g.scope.classes[0].inferable);
        assert!(!g.scope.classes[1].inferable, "`U` is named nowhere");
        assert_eq!((first, second), (g.scope.class_ty(0), g.scope.class_ty(1)));
    }

    #[test]
    fn a_type_argument_should_be_small_and_no_optional() {
        for byte in 0..=u8::MAX {
            let data = [byte, 0, 0];
            let mut g = Gen::new(&data);
            g.scope.classes.push(crate::generate::scope::Class {
                fields: Vec::new(),
                size: 99,
                applied: None,
                inferable: false,
            });
            g.scope.types.push(Ty::class(0, 99));
            let ty = g.type_arg();
            assert!(ty.floor() <= super::ARG_FLOOR, "{ty}");
            assert!(!matches!(ty, Ty::Optional(_) | Ty::Unit), "{ty}");
        }
    }

    #[test]
    fn a_body_should_use_each_argument_once() {
        let mut g = Gen::new(&[]);
        let mut env = vec![("a".to_string(), param(0), false)];
        assert_eq!(g.solve(&param(0), &mut env, 0).unwrap().text, "a");
        assert!(g.solve(&param(0), &mut env, 0).is_none());
        assert!(env[0].2);
    }

    #[test]
    fn a_body_that_fails_should_leave_the_arguments_unused() {
        let mut g = Gen::new(&[]);
        // `G0<T, T>` needs two `T`s for its first and its second field; it has one.
        g.scope.generics.push(Generic {
            params: 1,
            body: Body::Class(vec![
                field("g1", param(0), false),
                field("g2", param(0), false),
            ]),
        });
        let mut env = vec![("a".to_string(), param(0), false)];
        let target = GTy::Applied(0, vec![param(0)]);
        assert!(g.solve(&target, &mut env, 0).is_none());
        assert!(!env[0].2);
    }

    #[test]
    fn a_body_should_write_a_scalar_that_is_of_its_own_type() {
        let mut g = Gen::new(&[3, 0, 5]);
        let text = g.scalar(&plain(Ty::U8)).unwrap().text;
        assert!(text.starts_with("(u8(") && text.contains(") ?? "), "{text}");
        assert!(g.scalar(&plain(Ty::class(0, 1))).is_none());
        assert!(g.scalar(&param(0)).is_none());
    }

    #[test]
    fn a_body_should_wrap_an_optional_and_an_array_around_a_value() {
        let mut g = Gen::new(&[]);
        let mut env = vec![
            ("a".to_string(), param(0), false),
            ("b".to_string(), param(0), false),
        ];
        let text = g.build(&optional(param(0)), &mut env, 0).unwrap().text;
        assert_eq!(text, "(if true { a } else { None })");
        let built = g.build(&array(param(0)), &mut env, 0).unwrap();
        assert_eq!(built.text, "[b]");
        assert_eq!(built.extra, 1);
    }

    #[test]
    fn a_body_should_match_a_generic_enum_for_a_field_and_fall_back() {
        let mut g = Gen::new(&[]);
        declare(&mut g, vec![wrapper()]);
        let mut env = vec![
            ("o".to_string(), GTy::Applied(0, vec![param(0)]), false),
            ("d".to_string(), param(0), false),
        ];
        let solved = g.destructure(&param(0), &mut env, 0).unwrap();
        assert_eq!(solved.text, "(match o { G0.V2(p) => p; _ => d })");
        assert_eq!(solved.cost, 1);
        // The enum is used, so it fails the second time.
        assert!(g.destructure(&param(0), &mut env, 0).is_none());
    }

    #[test]
    fn a_body_should_call_an_earlier_function_and_count_its_cost() {
        // The first byte picks the argument for the call's own; the second leaves the type
        // arguments out when 0, and writes them when 1.
        for (data, text) in [(&[0u8, 0][..], "h1(a)"), (&[0, 1][..], "h1<T>(a)")] {
            let mut g = Gen::new(data);
            g.scope.gfuncs.push(identity(4));
            let mut env = vec![("a".to_string(), param(0), false)];
            let solved = g.gcall_in(&param(0), &mut env, 0).unwrap();
            assert_eq!(solved.text, text);
            assert_eq!(solved.cost, 4);
            assert!(env[0].2);
        }
    }

    #[test]
    fn a_body_should_skip_a_function_whose_result_is_another_type() {
        let mut g = Gen::new(&[]);
        g.scope.gfuncs.push(GFunc {
            ret: array(param(0)),
            ..identity(1)
        });
        g.scope.gfuncs.push(GFunc {
            name: "h2".to_string(),
            ..identity(2)
        });
        let mut env = vec![("a".to_string(), param(0), false)];
        let solved = g.gcall_in(&param(0), &mut env, 0).unwrap();
        assert!(solved.text.starts_with("h2"), "{}", solved.text);
        assert_eq!(solved.cost, 2);
    }

    #[test]
    fn an_argument_should_be_used_once() {
        let mut env = vec![
            ("a".to_string(), param(0), true),
            ("b".to_string(), param(1), false),
        ];
        assert!(Gen::argument(&param(0), &mut env).is_none());
        assert!(Gen::argument(&param(1), &mut env).is_some());
        assert!(Gen::argument(&param(1), &mut env).is_none());
    }

    #[test]
    fn a_parameter_or_a_scalar_should_not_be_built() {
        let mut g = Gen::new(&[]);
        let mut env = vec![("a".to_string(), param(0), false)];
        assert!(g.build(&param(0), &mut env, 0).is_none());
        assert!(g.build(&plain(Ty::I64), &mut env, 0).is_none());
        assert!(!env[0].2);
    }

    #[test]
    fn a_type_with_a_parameter_should_name_no_closed_type() {
        let g = Gen::new(&[]);
        for ty in [
            param(0),
            array(param(0)),
            optional(param(0)),
            GTy::Applied(0, vec![param(0)]),
        ] {
            assert_eq!(g.closed(&ty), None, "{ty}");
            assert!(!g.small(&ty), "{ty}");
        }
        assert_eq!(
            g.closed(&array(plain(Ty::U8))),
            Some(Ty::Array(Box::new(Ty::U8)))
        );
        let optional_string = optional(plain(Ty::String));
        assert_eq!(
            g.closed(&optional_string),
            Some(Ty::Optional(Box::new(Ty::String)))
        );
        assert!(g.small(&plain(Ty::I64)));
    }

    #[test]
    fn a_type_argument_should_not_be_small_when_its_instance_does_not_exist() {
        let mut g = Gen::new(&[]);
        g.scope.generics.push(wrapper());
        let applied = GTy::Applied(0, vec![plain(Ty::I64)]);
        assert_eq!(g.closed(&applied), None);
        assert!(!g.small(&applied));
        let ty = g.instance(0, &[Ty::I64]);
        assert_eq!(g.closed(&applied), Some(ty));
        assert!(g.small(&applied));
    }

    #[test]
    fn a_call_in_main_should_need_the_budget_for_its_cost() {
        let mut g = Gen::new(&[]);
        g.scope.gfuncs.push(identity(4));
        g.budget = 3;
        assert_eq!(g.gcallees(&Ty::I64, 0), NONE);
        g.budget = 4;
        assert_eq!(g.gcallees(&Ty::I64, 0), [0]);
        // In a function the cost goes into the function's own.
        g.budget = 0;
        g.scope.func = Some(0);
        assert_eq!(g.gcallees(&Ty::I64, 0), [0]);
    }

    #[test]
    fn a_call_should_not_bind_a_parameter_to_a_large_type() {
        let mut g = Gen::new(&[]);
        g.scope.gfuncs.push(identity(1));
        assert_eq!(g.gcallees(&Ty::class(0, super::ARG_FLOOR), 100), [0]);
        assert_eq!(g.gcallees(&Ty::class(0, super::ARG_FLOOR + 1), 100), NONE);
    }

    #[test]
    fn a_call_should_need_room_for_the_size_it_adds_to_a_sized_result() {
        let mut g = Gen::new(&[]);
        g.scope.gfuncs.push(GFunc {
            name: "h1".to_string(),
            params: 1,
            args: vec![param(0)],
            ret: array(param(0)),
            cost: 1,
            extra: 10,
        });
        let ty = Ty::Array(Box::new(Ty::I64));
        assert_eq!(g.gcallees(&ty, 10 + super::GARG - 1), NONE);
        assert_eq!(g.gcallees(&ty, 10 + super::GARG), [0]);
        // A result without a size needs no room.
        g.scope.gfuncs[0].ret = param(0);
        assert_eq!(g.gcallees(&Ty::I64, 0), [0]);
        assert_eq!(g.gcallees(&Ty::Unit, 100), NONE);
    }

    #[test]
    fn a_call_should_write_the_type_arguments_it_cannot_leave_out() {
        // `fn h1<T>() -> [T]` names `T` in no argument.
        let mut g = Gen::new(&[]);
        g.scope.gfuncs.push(GFunc {
            name: "h1".to_string(),
            params: 1,
            args: Vec::new(),
            ret: array(param(0)),
            cost: 2,
            extra: 0,
        });
        let ty = Ty::Array(Box::new(Ty::String));
        let info = g.gcall(&ty, MAX_SIZE);
        assert_eq!(g.w.out.program, "h1<string>()");
        assert!(!info.pure);
        // In `main`, the cost comes out of the budget; it goes into the statements, too.
        assert_eq!(g.cost.statements, 2);
        assert_eq!(g.budget, BUDGET - FUEL * STATEMENT_MOST - 2);
    }

    #[test]
    fn a_call_should_leave_the_type_arguments_to_the_arguments_that_name_them() {
        // Byte 0 chooses to infer, and the argument is then of its own type.
        let mut g = Gen::new(&[]);
        g.scope.gfuncs.push(identity(1));
        g.gcall(&Ty::U8, MAX_SIZE);
        assert!(g.w.out.program.starts_with("h1("), "{}", g.w.out.program);
        assert!(g.w.out.program.contains("u8("), "{}", g.w.out.program);
        // Another byte writes them out, and the argument takes its type from them.
        let mut g = Gen::new(&[1]);
        g.scope.gfuncs.push(identity(1));
        g.gcall(&Ty::U8, MAX_SIZE);
        assert!(
            g.w.out.program.starts_with("h1<u8>("),
            "{}",
            g.w.out.program
        );
    }

    #[test]
    fn a_call_in_a_loop_should_cost_the_cost_of_the_callee_each_run() {
        let mut g = Gen::new(&[]);
        g.scope.gfuncs.push(identity(7));
        g.cost.enter_loop(3);
        g.gcall(&Ty::I64, MAX_SIZE);
        // Three runs of a call that costs 7, and a literal argument.
        assert_eq!(g.cost.statements, 21);
    }

    #[test]
    fn a_class_result_should_count_the_size_of_its_class() {
        let mut g = Gen::new(&[]);
        g.scope.generics.push(pair_class());
        let ty = g.instance(0, &[Ty::I64, Ty::I64]);
        g.scope.gfuncs.push(GFunc {
            name: "h1".to_string(),
            params: 2,
            args: vec![param(0), array(param(1))],
            ret: GTy::Applied(0, vec![param(0), param(1)]),
            cost: 1,
            extra: 0,
        });
        let info = g.gcall(&ty, MAX_SIZE);
        assert_eq!(info.size, ty.floor());
    }

    #[test]
    fn a_class_should_be_built_with_its_type_arguments_unless_they_can_be_left_out() {
        for (bytes, start) in [(&[0u8][..], "G0("), (&[1][..], "G0<i64, string>(")] {
            let mut g = Gen::new(bytes);
            g.scope.generics.push(pair_class());
            let ty = g.instance(0, &[Ty::I64, Ty::String]);
            g.object(&ty);
            assert!(g.w.out.program.starts_with(start), "{}", g.w.out.program);
        }
    }

    #[test]
    fn a_variant_should_leave_out_its_type_arguments_only_with_fields_that_name_them() {
        let mut g = Gen::new(&[]);
        declare(&mut g, vec![wrapper()]);
        let ty = g.instance(0, &[Ty::I64]);
        g.w.out.program.clear();
        // `V1` has no field, so its type arguments stay.
        g.variant(&ty, 1, None);
        assert_eq!(g.w.out.program, "G0<i64>.V1");
        let mut g = Gen::new(&[]);
        declare(&mut g, vec![wrapper()]);
        let ty = g.instance(0, &[Ty::I64]);
        g.w.out.program.clear();
        g.variant(&ty, 3, Some(0));
        assert!(g.w.out.program.starts_with("G0.V2("), "{}", g.w.out.program);
        let mut g = Gen::new(&[1]);
        declare(&mut g, vec![wrapper()]);
        let ty = g.instance(0, &[Ty::I64]);
        g.w.out.program.clear();
        g.variant(&ty, 3, Some(0));
        assert!(
            g.w.out.program.starts_with("G0<i64>.V2("),
            "{}",
            g.w.out.program
        );
    }

    #[test]
    fn a_class_should_leave_out_type_arguments_only_if_each_field_fits_its_own_type() {
        // `class G0<T> { g1: [T] }`: an array of its own type holds an element, so its
        // field needs room for one (`Ty::least`).
        for (cap, start) in [(0, "G0<i64>("), (1, "G0(")] {
            let mut g = Gen::new(&[]);
            g.scope.generics.push(Generic {
                params: 1,
                body: Body::Class(vec![field("g1", array(param(0)), false)]),
            });
            let ty = g.instance(0, &[Ty::I64]);
            g.scope.classes[0].fields[0].cap = cap;
            g.object(&ty);
            assert!(g.w.out.program.starts_with(start), "{}", g.w.out.program);
        }
    }

    #[test]
    fn a_variant_should_leave_out_type_arguments_only_if_its_fields_fit_their_own_types() {
        // `enum G0<T> { V1  V2([T]) }`: a `[T]` of its own type holds an element, so `V2`
        // needs room for it in the variant, beyond its own node.
        for (max, start) in [(1, "G0<i64>.V2("), (2, "G0.V2(")] {
            let mut g = Gen::new(&[]);
            declare(
                &mut g,
                vec![Generic {
                    params: 1,
                    body: Body::Enum(vec![
                        variant("V1", Vec::new(), false),
                        variant("V2", vec![array(param(0))], false),
                    ]),
                }],
            );
            let ty = g.instance(0, &[Ty::I64]);
            g.w.out.program.clear();
            g.variant(&ty, max, Some(0));
            assert!(g.w.out.program.starts_with(start), "{}", g.w.out.program);
        }
    }

    #[test]
    fn a_generic_type_should_be_written_as_the_program_writes_it() {
        let ty = GTy::Applied(1, vec![array(param(0)), plain(Ty::I64)]);
        assert_eq!(optional(ty).to_string(), "G1<[T], i64>?");
        assert_eq!(array(param(1)).to_string(), "[U]");
        assert_eq!(super::angled(&[Ty::I64, Ty::Bool]), "<i64, bool>");
        assert_eq!(super::tparams(1), "<T>");
        assert_eq!(super::tparams(2), "<T, U>");
    }

    #[test]
    fn unify_should_refuse_applied_types_of_other_arguments_or_other_counts() {
        let one = GTy::Applied(0, vec![plain(Ty::I64)]);
        let two = GTy::Applied(0, vec![plain(Ty::I64), plain(Ty::I64)]);
        let other = GTy::Applied(0, vec![plain(Ty::U8)]);
        assert!(!unify(&one, &two, &mut []));
        assert!(!unify(&two, &one, &mut []));
        assert!(!unify(&one, &other, &mut []));
        assert!(unify(&one, &one, &mut []));
    }

    #[test]
    fn the_generics_of_a_given_size_should_be_found_by_their_parameters_and_kind() {
        let mut g = Gen::new(&[]);
        declare(&mut g, vec![wrapper(), pair_class()]);
        let pair = Generic {
            params: 2,
            body: Body::Enum(vec![variant("V9", vec![param(0), param(1)], false)]),
        };
        g.declare_enum(2, 2, vec![variant("V9", vec![param(0), param(1)], false)]);
        assert_eq!(pair.params, 2);
        assert_eq!(g.generic_enums(1), [0]);
        assert_eq!(g.generic_enums(2), [2]);
        assert_eq!(g.all_generic_enums(), [0, 2]);
    }

    #[test]
    fn each_count_of_generics_should_reach_its_most_and_stay_within_it() {
        // Each generic type and each generic function spends one fuel. A generic class has 3
        // fields at most; a generic enum, 3 variants and 3 fields in a variant. A function has 2
        // type parameters, and 4 arguments: the 3 of `(o: G<n><T, U>, d: T, e: U)` and one more.
        let (mut types, mut functions, mut params, mut args) = (0, 0, 0, 0);
        let (mut fields, mut variants, mut payload, mut mutable) =
            (0, (usize::MAX, 0), 0, [false; 2]);
        for state in 0..256 {
            let data = crate::generate::coverage::seed(state);
            let mut g = Gen::new(&data);
            g.generics();
            functions = functions.max(g.scope.gfuncs.len());
            let spent = usize::try_from(FUEL - g.fuel).unwrap();
            types = types.max(spent - g.scope.gfuncs.len());
            for generic in &g.scope.generics {
                params = params.max(generic.params);
                match &generic.body {
                    Body::Class(list) => {
                        fields = fields.max(list.len());
                        for field in list {
                            mutable[usize::from(field.mutable)] = true;
                        }
                    }
                    Body::Enum(list) => {
                        variants = (variants.0.min(list.len()), variants.1.max(list.len()));
                        payload =
                            payload.max(list.iter().map(|v| v.fields.len()).max().unwrap_or(0));
                    }
                }
            }
            for func in &g.scope.gfuncs {
                params = params.max(func.params);
                args = args.max(func.args.len());
            }
        }
        assert_eq!(
            (
                types, functions, params, args, fields, variants, payload, mutable
            ),
            (5, 3, 2, 4, 3, (2, 3), 3, [true; 2])
        );
    }

    #[test]
    fn a_generic_enum_of_the_plain_shape_should_have_a_second_variant_now_and_then() {
        for (data, variants) in [(&[0u8; 8][..], 2), (&[1; 8][..], 3)] {
            let mut g = Gen::new(data);
            g.generic_enum(Shape::Plain, &[]);
            let Body::Enum(list) = &g.scope.generics[0].body else {
                panic!("an enum")
            };
            assert_eq!(list.len(), variants);
            assert!(list.iter().all(|v| !v.recursive));
        }
        let mut g = Gen::new(&[]);
        g.generic_enum(Shape::Recursive, &[]);
        let Body::Enum(list) = &g.scope.generics[0].body else {
            panic!("an enum")
        };
        assert_eq!(
            list.iter().map(|v| v.recursive).collect::<Vec<_>>(),
            [false, true]
        );
        // The enum itself is its first field, which a `while let` can walk.
        assert_eq!(list[1].fields[0], GTy::Applied(0, vec![param(0)]));
    }

    #[test]
    fn a_base_variant_should_hold_nothing_a_parameter_or_a_scalar() {
        let fields = |byte: u8| {
            let data = [byte, 1];
            let mut g = Gen::new(&data);
            g.base_variant(2).fields
        };
        assert_eq!(fields(0), []);
        assert_eq!(fields(1), [param(1)]);
        assert!(matches!(fields(2)[..], [GTy::Plain(_)]));
    }

    #[test]
    fn a_field_should_be_a_parameter_an_array_an_optional_a_scalar_or_a_generic_enum() {
        let mut seen = [false; 5];
        for byte in 0..=u8::MAX {
            let data = [byte; 4];
            let mut g = Gen::new(&data);
            declare(&mut g, vec![wrapper()]);
            match g.gfield(1, &[0]) {
                GTy::Param(_) => seen[0] = true,
                GTy::Array(_) => seen[1] = true,
                GTy::Optional(_) => seen[2] = true,
                GTy::Plain(_) => seen[3] = true,
                GTy::Applied(..) => seen[4] = true,
            }
        }
        assert_eq!(seen, [true; 5]);
        // With no generic enum to apply, a field is never one.
        for byte in 0..=u8::MAX {
            let data = [byte; 4];
            assert!(!matches!(Gen::new(&data).gfield(1, &[]), GTy::Applied(..)));
        }
    }

    #[test]
    fn a_parameter_field_should_name_its_parameter_alone_in_an_array_or_in_an_optional() {
        let fields: Vec<GTy> = (0..3)
            .map(|byte| {
                let data = [byte];
                Gen::new(&data).param_field(1)
            })
            .collect();
        assert_eq!(fields, [param(1), array(param(1)), optional(param(1))]);
    }

    #[test]
    fn an_applied_generic_should_take_a_parameter_an_array_of_one_or_a_scalar() {
        let mut seen = [false; 3];
        for byte in 0..=u8::MAX {
            let data = [byte; 6];
            let mut g = Gen::new(&data);
            declare(&mut g, vec![wrapper()]);
            let GTy::Applied(0, args) = g.applied(0, 2) else {
                panic!("an application of `G0`")
            };
            match &args[..] {
                [GTy::Param(i)] => {
                    assert!(*i < 2);
                    seen[0] = true;
                }
                [GTy::Array(inner)] => {
                    assert!(matches!(**inner, GTy::Param(i) if i < 2));
                    seen[1] = true;
                }
                [GTy::Plain(_)] => seen[2] = true,
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(seen, [true; 3]);
    }

    #[test]
    fn a_type_should_follow_the_choices_of_values_declared_types_instances_arrays_and_optionals() {
        // The choices, in order: the 6 values, a declared type, an instance, an array, an
        // optional. Each is present only when it can be made.
        for (declared, instances) in [(false, false), (true, false), (false, true), (true, true)] {
            for byte in 0..=u8::MAX {
                let data = [byte; 8];
                let mut kinds = Ty::VALUES.to_vec();
                let mut g = Gen::new(&data);
                if declared {
                    g.scope.types.push(Ty::class(0, 1));
                    kinds.push(Ty::class(0, 1));
                }
                if instances {
                    g.scope.generic_types.push(Ty::enumeration(1, 1));
                    kinds.push(Ty::enumeration(1, 1));
                }
                let choice = Gen::new(&data).pick(kinds.len() + 2);
                let ty = g.any_type();
                match kinds.get(choice) {
                    Some(kind) => assert_eq!(ty, *kind, "choice {choice}"),
                    None if choice == kinds.len() => assert!(matches!(ty, Ty::Array(_))),
                    None => assert!(matches!(ty, Ty::Optional(_))),
                }
            }
        }
    }

    #[test]
    fn a_declared_class_should_be_built_with_fields_typed_by_the_class() {
        // `u8` literals come bare, not as `(u8(n) ?? n)`, which only a generic class's fields
        // use.
        let mut g = Gen::new(&[]);
        g.scope.classes.push(crate::generate::scope::Class {
            fields: vec![crate::generate::scope::Field {
                name: "g1".to_string(),
                ty: Ty::U8,
                mutable: false,
                cap: 0,
            }],
            size: 1,
            applied: None,
            inferable: false,
        });
        g.object(&Ty::class(0, 1));
        assert!(
            g.w.out.program.starts_with("K0(g1: "),
            "{}",
            g.w.out.program
        );
        assert!(!g.w.out.program.contains("u8("), "{}", g.w.out.program);
    }

    #[test]
    fn a_generic_enum_value_should_end_in_a_variant_that_holds_no_enum() {
        // At the depth where a recursive variant is no longer allowed, only the other is.
        let recursive = vec![
            variant("V1", Vec::new(), false),
            variant("V2", vec![GTy::Applied(0, vec![param(0)])], true),
        ];
        for (nesting, start) in [
            (super::MAX_SOLVE - 1, "G0<i64>.V1"),
            (super::MAX_SOLVE - 2, "G0.V2("),
        ] {
            let mut g = Gen::new(&[1]);
            declare(
                &mut g,
                vec![Generic {
                    params: 1,
                    body: Body::Enum(recursive.clone()),
                }],
            );
            let mut env = Vec::new();
            let solved = g
                .construct(0, &[plain(Ty::I64)], &mut env, nesting)
                .unwrap();
            assert!(solved.text.starts_with(start), "{}", solved.text);
        }
    }

    #[test]
    fn a_construction_should_leave_out_its_type_arguments_only_for_fields_that_name_them() {
        let enum_text = |byte: u8, fields: Vec<GTy>| {
            let data = [byte];
            let mut g = Gen::new(&data);
            declare(
                &mut g,
                vec![Generic {
                    params: 1,
                    body: Body::Enum(vec![variant("V1", fields, false)]),
                }],
            );
            let mut env = vec![("a".to_string(), param(0), false)];
            let solved = g.construct(0, &[param(0)], &mut env, 0).unwrap();
            (solved.text, solved.extra)
        };
        assert_eq!(enum_text(0, vec![param(0)]), ("G0.V1(a)".to_string(), 1));
        assert_eq!(enum_text(1, vec![param(0)]), ("G0<T>.V1(a)".to_string(), 1));
        // No field, so no type parameter is named: they stay, and `a` stays unused.
        assert_eq!(enum_text(0, Vec::new()), ("G0<T>.V1".to_string(), 1));
        let class_text = |byte: u8| {
            let data = [byte];
            let mut g = Gen::new(&data);
            g.scope.generics.push(Generic {
                params: 1,
                body: Body::Class(vec![field("g1", param(0), false)]),
            });
            let mut env = vec![("a".to_string(), param(0), false)];
            let solved = g.construct(0, &[param(0)], &mut env, 0).unwrap();
            (solved.text, solved.extra)
        };
        assert_eq!(class_text(0), ("G0(g1: a)".to_string(), 0));
        assert_eq!(class_text(1), ("G0<T>(g1: a)".to_string(), 0));
    }

    #[test]
    fn a_body_should_offer_a_call_and_a_match_only_above_the_depth_that_ends_it() {
        let solved = |byte: u8, nesting: u32| {
            let data = [byte];
            let mut g = Gen::new(&data);
            declare(&mut g, vec![wrapper()]);
            g.scope.gfuncs.push(identity(1));
            let mut env = vec![
                ("o".to_string(), GTy::Applied(0, vec![param(0)]), false),
                ("d".to_string(), param(0), false),
            ];
            g.solve(&param(0), &mut env, nesting).unwrap().text
        };
        // At the depth limit, only the argument itself remains.
        for byte in 0..3 {
            assert_eq!(solved(byte, super::MAX_SOLVE), "d");
        }
        // Above it, a byte picks the argument, the call or the `match`.
        assert_eq!(solved(0, 0), "d");
        assert!(solved(1, 0).starts_with("h1"), "{}", solved(1, 0));
        assert!(solved(2, 0).starts_with("(match o"), "{}", solved(2, 0));
    }

    #[test]
    fn a_body_should_not_offer_a_match_on_an_argument_that_is_a_class() {
        let mut g = Gen::new(&[1]);
        declare(&mut g, vec![pair_class()]);
        let mut env = vec![
            (
                "o".to_string(),
                GTy::Applied(0, vec![param(0), param(1)]),
                false,
            ),
            ("d".to_string(), param(0), false),
        ];
        let before = g.u.len();
        assert_eq!(g.solve(&param(0), &mut env, 0).unwrap().text, "d");
        // One option leaves nothing to pick, so the byte stays.
        assert_eq!(g.u.len(), before);
    }

    #[test]
    fn a_scalar_should_be_written_as_its_literal() {
        let text = |ty: Ty, byte: u8| {
            let data = [byte, 3, 2, 1];
            let mut g = Gen::new(&data);
            g.scalar(&plain(ty)).unwrap().text
        };
        assert_eq!(text(Ty::I64, 0), "3");
        assert_eq!(text(Ty::U8, 0), "(u8(0) ?? 0)");
        assert_eq!(text(Ty::Bool, 0), "false");
        assert_eq!(text(Ty::Bool, 1), "true");
        assert_eq!(text(Ty::Char, 0), "'a'");
        assert!(text(Ty::F64, 0).starts_with(|c: char| c.is_ascii_digit() || c == 'f'));
        let string = text(Ty::String, 2);
        assert!(string.starts_with('"') && string.ends_with('"'), "{string}");
        assert!(string.len() - 2 <= 4, "{string}");
    }

    #[test]
    fn an_optional_and_an_array_should_come_in_their_two_forms() {
        let make = |byte: u8, target: GTy| {
            let data = [byte, 1];
            let mut g = Gen::new(&data);
            let mut env = vec![
                ("a".to_string(), param(0), false),
                ("b".to_string(), param(0), false),
            ];
            let solved = g.build(&target, &mut env, 0).unwrap();
            (solved.text, solved.extra)
        };
        assert_eq!(
            make(0, optional(param(0))).0,
            "(if true { a } else { None })"
        );
        assert_eq!(
            make(1, optional(param(0))).0,
            "(if false { a } else { None })"
        );
        // An array of one element takes the first byte as 0, and of two as 1.
        assert_eq!(make(0, array(param(0))), ("[a]".to_string(), 1));
        assert_eq!(make(1, array(param(0))), ("[a, b]".to_string(), 2));
    }

    #[test]
    fn a_generic_function_should_be_written_with_its_type_parameters_and_arguments() {
        // Byte by byte: one type parameter; the argument `T` and no other; the result `T`.
        let mut g = Gen::new(&[]);
        g.generic_function();
        assert_eq!(g.w.out.program, "fn h1<T>(v2: T) -> T {\n    v2\n}\n");
        // Two type parameters, an argument for each, the result the first.
        let mut g = Gen::new(&[1]);
        g.generic_function();
        assert_eq!(
            g.w.out.program,
            "fn h1<T, U>(v2: T, v3: U) -> T {\n    v2\n}\n"
        );
        assert_eq!(g.scope.gfuncs[0].cost, 1);
        assert_eq!(g.scope.gfuncs[0].params, 2);
        assert_eq!(g.scope.gfuncs[0].args, [param(0), param(1)]);
        assert_eq!(g.scope.gfuncs[0].ret, param(0));
    }

    #[test]
    fn a_function_over_an_enum_and_a_parameter_should_match_the_enum_for_the_parameter() {
        // Byte by byte: two type parameters; the unwrap form (0 of 4); no extra argument; and
        // the `match`, the second of two ways to make a `T`.
        let mut g = Gen::new(&[1, 0, 0, 1]);
        declare(&mut g, vec![wrapper()]);
        g.w.out.program.clear();
        g.generic_function();
        let text = &g.w.out.program;
        assert!(
            text.starts_with("fn h1<T, U>(v2: G0<T>, v3: T, v4: U) -> T {"),
            "{text}"
        );
        assert!(
            text.contains("(match v2 { G0.V2(p) => p; _ => v3 })"),
            "{text}"
        );
    }

    #[test]
    fn a_match_should_leave_the_other_fields_of_its_variant_unnamed() {
        let mut g = Gen::new(&[]);
        g.declare_enum(
            0,
            1,
            vec![
                variant("V1", Vec::new(), false),
                variant("V2", vec![plain(Ty::I64), param(0), plain(Ty::Bool)], false),
            ],
        );
        let mut env = vec![
            ("o".to_string(), GTy::Applied(0, vec![param(0)]), false),
            ("d".to_string(), param(0), false),
        ];
        let solved = g.destructure(&param(0), &mut env, 0).unwrap();
        assert_eq!(solved.text, "(match o { G0.V2(_, p, _) => p; _ => d })");
    }

    #[test]
    fn a_call_should_skip_a_function_that_does_not_give_the_type() {
        let mut g = Gen::new(&[]);
        g.scope.gfuncs.push(GFunc {
            ret: array(param(0)),
            ..identity(1)
        });
        assert_eq!(g.gcallees(&Ty::I64, 0), NONE);
        g.scope.gfuncs.push(identity(1));
        assert_eq!(g.gcallees(&Ty::I64, 0), [1]);
    }

    #[test]
    fn a_call_should_separate_its_arguments_with_commas() {
        let mut g = Gen::new(&[]);
        g.scope.gfuncs.push(GFunc {
            args: vec![param(0), param(0)],
            ..identity(1)
        });
        g.gcall(&Ty::I64, MAX_SIZE);
        assert_eq!(g.w.out.program, "h1(0, 0)");
    }

    #[test]
    fn a_sized_result_should_hold_the_nodes_it_adds() {
        let mut g = Gen::new(&[]);
        g.scope.gfuncs.push(GFunc {
            ret: array(param(0)),
            extra: 3,
            ..identity(1)
        });
        // The argument is a `string` of no chars, and the call adds 3.
        assert_eq!(g.gcall(&Ty::Array(Box::new(Ty::String)), MAX_SIZE).size, 3);
    }

    #[test]
    fn a_type_argument_should_have_a_floor_of_the_most_and_no_more() {
        let mut found = [false; 2];
        for byte in 0..=u8::MAX {
            let data = [byte, 0, 0];
            let mut g = Gen::new(&data);
            g.scope.types.push(Ty::class(0, super::ARG_FLOOR));
            g.scope.types.push(Ty::class(1, super::ARG_FLOOR + 1));
            match g.type_arg() {
                Ty::Class { id: 0, .. } => found[0] = true,
                Ty::Class { id: 1, .. } => found[1] = true,
                _ => {}
            }
        }
        assert_eq!(found, [true, false]);
    }

    #[test]
    fn an_enum_instance_should_count_its_first_variant_as_its_least_value() {
        let mut g = Gen::new(&[]);
        g.scope.classes.push(crate::generate::scope::Class {
            fields: Vec::new(),
            size: 5,
            applied: None,
            inferable: false,
        });
        g.declare_enum(
            0,
            1,
            vec![
                variant("V1", vec![param(0), plain(Ty::I64)], false),
                variant("V2", Vec::new(), false),
            ],
        );
        let ty = g.instance(0, &[Ty::class(0, 5)]);
        // The node, and the object.
        assert!(matches!(ty, Ty::Enum { least: 6, .. }), "{ty:?}");
        assert_eq!(g.scope.enums[0].least, 6);
    }

    #[test]
    fn a_value_of_a_recursive_enum_should_take_the_recursive_variant_only_while_fuel_lasts() {
        // Byte 1 picks the second of two variants when both can come.
        for (fuel, nesting, start) in [
            (0, 0, "G0<i64>.V1"),
            (1, MAX_NESTING, "G0<i64>.V1"),
            (1, MAX_NESTING - 1, "G0.V2("),
        ] {
            let mut g = Gen::new(&[1]);
            declare(
                &mut g,
                vec![Generic {
                    params: 1,
                    body: Body::Enum(vec![
                        variant("V1", Vec::new(), false),
                        variant("V2", vec![GTy::Applied(0, vec![param(0)])], true),
                    ]),
                }],
            );
            let ty = g.instance(0, &[Ty::I64]);
            g.w.out.program.clear();
            g.fuel = fuel;
            g.nesting = nesting;
            g.variant(&ty, 10, None);
            assert!(g.w.out.program.starts_with(start), "{}", g.w.out.program);
        }
    }

    #[test]
    fn a_pattern_should_name_a_generic_enum_without_its_type_arguments() {
        let mut g = Gen::new(&[1]);
        declare(&mut g, vec![wrapper()]);
        let ty = g.instance(0, &[Ty::I64]);
        let (pattern, _) = g.variant_pattern(&ty, 0, None, 0);
        assert_eq!(pattern, "G0.V1");
        assert_eq!(ty.bare(), "G0");
        assert_eq!(Ty::enumeration(3, 1).bare(), "E3");
    }

    #[test]
    fn no_bytes_should_make_no_generic() {
        for data in [&[][..], &[0; 64][..]] {
            let program = generate(data).program;
            assert!(
                !program.contains("G0") && !program.contains("fn h"),
                "{program}"
            );
        }
    }

    #[test]
    fn a_type_should_be_an_instance_as_often_as_a_declared_type() {
        // One byte picks the type; the others, with an array or optional, pick the rest.
        let mut counts = [0; 3];
        for byte in 0..=u8::MAX {
            let data = [byte; 8];
            let mut g = Gen::new(&data);
            g.scope.types.push(Ty::class(0, 1));
            g.scope.generic_types.push(Ty::enumeration(1, 1));
            match g.any_type() {
                Ty::Class { .. } => counts[0] += 1,
                Ty::Enum { .. } => counts[1] += 1,
                _ => counts[2] += 1,
            }
        }
        assert!(counts[0] > 0 && counts[1] > 0, "{counts:?}");
        assert_eq!(counts[0], counts[1], "{counts:?}");
    }

    /// The `tir::Rank` of each payload field of each enum of `source`, which must check.
    fn ranks(source: &str) -> Vec<(String, Vec<Rank>)> {
        let (program, diagnostics) = crate::front_end::checked(source);
        let program = program.unwrap_or_else(|| panic!("{diagnostics:?}\n{source}"));
        program
            .enums
            .iter()
            .map(|e| {
                let fields = e.variants.iter().flat_map(|v| &v.fields);
                (e.name.clone(), fields.map(|p| p.rank).collect())
            })
            .collect()
    }

    #[test]
    fn each_shape_should_give_the_rank_it_is_for() {
        let program = |build: &dyn Fn(&mut Gen)| {
            let mut g = Gen::new(&[]);
            declare(&mut g, vec![wrapper()]);
            build(&mut g);
            g.w.text("\nfn main() {}\n");
            ranks(&g.w.out.program)
        };
        // `G0` is `enum G0<T> { V1  V2(T) }`; its field is a `Never`.
        let recursive = program(&|g| g.generic_enum(Shape::Recursive, &[0]));
        assert_eq!(recursive[0], ("G0".to_string(), vec![Rank::Never]));
        assert!(recursive[1].1.contains(&Rank::Own), "{recursive:?}");
        let wrapped = program(&|g| g.generic_enum(Shape::Wrapped, &[0]));
        assert!(wrapped[1].1.contains(&Rank::Own), "{wrapped:?}");
        let open = program(&|g| g.open_pair(&[0]));
        assert_eq!(open[1].1, [Rank::Open], "{open:?}");
        assert_eq!(open[2].1, [Rank::Never, Rank::Open], "{open:?}");
        let plain = program(&|g| g.generic_enum(Shape::Plain, &[0]));
        assert!(!plain[1].1.contains(&Rank::Own), "{plain:?}");
    }

    #[test]
    fn a_pair_should_give_a_late_field() {
        let mut g = Gen::new(&[]);
        let pair = Generic {
            params: 2,
            body: Body::Enum(vec![variant("V1", vec![param(0), param(1)], false)]),
        };
        declare(&mut g, vec![pair]);
        g.generic_enum(Shape::Late, &[0]);
        g.w.text("\nfn main() {}\n");
        let ranks = ranks(&g.w.out.program);
        assert_eq!(ranks[1].1, [Rank::Late], "{ranks:?}");
    }
}
