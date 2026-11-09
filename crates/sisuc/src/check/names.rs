//! The top-level names of a program, and what each one stands for.
//!
//! Functions, classes, enums, interfaces, constants and aliases share one namespace. `NameMaps`
//! holds one map for each kind, and its fields are private: a reader asks a getter, so that a
//! later change of the maps touches this module alone.

use rustc_hash::{FxHashMap, FxHashSet};

use super::enums::reserved_name;
use super::prelude::{self, PRELUDE};
use super::{Binding, Body, Checker, Declared, Definition, built_in_type, is_language_interface};
use crate::diagnostic::DiagnosticKind;
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, Type};

/// The place of a constant among `Checker::const_infos`, which is its place in the source among
/// the constants.
#[derive(Clone, Copy)]
pub(super) struct ConstantId(pub(super) usize);

/// Where a program writes a name. Each position tries the meanings of a name in its own order.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Position {
    /// A read of the name.
    Value,
    /// The name of a call, `N(args)`.
    Callee,
    /// A written type.
    Type,
    /// A bound, an inheritance list, a conformance header or an `any`.
    Bound,
    /// A name in a pattern.
    Pattern,
    /// The base of a path, `N.x`.
    Path,
    /// The base of a path call, `N.x(args)`.
    PathCall,
}

/// A place where a name is written.
#[derive(Clone, Copy)]
pub(super) struct Site {
    pub(super) at: Position,
    /// Whether type arguments follow the name.
    pub(super) angles: bool,
    /// Whether a read of the name must not count as a use: the caller is the quiet value.
    pub(super) quiet: bool,
}

impl Site {
    /// The site at `at`, with type arguments or not.
    pub(super) fn new(at: Position, angles: bool) -> Self {
        Self {
            at,
            angles,
            quiet: false,
        }
    }

    /// A read of a name, which is `quiet` where the caller is (`Body::quiet`).
    pub(super) fn value(quiet: bool) -> Self {
        Self {
            at: Position::Value,
            angles: false,
            quiet,
        }
    }
}

/// The kind of a top-level item.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ItemKind {
    Function,
    Class,
    Enum,
    Interface,
    Constant,
    Alias,
}

/// A top-level item that is intact.
#[derive(Clone, Copy)]
pub(super) enum ItemId {
    Function(tir::FuncId),
    Class(tir::ClassId),
    Enum(tir::EnumId),
    Interface(tir::InterfaceId),
    /// A place in `Aliases::infos`.
    Alias(usize),
}

/// What a name stands for at a site.
#[derive(Clone, Copy)]
pub(super) enum Resolved {
    /// A binding of an open scope. `ty` is `None` for `Error`. `hidden` is the kind of the
    /// poisoned item that a pattern made this local to hide.
    Local {
        local: tir::LocalId,
        ty: Option<Type>,
        #[expect(
            dead_code,
            reason = "no position tells a hidden item from a plain local yet"
        )]
        hidden: Option<ItemKind>,
    },
    Constant(ConstantId),
    /// The type parameter at this place of `Body::type_params`.
    TypeParam(usize),
    AssocType,
    Item(ItemId),
    BuiltinType(Type),
    /// `Option`, which makes no enum.
    OptionType,
    BuiltinFunction,
    /// `Some`, `Ok` or `Err`, which a program writes without the enum.
    PreludeVariant,
    /// A rejected or broken item has the name, and reports its own error.
    Poisoned(
        #[expect(
            dead_code,
            reason = "no position reads the kind of a poisoned item yet"
        )]
        ItemKind,
    ),
    NotFound,
}

