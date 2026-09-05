//! Deterministic export entry for the SDK contract artifacts.
//!
//! Default mode is a read-only `check`: every artifact is regenerated into
//! memory and compared against the committed copy; any drift exits non-zero
//! without touching the worktree. `--update` is the only mode that writes.
//!
//! Artifacts governed here:
//! - `contracts/sdk/public-api.txt` — facade inventory snapshot per profile
//! - `contracts/sdk/parity-manifest.json` — classification of every item
//! - (extension schema + fixtures are added by `schema.rs` in the same flow)
//!
//! The rustdoc toolchain is pinned in `contracts/sdk/toolchain.json`; if it
//! is missing locally the tool fails with the exact install prerequisite
//! instead of auto-installing anything (design §16/§20.5).

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use echo_sdk_protocol::EXTENSION_PROTOCOL_VERSION;
use echo_sdk_protocol::inventory::{
    self as inv, FeatureProfile, InventoryError, PublicItem, RUSTDOC_FORMAT_VERSION,
};

const TOOLCHAIN_JSON: &str = "contracts/sdk/toolchain.json";
const PUBLIC_API_TXT: &str = "contracts/sdk/public-api.txt";
const PARITY_MANIFEST_JSON: &str = "contracts/sdk/parity-manifest.json";

fn main() -> ExitCode {
    let update = std::env::args().any(|arg| arg == "--update");
    let mode = if update { "update" } else { "check" };
    eprintln!("echo-sdk-protocol export_schema: mode={mode}");

    let repo_root = match locate_repo_root() {
        Some(root) => root,
        None => {
            eprintln!("error: cannot locate repository root (missing {TOOLCHAIN_JSON})");
            return ExitCode::FAILURE;
        }
    };
    let contracts_dir = repo_root.join("contracts/sdk");
    if !contracts_dir.is_dir() {
        eprintln!("error: {} not found", contracts_dir.display());
        return ExitCode::FAILURE;
    }

    let toolchain = match load_rustdoc_toolchain(&repo_root) {
        Ok(toolchain) => toolchain,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("rustdoc toolchain: {toolchain}");

    let leaf_features = match leaf_features_of_root_crate(&repo_root) {
        Ok(features) => features,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };
    if leaf_features.is_empty() {
        eprintln!("error: root echo_agent crate exposes no leaf features; refusing empty matrix");
        return ExitCode::FAILURE;
    }
    eprintln!(
        "leaf features ({}): {}",
        leaf_features.len(),
        leaf_features.join(", ")
    );

    let profiles = inv::profiles_for_leaf_features(&leaf_features);
    let per_profile = match generate_all_profiles(&repo_root, &toolchain, &profiles) {
        Ok(per_profile) => per_profile,
        Err(first_error) => {
            // The rustdoc cache anomaly is intermittent; one full retry with
            // a cleared doc output resolves it in practice. The invariant
            // check inside keeps any second failure from being written out.
            eprintln!("warning: first generation attempt failed ({first_error}); retrying once");
            let doc_json = repo_root.join("target/doc/echo_agent.json");
            let _ = std::fs::remove_file(&doc_json);
            match generate_all_profiles(&repo_root, &toolchain, &profiles) {
                Ok(per_profile) => per_profile,
                Err(second_error) => {
                    eprintln!("error: {second_error}");
                    eprintln!(
                        "hint: run `cargo clean -p echo_agent` and retry; if it persists, \
                         file the rustdoc output of `cargo rustdoc -p echo_agent --lib \
                         --no-default-features --features <feature>` for the failing profile"
                    );
                    return ExitCode::FAILURE;
                }
            }
        }
    };

    let merged = inv::merge_profiles(&per_profile);
    let snapshot = inv::render_public_api_snapshot(&profiles, &merged);
    let manifest = inv::render_parity_manifest(EXTENSION_PROTOCOL_VERSION, &profiles, &merged);

    let mut artifacts: Vec<(PathBuf, String)> = vec![
        (repo_root.join(PUBLIC_API_TXT), snapshot),
        (repo_root.join(PARITY_MANIFEST_JSON), manifest),
    ];
    // Boundary validation before writing: the extension schema must never
    // shadow official ACP concepts.
    let schema_doc = echo_sdk_protocol::schema::build_extension_schema_doc();
    let boundary_problems = echo_sdk_protocol::schema::validate_schema_boundaries(&schema_doc);
    if !boundary_problems.is_empty() {
        for problem in &boundary_problems {
            eprintln!("schema boundary violation: {problem}");
        }
        return ExitCode::FAILURE;
    }
    artifacts.extend(extension_schema_artifacts(&repo_root));

    let mut drifted: Vec<&PathBuf> = Vec::new();
    for (path, content) in &artifacts {
        match std::fs::read_to_string(path) {
            Ok(existing) if &existing == content => {
                eprintln!(
                    "ok: {} matches ({} bytes)",
                    rel(&repo_root, path),
                    content.len()
                );
            }
            Ok(existing) => {
                drifted.push(path);
                eprintln!(
                    "DRIFT: {} committed {} bytes, regenerated {} bytes",
                    rel(&repo_root, path),
                    existing.len(),
                    content.len()
                );
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                drifted.push(path);
                eprintln!("MISSING: {}", rel(&repo_root, path));
            }
            Err(err) => {
                eprintln!("error: reading {}: {err}", rel(&repo_root, path));
                return ExitCode::FAILURE;
            }
        }
    }

    if update {
        for (path, content) in &artifacts {
            let parent = path.parent().unwrap_or(Path::new("."));
            if let Err(err) = std::fs::create_dir_all(parent) {
                eprintln!("error: creating {}: {err}", parent.display());
                return ExitCode::FAILURE;
            }
            if let Err(err) = std::fs::write(path, content) {
                eprintln!("error: writing {}: {err}", rel(&repo_root, path));
                return ExitCode::FAILURE;
            }
            eprintln!("updated: {}", rel(&repo_root, path));
        }
        eprintln!("update complete: {} artifacts written", artifacts.len());
        return ExitCode::SUCCESS;
    }

    if drifted.is_empty() {
        eprintln!(
            "check passed: {} artifacts current ({} inventory items, rustdoc format {RUSTDOC_FORMAT_VERSION})",
            artifacts.len(),
            merged.len()
        );
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "check FAILED: {} artifact(s) drifted; run \
             `cargo run -p echo-sdk-protocol --bin export_schema -- --update` and review the diff",
            drifted.len()
        );
        ExitCode::FAILURE
    }
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// The workspace root is the ancestor of this crate that carries
/// `contracts/sdk/toolchain.json`.
fn locate_repo_root() -> Option<PathBuf> {
    let mut current: &Path = Path::new(env!("CARGO_MANIFEST_DIR"));
    loop {
        if current.join(TOOLCHAIN_JSON).is_file() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

fn load_rustdoc_toolchain(repo_root: &Path) -> Result<String, String> {
    let path = repo_root.join(TOOLCHAIN_JSON);
    let raw =
        std::fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let doc: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("parsing {}: {e}", path.display()))?;
    let toolchain = doc
        .pointer("/rustdoc/toolchain")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("{} missing rustdoc.toolchain", path.display()))?;
    if toolchain.trim().is_empty() {
        return Err(format!("{} has empty rustdoc.toolchain", path.display()));
    }
    Ok(toolchain.to_string())
}

/// Leaf features of the root `echo_agent` package via `cargo metadata`
/// (everything except `default` and `full`), sorted deterministically.
fn leaf_features_of_root_crate(repo_root: &Path) -> Result<Vec<String>, String> {
    let output = Command::new("cargo")
        .arg("metadata")
        .arg("--no-deps")
        .arg("--format-version")
        .arg("1")
        .current_dir(repo_root)
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("running cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err("cargo metadata failed".to_string());
    }
    let doc: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("parsing cargo metadata output: {e}"))?;
    let packages = doc
        .get("packages")
        .and_then(|v| v.as_array())
        .ok_or("cargo metadata output missing packages")?;
    for package in packages {
        if package.get("name").and_then(|v| v.as_str()) == Some("echo_agent") {
            let features = package
                .get("features")
                .and_then(|v| v.as_object())
                .ok_or("echo_agent package missing features table")?;
            let mut leaves: Vec<String> = features
                .keys()
                .filter(|name| name.as_str() != "default" && name.as_str() != "full")
                .map(|name| name.to_string())
                .collect();
            leaves.sort();
            return Ok(leaves);
        }
    }
    Err("echo_agent package not found in cargo metadata".to_string())
}

