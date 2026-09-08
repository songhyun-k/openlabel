use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};
use syn::{
    Attribute, Item, Meta, Token,
    parse::Parser,
    punctuated::Punctuated,
    visit::{self, Visit},
};

fn allowed(name: &str) -> Option<&'static [&'static str]> {
    Some(match name {
        "lib" => &[],
        "job" => &["label", "ble", "m110", "operation", "settings"],
        "ipc" => &["job", "operation"],
        "label" => &["raster", "settings", "operation"],
        "raster" | "m110" => &["settings"],
        "ble" => &["operation"],
        "operation" | "settings" => &[],
        "main" => &["commands", "job", "label", "ipc"],
        "commands" => &["ble", "job", "label", "operation", "raster", "settings"],
        "bin/openlabel" => &["ble", "job", "label", "operation", "settings", "ipc"],
        _ => return None,
    })
}
fn entry(name: &str) -> bool {
    matches!(name, "main" | "commands" | "bin/openlabel")
}

// Evaluate only test=false; unknown platform predicates remain in the production graph.
fn production_cfg(meta: &Meta) -> Option<bool> {
    match meta {
        Meta::Path(path) if path.is_ident("test") => Some(false),
        Meta::List(list) => {
            let args = Punctuated::<Meta, Token![,]>::parse_terminated
                .parse2(list.tokens.clone())
                .ok()?;
            let values: Vec<_> = args.iter().map(production_cfg).collect();
            if list.path.is_ident("all") {
                if values.contains(&Some(false)) {
                    Some(false)
                } else if values.iter().all(|v| *v == Some(true)) {
                    Some(true)
                } else {
                    None
                }
            } else if list.path.is_ident("any") {
                if values.contains(&Some(true)) {
                    Some(true)
                } else if values.iter().all(|v| *v == Some(false)) {
                    Some(false)
                } else {
                    None
                }
            } else if list.path.is_ident("not") && values.len() == 1 {
                values[0].map(|v| !v)
            } else {
                None
            }
        }
        _ => None,
    }
}
fn test_only(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("cfg")
            && attr
                .parse_args::<Meta>()
                .is_ok_and(|meta| production_cfg(&meta) == Some(false))
    })
}
fn attrs(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(x) => &x.attrs,
        Item::Enum(x) => &x.attrs,
        Item::ExternCrate(x) => &x.attrs,
        Item::Fn(x) => &x.attrs,
        Item::ForeignMod(x) => &x.attrs,
        Item::Impl(x) => &x.attrs,
        Item::Macro(x) => &x.attrs,
        Item::Mod(x) => &x.attrs,
        Item::Static(x) => &x.attrs,
        Item::Struct(x) => &x.attrs,
        Item::Trait(x) => &x.attrs,
        Item::TraitAlias(x) => &x.attrs,
        Item::Type(x) => &x.attrs,
        Item::Union(x) => &x.attrs,
        Item::Use(x) => &x.attrs,
        _ => &[],
    }
}
struct Edges<'a> {
    owner: &'a str,
    edges: BTreeSet<String>,
    errors: Vec<String>,
}
impl Edges<'_> {
    fn path(&mut self, path: &[String]) {
        let Some(first) = path.first().map(String::as_str) else {
            return;
        };
        if (first == "self" && path.len() > 1)
            || (first == "super" && path.get(1).is_some_and(|part| part == "super"))
        {
            self.errors.push(
                "qualified self/repeated-super module paths require an explicit architecture rule"
                    .into(),
            );
        }
        if (!entry(self.owner) && (first == "tauri" || first.starts_with("tauri_plugin_")))
            || (first == "btleplug" && self.owner != "ble")
        {
            self.errors
                .push(format!("forbidden framework: {} -> {first}", self.owner));
        }
        let target = if matches!(first, "crate" | "openlabel_core" | "super") {
            path.get(1).map(String::as_str)
        } else {
            Some(first)
        };
        if let Some(target) = target {
            if allowed(target).is_some() && target != self.owner {
                self.edges.insert(target.into());
            }
            // A library path cannot silently resolve to a new/unclassified module.
            if matches!(first, "crate" | "openlabel_core")
                && !["Error", "Result", "sha256"].contains(&target)
                && allowed(target).is_none()
            {
                self.errors.push(format!(
                    "unclassified core path: {} -> {target}",
                    self.owner
                ));
            }
        }
    }
    fn uses(&mut self, tree: &syn::UseTree, prefix: &mut Vec<String>) {
        match tree {
            syn::UseTree::Path(x) => {
                prefix.push(x.ident.to_string());
                self.uses(&x.tree, prefix);
                prefix.pop();
            }
            syn::UseTree::Name(x) => {
                prefix.push(x.ident.to_string());
                self.path(prefix);
                prefix.pop();
            }
            syn::UseTree::Rename(x) => {
                if prefix.is_empty()
                    && ["crate", "openlabel_core", "super", "self"]
                        .contains(&x.ident.to_string().as_str())
                {
                    self.errors
                        .push("root module aliases require an explicit architecture rule".into());
                }
                prefix.push(x.ident.to_string());
                self.path(prefix);
                prefix.pop();
            }
            syn::UseTree::Glob(_) => {
                if prefix.len() == 1
                    && ["crate", "openlabel_core", "self", "super"].contains(&prefix[0].as_str())
                {
                    self.errors
                        .push("root glob imports require an explicit architecture rule".into());
                }
                self.path(prefix);
            }
            syn::UseTree::Group(x) => {
                for tree in &x.items {
                    self.uses(tree, prefix);
                }
            }
        }
    }
}
impl<'ast> Visit<'ast> for Edges<'_> {
    fn visit_item_extern_crate(&mut self, item: &'ast syn::ItemExternCrate) {
        self.path(&[item.ident.to_string()]);
        if (item.ident == "openlabel_core" || item.ident == "self") && item.rename.is_some() {
            self.errors
                .push("root crate aliases require an explicit architecture rule".into());
        }
    }
    fn visit_item(&mut self, item: &'ast Item) {
        if !test_only(attrs(item)) {
            visit::visit_item(self, item);
        }
    }
    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        self.uses(&item.tree, &mut Vec::new());
    }
    fn visit_path(&mut self, path: &'ast syn::Path) {
        self.path(
            &path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect::<Vec<_>>(),
        );
        visit::visit_path(self, path);
    }
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        // lib.rs module declarations register leaves, not primitive dependencies on them.
        if self.owner != "lib" && item.content.is_none() {
            self.edges.insert(item.ident.to_string());
        }
        visit::visit_item_mod(self, item);
    }
}
fn cycle(graph: &BTreeMap<String, BTreeSet<String>>) -> Result<(), String> {
    fn walk<'a>(
        name: &'a str,
        graph: &'a BTreeMap<String, BTreeSet<String>>,
        stack: &mut Vec<&'a str>,
        done: &mut BTreeSet<&'a str>,
    ) -> Result<(), String> {
        if let Some(index) = stack.iter().position(|n| *n == name) {
            return Err(format!("cycle: {} -> {name}", stack[index..].join(" -> ")));
        }
        if done.contains(name) {
            return Ok(());
        }
        stack.push(name);
        if let Some(edges) = graph.get(name) {
            for edge in edges {
                walk(edge, graph, stack, done)?;
            }
        }
        stack.pop();
        done.insert(name);
        Ok(())
    }
    let mut done = BTreeSet::new();
    for name in graph.keys() {
        walk(name, graph, &mut Vec::new(), &mut done)?;
    }
    Ok(())
}
fn check(sources: &BTreeMap<String, String>) -> Result<BTreeMap<String, BTreeSet<String>>, String> {
    let mut graph = BTreeMap::new();
    for (name, source) in sources {
        if allowed(name).is_none() {
            return Err(format!("unclassified production file: {name}"));
        }
        let parsed = syn::parse_file(source).map_err(|e| format!("{name}: {e}"))?;
        let mut visitor = Edges {
            owner: name,
            edges: BTreeSet::new(),
            errors: Vec::new(),
        };
        visitor.visit_file(&parsed);
        if let Some(error) = visitor.errors.first() {
            return Err(error.clone());
        }
        graph.insert(name.clone(), visitor.edges);
    }
    cycle(&graph)?;
    for (name, edges) in &graph {
        for edge in edges {
            if !allowed(name).unwrap().contains(&edge.as_str()) {
                return Err(format!("forbidden edge: {name} -> {edge}"));
            }
        }
    }
    Ok(graph)
}
#[test]
fn architecture_guard_self_tests() {
    let one = |name: &str, source: &str| BTreeMap::from([(name.into(), source.into())]);
    for source in [
        "use crate as core; type Bad = core::job::Job;",
        "extern crate btleplug as radio;",
        "pub use crate::*;",
        "pub use super::*;",
        "extern crate self as core; pub type RootAlias = core::job::Job;",
        "pub mod detail { pub use super::super::job::Job as Reexport; }",
    ] {
        assert!(check(&one("raster", source)).is_err(), "accepted {source}");
    }
    for source in [
        "pub mod job; pub use self::job::Job;",
        "pub mod job; pub type RootSelfPath = self::job::Job;",
        "pub mod job; extern crate self as root; pub use root::job::Job;",
    ] {
        assert!(check(&one("lib", source)).is_err(), "accepted {source}");
    }
    assert!(
        check(&one(
            "settings",
            "struct Value; impl Value { fn same(self) -> Self { self } }"
        ))
        .is_ok()
    );
    for source in [
        "use crate::job::Control;",
        "pub use crate::job::{Control as Alias};",
        "type Bad = crate::job::Job;",
        "use tauri as ui;",
        "fn f() { crate::commands::scan_devices(); }",
    ] {
        assert!(check(&one("raster", source)).is_err(), "accepted {source}");
    }
    assert!(
        check(&one("future", ""))
            .unwrap_err()
            .contains("unclassified")
    );
    assert!(check(&one("settings", "#[derive(clap::Args)] struct Options {} ")).is_ok());
    assert!(check(&one("ipc", "use crate::{job, operation};")).is_ok());
    for source in [
        "use crate::ble;",
        "use crate::label;",
        "use crate::settings;",
        "use tauri;",
        "use btleplug;",
    ] {
        assert!(check(&one("ipc", source)).is_err());
    }
    assert!(check(&one("job", "use crate::ipc;")).is_err());
    assert!(check(&one("bin/openlabel", "use openlabel_core::ipc;")).is_ok());
    assert!(
        check(&one(
            "ble",
            "#[cfg(test)] mod tests { use crate::job; } #[cfg(all(test, unix))] use crate::job; "
        ))
        .is_ok()
    );
    for cfg in ["not(test)", "any(target_os = \"macos\", test)"] {
        assert!(check(&one("ble", &format!("#[cfg({cfg})] use crate::job;"))).is_err());
    }
    let old = BTreeMap::from([
        ("job".into(), "use crate::{label, ble};".into()),
        ("label".into(), "use crate::{job, raster};".into()),
        ("raster".into(), "use crate::label;".into()),
        ("ble".into(), "use crate::job;".into()),
    ]);
    assert!(check(&old).unwrap_err().starts_with("cycle:"));
}
#[test]
fn production_architecture_is_one_way() {
    fn collect(dir: &Path, prefix: &str, sources: &mut BTreeMap<String, String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let file = entry.path();
            let name = format!("{prefix}{}", entry.file_name().to_str().unwrap());
            if file.is_dir() {
                collect(&file, &format!("{name}/"), sources);
            } else if let Some(name) = name.strip_suffix(".rs") {
                sources.insert(name.into(), fs::read_to_string(file).unwrap());
            }
        }
    }
    let mut sources = BTreeMap::new();
    collect(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        "",
        &mut sources,
    );
    let graph = check(&sources).unwrap();
    println!("Rust architecture: production cycles 0; {graph:?}");
}

