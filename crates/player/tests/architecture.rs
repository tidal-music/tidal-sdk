//! Source layout rules for the player crate, checked on the parsed `src/` and `tests/` trees.
//!
//! Every rule reports [`Violation`]s. Known exceptions are listed in [`ALLOWANCES`]; an
//! allowance that no longer matches a violation fails the test, so it must be deleted
//! when the exception is fixed.
#![cfg(not(target_arch = "wasm32"))]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::Path;

use syn::punctuated::Punctuated;
use syn::visit::Visit;
use syn::{
    Attribute, FnArg, GenericArgument, ImplItem, Item, ItemMod, ItemUse, Meta, PathArguments,
    ReturnType, Signature, Token, TraitItem, Type, TypeParamBound, UseTree, Visibility,
};

const DEPENDENCY: &str = "module-dependency";
const LIB_CONTENT: &str = "lib-content";
const UNIFFI_PATH: &str = "uniffi-path";
const UNIFFI_EXPORT: &str = "uniffi-export";
const PATH_ATTRIBUTE: &str = "path-attribute";
const MOD_RS_CONTENT: &str = "mod-rs-content";
const MODULE_NAME: &str = "module-name";
const TEST_PLACEMENT: &str = "test-placement";
const FOREIGN_TRAIT: &str = "foreign-trait-nesting";
const RUST_ONLY_TRAIT: &str = "rust-only-trait";

/// Top-level modules and the modules each may use. `bindings` may use anything.
const DEPENDENCIES: &[(&str, &[&str])] = &[
    ("model", &[]),
    ("platforms", &["model"]),
    ("reporting", &["platforms", "model"]),
    (
        "player_item",
        &["backend", "reporting", "platforms", "model"],
    ),
    ("backend", &["reporting", "platforms", "model"]),
    ("loading", &["backend", "reporting", "platforms", "model"]),
    (
        "playback",
        &["player_item", "reporting", "platforms", "model"],
    ),
    (
        "player",
        &[
            "playback",
            "loading",
            "backend",
            "player_item",
            "reporting",
            "platforms",
            "model",
        ],
    ),
    ("bindings", &[]),
];

/// File names that mark a separate test file. Unit tests are an inline `mod tests` in the
/// file they test; everything else is an integration test in `tests/`.
const TEST_FILE_SUFFIXES: &[&str] = &["_tests.rs", "_test.rs"];
const TEST_FILE_NAMES: &[&str] = &["tests.rs", "test_support.rs"];

const FORBIDDEN_MODULE_NAMES: &[&str] = &["types", "internal", "utils", "common", "helpers"];

/// Exported traits that only Rust implements, with the reason. The host receives their
/// objects and hands them back, so they must not be `with_foreign`.
const RUST_ONLY_TRAITS: &[(&str, &str)] = &[(
    "MediaItem",
    "the host only receives items and returns them in events; Rust implements it and downcasts \
     the items it gets back",
)];

struct Allowance {
    rule: &'static str,
    from: &'static str,
    to: &'static str,
    reason: &'static str,
}

const ALLOWANCES: &[Allowance] = &[];

struct Violation {
    rule: &'static str,
    /// What the allowance matches on: a module for dependencies, otherwise a file.
    from: String,
    /// A module for dependencies, otherwise the offending item.
    to: String,
    message: String,
}

struct SourceFile {
    /// Path relative to the crate root, e.g. `src/model/mod.rs`.
    path: String,
    /// Module path of the file; empty for `lib.rs`.
    module: Vec<String>,
    ast: syn::File,
}

impl SourceFile {
    fn parse(path: &str, code: &str) -> Self {
        let module = path
            .strip_prefix("src/")
            .or_else(|| path.strip_prefix("tests/"))
            .map(|relative| {
                let mut parts: Vec<String> = relative
                    .trim_end_matches(".rs")
                    .split('/')
                    .map(String::from)
                    .collect();
                if matches!(parts.last().map(String::as_str), Some("lib" | "mod")) {
                    parts.pop();
                }
                parts
            })
            .unwrap_or_default();
        let ast = syn::parse_file(code).unwrap_or_else(|error| panic!("{path}: {error}"));
        Self {
            path: path.into(),
            module,
            ast,
        }
    }

    fn is_lib(&self) -> bool {
        self.path == "src/lib.rs"
    }

    fn is_mod_rs(&self) -> bool {
        self.path.ends_with("/mod.rs")
    }
}

