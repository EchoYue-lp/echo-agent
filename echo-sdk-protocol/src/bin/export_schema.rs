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
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use echo_sdk_protocol::EXTENSION_PROTOCOL_VERSION;
use echo_sdk_protocol::inventory::{
    self as inv, FeatureProfile, InventoryError, PublicItem, RUSTDOC_FORMAT_VERSION,
};

const TOOLCHAIN_JSON: &str = "contracts/sdk/toolchain.json";
const PUBLIC_API_TXT: &str = "contracts/sdk/public-api.txt";
const PARITY_MANIFEST_SCHEMA_JSON: &str = "contracts/sdk/parity-manifest.schema.json";
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
    eprintln!("rustdoc toolchain: {}", toolchain.name);

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
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };

    let merged = match inv::merge_profiles(&per_profile) {
        Ok(merged) => merged,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let snapshot = inv::render_public_api_snapshot(&profiles, &merged);
    let manifest = inv::render_parity_manifest(EXTENSION_PROTOCOL_VERSION, &profiles, &merged);
    let manifest_schema = inv::render_parity_manifest_schema();

    // Source-compatibility digest over the fixed inputs: Cargo.lock plus the
    // two freshly generated inventory artifacts. The Host embeds only this
    // small document (design §17/§18); it never embeds the inventory.
    let cargo_lock_bytes = match std::fs::read(repo_root.join("Cargo.lock")) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("error: reading Cargo.lock: {error}");
            return ExitCode::FAILURE;
        }
    };
    let source_contract = echo_sdk_protocol::schema::build_source_contract_doc(&[
        (
            echo_sdk_protocol::schema::SOURCE_CONTRACT_INPUTS[0],
            cargo_lock_bytes.as_slice(),
        ),
        (
            echo_sdk_protocol::schema::SOURCE_CONTRACT_INPUTS[1],
            snapshot.as_bytes(),
        ),
        (
            echo_sdk_protocol::schema::SOURCE_CONTRACT_INPUTS[2],
            manifest.as_bytes(),
        ),
    ]);

    let mut artifacts: Vec<(PathBuf, String)> = vec![
        (repo_root.join(PUBLIC_API_TXT), snapshot),
        (repo_root.join(PARITY_MANIFEST_SCHEMA_JSON), manifest_schema),
        (repo_root.join(PARITY_MANIFEST_JSON), manifest),
        (
            repo_root.join("contracts/sdk/source-contract.json"),
            echo_sdk_protocol::schema::canonical_json(&source_contract),
        ),
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
    if let Err(error) = validate_generated_schemas(&repo_root, &artifacts) {
        eprintln!("error: {error}");
        return ExitCode::FAILURE;
    }
    if let Err(error) = validate_generated_file_set(&repo_root, &artifacts) {
        eprintln!("error: {error}");
        return ExitCode::FAILURE;
    }

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

struct RustdocToolchain {
    name: String,
}

fn load_rustdoc_toolchain(repo_root: &Path) -> Result<RustdocToolchain, String> {
    let path = repo_root.join(TOOLCHAIN_JSON);
    let raw =
        std::fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let doc: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("parsing {}: {e}", path.display()))?;
    let name = doc
        .pointer("/rustdoc/toolchain")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("{} missing rustdoc.toolchain", path.display()))?;
    if name.trim().is_empty() {
        return Err(format!("{} has empty rustdoc.toolchain", path.display()));
    }
    let format_version = doc
        .pointer("/rustdoc/rustdoc_json_format_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            format!(
                "{} missing rustdoc.rustdoc_json_format_version",
                path.display()
            )
        })?;
    if format_version != RUSTDOC_FORMAT_VERSION {
        return Err(format!(
            "{} declares rustdoc format {format_version}, but the parser requires {RUSTDOC_FORMAT_VERSION}",
            path.display()
        ));
    }
    Ok(RustdocToolchain {
        name: name.to_string(),
    })
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

