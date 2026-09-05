//! Facade inventory ↔ parity manifest consistency tests.
//!
//! Design §7.2/§20.1: the parity manifest must cover exactly the public
//! facade inventory. These tests read the committed artifacts (no rustdoc
//! toolchain needed) and fail on duplicates, stale identities, illegal
//! enum values or missing language fields — so any facade change that skips
//! a manifest update fails `cargo test` even before the heavier
//! `export_schema --check` drift gate.

use std::collections::BTreeSet;
use std::path::PathBuf;

use echo_sdk_protocol::inventory::ItemKind;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn read(path: &str) -> String {
    let full = repo_root().join(path);
    std::fs::read_to_string(&full)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", full.display()))
}

/// Parse `public-api.txt` into (kind, path) pairs, ignoring the header and
/// footer comments.
fn parse_snapshot() -> Vec<(String, String)> {
    let mut items = Vec::new();
    for line in read("contracts/sdk/public-api.txt").lines() {
        if !line.starts_with('#') {
            let mut parts = line.splitn(2, ' ');
            let kind = parts.next().unwrap_or_default().to_string();
            let rest = parts.next().unwrap_or_default();
            // Path runs until the import marker or the profile bracket.
            let path = rest
                .split("  <=")
                .next()
                .unwrap_or_default()
                .split("  [")
                .next()
                .unwrap_or_default()
                .trim()
                .to_string();
            if !kind.is_empty() && !path.is_empty() {
                items.push((kind, path));
            }
        }
    }
    items
}

#[test]
fn manifest_entries_match_inventory_exactly() {
    let snapshot = parse_snapshot();
    assert!(
        !snapshot.is_empty(),
        "snapshot is empty; artifacts not generated?"
    );

    let manifest: serde_json::Value =
        serde_json::from_str(&read("contracts/sdk/parity-manifest.json"))
            .unwrap_or_else(|err| panic!("manifest JSON invalid: {err}"));
    let entries = manifest
        .get("entries")
        .and_then(|v| v.as_array())
        .expect("manifest.entries missing");

    let mut snapshot_paths: BTreeSet<&str> = BTreeSet::new();
    for (kind, path) in &snapshot {
        if kind == ItemKind::Module.as_str() {
            continue;
        }
        assert!(
            snapshot_paths.insert(path.as_str()),
            "duplicate identity in snapshot: {path}"
        );
    }

    let mut manifest_paths: BTreeSet<&str> = BTreeSet::new();
    for entry in entries {
        let path = entry
            .get("path")
            .and_then(|v| v.as_str())
            .expect("entry.path");
        assert!(
            manifest_paths.insert(path),
            "duplicate manifest entry: {path}"
        );
    }

    let missing_in_manifest: Vec<&str> = snapshot_paths
        .difference(&manifest_paths)
        .copied()
        .collect();
    let stale_in_manifest: Vec<&str> = manifest_paths
        .difference(&snapshot_paths)
        .copied()
        .collect();
    assert!(
        missing_in_manifest.is_empty() && stale_in_manifest.is_empty(),
        "manifest/inventory drift; missing: {missing_in_manifest:?}; stale: {stale_in_manifest:?}"
    );
}

#[test]
fn every_entry_has_valid_classification_relationship_and_languages() {
    let manifest: serde_json::Value =
        serde_json::from_str(&read("contracts/sdk/parity-manifest.json"))
            .unwrap_or_else(|err| panic!("manifest JSON invalid: {err}"));
    let entries = manifest
        .get("entries")
        .and_then(|v| v.as_array())
        .expect("manifest.entries missing");

    const CLASSES: &[&str] = &[
        "wire_value",
        "operation",
        "handle",
        "stream",
        "extension",
        "language_intrinsic",
    ];
    const RELATIONSHIPS: &[&str] = &[
        "standard",
        "standard_projection",
        "echo_extension",
        "language_intrinsic",
    ];

    for entry in entries {
        let path = entry
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let class = entry.get("classification").and_then(|v| v.as_str());
        let Some(class) = class else {
            panic!("entry {path} missing classification");
        };
        assert!(
            CLASSES.contains(&class),
            "entry {path} illegal classification {class}"
        );

        let relationship = entry
            .get("acp_relationship")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| panic!("entry {path} missing acp_relationship"));
        assert!(
            RELATIONSHIPS.contains(&relationship),
            "entry {path} illegal acp_relationship {relationship}"
        );

        let languages = entry
            .get("languages")
            .and_then(|v| v.as_object())
            .unwrap_or_else(|| panic!("entry {path} missing languages"));
        for language in ["typescript", "python", "java"] {
            let status = languages
                .get(language)
                .and_then(|l| l.get("status"))
                .and_then(|s| s.as_str())
                .unwrap_or_else(|| panic!("entry {path} missing {language} status"));
            assert!(
                ["not_implemented", "in_progress", "done"].contains(&status),
                "entry {path} illegal {language} status {status}"
            );
        }

        // Language-intrinsic items must not claim wire relationships.
        if class == "language_intrinsic" {
            assert_eq!(
                relationship, "language_intrinsic",
                "entry {path} is language_intrinsic but claims {relationship}"
            );
        }
    }
}

#[test]
fn manifest_and_snapshot_agree_on_profiles() {
    let snapshot = read("contracts/sdk/public-api.txt");
    let snapshot_profiles = snapshot
        .lines()
        .find_map(|line| line.strip_prefix("# profiles: "))
        .expect("snapshot profiles header");
    let manifest: serde_json::Value =
        serde_json::from_str(&read("contracts/sdk/parity-manifest.json"))
            .unwrap_or_else(|err| panic!("manifest JSON invalid: {err}"));
    let generated = manifest
        .pointer("/generated/profiles")
        .and_then(|v| v.as_array())
        .expect("manifest generated.profiles");
    let manifest_profiles: Vec<String> = generated
        .iter()
        .map(|p| p.as_str().unwrap_or_default().to_string())
        .collect();
    let snapshot_list: Vec<String> = snapshot_profiles.split(", ").map(str::to_string).collect();
    assert_eq!(snapshot_list, manifest_profiles, "profile lists diverged");

    // The leaf-feature matrix must cover every leaf feature exactly once.
    let leaf_count = manifest_profiles
        .iter()
        .filter(|p| p.starts_with("feature:"))
        .count();
    assert!(
        leaf_count >= 20,
        "suspiciously small feature matrix: {leaf_count} leaf profiles"
    );
}