fn read_sources(dir: &str) -> Vec<SourceFile> {
    fn collect(dir: &Path, files: &mut Vec<String>) {
        let mut entries: Vec<_> = fs::read_dir(dir)
            .unwrap_or_else(|error| panic!("{}: {error}", dir.display()))
            .map(|entry| entry.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                // Foreign-language binding probes and their tooling hold no Rust sources of ours.
                if !path.ends_with("tests/bindings") {
                    collect(&path, files);
                }
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path.to_string_lossy().into_owned());
            }
        }
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut paths = Vec::new();
    collect(&root.join(dir), &mut paths);
    paths
        .iter()
        .map(|path| {
            let relative = Path::new(path).strip_prefix(root).unwrap();
            let relative = relative.to_string_lossy();
            SourceFile::parse(&relative, &fs::read_to_string(path).unwrap())
        })
        .collect()
}

// --- Attributes ---------------------------------------------------------------------------

/// Attributes with `cfg_attr(condition, ...)` replaced by the attributes it applies.
fn effective_attributes(attributes: &[Attribute]) -> Vec<Meta> {
    fn flatten(meta: &Meta, out: &mut Vec<Meta>) {
        match meta {
            Meta::List(list) if list.path.is_ident("cfg_attr") => {
                let args = list
                    .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
                    .unwrap();
                for applied in args.iter().skip(1) {
                    flatten(applied, out);
                }
            }
            _ => out.push(meta.clone()),
        }
    }

    let mut metas = Vec::new();
    for attribute in attributes {
        flatten(&attribute.meta, &mut metas);
    }
    metas
}

fn is_path(path: &syn::Path, expected: &[&str]) -> bool {
    path.segments.len() == expected.len()
        && path
            .segments
            .iter()
            .zip(expected)
            .all(|(segment, expected)| segment.ident == expected)
}

fn derives_uniffi(attributes: &[Attribute], kinds: &[&str]) -> bool {
    effective_attributes(attributes).iter().any(|meta| {
        let Meta::List(list) = meta else { return false };
        list.path.is_ident("derive")
            && list
                .parse_args_with(Punctuated::<syn::Path, Token![,]>::parse_terminated)
                .unwrap()
                .iter()
                .any(|path| kinds.iter().any(|kind| is_path(path, &["uniffi", kind])))
    })
}

fn uniffi_export(attributes: &[Attribute]) -> Option<Meta> {
    effective_attributes(attributes)
        .into_iter()
        .find(|meta| is_path(meta.path(), &["uniffi", "export"]))
}

fn is_with_foreign(attributes: &[Attribute]) -> bool {
    uniffi_export(attributes).is_some_and(|meta| {
        matches!(&meta, Meta::List(list) if list.tokens.to_string().contains("with_foreign"))
    })
}

// --- Paths --------------------------------------------------------------------------------

struct PathRef {
    /// Module the path appears in, including inline modules.
    module: Vec<String>,
    segments: Vec<String>,
}

struct UsePath {
    segments: Vec<String>,
    alias: Option<String>,
}

fn flatten_use(tree: &UseTree, prefix: &mut Vec<String>, out: &mut Vec<UsePath>) {
    let leaf = |ident: String, alias: Option<String>, prefix: &[String], out: &mut Vec<UsePath>| {
        let mut segments = prefix.to_vec();
        segments.push(ident);
        out.push(UsePath { segments, alias });
    };
    match tree {
        UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            flatten_use(&path.tree, prefix, out);
            prefix.pop();
        }
        UseTree::Name(name) => leaf(name.ident.to_string(), None, prefix, out),
        UseTree::Rename(rename) => leaf(
            rename.ident.to_string(),
            Some(rename.rename.to_string()),
            prefix,
            out,
        ),
        UseTree::Glob(_) => leaf("*".into(), None, prefix, out),
        UseTree::Group(group) => {
            for tree in &group.items {
                flatten_use(tree, prefix, out);
            }
        }
    }
}

/// Collects `use` items and in-code paths, skipping attributes and macro bodies.
struct PathCollector {
    module: Vec<String>,
    paths: Vec<PathRef>,
}

