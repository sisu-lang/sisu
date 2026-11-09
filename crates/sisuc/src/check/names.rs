//! The top-level names of a program, and what each one stands for.
//!
//! Functions, classes, enums, interfaces, constants and aliases share one namespace. `NameMaps`
//! holds one map for each kind, and its fields are private: a reader asks a getter, so that a
//! later change of the maps touches this module alone.

use rustc_hash::FxHashMap;

use super::{Body, Checker, Declared, Definition, is_language_interface};
use crate::symbol::Symbol;
use crate::tir;

/// The place of a constant among `Checker::const_infos`, which is its place in the source among
/// the constants.
#[derive(Clone, Copy)]
pub(super) struct ConstantId(pub(super) usize);

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
    /// Whether a rejected or broken item poisons the top-level name `name`, so that a use of it
    /// gives no more errors.
    pub(super) fn poisoned(&self, name: Symbol) -> bool {
        matches!(self.names.class(name), Some(Declared::Broken))
            || matches!(self.names.function(name), Some(Declared::Broken))
            || matches!(self.names.enum_id(name), Some(Declared::Broken))
            || matches!(self.names.interface(name), Some(Declared::Broken))
            || matches!(self.names.constant(name), Some(Declared::Broken))
            || matches!(self.names.alias(name), Some(Declared::Broken))
    }

    /// Whether a call or a read of the value `name` is poisoned. `Ok` and `Err`, when no user's
    /// function or class takes them, belong to the prelude's `Result`.
    pub(super) fn poisoned_use(&self, name: Symbol) -> bool {
        self.poisoned(name)
            || ((name == Symbol::OK || name == Symbol::ERR)
                && self.names.function(name).is_none()
                && self.names.class(name).is_none()
                && matches!(self.names.enum_id(Symbol::RESULT), Some(Declared::Broken)))
    }
}