/// The maps from a top-level name to the item that it stands for. Each map holds the first intact
/// item of a name, and every broken or rejected one.
#[derive(Default)]
pub(super) struct NameMaps {
    functions: FxHashMap<Symbol, Declared<tir::FuncId>>,
    classes: FxHashMap<Symbol, Declared<tir::ClassId>>,
    enums: FxHashMap<Symbol, Declared<tir::EnumId>>,
    interfaces: FxHashMap<Symbol, Declared<tir::InterfaceId>>,
    constants: FxHashMap<Symbol, Declared<ConstantId>>,
    /// Indexed by the place in `Aliases::infos`.
    aliases: FxHashMap<Symbol, Declared<usize>>,
    /// The language interfaces that a program declares twice. `poison` keeps their names out of
    /// `interfaces`, so a bound, a header and an `any` read the prelude's interface. This set tells
    /// every other position that the name is poisoned.
    language_poisoned: FxHashSet<Symbol>,
}

impl NameMaps {
    /// Records `id` as the function of `name`.
    pub(super) fn add_function(&mut self, name: Symbol, id: tir::FuncId) {
        self.functions.insert(name, Declared::Known(id));
    }

    /// Records `id` as the class of `name`.
    pub(super) fn add_class(&mut self, name: Symbol, id: tir::ClassId) {
        self.classes.insert(name, Declared::Known(id));
    }

    /// Records `id` as the enum of `name`.
    pub(super) fn add_enum(&mut self, name: Symbol, id: tir::EnumId) {
        self.enums.insert(name, Declared::Known(id));
    }

    /// Records `id` as the interface of `name`.
    pub(super) fn add_interface(&mut self, name: Symbol, id: tir::InterfaceId) {
        self.interfaces.insert(name, Declared::Known(id));
    }

    /// Records `id` as the constant of `name`.
    pub(super) fn add_constant(&mut self, name: Symbol, id: ConstantId) {
        self.constants.insert(name, Declared::Known(id));
    }

    /// Records `id`, a place in `Aliases::infos`, as the alias of `name`.
    pub(super) fn add_alias(&mut self, name: Symbol, id: usize) {
        self.aliases.insert(name, Declared::Known(id));
    }

    /// Poisons the top-level name `name` of an item of `kind` that is rejected or broken: its
    /// uses give no more errors. The kinds share a namespace, so an item of another kind with
    /// this name is poisoned too.
    pub(super) fn poison(&mut self, kind: Definition, name: Symbol) {
        // The prelude's code reads a language interface by its name. No rejected or broken item
        // takes it, as no intact item of the user's does.
        let language = is_language_interface(name.as_str());
        match kind {
            Definition::Function => {
                self.functions.insert(name, Declared::Broken);
            }
            Definition::Class => {
                self.classes.insert(name, Declared::Broken);
            }
            Definition::Interface => {
                if !language {
                    self.interfaces.insert(name, Declared::Broken);
                }
            }
            Definition::Enum => {
                self.enums.insert(name, Declared::Broken);
            }
            Definition::Constant => {
                self.constants.insert(name, Declared::Broken);
            }
            Definition::Alias => {
                self.aliases.insert(name, Declared::Broken);
            }
            Definition::Field
            | Definition::Method
            | Definition::Binding
            | Definition::Type
            | Definition::Variant
            | Definition::LoopLabel => {}
        }
        if !language && let Some(interface) = self.interfaces.get_mut(&name) {
            *interface = Declared::Broken;
        }
        if let Some(class) = self.classes.get_mut(&name) {
            *class = Declared::Broken;
        }
        if let Some(e) = self.enums.get_mut(&name) {
            *e = Declared::Broken;
        }
        if let Some(function) = self.functions.get_mut(&name) {
            *function = Declared::Broken;
        }
        if let Some(constant) = self.constants.get_mut(&name) {
            *constant = Declared::Broken;
        }
        if let Some(alias) = self.aliases.get_mut(&name) {
            *alias = Declared::Broken;
        }
    }

    /// Poisons `name` at every position but a bound when it is a language interface's name. A
    /// program declares it twice, or declares it and another item of the name.
    pub(super) fn poison_language(&mut self, name: Symbol) {
        if is_language_interface(name.as_str()) {
            self.language_poisoned.insert(name);
        }
    }