impl<'ast> Visit<'ast> for PathCollector {
    fn visit_attribute(&mut self, _: &'ast Attribute) {}

    fn visit_item_mod(&mut self, item: &'ast ItemMod) {
        self.module.push(item.ident.to_string());
        syn::visit::visit_item_mod(self, item);
        self.module.pop();
    }

    fn visit_item_use(&mut self, item: &'ast ItemUse) {
        let mut uses = Vec::new();
        flatten_use(&item.tree, &mut Vec::new(), &mut uses);
        for used in uses {
            self.paths.push(PathRef {
                module: self.module.clone(),
                segments: used.segments,
            });
        }
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        self.paths.push(PathRef {
            module: self.module.clone(),
            segments: path.segments.iter().map(|s| s.ident.to_string()).collect(),
        });
        syn::visit::visit_path(self, path);
    }
}

fn collect_paths(file: &SourceFile) -> Vec<PathRef> {
    let mut collector = PathCollector {
        module: file.module.clone(),
        paths: Vec::new(),
    };
    collector.visit_file(&file.ast);
    collector.paths
}

// --- Rule 1: module dependencies ---------------------------------------------------------

/// Names re-exported by `lib.rs`, with the top-level module that owns each.
fn root_exports(files: &[SourceFile]) -> Vec<(String, UsePath)> {
    let Some(lib) = files.iter().find(|file| file.is_lib()) else {
        return Vec::new();
    };
    let mut exports = Vec::new();
    for item in &lib.ast.items {
        if let Item::Use(item) = item {
            let mut uses = Vec::new();
            flatten_use(&item.tree, &mut Vec::new(), &mut uses);
            exports.extend(
                uses.into_iter()
                    .map(|used| (used.segments[0].clone(), used)),
            );
        }
    }
    exports
}

/// The top-level module a path refers to, if it points into the crate.
fn target_module(
    path: &PathRef,
    modules: &BTreeSet<String>,
    exports: &HashMap<String, String>,
) -> Option<String> {
    let segments = &path.segments;
    let mut module = Vec::new();
    let mut rest = &segments[..];
    match segments.first()?.as_str() {
        "crate" => rest = &segments[1..],
        "self" | "super" => {
            module = path.module.clone();
            while let Some(first) = rest.first() {
                match first.as_str() {
                    "super" => {
                        module.pop()?;
                    }
                    "self" => {}
                    _ => break,
                }
                rest = &rest[1..];
            }
        }
        _ => return None,
    }
    let top = module.first().or(rest.first())?;
    if modules.contains(top) {
        Some(top.clone())
    } else if module.is_empty() {
        exports.get(top).cloned()
    } else {
        None
    }
}

fn may_depend(from: &str, to: &str) -> bool {
    from == "bindings"
        || DEPENDENCIES
            .iter()
            .any(|(module, allowed)| *module == from && allowed.contains(&to))
}

fn module_dependencies(files: &[SourceFile]) -> Vec<Violation> {
    let modules: BTreeSet<String> = files
        .iter()
        .filter_map(|file| file.module.first().cloned())
        .collect();
    let exports: HashMap<String, String> = root_exports(files)
        .into_iter()
        .map(|(module, used)| {
            (
                used.alias.unwrap_or(used.segments.last().unwrap().clone()),
                module,
            )
        })
        .collect();

    let mut violations = Vec::new();
    for module in &modules {
        if !DEPENDENCIES.iter().any(|(known, _)| known == module) {
            violations.push(Violation {
                rule: DEPENDENCY,
                from: module.clone(),
                to: module.clone(),
                message: format!("module `{module}` is missing from the dependency table"),
            });
        }
    }

    for file in files.iter().filter(|file| !file.is_lib()) {
        let Some(from) = file.module.first() else {
            continue;
        };
        let mut found: BTreeMap<String, String> = BTreeMap::new();
        for path in collect_paths(file) {
            let Some(to) = target_module(&path, &modules, &exports) else {
                continue;
            };
            if to != *from && !may_depend(from, &to) {
                found.entry(to).or_insert_with(|| path.segments.join("::"));
            }
        }
        for (to, example) in found {
            violations.push(Violation {
                rule: DEPENDENCY,
                from: from.clone(),
                to: to.clone(),
                message: format!(
                    "{}: `{from}` must not depend on `{to}` (uses `{example}`)",
                    file.path
                ),
            });
        }
    }
    violations
}

// --- Rule 2: lib.rs content --------------------------------------------------------------

fn lib_content(files: &[SourceFile]) -> Vec<Violation> {
    let Some(lib) = files.iter().find(|file| file.is_lib()) else {
        return Vec::new();
    };
    let mut violations = Vec::new();
    for item in &lib.ast.items {
        let allowed = match item {
            Item::Mod(item) => item.content.is_none(),
            Item::Use(item) => matches!(item.vis, Visibility::Public(_)),
            Item::Macro(item) => is_path(&item.mac.path, &["uniffi", "setup_scaffolding"]),
            Item::ExternCrate(item) => item.ident == "uniffi_runtime_wasm",
            _ => false,
        };
        if !allowed {
            violations.push(Violation {
                rule: LIB_CONTENT,
                from: lib.path.clone(),
                to: item_label(item),
                message: format!(
                    "{}: lib.rs may only hold `mod`, `pub use`, the UniFFI scaffolding and the \
                     wasm runtime `extern crate`, found {}",
                    lib.path,
                    item_label(item)
                ),
            });
        }
    }
    violations
}

fn item_label(item: &Item) -> String {
    let (kind, name) = match item {
        Item::Const(item) => ("const", Some(item.ident.to_string())),
        Item::Enum(item) => ("enum", Some(item.ident.to_string())),
        Item::Fn(item) => ("fn", Some(item.sig.ident.to_string())),
        Item::Impl(_) => ("impl", None),
        Item::Mod(item) => ("inline mod", Some(item.ident.to_string())),
        Item::Static(item) => ("static", Some(item.ident.to_string())),
        Item::Struct(item) => ("struct", Some(item.ident.to_string())),
        Item::Trait(item) => ("trait", Some(item.ident.to_string())),
        Item::Type(item) => ("type", Some(item.ident.to_string())),
        Item::Use(_) => ("non-public use", None),
        Item::Macro(_) => ("macro call", None),
        _ => ("item", None),
    };
    match name {
        Some(name) => format!("{kind} `{name}`"),
        None => kind.into(),
    }
}

// --- Rule 3: UniFFI paths ----------------------------------------------------------------

fn uniffi_paths(files: &[SourceFile]) -> Vec<Violation> {
    let mut violations = Vec::new();
    for file in files {
        let allowed = file.is_lib() || file.path == "src/bindings.rs";
        let uses_uniffi = collect_paths(file)
            .into_iter()
            .find(|path| path.segments.first().is_some_and(|first| first == "uniffi"));
        if let (false, Some(path)) = (allowed, uses_uniffi) {
            violations.push(Violation {
                rule: UNIFFI_PATH,
                from: file.path.clone(),
                to: path.segments.join("::"),
                message: format!(
                    "{}: `{}` belongs in bindings.rs; elsewhere use only optional UniFFI attributes",
                    file.path,
                    path.segments.join("::")
                ),
            });
        }
    }
    violations
}

// --- Rule 4: exported UniFFI items are re-exported ----------------------------------------

/// Whether every module on the way to `module` is declared `pub mod`.
fn is_pub_mod_chain(files: &[SourceFile], module: &[String]) -> bool {
    (1..=module.len()).all(|depth| {
        let Some(parent) = files.iter().find(|file| file.module == module[..depth - 1]) else {
            return false;
        };
        parent.ast.items.iter().any(|item| {
            matches!(item, Item::Mod(item) if item.ident == module[depth - 1]
                && matches!(item.vis, Visibility::Public(_)))
        })
    })
}

fn uniffi_exports(files: &[SourceFile]) -> Vec<Violation> {
    let exported_by_lib: BTreeSet<(String, String)> = root_exports(files)
        .into_iter()
        .map(|(module, used)| (module, used.segments.last().unwrap().clone()))
        .collect();

    let mut violations = Vec::new();
    for file in files.iter().filter(|file| !file.is_lib()) {
        for item in &file.ast.items {
            let (name, attributes, vis) = match item {
                Item::Struct(item) => (&item.ident, &item.attrs, &item.vis),
                Item::Enum(item) => (&item.ident, &item.attrs, &item.vis),
                Item::Trait(item) => (&item.ident, &item.attrs, &item.vis),
                _ => continue,
            };
            let carries_uniffi = derives_uniffi(attributes, &["Record", "Enum", "Error", "Object"])
                || uniffi_export(attributes).is_some();
            if !carries_uniffi {
                continue;
            }
            let top = file.module[0].clone();
            let reachable = matches!(vis, Visibility::Public(_))
                && (is_pub_mod_chain(files, &file.module)
                    || exported_by_lib.contains(&(top, name.to_string())));
            if !reachable {
                violations.push(Violation {
                    rule: UNIFFI_EXPORT,
                    from: file.path.clone(),
                    to: name.to_string(),
                    message: format!(
                        "{}: `{name}` has UniFFI attributes but is not public API (re-export it \
                         from lib.rs)",
                        file.path
                    ),
                });
            }
        }
    }
    violations
}

// --- Rules 5 and 6: layout ----------------------------------------------------------------

struct ModuleScan {
    names: Vec<String>,
}

impl<'ast> Visit<'ast> for ModuleScan {
    fn visit_item_mod(&mut self, item: &'ast ItemMod) {
        self.names.push(item.ident.to_string());
        syn::visit::visit_item_mod(self, item);
    }
}

struct PathAttributeScan {
    found: bool,
}

impl<'ast> Visit<'ast> for PathAttributeScan {
    fn visit_attribute(&mut self, attribute: &'ast Attribute) {
        self.found |= effective_attributes(std::slice::from_ref(attribute))
            .iter()
            .any(|meta| matches!(meta, Meta::NameValue(value) if value.path.is_ident("path")));
    }
}

fn path_attributes(files: &[SourceFile]) -> Vec<Violation> {
    let mut violations = Vec::new();
    for file in files {
        let mut scan = PathAttributeScan { found: false };
        scan.visit_file(&file.ast);
        if scan.found {
            violations.push(Violation {
                rule: PATH_ATTRIBUTE,
                from: file.path.clone(),
                to: "#[path]".into(),
                message: format!(
                    "{}: `#[path]` is not allowed; use normal module resolution",
                    file.path
                ),
            });
        }
    }
    violations
}

fn mod_rs_content(files: &[SourceFile]) -> Vec<Violation> {
    let mut violations = Vec::new();
    for file in files.iter().filter(|file| file.is_mod_rs()) {
        for item in &file.ast.items {
            let allowed = match item {
                Item::Mod(item) => item.content.is_none(),
                Item::Use(_) => true,
                _ => false,
            };
            if !allowed {
                violations.push(Violation {
                    rule: MOD_RS_CONTENT,
                    from: file.path.clone(),
                    to: item_label(item),
                    message: format!(
                        "{}: mod.rs may only hold `mod` declarations and re-exports, found {}",
                        file.path,
                        item_label(item)
                    ),
                });
            }
        }
    }
    violations
}

fn module_names(files: &[SourceFile]) -> Vec<Violation> {
    let mut violations = Vec::new();
    for file in files {
        let mut scan = ModuleScan {
            names: file.module.clone(),
        };
        scan.visit_file(&file.ast);
        for name in scan.names {
            if FORBIDDEN_MODULE_NAMES.contains(&name.as_str()) {
                violations.push(Violation {
                    rule: MODULE_NAME,
                    from: file.path.clone(),
                    to: name.clone(),
                    message: format!(
                        "{}: module name `{name}` says nothing about its content",
                        file.path
                    ),
                });
            }
        }
    }
    violations
}

// --- Test placement ----------------------------------------------------------------------

/// Whether a `cfg` predicate is enabled by `test`, also inside `all(...)` and `any(...)`.
fn requires_test_cfg(predicate: &Meta) -> bool {
    match predicate {
        Meta::Path(path) => path.is_ident("test"),
        Meta::List(list) if list.path.is_ident("all") || list.path.is_ident("any") => list
            .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            .unwrap()
            .iter()
            .any(requires_test_cfg),
        _ => false,
    }
}

fn is_cfg_test(attributes: &[Attribute]) -> bool {
    effective_attributes(attributes)
        .iter()
        .any(|meta| match meta {
            Meta::List(list) if list.path.is_ident("cfg") => list
                .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
                .unwrap()
                .iter()
                .any(requires_test_cfg),
            _ => false,
        })
}

struct TestModuleScan {
    out_of_line: Vec<String>,
}

impl<'ast> Visit<'ast> for TestModuleScan {
    fn visit_item_mod(&mut self, item: &'ast ItemMod) {
        if is_cfg_test(&item.attrs) && item.content.is_none() {
            self.out_of_line.push(item.ident.to_string());
        }
        syn::visit::visit_item_mod(self, item);
    }
}

fn test_placement(files: &[SourceFile]) -> Vec<Violation> {
    let mut violations = Vec::new();
    for file in files {
        let name = file.path.rsplit('/').next().unwrap();
        if TEST_FILE_NAMES.contains(&name)
            || TEST_FILE_SUFFIXES
                .iter()
                .any(|suffix| name.ends_with(suffix))
        {
            violations.push(Violation {
                rule: TEST_PLACEMENT,
                from: file.path.clone(),
                to: name.into(),
                message: format!(
                    "{}: no test files under src/; write an inline `mod tests` in the tested \
                     file or an integration test in tests/",
                    file.path
                ),
            });
        }

        let mut scan = TestModuleScan {
            out_of_line: Vec::new(),
        };
        scan.visit_file(&file.ast);
        for module in scan.out_of_line {
            violations.push(Violation {
                rule: TEST_PLACEMENT,
                from: file.path.clone(),
                to: module.clone(),
                message: format!(
                    "{}: `#[cfg(test)] mod {module};` must be an inline module in the file it tests",
                    file.path
                ),
            });
        }
    }
    violations
}

// --- Rule 7: foreign traits are never nested ----------------------------------------------

fn foreign_traits(files: &[SourceFile]) -> BTreeSet<String> {
    files
        .iter()
        .flat_map(|file| &file.ast.items)
        .filter_map(|item| match item {
            Item::Trait(item) if is_with_foreign(&item.attrs) => Some(item.ident.to_string()),
            _ => None,
        })
        .collect()
}

fn object_trait_name(object: &syn::TypeTraitObject) -> Option<String> {
    object.bounds.iter().find_map(|bound| match bound {
        TypeParamBound::Trait(bound) => Some(bound.path.segments.last()?.ident.to_string()),
        _ => None,
    })
}

fn trait_object_name(ty: &Type) -> Option<String> {
    match ty {
        Type::TraitObject(object) => object_trait_name(object),
        _ => None,
    }
}

fn type_arguments(ty: &Type, wrapper: &str) -> Option<Vec<Type>> {
    let Type::Path(path) = ty else { return None };
    let segment = path.path.segments.last()?;
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    (segment.ident == wrapper).then(|| {
        arguments
            .args
            .iter()
            .filter_map(|argument| match argument {
                GenericArgument::Type(ty) => Some(ty.clone()),
                _ => None,
            })
            .collect()
    })
}

/// Finds foreign traits used as `dyn Trait` anywhere inside a type.
struct ForeignTraitScan<'a> {
    foreign: &'a BTreeSet<String>,
    found: Vec<String>,
}

