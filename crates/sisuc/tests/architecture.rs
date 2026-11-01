//! The compiler's modules depend on each other in pipeline order: a top-level module of `src/`
//! uses only the modules of its own layer and of earlier layers. The test reads the source with
//! `syn` from `lib.rs` through each `mod` item, and skips the items under `#[cfg(test)]`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use syn::visit::{self, Visit};

/// The top-level modules of `src/`, in pipeline order: the shared modules first, then the stages,
/// then the modules that run the stages. `crate` names the items of `lib.rs`.
const LAYERS: &[&[&str]] = &[
    &["source", "symbol", "graph", "diagnostic"],
    &["lexer"],
    &["ast"],
    &["parser"],
    // `types` names the ids of `tir`, and `tir` holds types.
    &["tir", "types"],
    &["lang", "conformance", "payload_limit"],
    &["check"],
    &["mono"],
    &["mir"],
    &["codegen"],
    &["link"],
    &["crate", "front_end", "timings", "report", "run", "args"],
    &["fuzz", "fuzz_program", "generate"],
];

/// The uses of a later layer that stay, as `(user, used, reason)`. The list holds no entry that
/// no use needs.
const BACK_EDGES: &[(&str, &str, &str)] = &[
    (
        "check",
        "front_end",
        "front_end builds the prelude program once, and each check reads it",
    ),
    (
        "diagnostic",
        "conformance",
        "the registry quotes the choice depth limit that conformance owns",
    ),
    (
        "diagnostic",
        "payload_limit",
        "the registry quotes the payload limit that payload_limit owns",
    ),
    (
        "link",
        "crate",
        "`Allocator` is a command-line choice in lib.rs, and link picks the runtime by it",
    ),
];

fn layer(module: &str) -> Option<usize> {
    LAYERS.iter().position(|layer| layer.contains(&module))
}

fn is_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("cfg")
            && attr
                .parse_args::<syn::Ident>()
                .is_ok_and(|arg| arg == "test")
    })
}

/// The top-level modules each top-level module uses, from one file. `module` is the path of the
/// module that the visitor is in, from the crate root; `top` is the modules `lib.rs` declares.
struct Uses<'a> {
    file: &'a Path,
    module: Vec<String>,
    top: &'a [String],
    /// `(user, used)` to the first file that has the use.
    edges: &'a mut BTreeMap<(String, String), PathBuf>,
    /// Out-of-line `mod name;` items, as the module path and the file that declares them.
    pending: Vec<(Vec<String>, PathBuf)>,
}

impl Uses<'_> {
    /// Records the use of `path`, written in the current module, when it leaves its top-level
    /// module.
    fn record(&mut self, path: &[String]) {
        let mut at = self.module.clone();
        let mut rest = path;
        match rest.first().map(String::as_str) {
            Some("crate") => {
                at.clear();
                rest = &rest[1..];
            }
            Some("self" | "super") => {
                while let Some(first) = rest.first() {
                    match first.as_str() {
                        "self" => {}
                        "super" => {
                            at.pop();
                        }
                        _ => break,
                    }
                    rest = &rest[1..];
                }
            }
            // A bare path names a module only in `lib.rs`; elsewhere it starts at a local name.
            _ if !self.module.is_empty() => return,
            _ => {}
        }
        let target = match at.first() {
            Some(top) => top.clone(),
            // A path to the root itself, such as `use crate as c;` or `use crate::*;`, uses it all.
            None => match rest.first() {
                Some(name) if self.top.contains(name) => name.clone(),
                _ => "crate".to_owned(),
            },
        };
        let user = self
            .module
            .first()
            .map_or("crate", String::as_str)
            .to_owned();
        if user != target {
            self.edges
                .entry((user, target))
                .or_insert_with(|| self.file.to_owned());
        }
    }

    fn use_tree(&mut self, prefix: &mut Vec<String>, tree: &syn::UseTree) {
        match tree {
            syn::UseTree::Path(path) => {
                prefix.push(path.ident.to_string());
                self.use_tree(prefix, &path.tree);
                prefix.pop();
            }
            syn::UseTree::Name(name) => self.use_path(prefix, &name.ident),
            syn::UseTree::Rename(rename) => self.use_path(prefix, &rename.ident),
            syn::UseTree::Glob(_) => self.record(prefix),
            syn::UseTree::Group(group) => {
                for tree in &group.items {
                    self.use_tree(prefix, tree);
                }
            }
        }
    }

    fn use_path(&mut self, prefix: &[String], ident: &syn::Ident) {
        let mut path = prefix.to_vec();
        path.push(ident.to_string());
        self.record(&path);
    }
}