/// Run `cargo rustdoc` for one profile on the pinned toolchain and parse the
/// emitted JSON. cargo pins rustdoc output at `target/doc/echo_agent.json`,
/// so profiles are generated sequentially. Two defenses keep a stale JSON
/// from an earlier profile or an incremental-cache rebuild from silently
/// poisoning the inventory:
///
/// - the previous JSON is deleted first, so a skipped rustdoc run surfaces
///   as a missing-file error instead of the wrong content;
/// - incremental compilation is disabled (`CARGO_INCREMENTAL=0`), because
///   flipping feature sets across many rapid rustdoc invocations has been
///   observed to reuse stale incremental metadata.
fn generate_profile(
    repo_root: &Path,
    toolchain: &str,
    profile: &FeatureProfile,
) -> Result<Vec<PublicItem>, String> {
    let json_path = repo_root.join("target/doc/echo_agent.json");
    match std::fs::remove_file(&json_path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(format!("removing {}: {err}", json_path.display())),
    }

    let mut cmd = Command::new("cargo");
    cmd.env("RUSTUP_TOOLCHAIN", toolchain)
        .env("CARGO_INCREMENTAL", "0")
        .arg("rustdoc")
        .arg("-p")
        .arg("echo_agent")
        .arg("--lib")
        .arg("--locked")
        .arg("--no-default-features");
    if !profile.features.is_empty() {
        cmd.arg("--features").arg(profile.features.join(","));
    }
    cmd.arg("--")
        .arg("-Zunstable-options")
        .arg("--output-format")
        .arg("json")
        .current_dir(repo_root);

    let output = cmd
        .output()
        .map_err(|e| format!("spawning cargo rustdoc: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "cargo rustdoc failed for profile {} (toolchain {toolchain}).\n\
             If the toolchain is missing, install it with: rustup toolchain install {toolchain}\n\
             stderr tail: {}",
            profile.name,
            tail_chars(&stderr, 2000)
        ));
    }
    let mut json = String::new();
    std::fs::File::open(&json_path)
        .and_then(|mut file| file.read_to_string(&mut json))
        .map_err(|e| format!("reading {}: {e}", json_path.display()))?;
    inv::extract_public_items(&json).map_err(|e: InventoryError| e.to_string())
}