impl<'ast> Visit<'ast> for ForeignTraitScan<'_> {
    fn visit_type_trait_object(&mut self, object: &'ast syn::TypeTraitObject) {
        let name = object_trait_name(object);
        self.found
            .extend(name.filter(|name| self.foreign.contains(name)));
        syn::visit::visit_type_trait_object(self, object);
    }
}

struct ForeignTraitCheck<'a> {
    file: &'a SourceFile,
    foreign: &'a BTreeSet<String>,
    violations: Vec<Violation>,
}

impl ForeignTraitCheck<'_> {
    /// A foreign trait anywhere in `ty` is a violation.
    fn reject(&mut self, owner: &str, ty: &Type) {
        let mut scan = ForeignTraitScan {
            foreign: self.foreign,
            found: Vec::new(),
        };
        scan.visit_type(ty);
        for name in scan.found {
            self.violations.push(Violation {
                rule: FOREIGN_TRAIT,
                from: self.file.path.clone(),
                to: name.clone(),
                message: format!(
                    "{}: `{owner}` nests with_foreign trait `{name}`; pass it only as a direct \
                     `Arc<dyn {name}>` argument or return value",
                    self.file.path
                ),
            });
        }
    }

    /// `Arc<dyn Foreign>` is fine as a whole argument or return value.
    fn direct(&mut self, owner: &str, ty: &Type) {
        let is_direct = type_arguments(ty, "Arc")
            .and_then(|args| args.first().and_then(trait_object_name))
            .is_some_and(|name| self.foreign.contains(&name));
        if !is_direct {
            self.reject(owner, ty);
        }
    }

    fn signature(&mut self, owner: &str, signature: &Signature) {
        let owner = format!("{owner}::{}", signature.ident);
        for input in &signature.inputs {
            if let FnArg::Typed(argument) = input {
                self.direct(&owner, &argument.ty);
            }
        }
        if let ReturnType::Type(_, ty) = &signature.output {
            match type_arguments(ty, "Result").as_deref() {
                Some([ok, error]) => {
                    self.direct(&owner, ok);
                    self.reject(&owner, error);
                }
                _ => self.direct(&owner, ty),
            }
        }
    }
}

