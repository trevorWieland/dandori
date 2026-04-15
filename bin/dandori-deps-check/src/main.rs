use std::collections::{HashMap, HashSet};
use std::process::Command;

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;

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

    let mut errors = Vec::new();

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

    if errors.is_empty() {
        return Ok(());
    }

    Err(anyhow!(
        "dependency boundary violations found: {}",
        errors.join("; ")
    ))
}