#[test]
fn native_message_keys_and_parameters_exist_in_both_catalogs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let read = |locale| {
        serde_json::from_str::<BTreeMap<String, String>>(
            &fs::read_to_string(root.join(format!("../src/locales/{locale}.json"))).unwrap(),
        )
        .unwrap()
    };
    let en = read("en");
    let ko = read("ko");
    assert_eq!(en.keys().collect::<Vec<_>>(), ko.keys().collect::<Vec<_>>());
    struct Messages<'a>(&'a BTreeMap<String, String>);
    impl Messages<'_> {
        fn check(&self, expr: &syn::Expr, params: BTreeSet<String>) {
            let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(key),
                ..
            }) = expr
            else {
                panic!("literal message key required")
            };
            let key = key.value();
            let template = self
                .0
                .get(&key)
                .unwrap_or_else(|| panic!("missing message: {key}"));
            let expected: BTreeSet<_> = template
                .split('}')
                .filter_map(|part| part.rsplit_once('{').map(|(_, name)| name.to_owned()))
                .collect();
            assert_eq!(params, expected, "{key}");
        }
    }
    impl<'ast> Visit<'ast> for Messages<'_> {
        fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
            if let syn::Expr::Path(path) = &*call.func
                && path
                    .path
                    .segments
                    .last()
                    .is_some_and(|part| part.ident == "localized")
            {
                let syn::Expr::Reference(params) = &call.args[2] else {
                    panic!("literal params required")
                };
                let syn::Expr::Array(params) = &*params.expr else {
                    panic!("literal params required")
                };
                let names = params
                    .elems
                    .iter()
                    .map(|param| {
                        let syn::Expr::Tuple(pair) = param else {
                            panic!("parameter pair required")
                        };
                        let syn::Expr::Lit(syn::ExprLit {
                            lit: syn::Lit::Str(name),
                            ..
                        }) = &pair.elems[0]
                        else {
                            panic!("literal parameter name required")
                        };
                        name.value()
                    })
                    .collect();
                self.check(&call.args[1], names);
            }
            if let syn::Expr::Path(path) = &*call.func
                && path.path.is_ident("menu_text")
            {
                self.check(&call.args[0], BTreeSet::new());
            }
            visit::visit_expr_call(self, call);
        }
        fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
            if call.method == "with_context" {
                self.check(&call.args[0], BTreeSet::from(["cause".into()]));
            }
            visit::visit_expr_method_call(self, call);
        }
    }
    fn check(dir: &Path, messages: &mut Messages<'_>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                check(&path, messages);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                messages.visit_file(&syn::parse_file(&fs::read_to_string(path).unwrap()).unwrap());
            }
        }
    }
    check(&root.join("src"), &mut Messages(&en));
    check(&root.join("src"), &mut Messages(&ko));
}