#[derive(Debug, Clone)]
struct WorkspaceRustdocPackage {
    package_name: String,
    target_name: String,
    features: Vec<String>,
    manifest_path: PathBuf,
    registry_source: bool,
}

struct RustdocRequest<'a> {
    package_name: &'a str,
    target_name: &'a str,
    features: &'a [String],
    profile_name: &'a str,
    external_manifest: Option<&'a Path>,
}

fn generate_profile(
    repo_root: &Path,
    toolchain: &RustdocToolchain,
    profile: &FeatureProfile,
    run_identity: &str,
    dependency_cache: &mut BTreeMap<(String, Vec<String>), String>,
) -> Result<Vec<PublicItem>, String> {
    let root_json = generate_rustdoc_json(
        repo_root,
        toolchain,
        run_identity,
        RustdocRequest {
            package_name: "echo_agent",
            target_name: "echo_agent",
            features: &profile.features,
            profile_name: &profile.name,
            external_manifest: None,
        },
    )?;
    let root_json = attach_macro_behavior_digests(repo_root, "echo_agent", &root_json)?;
    let packages = resolved_workspace_dependencies(repo_root, profile)?;
    let mut dependencies = BTreeMap::new();
    for package in packages {
        let cache_key = (package.package_name.clone(), package.features.clone());
        let json = match dependency_cache.get(&cache_key) {
            Some(json) => json.clone(),
            None => {
                let generated = generate_rustdoc_json(
                    repo_root,
                    toolchain,
                    run_identity,
                    RustdocRequest {
                        package_name: &package.package_name,
                        target_name: &package.target_name,
                        features: &package.features,
                        profile_name: &profile.name,
                        external_manifest: package
                            .registry_source
                            .then_some(package.manifest_path.as_path()),
                    },
                )?;
                let generated =
                    attach_macro_behavior_digests(repo_root, &package.target_name, &generated)?;
                dependency_cache.insert(cache_key, generated.clone());
                generated
            }
        };
        dependencies.insert(package.target_name, json);
    }
    inv::extract_public_items_with_dependencies(&root_json, &dependencies)
        .map_err(|error: InventoryError| error.to_string())
}

fn attach_macro_behavior_digests(
    repo_root: &Path,
    target_name: &str,
    json: &str,
) -> Result<String, String> {
    let proc_macro_digest = if target_name == "echo_macros" {
        let mut files = Vec::new();
        collect_rust_sources(&repo_root.join("echo-macros/src"), &mut files)?;
        files.push(repo_root.join("echo-macros/Cargo.toml"));
        Some(source_files_digest(repo_root, &files)?)
    } else {
        None
    };
    let mut document: serde_json::Value = serde_json::from_str(json)
        .map_err(|error| format!("parsing {target_name} rustdoc JSON: {error}"))?;
    let existing_ids: std::collections::BTreeSet<String> = {
        let index = document
            .get_mut("index")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or_else(|| format!("{target_name} rustdoc JSON missing index"))?;
        for item in index.values_mut() {
            let is_proc_macro = item.pointer("/inner/proc_macro").is_some();
            let is_declarative_macro = item.pointer("/inner/macro").is_some();
            let digest = if is_proc_macro {
                proc_macro_digest.clone()
            } else if is_declarative_macro {
                item.pointer("/span/filename")
                    .and_then(serde_json::Value::as_str)
                    .map(|filename| repo_root.join(filename))
                    .map(|path| source_files_digest(repo_root, &[path]))
                    .transpose()?
            } else {
                None
            };
            if let Some(digest) = digest
                && let Some(attrs) = item
                    .get_mut("attrs")
                    .and_then(serde_json::Value::as_array_mut)
            {
                attrs.push(serde_json::json!({
                    "other": format!("#[echo_sdk_behavior_digest = \"{digest}\"]")
                }));
            }
        }
        index.keys().cloned().collect()
    };
    if let Some(digest) = proc_macro_digest {
        let missing_derives: Vec<(String, String)> = document
            .get("paths")
            .and_then(serde_json::Value::as_object)
            .into_iter()
            .flatten()
            .filter_map(|(id, summary)| {
                let is_derive =
                    summary.get("kind").and_then(serde_json::Value::as_str) == Some("proc_derive");
                let name = summary
                    .get("path")
                    .and_then(serde_json::Value::as_array)
                    .and_then(|path| path.last())
                    .and_then(serde_json::Value::as_str);
                (is_derive && !existing_ids.contains(id))
                    .then(|| name.map(|name| (id.clone(), name.to_string())))
                    .flatten()
            })
            .collect();
        let index = document
            .get_mut("index")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or("echo_macros rustdoc JSON missing index")?;
        for (id, name) in missing_derives {
            let numeric_id = id.parse::<u64>().unwrap_or_default();
            index.insert(
                id,
                serde_json::json!({
                    "id": numeric_id,
                    "crate_id": 0,
                    "name": name,
                    "visibility": "public",
                    "attrs": [{
                        "other": format!("#[echo_sdk_behavior_digest = \"{digest}\"]")
                    }],
                    "inner": {"proc_macro": {"kind": "derive", "helpers": []}}
                }),
            );
        }
    }
    serde_json::to_string(&document)
        .map_err(|error| format!("serializing augmented {target_name} rustdoc JSON: {error}"))
}