impl<'ast> Visit<'ast> for Uses<'_> {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        let attrs = match item {
            syn::Item::Const(i) => &i.attrs,
            syn::Item::Enum(i) => &i.attrs,
            syn::Item::Fn(i) => &i.attrs,
            syn::Item::Impl(i) => &i.attrs,
            syn::Item::Mod(i) => &i.attrs,
            syn::Item::Static(i) => &i.attrs,
            syn::Item::Struct(i) => &i.attrs,
            syn::Item::Trait(i) => &i.attrs,
            syn::Item::Type(i) => &i.attrs,
            syn::Item::Use(i) => &i.attrs,
            syn::Item::Macro(i) => &i.attrs,
            _ => return visit::visit_item(self, item),
        };
        if !is_test(attrs) {
            visit::visit_item(self, item);
        }
    }

    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        self.module.push(item.ident.to_string());
        match &item.content {
            Some(_) => visit::visit_item_mod(self, item),
            None => self
                .pending
                .push((self.module.clone(), self.file.to_owned())),
        }
        self.module.pop();
    }

    fn visit_impl_item(&mut self, item: &'ast syn::ImplItem) {
        let test = match item {
            syn::ImplItem::Const(i) => is_test(&i.attrs),
            syn::ImplItem::Fn(i) => is_test(&i.attrs),
            syn::ImplItem::Type(i) => is_test(&i.attrs),
            _ => false,
        };
        if !test {
            visit::visit_impl_item(self, item);
        }
    }

    fn visit_field(&mut self, field: &'ast syn::Field) {
        if !is_test(&field.attrs) {
            visit::visit_field(self, field);
        }
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        // A `pub use` in lib.rs is the crate's interface, for the fuzz targets, not a use.
        let interface = self.module.is_empty() && matches!(item.vis, syn::Visibility::Public(_));
        if !interface {
            self.use_tree(&mut Vec::new(), &item.tree);
        }
    }

    // shortcut: a path inside a macro's tokens is not read, nor a method that an inherent `impl` in
    // another module adds; parse the tokens, or map each impl to its module, if a use hides there.
    fn visit_path(&mut self, path: &'ast syn::Path) {
        let segments: Vec<String> = path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect();
        if segments.len() > 1 {
            self.record(&segments);
        }
        visit::visit_path(self, path);
    }
}

/// The file of the out-of-line module `name`, declared in `parent`.
fn module_file(parent: &Path, name: &str) -> PathBuf {
    let stem = parent.file_stem().and_then(|s| s.to_str());
    let dir = parent
        .parent()
        .expect("a source file should have a directory");
    let dir = match stem {
        Some("lib" | "mod") => dir.to_owned(),
        _ => dir.join(stem.expect("a source file name should be UTF-8")),
    };
    let flat = dir.join(format!("{name}.rs"));
    if flat.exists() {
        flat
    } else {
        dir.join(name).join("mod.rs")
    }
}

fn source(path: &Path) -> syn::File {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{} should be readable: {e}", path.display()));
    syn::parse_file(&text).unwrap_or_else(|e| panic!("{} should parse: {e}", path.display()))
}

fn lib() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs")
}

/// The top-level modules that `lib.rs` declares, outside test code.
fn top_modules() -> Vec<String> {
    source(&lib())
        .items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Mod(m) if m.content.is_none() && !is_test(&m.attrs) => {
                Some(m.ident.to_string())
            }
            _ => None,
        })
        .collect()
}

/// Every use of one top-level module by another, outside test code, with the first file that has
/// it.
fn uses() -> BTreeMap<(String, String), PathBuf> {
    let top = top_modules();
    let mut edges = BTreeMap::new();
    let mut files = vec![(Vec::new(), lib())];
    while let Some((module, file)) = files.pop() {
        let ast = source(&file);
        let mut uses = Uses {
            file: &file,
            module,
            top: &top,
            edges: &mut edges,
            pending: Vec::new(),
        };
        uses.visit_file(&ast);
        let pending = uses.pending;
        for (module, parent) in pending {
            let name = module.last().expect("a module path should not be empty");
            let path = module_file(&parent, name);
            files.push((module, path));
        }
    }
    edges
}

/// GIVEN the top-level modules that lib.rs declares
/// WHEN each one is looked up in `LAYERS`
/// THEN each module has one layer, and each layer names a real module
#[test]
fn module_layers_should_name_each_top_level_module_once() {
    let top = top_modules();
    let names: Vec<&str> = LAYERS
        .iter()
        .flat_map(|layer| layer.iter().copied())
        .collect();
    let missing: Vec<&String> = top.iter().filter(|m| layer(m).is_none()).collect();
    let extra: Vec<&&str> = names
        .iter()
        .filter(|name| **name != "crate" && !top.iter().any(|m| m == *name))
        .collect();
    let twice: Vec<&&str> = names
        .iter()
        .enumerate()
        .filter(|(i, name)| names[..*i].contains(name))
        .map(|(_, name)| name)
        .collect();
    assert!(
        missing.is_empty() && extra.is_empty() && twice.is_empty(),
        "LAYERS should name each module of lib.rs once: missing {missing:?}, not a module \
         {extra:?}, twice {twice:?}"
    );
}

/// GIVEN every use of one top-level module by another, outside test code
/// WHEN each use is compared with the layers and the kept back edges
/// THEN no use goes to a later layer, except the kept ones
#[test]
fn modules_should_use_only_their_own_and_earlier_layers() {
    let edges = uses();
    let back: Vec<String> = edges
        .iter()
        .filter(|((user, used), _)| {
            layer(used) > layer(user) && !BACK_EDGES.iter().any(|(u, d, _)| u == user && d == used)
        })
        .map(|((user, used), file)| format!("{user} uses {used} in {}", file.display()))
        .collect();
    assert!(
        back.is_empty(),
        "a module should use only its own layer and earlier ones; move the item, or add a \
         BACK_EDGES entry with a reason:\n{}",
        back.join("\n")
    );
}

/// GIVEN every use of one top-level module by another, outside test code
/// WHEN each `BACK_EDGES` entry is looked up in those uses
/// THEN each entry names a use that the code still has
#[test]
fn back_edges_should_each_name_a_use_that_remains() {
    let edges = uses();
    let stale: Vec<String> = BACK_EDGES
        .iter()
        .filter(|(u, d, _)| !edges.contains_key(&((*u).to_owned(), (*d).to_owned())))
        .map(|(u, d, _)| format!("{u} -> {d}"))
        .collect();
    assert!(
        stale.is_empty(),
        "each BACK_EDGES entry should still be used; delete these: {stale:?}"
    );
}