fn self_type_name(ty: &Type) -> String {
    match ty {
        Type::Path(path) => path.path.segments.last().unwrap().ident.to_string(),
        _ => "<impl>".into(),
    }
}

fn foreign_trait_nesting(files: &[SourceFile]) -> Vec<Violation> {
    let foreign = foreign_traits(files);
    let mut violations = Vec::new();
    for file in files {
        let mut check = ForeignTraitCheck {
            file,
            foreign: &foreign,
            violations: Vec::new(),
        };
        for item in &file.ast.items {
            match item {
                Item::Struct(item) if derives_uniffi(&item.attrs, &["Record"]) => {
                    for field in &item.fields {
                        check.reject(&item.ident.to_string(), &field.ty);
                    }
                }
                Item::Enum(item) if derives_uniffi(&item.attrs, &["Enum", "Error"]) => {
                    for field in item.variants.iter().flat_map(|variant| &variant.fields) {
                        check.reject(&item.ident.to_string(), &field.ty);
                    }
                }
                Item::Impl(item) if uniffi_export(&item.attrs).is_some() => {
                    let owner = self_type_name(&item.self_ty);
                    for item in &item.items {
                        if let ImplItem::Fn(function) = item {
                            check.signature(&owner, &function.sig);
                        }
                    }
                }
                Item::Trait(item) if uniffi_export(&item.attrs).is_some() => {
                    for trait_item in &item.items {
                        if let TraitItem::Fn(function) = trait_item {
                            check.signature(&item.ident.to_string(), &function.sig);
                        }
                    }
                }
                Item::Fn(item) if uniffi_export(&item.attrs).is_some() => {
                    check.signature("fn", &item.sig);
                }
                _ => {}
            }
        }
        violations.append(&mut check.violations);
    }
    violations
}

