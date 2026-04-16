//! Architecture/layering guard.
//!
//! Two independent checks, both enforced:
//!
//! 1. **Dependency allowlist** (via `cargo metadata`): each crate must only
//!    depend on crates permitted for its architectural layer. Foundation
//!    crates cannot reach upward to capability crates; transport crates
//!    cannot pull in `dandori-domain`/`dandori-store`/`dandori-orchestrator`.
//!
//! 2. **AST-level source scan** (via `syn`): every `use` path in transport
//!    and foundation crate sources is walked and checked against a per-crate
//!    forbidden-prefix list. This catches drift that a pure regex can miss
//!    (comments, macro-rules that emit imports, formatting) and is exact
//!    about what a file imports.
//!
//! A failing check exits non-zero with a combined report so CI shows the
//! full set of offenders, not just the first one.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use syn::visit::Visit;
use syn::{Item, UseTree};

#[derive(Debug, Deserialize)]
struct Metadata {
    packages: Vec<Package>,
}

#[derive(Debug, Deserialize)]
struct Package {
    name: String,
    dependencies: Vec<Dependency>,
}

#[derive(Debug, Deserialize)]
struct Dependency {
    name: String,
}

fn main() -> Result<()> {
    let mut errors = Vec::new();
    check_dependencies(&mut errors)?;
    check_source_imports(&mut errors);

    if errors.is_empty() {
        return Ok(());
    }

    Err(anyhow!(
        "architecture violations ({} total):\n  - {}",
        errors.len(),
        errors.join("\n  - ")
    ))
}

fn check_dependencies(errors: &mut Vec<String>) -> Result<()> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .context("failed to execute cargo metadata")?;

    if !output.status.success() {
        return Err(anyhow!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let metadata: Metadata =
        serde_json::from_slice(&output.stdout).context("failed to parse cargo metadata json")?;
    let deps_by_package: HashMap<String, HashSet<String>> = metadata
        .packages
        .into_iter()
        .map(|pkg| {
            (
                pkg.name,
                pkg.dependencies.into_iter().map(|dep| dep.name).collect(),
            )
        })
        .collect();

    let foundation = [
        "dandori-domain",
        "dandori-contract",
        "dandori-policy",
        "dandori-observability",
    ];
    let capability = [
        "dandori-store",
        "dandori-graph",
        "dandori-workflow",
        "dandori-sync-github",
        "dandori-orchestrator",
        "dandori-app-services",
    ];
    let transport = ["dandori-api", "dandori-mcp", "dandori-worker"];
    let forbidden_transport = ["dandori-domain", "dandori-store", "dandori-orchestrator"];

    for foundation_crate in foundation {
        let deps = deps_by_package.get(foundation_crate);
        for capability_crate in capability {
            if deps.is_some_and(|deps| deps.contains(capability_crate)) {
                errors.push(format!(
                    "foundation crate '{foundation_crate}' depends on capability crate '{capability_crate}'"
                ));
            }
        }
    }

    for transport_crate in transport {
        let deps = deps_by_package.get(transport_crate);
        for forbidden in forbidden_transport {
            if deps.is_some_and(|deps| deps.contains(forbidden)) {
                errors.push(format!(
                    "transport crate '{transport_crate}' depends directly on '{forbidden}'"
                ));
            }
        }
    }

    Ok(())
}

fn check_source_imports(errors: &mut Vec<String>) {
    // Transport crates must stay thin: no reaching into sqlx, the domain
    // crate, the store crate, or the policy crate. They consume services
    // exclusively through `dandori-app-services` and `dandori-contract`.
    let transport_dirs = [
        ("dandori-api", "bin/dandori-api/src"),
        ("dandori-mcp", "bin/dandori-mcp/src"),
        ("dandori-worker", "bin/dandori-worker/src"),
    ];
    let transport_forbidden: &[&[&str]] = &[
        &["sqlx"],
        &["dandori_store"],
        &["dandori_domain"],
        &["dandori_policy"],
    ];

    for (crate_name, dir) in transport_dirs {
        scan_crate(crate_name, Path::new(dir), transport_forbidden, errors);
    }

    // Foundation crates must not import from capability crates.
    let foundation_dirs = [
        ("dandori-domain", "crates/dandori-domain/src"),
        ("dandori-contract", "crates/dandori-contract/src"),
        ("dandori-policy", "crates/dandori-policy/src"),
        ("dandori-observability", "crates/dandori-observability/src"),
    ];
    let foundation_forbidden: &[&[&str]] = &[
        &["dandori_store"],
        &["dandori_app_services"],
        &["dandori_orchestrator"],
        &["sqlx"],
    ];

    for (crate_name, dir) in foundation_dirs {
        scan_crate(crate_name, Path::new(dir), foundation_forbidden, errors);
    }

    // Outside of `dandori-store` and `dandori-test-support`, no crate may
    // use `sqlx::` — database access is firewalled behind the store.
    let non_store_crates = [
        ("dandori-app-services", "crates/dandori-app-services/src"),
        ("dandori-auth", "crates/dandori-auth/src"),
    ];
    for (crate_name, dir) in non_store_crates {
        scan_crate(crate_name, Path::new(dir), &[&["sqlx"]], errors);
    }
}

fn scan_crate(
    crate_name: &str,
    dir: &Path,
    forbidden_prefixes: &[&[&str]],
    errors: &mut Vec<String>,
) {
    let files = collect_rs_files(dir);
    for file in files {
        let Ok(source) = fs::read_to_string(&file) else {
            continue;
        };
        let Ok(ast) = syn::parse_file(&source) else {
            errors.push(format!("{crate_name}: failed to parse {}", file.display()));
            continue;
        };
        let mut visitor = ImportVisitor { paths: Vec::new() };
        visitor.visit_file(&ast);
        for imported in visitor.paths {
            for forbidden in forbidden_prefixes {
                if path_starts_with(&imported, forbidden) {
                    errors.push(format!(
                        "{crate_name}: {} imports forbidden path `{}`",
                        file.display(),
                        imported.join("::")
                    ));
                }
            }
        }
    }
}

fn collect_rs_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if !dir.exists() {
        return files;
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                files.push(path);
            }
        }
    }
    files
}