    /// What `name` stands for as a function, if it is the name of one.
    pub(super) fn function(&self, name: Symbol) -> Option<Declared<tir::FuncId>> {
        self.functions.get(&name).copied()
    }

    /// What `name` stands for as a class, if it is the name of one.
    pub(super) fn class(&self, name: Symbol) -> Option<Declared<tir::ClassId>> {
        self.classes.get(&name).copied()
    }

    /// What `name` stands for as an enum, if it is the name of one.
    pub(super) fn enum_id(&self, name: Symbol) -> Option<Declared<tir::EnumId>> {
        self.enums.get(&name).copied()
    }

    /// What `name` stands for as an interface, if it is the name of one.
    pub(super) fn interface(&self, name: Symbol) -> Option<Declared<tir::InterfaceId>> {
        self.interfaces.get(&name).copied()
    }

    /// What `name` stands for as a constant, if it is the name of one.
    pub(super) fn constant(&self, name: Symbol) -> Option<Declared<ConstantId>> {
        self.constants.get(&name).copied()
    }

    /// What `name` stands for as an alias, if it is the name of one.
    pub(super) fn alias(&self, name: Symbol) -> Option<Declared<usize>> {
        self.aliases.get(&name).copied()
    }

    /// The names of the functions, in no fixed order.
    pub(super) fn function_names(&self) -> impl Iterator<Item = Symbol> + '_ {
        self.functions.keys().copied()
    }

    /// The names of the classes, in no fixed order.
    pub(super) fn class_names(&self) -> impl Iterator<Item = Symbol> + '_ {
        self.classes.keys().copied()
    }

    /// The names of the enums, in no fixed order.
    pub(super) fn enum_names(&self) -> impl Iterator<Item = Symbol> + '_ {
        self.enums.keys().copied()
    }

    /// The names of the interfaces, in no fixed order.
    pub(super) fn interface_names(&self) -> impl Iterator<Item = Symbol> + '_ {
        self.interfaces.keys().copied()
    }

    /// The names of the aliases, in no fixed order.
    pub(super) fn alias_names(&self) -> impl Iterator<Item = Symbol> + '_ {
        self.aliases.keys().copied()
    }

    /// The constants that are intact, with their names, in no fixed order. A rejected or broken
    /// constant is not one.
    pub(super) fn known_constants(&self) -> impl Iterator<Item = (Symbol, ConstantId)> + '_ {
        self.constants
            .iter()
            .filter_map(|(&name, &declared)| match declared {
                Declared::Known(id) => Some((name, id)),
                Declared::Broken => None,
            })
    }
}

impl Body {
    /// Where the type parameter `name` is in `type_params`, if it is one.
    pub(super) fn type_param(&self, name: Symbol) -> Option<usize> {
        #[cfg(test)]
        super::PARAM_STEPS.set(super::PARAM_STEPS.get() + 1);
        self.type_param_at.get(&name).copied()
    }
}

