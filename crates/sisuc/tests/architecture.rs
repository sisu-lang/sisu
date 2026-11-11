//! The compiler's modules depend on each other in pipeline order: a top-level module of `src/`
//! uses only the modules of its own layer and of earlier layers. The test reads the source with
//! `syn` from `lib.rs` through each `mod` item, and skips the items under `#[cfg(test)]`. It also
//! checks each field count that `docs/register.md` gives for a struct. It checks that the compiler
//! turns on no feature in a package that the runtime archives link. It also checks that the codegen
//! tests optimize a module only at the level that built it.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use syn::visit::{self, Visit};

/// The top-level modules of `src/`, in pipeline order: the shared modules first, then the stages
/// and the IR modules they share, then the modules that run the stages, then the fuzz modules.
/// `crate` names the items of `lib.rs`.
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
        "codegen",
        "crate",
        "`STACK_SIZE` is the one item of the crate root that codegen uses. It stays there, where \
         `main.rs` and the fuzz targets read it as `sisuc::STACK_SIZE`; the scheduler gives it to \
         its workers",
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
        if let syn::Item::ExternCrate(i) = item
            && i.ident == "self"
        {
            // The paths through the alias are bare, so the test cannot see them.
            panic!(
                "{} should write `crate::`, not an `extern crate self` alias",
                self.file.display()
            );
        }
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

    // shortcut: the test does not read a path inside a macro's tokens. It also does not see a
    // method that an inherent `impl` in another module adds. Parse the tokens, or map each impl to
    // its module, if a use hides there.
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

/// The named fields of the struct at `path`, such as `check::Body`: the modules from `lib.rs`,
/// then the struct, at the top of its module's file.
fn field_count(path: &str) -> usize {
    let (modules, name) = path
        .rsplit_once("::")
        .unwrap_or_else(|| panic!("`{path}` should name a module and a struct"));
    let file = modules
        .split("::")
        .fold(lib(), |file, module| module_file(&file, module));
    source(&file)
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Struct(s) if s.ident == name => Some(s.fields.len()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{} should define struct `{name}`", file.display()))
}

/// GIVEN each row of docs/register.md that says a struct holds "in N fields"
/// WHEN the struct that its Where cell names is read with `syn`
/// THEN the struct has N fields, so a new field updates the row in the same change
#[test]
fn register_field_counts_should_match_their_structs() {
    let register = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/register.md");
    let text = fs::read_to_string(&register).expect("docs/register.md should be readable");
    let mut rows = 0;
    let mut wrong = Vec::new();
    for line in text.lines().filter(|line| line.starts_with('|')) {
        let Some((_, after)) = line.split_once(" in ") else {
            continue;
        };
        let Some((count, _)) = after.split_once(" fields") else {
            continue;
        };
        let Ok(count) = count.parse::<usize>() else {
            continue;
        };
        let place = line.split('|').nth(2).unwrap_or_default().trim();
        let path = place.trim_matches('`');
        rows += 1;
        let found = field_count(path);
        if found != count {
            wrong.push(format!(
                "`{path}`: the row says {count}, the struct has {found}"
            ));
        }
    }
    assert!(
        rows > 0,
        "docs/register.md should have a row with \"in N fields\""
    );
    assert!(
        wrong.is_empty(),
        "correct the field count in docs/register.md:\n{}",
        wrong.join("\n")
    );
}

/// Each package that `cargo tree` reaches through normal dependencies with `args`, on each target
/// that Sisu supports, with the features that it resolves to. A package of another target, such as
/// `windows-sys` under `stacker`, is never in a compiled program.
fn resolved_features(args: &[&str]) -> BTreeMap<String, String> {
    let out = Command::new(env!("CARGO"))
        .args(["tree", "--offline", "--locked", "--color", "never"])
        .args(["--target", "x86_64-unknown-linux-gnu"])
        .args(["--target", "aarch64-unknown-linux-gnu"])
        .args(["--target", "aarch64-apple-darwin"])
        .args(["-e", "normal"])
        .args(["--prefix", "none", "-f", "{p}|{f}"])
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo should start");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "cargo tree failed: {stderr}");
    String::from_utf8(out.stdout)
        .expect("cargo tree should print UTF-8")
        .lines()
        .filter_map(|line| line.trim_end_matches(" (*)").split_once('|'))
        .map(|(package, features)| (package.to_string(), features.to_string()))
        .collect()
}

/// GIVEN the runtime packages, resolved alone and with the whole workspace
/// WHEN `cargo tree` prints the features of each package they reach
/// THEN each package has the same features both times
///
/// `make build` builds the runtime archives and `sisuc` in one cargo run, so a feature that
/// `sisuc` turns on in a package they share reaches each compiled program. The tests build the
/// archives alone (`scripts/build-runtime.sh`), so no other test sees it.
#[test]
fn runtime_archives_should_get_no_feature_from_the_compiler() {
    let alone = resolved_features(&["-p", "sisu-runtime-mimalloc", "-p", "sisu-runtime-system"]);
    let workspace = resolved_features(&["--workspace"]);
    for (package, features) in &alone {
        assert_eq!(workspace.get(package), Some(features), "{package}");
    }
}

/// The functions of a file that name `optimize`, by name, once for each use. A use is a call, a
/// function pointer, or either one in the arguments of a macro such as `assert!`. The visitor keeps
/// the name of the function it is in.
#[derive(Default)]
struct OptimizeCalls {
    inside: Vec<String>,
    callers: Vec<String>,
}

impl<'ast> Visit<'ast> for OptimizeCalls {
    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        self.inside.push(item.sig.ident.to_string());
        visit::visit_item_fn(self, item);
        self.inside.pop();
    }

    fn visit_expr_path(&mut self, path: &'ast syn::ExprPath) {
        if path
            .path
            .segments
            .last()
            .is_some_and(|s| s.ident == "optimize")
        {
            self.callers
                .push(self.inside.last().cloned().unwrap_or_default());
        }
        visit::visit_expr_path(self, path);
    }

    // `syn` keeps the body of a macro as tokens, so the default visitor does not see a call in
    // `assert!(optimize(..).is_ok())`. A body that is not a list of expressions is not read.
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        let parser = syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated;
        if let Ok(args) = mac.parse_body_with(parser) {
            for arg in &args {
                self.visit_expr(arg);
            }
        }
        visit::visit_macro(self, mac);
    }
}