fn path_starts_with(imported: &[String], prefix: &[&str]) -> bool {
    if imported.len() < prefix.len() {
        return false;
    }
    imported
        .iter()
        .zip(prefix.iter())
        .all(|(a, b)| a.as_str() == *b)
}

/// Walks every `use` tree in a parsed file and records each leaf import
/// path as a `Vec<String>`. Glob/renamed imports are flattened so the
/// forbidden-prefix check still works.
struct ImportVisitor {
    paths: Vec<Vec<String>>,
}

impl<'ast> Visit<'ast> for ImportVisitor {
    fn visit_item(&mut self, item: &'ast Item) {
        if let Item::Use(use_item) = item {
            collect_use_tree(&use_item.tree, Vec::new(), &mut self.paths);
        }
        syn::visit::visit_item(self, item);
    }
}

fn collect_use_tree(tree: &UseTree, prefix: Vec<String>, out: &mut Vec<Vec<String>>) {
    match tree {
        UseTree::Path(path) => {
            let mut next = prefix;
            next.push(path.ident.to_string());
            collect_use_tree(&path.tree, next, out);
        }
        UseTree::Name(name) => {
            let mut next = prefix;
            next.push(name.ident.to_string());
            out.push(next);
        }
        UseTree::Rename(rename) => {
            let mut next = prefix;
            next.push(rename.ident.to_string());
            out.push(next);
        }
        UseTree::Glob(_) => {
            out.push(prefix);
        }
        UseTree::Group(group) => {
            for item in &group.items {
                collect_use_tree(item, prefix.clone(), out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_forbidden_import() {
        let src = r#"
            use sqlx::PgPool;
            fn main() {}
        "#;
        let ast = syn::parse_file(src).expect("valid rust source");
        let mut v = ImportVisitor { paths: Vec::new() };
        v.visit_file(&ast);
        assert_eq!(v.paths, vec![vec!["sqlx".to_owned(), "PgPool".to_owned()]]);
        assert!(path_starts_with(&v.paths[0], &["sqlx"]));
    }

    #[test]
    fn group_import_flattened() {
        let src = r#"
            use dandori_store::{PgStore, StoreError};
        "#;
        let ast = syn::parse_file(src).expect("valid rust source");
        let mut v = ImportVisitor { paths: Vec::new() };
        v.visit_file(&ast);
        assert_eq!(v.paths.len(), 2);
        assert!(
            v.paths
                .iter()
                .all(|p| path_starts_with(p, &["dandori_store"]))
        );
    }

    #[test]
    fn unrelated_path_does_not_match_prefix() {
        let paths = vec![
            "dandori_contract".to_owned(),
            "CreateIssueRequest".to_owned(),
        ];
        assert!(!path_starts_with(&paths, &["dandori_store"]));
    }
}