impl Checker<'_> {
    /// The kind of the rejected or broken item that poisons the top-level name `name`, so that a
    /// use of it gives no more errors.
    fn poison_kind(&self, name: Symbol) -> Option<ItemKind> {
        let kinds = [
            (
                matches!(self.names.class(name), Some(Declared::Broken)),
                ItemKind::Class,
            ),
            (
                matches!(self.names.function(name), Some(Declared::Broken)),
                ItemKind::Function,
            ),
            (
                matches!(self.names.enum_id(name), Some(Declared::Broken)),
                ItemKind::Enum,
            ),
            (
                matches!(self.names.interface(name), Some(Declared::Broken)),
                ItemKind::Interface,
            ),
            (
                matches!(self.names.constant(name), Some(Declared::Broken)),
                ItemKind::Constant,
            ),
            (self.alias_error(name), ItemKind::Alias),
            (
                self.names.language_poisoned.contains(&name),
                ItemKind::Interface,
            ),
        ];
        kinds
            .into_iter()
            .find_map(|(poisoned, kind)| poisoned.then_some(kind))
    }

    /// Whether a rejected or broken item poisons the top-level name `name`, so that a use of it
    /// gives no more errors.
    pub(super) fn poisoned(&self, name: Symbol) -> bool {
        self.poison_kind(name).is_some()
    }

    /// `poison_kind` for a call or a read of the value `name`. `Ok` and `Err`, when no user's
    /// function or class takes them, belong to the prelude's `Result`.
    fn poison_kind_use(&self, name: Symbol) -> Option<ItemKind> {
        self.poison_kind(name).or_else(|| {
            ((name == Symbol::OK || name == Symbol::ERR)
                && self.names.function(name).is_none()
                && self.names.class(name).is_none()
                && matches!(self.names.enum_id(Symbol::RESULT), Some(Declared::Broken)))
            .then_some(ItemKind::Enum)
        })
    }

    /// Whether a call or a read of the value `name` is poisoned.
    pub(super) fn poisoned_use(&self, name: Symbol) -> bool {
        self.poison_kind_use(name).is_some()
    }

    /// The binding of `name` in the innermost open scope that has one.
    fn binding(&self, name: Symbol) -> Option<&Binding> {
        #[cfg(test)]
        super::BINDING_STEPS.set(super::BINDING_STEPS.get() + 1);
        (self.body.scopes.iter().rev()).find_map(|scope| scope.get(&name))
    }

    /// The answer `Local` for `name`, bound as `b`.
    fn local_answer(&self, name: Symbol, b: &Binding) -> Resolved {
        self.local_of(name, b.local, b.ty, b.hides_name)
    }

    /// The answer `Local` for `name`, bound as `local` of type `ty`. `hides_name` is whether a
    /// pattern made it to hide a poisoned item.
    fn local_of(
        &self,
        name: Symbol,
        local: tir::LocalId,
        ty: Option<Type>,
        hides_name: bool,
    ) -> Resolved {
        Resolved::Local {
            local,
            ty,
            hidden: if hides_name {
                self.poison_kind_use(name)
            } else {
                None
            },
        }
    }

    /// Whether `name` is an associated type in scope.
    fn assoc_type(&self, name: Symbol) -> bool {
        self.body.choosing.contains(&name) || self.body.assoc.get(name).is_some()
    }

    /// What `name` stands for at `site`. Each position has its own order of answers.
    pub(super) fn meaning(&self, name: Symbol, site: Site) -> Resolved {
        match site.at {
            Position::Value => self.value_meaning(name),
            Position::Callee => self.callee_meaning(name),
            Position::Type => self.type_meaning(name),
            Position::Bound => self.bound_meaning(name),
            Position::Pattern => self.pattern_meaning(name),
            Position::Path | Position::PathCall => self.path_meaning(name, site.angles),
        }
    }

    /// `meaning`, and for a read of a local, the mark that an expression reads it.
    pub(super) fn resolve_name(&mut self, name: Symbol, site: Site) -> Resolved {
        if site.at == Position::Value
            && let Some(b) = self.lookup(name)
        {
            b.read |= !site.quiet;
            let (local, ty, hides_name) = (b.local, b.ty, b.hides_name);
            return self.local_of(name, local, ty, hides_name);
        }
        self.meaning(name, site)
    }

    fn value_meaning(&self, name: Symbol) -> Resolved {
        if let Some(b) = self.binding(name) {
            return self.local_answer(name, b);
        }
        match self.names.constant(name) {
            Some(Declared::Known(id)) => return Resolved::Constant(id),
            Some(Declared::Broken) => return Resolved::Poisoned(ItemKind::Constant),
            None => {}
        }
        if let Some(kind) = self.poison_kind_use(name) {
            return Resolved::Poisoned(kind);
        }
        if name == Symbol::SOME || self.result_variant(name).is_some() {
            return Resolved::PreludeVariant;
        }
        self.value_kind(name)
    }

    /// What `name` stands for in a read, when it is no local, constant or prelude variant: the
    /// wrong-kind answers. An inner scope wins, so a type parameter comes first.
    fn value_kind(&self, name: Symbol) -> Resolved {
        if let Some(place) = self.body.type_param(name) {
            return Resolved::TypeParam(place);
        }
        if self.assoc_type(name) {
            return Resolved::AssocType;
        }
        let taken = self.names.class(name).is_some()
            || self.names.enum_id(name).is_some()
            || self.names.function(name).is_some();
        if !taken && let Some(Declared::Known(id)) = self.names.interface(name) {
            return Resolved::Item(ItemId::Interface(id));
        }
        if let Some(ty) = built_in_type(name.as_str()) {
            return Resolved::BuiltinType(ty);
        }
        if name == Symbol::OPTION {
            return Resolved::OptionType;
        }
        if let Some(Declared::Known(id)) = self.names.class(name) {
            return Resolved::Item(ItemId::Class(id));
        }
        if let Some(Declared::Known(id)) = self.names.enum_id(name) {
            return Resolved::Item(ItemId::Enum(id));
        }
        if let Some(Declared::Known(id)) = self.names.alias(name) {
            return Resolved::Item(ItemId::Alias(id));
        }
        if let Some(Declared::Known(id)) = self.names.function(name) {
            return Resolved::Item(ItemId::Function(id));
        }
        if PRELUDE.contains(&name.as_str()) {
            return Resolved::BuiltinFunction;
        }
        Resolved::NotFound
    }

    fn callee_meaning(&self, name: Symbol) -> Resolved {
        if let Some(kind) = self.poison_kind_use(name) {
            return Resolved::Poisoned(kind);
        }
        if PRELUDE.contains(&name.as_str()) {
            return Resolved::BuiltinFunction;
        }
        if let Some(Declared::Known(id)) = self.names.alias(name) {
            return Resolved::Item(ItemId::Alias(id));
        }
        // `bool` has no conversion: it comes after a local's name, and gives E0105.
        let built_in = built_in_type(name.as_str());
        if let Some(ty @ (Type::I64 | Type::U8 | Type::F64 | Type::CHAR | Type::STRING)) = built_in
        {
            return Resolved::BuiltinType(ty);
        }
        if let Some(Declared::Known(id)) = self.names.class(name) {
            return Resolved::Item(ItemId::Class(id));
        }
        if let Some(Declared::Known(id)) = self.names.function(name) {
            return Resolved::Item(ItemId::Function(id));
        }
        if let Some(Declared::Known(id)) = self.names.interface(name) {
            return Resolved::Item(ItemId::Interface(id));
        }
        if name == Symbol::SOME || self.result_variant(name).is_some() {
            return Resolved::PreludeVariant;
        }
        if let Some(b) = self.binding(name) {
            return self.local_answer(name, b);
        }
        if let Some(Declared::Known(id)) = self.names.constant(name) {
            return Resolved::Constant(id);
        }
        if let Some(place) = self.body.type_param(name) {
            return Resolved::TypeParam(place);
        }
        if self.assoc_type(name) {
            return Resolved::AssocType;
        }
        if let Some(Declared::Known(id)) = self.names.enum_id(name) {
            return Resolved::Item(ItemId::Enum(id));
        }
        if let Some(ty) = built_in {
            return Resolved::BuiltinType(ty);
        }
        if name == Symbol::OPTION {
            return Resolved::OptionType;
        }
        Resolved::NotFound
    }

    fn type_meaning(&self, name: Symbol) -> Resolved {
        if let Some(place) = self.body.type_param(name) {
            return Resolved::TypeParam(place);
        }
        // The caller asks `assoc_named`, which reports a name whose choice is being resolved.
        if self.assoc_type(name) {
            return Resolved::AssocType;
        }
        let declared = (self.names.class(name), self.names.enum_id(name));
        if let (Some(Declared::Broken), _) = declared {
            return Resolved::Poisoned(ItemKind::Class);
        }
        if let (_, Some(Declared::Broken)) = declared {
            return Resolved::Poisoned(ItemKind::Enum);
        }
        match self.names.alias(name) {
            Some(Declared::Broken) => return Resolved::Poisoned(ItemKind::Alias),
            Some(Declared::Known(id)) => return Resolved::Item(ItemId::Alias(id)),
            None => {}
        }
        // A class or an enum that took a language interface's name is the type.
        let taken = declared.0.is_some() || declared.1.is_some();
        if let (false, Some(found)) = (taken, self.names.interface(name)) {
            return match found {
                Declared::Known(id) if !self.poisoned(name) => {
                    Resolved::Item(ItemId::Interface(id))
                }
                _ => Resolved::Poisoned(ItemKind::Interface),
            };
        }
        if let Some(ty) = built_in_type(name.as_str()) {
            return Resolved::BuiltinType(ty);
        }
        if let (Some(Declared::Known(id)), _) = declared {
            return Resolved::Item(ItemId::Class(id));
        }
        if let (_, Some(Declared::Known(id))) = declared {
            return Resolved::Item(ItemId::Enum(id));
        }
        if name == Symbol::OPTION {
            return Resolved::OptionType;
        }
        // A broken or rejected function or constant of the name has its own error. An intact one
        // is no type: the caller reports it.
        match self.poison_kind(name) {
            Some(kind) => Resolved::Poisoned(kind),
            None => Resolved::NotFound,
        }
    }

    fn bound_meaning(&self, name: Symbol) -> Resolved {
        if let Some(place) = self.body.type_param(name) {
            return Resolved::TypeParam(place);
        }
        match self.names.interface(name) {
            Some(Declared::Known(id)) => Resolved::Item(ItemId::Interface(id)),
            Some(Declared::Broken) => Resolved::Poisoned(ItemKind::Interface),
            None => Resolved::NotFound,
        }
    }

    fn pattern_meaning(&self, name: Symbol) -> Resolved {
        // A name that the prelude takes is a binding, which `declare` reports.
        if reserved_name(name).is_some() {
            return Resolved::PreludeVariant;
        }
        let constant = self.names.constant(name);
        if (constant.is_some() || name.as_str().starts_with(char::is_uppercase))
            && let Some(kind) = self.poison_kind_use(name)
        {
            return Resolved::Poisoned(kind);
        }
        match constant {
            Some(Declared::Known(id)) => Resolved::Constant(id),
            _ => Resolved::NotFound,
        }
    }

    /// A walk of the open scopes for a local costs the depth of the scopes. So a path looks for
    /// one once, and only where its answer needs one.
    fn path_meaning(&self, name: Symbol, angles: bool) -> Resolved {
        // A local cannot take an enum's name, so the enum wins over one that tried.
        match self.names.enum_id(name) {
            Some(Declared::Known(id)) => return Resolved::Item(ItemId::Enum(id)),
            // The type arguments make the path to a broken enum a value, which the local answers.
            Some(Declared::Broken) => {
                return match angles.then(|| self.binding(name)).flatten() {
                    Some(b) => self.local_answer(name, b),
                    None => Resolved::Poisoned(ItemKind::Enum),
                };
            }
            None => {}
        }
        // A poisoned name that no local hides gives no more error.
        let poison = self.poison_kind(name);
        let early = poison.map(|_| self.binding(name));
        if let (Some(kind), Some(None)) = (poison, early) {
            return Resolved::Poisoned(kind);
        }
        if name == Symbol::OPTION {
            return Resolved::OptionType;
        }
        // An alias is the type that it names, unless a local takes the name. A local cannot hide
        // an alias of an enum or of an optional type.
        let alias = match self.names.alias(name) {
            Some(Declared::Known(id)) => Some(id),
            _ => None,
        };
        if let Some(id) = alias
            && self.alias_unhideable(name)
        {
            return Resolved::Item(ItemId::Alias(id));
        }
        let local = early.unwrap_or_else(|| self.binding(name));
        if let (Some(id), None) = (alias, local) {
            return Resolved::Item(ItemId::Alias(id));
        }
        if let Some(b) = local {
            return self.local_answer(name, b);
        }
        if let Some(Declared::Known(id)) = self.names.constant(name) {
            return Resolved::Constant(id);
        }
        if let Some(Declared::Known(id)) = self.names.class(name) {
            return Resolved::Item(ItemId::Class(id));
        }
        if let Some(Declared::Known(id)) = self.names.function(name) {
            return Resolved::Item(ItemId::Function(id));
        }
        if let Some(Declared::Known(id)) = self.names.interface(name) {
            return Resolved::Item(ItemId::Interface(id));
        }
        match built_in_type(name.as_str()) {
            Some(ty) => Resolved::BuiltinType(ty),
            // A type parameter or an associated type is no path: the value check says so.
            None => Resolved::NotFound,
        }
    }

    /// The error for `name`, written at `at` in `site`, when `answer` is the wrong kind of thing
    /// for the site. `None` when the answer is fine there.
    pub(super) fn wrong_kind(
        &self,
        name: Symbol,
        at: Span,
        site: Site,
        answer: Resolved,
    ) -> Option<DiagnosticKind> {
        let text = || name.to_string();
        match site.at {
            Position::Value => Some(match answer {
                // A read of a prelude variant is no error. An assignment to the name is, when a
                // program item took it: the item's kind says which.
                Resolved::PreludeVariant => {
                    return self.wrong_kind(name, at, site, self.value_kind(name));
                }
                Resolved::TypeParam(_) => DiagnosticKind::ParamAsValue {
                    name: text(),
                    what: "a type parameter",
                    at,
                },
                Resolved::AssocType => DiagnosticKind::ParamAsValue {
                    name: text(),
                    what: "an associated type",
                    at,
                },
                Resolved::Item(ItemId::Class(_) | ItemId::Enum(_) | ItemId::Alias(_))
                | Resolved::BuiltinType(_)
                | Resolved::OptionType => DiagnosticKind::TypeAsValue {
                    name: text(),
                    at,
                    alias: None,
                },
                Resolved::Item(ItemId::Interface(_)) => {
                    DiagnosticKind::InterfaceAsValue { name: text(), at }
                }
                Resolved::Item(ItemId::Function(id)) => DiagnosticKind::FunctionAsValue {
                    name: text(),
                    at,
                    takes_arguments: !self.signatures[id.0].params.is_empty(),
                },
                Resolved::BuiltinFunction => DiagnosticKind::FunctionAsValue {
                    name: text(),
                    at,
                    takes_arguments: prelude::takes_arguments(name.as_str()),
                },
                _ => return None,
            }),
            Position::Callee => {
                let what = match answer {
                    Resolved::Local { .. } => "a binding",
                    Resolved::Constant(_) => "a constant",
                    Resolved::TypeParam(_) => "a type parameter",
                    Resolved::AssocType => "an associated type",
                    Resolved::Item(ItemId::Enum(id)) => {
                        // The prelude's enums, as `Result`, name no variant that the program wrote.
                        let info = &self.enums[id.0];
                        let variant = (!info.name.span.in_prelude())
                            .then(|| info.variants.first())
                            .flatten()
                            .map(|first| format!("{name}.{}", first.name.name));
                        return Some(DiagnosticKind::EnumAsValue {
                            name: text(),
                            variant,
                            at,
                        });
                    }
                    Resolved::Item(ItemId::Interface(_)) => {
                        return Some(DiagnosticKind::InterfaceAsValue { name: text(), at });
                    }
                    Resolved::BuiltinType(_) | Resolved::OptionType => {
                        return Some(DiagnosticKind::TypeAsValue {
                            name: text(),
                            at,
                            alias: None,
                        });
                    }
                    _ => return None,
                };
                Some(DiagnosticKind::NotAFunction {
                    name: text(),
                    what,
                    at,
                })
            }
            _ => None,
        }
    }
}