fn generate_all_profiles(
    repo_root: &Path,
    toolchain: &str,
    profiles: &[FeatureProfile],
) -> Result<BTreeMap<String, Vec<PublicItem>>, String> {
    let mut per_profile = BTreeMap::new();
    for profile in profiles {
        eprintln!("rustdoc profile {} ...", profile.name);
        let items = generate_profile(repo_root, toolchain, profile)?;
        eprintln!(
            "rustdoc profile {}: {} public items",
            profile.name,
            items.len()
        );
        per_profile.insert(profile.name.clone(), items);
    }
    verify_profile_invariants(&per_profile)?;
    Ok(per_profile)
}

/// Deterministic-output guard: mathematically, `default` (no features) is a
/// subset of every profile and `full` (every feature) is a superset of all of
/// them. Violations mean the rustdoc toolchain returned stale or degraded
/// JSON for some profile (observed once as a caching anomaly during rapid
/// feature flips) — fail closed instead of freezing a broken inventory.
fn verify_profile_invariants(
    per_profile: &BTreeMap<String, Vec<PublicItem>>,
) -> Result<(), String> {
    let set_of = |name: &str| -> Option<std::collections::BTreeSet<String>> {
        per_profile
            .get(name)
            .map(|items| items.iter().map(|i| i.path.clone()).collect())
    };
    let default_set = set_of("default").ok_or("default profile missing")?;
    let full_set = set_of("full").ok_or("full profile missing")?;
    for (profile, items) in per_profile {
        let paths: std::collections::BTreeSet<String> =
            items.iter().map(|i| i.path.clone()).collect();
        let missing_default: Vec<&String> = default_set.difference(&paths).take(5).collect();
        if !missing_default.is_empty() {
            return Err(format!(
                "profile {profile} is missing {}/{} items present in the default \
                 profile (e.g. {missing_default:?}); rustdoc returned degraded output",
                default_set.difference(&paths).count(),
                default_set.len()
            ));
        }
        let missing_from_full: Vec<&String> = paths.difference(&full_set).take(5).collect();
        if !missing_from_full.is_empty() {
            return Err(format!(
                "profile {profile} has {}/{} items absent from the full profile \
                 (e.g. {missing_from_full:?}); rustdoc returned inconsistent output",
                paths.difference(&full_set).count(),
                paths.len()
            ));
        }
    }
    Ok(())
}

/// Extension schema + golden fixture artifacts, generated from the extension
/// DTOs and the method catalog (deterministic; see `schema.rs`).
fn extension_schema_artifacts(repo_root: &Path) -> Vec<(PathBuf, String)> {
    let mut artifacts = Vec::new();
    let doc = echo_sdk_protocol::schema::build_extension_schema_doc();
    artifacts.push((
        repo_root.join("contracts/sdk/schema/echo-agent-extension-v1.schema.json"),
        echo_sdk_protocol::schema::canonical_json(&doc),
    ));
    for (relative, content) in echo_sdk_protocol::schema::build_fixtures() {
        artifacts.push((repo_root.join(relative), content));
    }
    artifacts
}

/// UTF-8 safe suffix of at most `n` characters for error reporting.
fn tail_chars(s: &str, n: usize) -> String {
    s.chars()
        .rev()
        .take(n)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}