// --- Rule 8: Rust-only traits are not implemented by the host -----------------------------

fn rust_only_traits(files: &[SourceFile]) -> Vec<Violation> {
    let traits: Vec<_> = files
        .iter()
        .flat_map(|file| file.ast.items.iter().map(move |item| (file, item)))
        .filter_map(|(file, item)| match item {
            Item::Trait(item) => Some((file, item)),
            _ => None,
        })
        .collect();

    let mut violations = Vec::new();
    for (name, reason) in RUST_ONLY_TRAITS {
        let exported: Vec<_> = traits
            .iter()
            .filter(|(_, item)| item.ident == name && uniffi_export(&item.attrs).is_some())
            .collect();
        if exported.is_empty() {
            violations.push(Violation {
                rule: RUST_ONLY_TRAIT,
                from: "src".into(),
                to: name.to_string(),
                message: format!(
                    "`{name}` is listed in RUST_ONLY_TRAITS but no exported trait has that name"
                ),
            });
        }
        for (file, item) in exported {
            if is_with_foreign(&item.attrs) {
                violations.push(Violation {
                    rule: RUST_ONLY_TRAIT,
                    from: file.path.clone(),
                    to: name.to_string(),
                    message: format!(
                        "{}: `{name}` must not be exported `with_foreign`: {reason}",
                        file.path
                    ),
                });
            }
        }
    }
    violations
}