fn collect_rust_sources(directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("reading {}: {error}", directory.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("reading source entry: {error}"))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("reading type for {}: {error}", path.display()))?;
        if file_type.is_dir() {
            collect_rust_sources(&path, files)?;
        } else if file_type.is_file() && path.extension().is_some_and(|extension| extension == "rs")
        {
            files.push(path);
        }
    }
    Ok(())
}

fn source_files_digest(repo_root: &Path, files: &[PathBuf]) -> Result<String, String> {
    let mut sorted = files.to_vec();
    sorted.sort();
    let mut sources = Vec::new();
    for path in sorted {
        let content = std::fs::read_to_string(&path)
            .map_err(|error| format!("reading {}: {error}", path.display()))?;
        sources.push(serde_json::json!({
            "path": rel(repo_root, &path),
            "content": content,
        }));
    }
    Ok(echo_sdk_protocol::schema::digest_of(
        &serde_json::Value::Array(sources),
    ))
}

fn generate_rustdoc_json(
    repo_root: &Path,
    toolchain: &RustdocToolchain,
    run_identity: &str,
    request: RustdocRequest<'_>,
) -> Result<String, String> {
    let mut command = Command::new("cargo");
    command
        .env("RUSTUP_TOOLCHAIN", &toolchain.name)
        .env("CARGO_INCREMENTAL", "0")
        .arg("rustdoc")
        .arg("--lib")
        .arg("--locked")
        .arg("--no-default-features");
    if let Some(manifest_path) = request.external_manifest {
        command
            .arg("--manifest-path")
            .arg(manifest_path)
            .arg("--target-dir")
            .arg(repo_root.join("target"));
    } else {
        command.arg("-p").arg(request.package_name);
    }
    if !request.features.is_empty() {
        command.arg("--features").arg(request.features.join(","));
    }
    let profile_digest = echo_sdk_protocol::schema::digest_of(&serde_json::json!({
        "package": request.package_name,
        "profile": request.profile_name,
        "features": request.features,
    }));
    let profile_cfg: String = profile_digest
        .chars()
        .filter(|character| character.is_ascii_hexdigit())
        .take(16)
        .collect();
    command
        .arg("--")
        .arg("-Zunstable-options")
        .arg("--output-format")
        .arg("json")
        .arg("--cfg")
        .arg(format!("echo_sdk_contract_run_{run_identity}"))
        .arg("--cfg")
        .arg(format!("echo_sdk_contract_profile_{profile_cfg}"))
        .current_dir(repo_root);

    let output = command.output().map_err(|error| {
        format!(
            "spawning cargo rustdoc for {}: {error}",
            request.package_name
        )
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "cargo rustdoc failed for {} profile {} (toolchain {}).\n\
             Install it with: rustup toolchain install {}\n\
             stderr tail: {}",
            request.package_name,
            request.profile_name,
            toolchain.name,
            toolchain.name,
            tail_chars(&stderr, 2000)
        ));
    }
    let path = repo_root
        .join("target/doc")
        .join(format!("{}.json", request.target_name));
    std::fs::read_to_string(&path).map_err(|error| format!("reading {}: {error}", path.display()))
}