/// The functions of `file` that name `optimize`, once for each use.
fn optimize_callers(file: &syn::File) -> Vec<String> {
    let mut calls = OptimizeCalls::default();
    calls.visit_file(file);
    calls.callers
}

/// GIVEN the codegen tests in `src/codegen/tests.rs`, which build a module at an `OptLevel`
/// WHEN each use of `optimize` is looked up in the syntax tree
/// THEN only `optimized_after` uses it, which builds and optimizes at one `level`. Codegen
/// writes different IR at each level, so a module built at `-O0` and optimized at `-O2` is IR that
/// `sisu -O2` never makes. A walk of the syntax tree stands in for an API that hides `optimize`:
/// `tests.rs` reads it through `use super::*`, and a glob import cannot be hidden. The walk does
/// not see `optimize` under another name, or a direct call of `run_passes`.
#[test]
fn codegen_tests_should_call_optimize_only_in_optimized_after() {
    let ast = source(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src/codegen/tests.rs"));
    assert_eq!(
        optimize_callers(&ast),
        ["optimized_after"],
        "build each module that a test optimizes with `optimized_at` or `optimized_after`"
    );
}

/// GIVEN tests that use `optimize` in a call, in a function pointer, and in the arguments of
/// `assert!` and of `assert_eq!` inside a closure
/// WHEN the guard looks up each use of `optimize`
/// THEN it finds each one in the function that holds it
#[test]
fn the_optimize_guard_should_find_each_use_of_optimize() {
    let file = syn::parse_file(
        "fn call() { optimize(&m, &t, level, None).expect(\"ok\"); }
         fn pointer() { let f = optimize; }
         fn in_assert() { assert!(optimize(&m, &t, level, None).is_ok()); }
         fn in_closure() { let g = || assert_eq!(super::optimize(&m, &t, level, None), Ok(())); }
         fn clean() { optimized_at(&c, \"fn main() {}\", level); }",
    )
    .expect("the probe should parse");
    assert_eq!(
        optimize_callers(&file),
        ["call", "pointer", "in_assert", "in_closure"]
    );
}