// --- Allowances and the test itself -------------------------------------------------------

/// Failure messages for violations that no allowance covers and allowances that cover nothing.
fn failures(violations: Vec<Violation>, allowances: &[Allowance]) -> Vec<String> {
    let mut used = vec![false; allowances.len()];
    let mut failures = Vec::new();
    for violation in violations {
        let allowance = allowances.iter().position(|allowance| {
            allowance.rule == violation.rule
                && allowance.from == violation.from
                && allowance.to == violation.to
        });
        match allowance {
            Some(index) => used[index] = true,
            None => failures.push(format!("[{}] {}", violation.rule, violation.message)),
        }
    }
    for (allowance, used) in allowances.iter().zip(used) {
        if !used {
            failures.push(format!(
                "[{}] unused allowance {} -> {} ({}); delete it",
                allowance.rule, allowance.from, allowance.to, allowance.reason
            ));
        }
    }
    failures
}

#[test]
fn architecture_rules_hold() {
    let src = read_sources("src");
    let tests = read_sources("tests");

    let mut violations = Vec::new();
    violations.extend(module_dependencies(&src));
    violations.extend(lib_content(&src));
    violations.extend(uniffi_paths(&src));
    violations.extend(uniffi_exports(&src));
    violations.extend(mod_rs_content(&src));
    violations.extend(test_placement(&src));
    violations.extend(foreign_trait_nesting(&src));
    violations.extend(rust_only_traits(&src));
    for files in [&src, &tests] {
        violations.extend(path_attributes(files));
        violations.extend(module_names(files));
    }

    let failures = failures(violations, ALLOWANCES);
    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}

// --- Rule checks on small examples ---------------------------------------------------------

/// A crate with empty `model`, `platforms` and `reporting` modules plus one more file.
fn example(lib: &str, path: &str, code: &str) -> Vec<SourceFile> {
    vec![
        SourceFile::parse("src/lib.rs", lib),
        SourceFile::parse("src/model/mod.rs", ""),
        SourceFile::parse("src/platforms/mod.rs", ""),
        SourceFile::parse("src/reporting/mod.rs", ""),
        SourceFile::parse(path, code),
    ]
}

fn dependencies_of(path: &str, code: &str) -> Vec<String> {
    module_dependencies(&example("", path, code))
        .into_iter()
        .map(|violation| format!("{} -> {}", violation.from, violation.to))
        .collect()
}

#[test]
fn dependency_rule_follows_use_items_expressions_and_super_paths() {
    let uses = dependencies_of("src/model/a.rs", "use crate::platforms::drm::Error;");
    assert_eq!(uses, ["model -> platforms"]);

    let expression = dependencies_of(
        "src/model/a.rs",
        "fn f() { let _ = crate::reporting::item_event::Failed; }",
    );
    assert_eq!(expression, ["model -> reporting"]);

    let climbing = dependencies_of(
        "src/model/a.rs",
        "use super::super::platforms::Authenticator;",
    );
    assert_eq!(climbing, ["model -> platforms"]);

    let inline = dependencies_of(
        "src/model/a.rs",
        "mod tests { use super::*; use super::super::super::reporting::X; }",
    );
    assert_eq!(inline, ["model -> reporting"]);

    let allowed = dependencies_of(
        "src/platforms/a.rs",
        "use crate::model::Error; use super::drm;",
    );
    assert!(allowed.is_empty());
}