fn resolved_workspace_dependencies(
    repo_root: &Path,
    profile: &FeatureProfile,
) -> Result<Vec<WorkspaceRustdocPackage>, String> {
    let mut command = Command::new("cargo");
    command
        .arg("metadata")
        .arg("--format-version")
        .arg("1")
        .arg("--locked")
        .arg("--no-default-features")
        .current_dir(repo_root);
    if !profile.features.is_empty() {
        command.arg("--features").arg(profile.features.join(","));
    }
    let output = command
        .output()
        .map_err(|error| format!("running cargo metadata for {}: {error}", profile.name))?;
    if !output.status.success() {
        return Err(format!("cargo metadata failed for {}", profile.name));
    }
    let document: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("parsing cargo metadata for {}: {error}", profile.name))?;
    let root = document
        .pointer("/resolve/root")
        .and_then(serde_json::Value::as_str)
        .ok_or("cargo metadata resolve.root is missing")?;
    let nodes = document
        .pointer("/resolve/nodes")
        .and_then(serde_json::Value::as_array)
        .ok_or("cargo metadata resolve.nodes is missing")?;
    let node_by_id: BTreeMap<&str, &serde_json::Value> = nodes
        .iter()
        .filter_map(|node| {
            node.get("id")
                .and_then(serde_json::Value::as_str)
                .map(|id| (id, node))
        })
        .collect();
    let mut reachable = BTreeMap::<String, Vec<String>>::new();
    let mut pending = vec![root.to_string()];
    while let Some(id) = pending.pop() {
        if reachable.contains_key(&id) {
            continue;
        }
        let Some(node) = node_by_id.get(id.as_str()) else {
            continue;
        };
        let mut features: Vec<String> = node
            .get("features")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_string)
            .collect();
        features.sort();
        reachable.insert(id.clone(), features);
        for dependency in node
            .get("deps")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(package_id) = dependency.get("pkg").and_then(serde_json::Value::as_str) {
                pending.push(package_id.to_string());
            }
        }
    }

    let packages = document
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .ok_or("cargo metadata packages is missing")?;
    let mut result = Vec::new();
    for package in packages {
        let Some(id) = package.get("id").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some(features) = reachable.get(id) else {
            continue;
        };
        let registry_source = package
            .get("source")
            .is_some_and(|source| !source.is_null());
        let Some(package_name) = package.get("name").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if registry_source && package_name != "tokio-util" {
            continue;
        }
        if matches!(
            package_name,
            "echo_agent" | "echo-sdk-protocol" | "echo-agent-learning"
        ) {
            continue;
        }
        let target_name = package
            .get("targets")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .find(|target| {
                target
                    .get("kind")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|kinds| {
                        kinds
                            .iter()
                            .any(|kind| matches!(kind.as_str(), Some("lib" | "proc-macro")))
                    })
            })
            .and_then(|target| target.get("name"))
            .and_then(serde_json::Value::as_str);
        let Some(target_name) = target_name else {
            continue;
        };
        let manifest_path = package
            .get("manifest_path")
            .and_then(serde_json::Value::as_str)
            .map(PathBuf::from)
            .ok_or_else(|| format!("package {package_name} missing manifest_path"))?;
        result.push(WorkspaceRustdocPackage {
            package_name: package_name.to_string(),
            target_name: target_name.to_string(),
            features: features.clone(),
            manifest_path,
            registry_source,
        });
    }
    result.sort_by(|left, right| left.package_name.cmp(&right.package_name));
    Ok(result)
}