#[test]
fn dependency_rule_follows_names_re_exported_by_lib() {
    let files = example(
        "pub use platforms::Authenticator;",
        "src/model/a.rs",
        "use crate::Authenticator;",
    );
    let found = module_dependencies(&files);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].to, "platforms");
}

#[test]
fn foreign_traits_may_only_be_direct_arguments_and_return_values() {
    let code = r#"
        #[cfg_attr(feature = "uniffi", uniffi::export(with_foreign))]
        pub trait Port: Send + Sync {}

        #[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
        pub struct Direct { pub port: Arc<dyn Port> }

        #[cfg_attr(feature = "uniffi", uniffi::export)]
        impl Service {
            pub fn accepts(&self, port: Arc<dyn Port>) -> Arc<dyn Port> { todo!() }
            pub fn fallible(&self) -> Result<Arc<dyn Port>, Error> { todo!() }
            pub fn optional(&self, port: Option<Arc<dyn Port>>) {}
            pub fn many(&self) -> Vec<Arc<dyn Port>> { todo!() }
        }
    "#;
    let files = [SourceFile::parse("src/example.rs", code)];
    let nested: Vec<_> = foreign_trait_nesting(&files)
        .into_iter()
        .map(|violation| violation.message)
        .collect();
    assert_eq!(nested.len(), 3, "{nested:#?}");
    assert!(nested[0].contains("`Direct`"));
    assert!(nested[1].contains("`Service::optional`"));
    assert!(nested[2].contains("`Service::many`"));
}

#[test]
fn rust_only_traits_must_be_exported_without_foreign_implementations() {
    let messages = |code: &str| -> Vec<String> {
        rust_only_traits(&[SourceFile::parse("src/example.rs", code)])
            .into_iter()
            .map(|violation| violation.message)
            .collect()
    };

    let exported = messages(
        r#"
        #[cfg_attr(feature = "uniffi", uniffi::export)]
        pub trait MediaItem {}
    "#,
    );
    assert!(exported.is_empty(), "{exported:#?}");

    let foreign = messages(
        r#"
        #[cfg_attr(feature = "uniffi", uniffi::export(with_foreign))]
        pub trait MediaItem {}
    "#,
    );
    assert_eq!(foreign.len(), 1, "{foreign:#?}");
    assert!(foreign[0].contains("`MediaItem` must not be exported `with_foreign`"));

    let missing = messages("pub trait MediaItem {}");
    assert_eq!(missing.len(), 1, "{missing:#?}");
    assert!(missing[0].contains("no exported trait"));
}

#[test]
fn test_placement_rejects_test_files_and_out_of_line_test_modules() {
    let files = [
        SourceFile::parse(
            "src/backend/mod.rs",
            r#"
            #[cfg(test)] mod inline_ok { fn f() {} }
            #[cfg(test)] mod declared;
            #[cfg(all(test, not(target_arch = "wasm32")))] mod all_tests;
            #[cfg(any(test, feature = "x"))] mod any_tests;
            #[cfg(not(test))] mod production;
            #[cfg_attr(test, cfg(test))] mod through_cfg_attr;
            "#,
        ),
        SourceFile::parse("src/backend/fetcher_tests.rs", ""),
        SourceFile::parse("src/backend/fetcher_test.rs", ""),
        SourceFile::parse("src/backend/tests.rs", ""),
        SourceFile::parse("src/test_support.rs", ""),
        SourceFile::parse("src/backend/fetcher.rs", "#[cfg(test)] mod tests {}"),
    ];
    let found: Vec<_> = test_placement(&files)
        .into_iter()
        .map(|violation| violation.to)
        .collect();
    assert_eq!(
        found,
        [
            "declared",
            "all_tests",
            "any_tests",
            "through_cfg_attr",
            "fetcher_tests.rs",
            "fetcher_test.rs",
            "tests.rs",
            "test_support.rs",
        ]
    );
}

#[test]
fn unused_allowances_fail() {
    const UNUSED: &[Allowance] = &[Allowance {
        rule: DEPENDENCY,
        from: "model",
        to: "platforms",
        reason: "example",
    }];
    let found = failures(Vec::new(), UNUSED);
    assert_eq!(found.len(), 1);
    assert!(found[0].contains("unused allowance model -> platforms"));

    let covered = vec![Violation {
        rule: DEPENDENCY,
        from: "model".into(),
        to: "platforms".into(),
        message: "covered".into(),
    }];
    assert!(failures(covered, UNUSED).is_empty());
}