fn generate_all_profiles(
    repo_root: &Path,
    toolchain: &RustdocToolchain,
    profiles: &[FeatureProfile],
) -> Result<BTreeMap<String, Vec<PublicItem>>, String> {
    let mut per_profile = BTreeMap::new();
    let run_identity = format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    );
    let mut dependency_cache = BTreeMap::new();
    for profile in profiles {
        eprintln!("rustdoc profile {} ...", profile.name);
        let items = generate_profile(
            repo_root,
            toolchain,
            profile,
            &run_identity,
            &mut dependency_cache,
        )?;
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

fn validate_generated_schemas(
    repo_root: &Path,
    artifacts: &[(PathBuf, String)],
) -> Result<(), String> {
    let content = |relative: &str| -> Result<&str, String> {
        let expected = repo_root.join(relative);
        artifacts
            .iter()
            .find(|(path, _)| path == &expected)
            .map(|(_, content)| content.as_str())
            .ok_or_else(|| format!("generated artifact {relative} is missing"))
    };
    let manifest_schema: serde_json::Value =
        serde_json::from_str(content(PARITY_MANIFEST_SCHEMA_JSON)?)
            .map_err(|error| format!("parsing generated parity manifest schema: {error}"))?;
    let manifest: serde_json::Value = serde_json::from_str(content(PARITY_MANIFEST_JSON)?)
        .map_err(|error| format!("parsing generated parity manifest: {error}"))?;
    let validator = jsonschema::validator_for(&manifest_schema)
        .map_err(|error| format!("compiling parity manifest schema: {error}"))?;
    if let Err(error) = validator.validate(&manifest) {
        return Err(format!(
            "generated parity manifest violates its schema: {error}"
        ));
    }

    let extension_relative = "contracts/sdk/schema/echo-agent-extension-v1.schema.json";
    let extension_schema: serde_json::Value = serde_json::from_str(content(extension_relative)?)
        .map_err(|error| format!("parsing generated extension schema: {error}"))?;
    echo_sdk_protocol::schema::build_extension_validator(&extension_schema)
        .map_err(|error| format!("compiling generated extension schema: {error}"))?;
    Ok(())
}

fn validate_generated_file_set(
    repo_root: &Path,
    artifacts: &[(PathBuf, String)],
) -> Result<(), String> {
    let fixture_root = repo_root.join("contracts/sdk/fixtures/extension/v1");
    let expected: std::collections::BTreeSet<PathBuf> = artifacts
        .iter()
        .map(|(path, _)| path)
        .filter(|path| path.starts_with(&fixture_root))
        .cloned()
        .collect();
    let entries = std::fs::read_dir(&fixture_root)
        .map_err(|error| format!("reading {}: {error}", fixture_root.display()))?;
    let mut actual = std::collections::BTreeSet::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "reading fixture entry in {}: {error}",
                fixture_root.display()
            )
        })?;
        if entry
            .file_type()
            .map_err(|error| format!("reading fixture type: {error}"))?
            .is_file()
        {
            actual.insert(entry.path());
        }
    }
    let stale: Vec<String> = actual
        .difference(&expected)
        .map(|path| rel(repo_root, path))
        .collect();
    if stale.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "stale generated extension fixtures are not in the canonical set: {}",
            stale.join(", ")
        ))
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macro_source_digest_covers_helper_modules() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        let entry = root.join("lib.rs");
        let helper = root.join("helper.rs");
        std::fs::write(&entry, "mod helper;\n")?;
        std::fs::write(&helper, "fn expand() -> bool { true }\n")?;
        let files = vec![entry, helper.clone()];
        let before = source_files_digest(root, &files)?;
        std::fs::write(&helper, "fn expand() -> bool { false }\n")?;
        let after = source_files_digest(root, &files)?;
        assert_ne!(before, after);
        Ok(())
    }
}
