//! Artifact-level facade inventory and parity-manifest checks.

use std::collections::BTreeSet;
use std::path::PathBuf;

use echo_sdk_protocol::facade::{FACADE_FAMILIES, validate_facade_route_table};
use echo_sdk_protocol::inventory::{
    AcpRelationship, FeatureSemantics, ItemKind, ManifestEntry, ParityManifest, SemanticClass,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn read(path: &str) -> TestResult<String> {
    Ok(std::fs::read_to_string(repo_root().join(path))?)
}

fn parse_snapshot() -> TestResult<Vec<(String, String, String)>> {
    Ok(read("contracts/sdk/public-api.txt")?
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            Some((
                parts.next()?.to_string(),
                parts.next()?.to_string(),
                parts.next()?.to_string(),
            ))
        })
        .collect())
}

fn manifest() -> TestResult<ParityManifest> {
    Ok(serde_json::from_str(&read(
        "contracts/sdk/parity-manifest.json",
    )?)?)
}

fn find_entry<'a>(manifest: &'a ParityManifest, path: &str) -> TestResult<&'a ManifestEntry> {
    manifest
        .entries
        .iter()
        .find(|entry| entry.path == path)
        .ok_or_else(|| format!("missing facade identity {path}").into())
}

#[test]
fn manifest_entries_match_every_inventory_signature() -> TestResult {
    let snapshot: BTreeSet<(String, String)> = parse_snapshot()?
        .into_iter()
        .map(|(_, path, digest)| (path, digest))
        .collect();
    assert!(!snapshot.is_empty(), "snapshot must not be empty");
    let manifest: BTreeSet<(String, String)> = manifest()?
        .entries
        .into_iter()
        .flat_map(|entry| {
            entry
                .signatures
                .into_iter()
                .map(move |signature| (entry.path.clone(), signature.digest))
        })
        .collect();
    assert_eq!(
        snapshot, manifest,
        "manifest and inventory signatures drifted"
    );
    Ok(())
}

#[test]
fn manifest_schema_compiles_and_validates_document() -> TestResult {
    let schema: serde_json::Value =
        serde_json::from_str(&read("contracts/sdk/parity-manifest.schema.json")?)?;
    let document: serde_json::Value =
        serde_json::from_str(&read("contracts/sdk/parity-manifest.json")?)?;
    let validator = jsonschema::validator_for(&schema)?;
    assert!(
        validator.validate(&document).is_ok(),
        "manifest does not satisfy its schema"
    );
    Ok(())
}

#[test]
fn entries_have_complete_mapping_and_language_obligations() -> TestResult {
    let manifest = manifest()?;
    let expected_languages: BTreeSet<&str> = ["typescript", "python", "java"].into_iter().collect();
    let mut classes = BTreeSet::new();
    let mut relationships = BTreeSet::new();
    let paths: BTreeSet<&str> = manifest.entries.iter().map(|e| e.path.as_str()).collect();
    let route_by_path: std::collections::BTreeMap<&str, &str> = manifest
        .entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry.route.route.as_str()))
        .collect();
    for entry in &manifest.entries {
        assert!(
            !entry.path.ends_with("::*"),
            "unexpanded glob: {}",
            entry.path
        );
        assert!(
            !entry.signatures.is_empty(),
            "missing signatures: {}",
            entry.path
        );
        assert!(
            !entry.route.route.is_empty(),
            "missing canonical route: {}",
            entry.path
        );
        assert!(
            !entry.route.route.contains('*'),
            "wildcard route on {}: {}",
            entry.path,
            entry.route.route
        );
        assert!(
            !entry.semantic_rule.is_empty(),
            "missing semantic rule: {}",
            entry.path
        );
        assert!(
            !entry.route.validation.is_empty(),
            "missing validation: {}",
            entry.path
        );
        if let Some(alias_of) = &entry.alias_of {
            assert!(!entry.canonical, "alias marked canonical: {}", entry.path);
            assert!(
                paths.contains(alias_of.as_str()),
                "alias {} points at missing canonical {alias_of}",
                entry.path
            );
            let canonical_route = route_by_path
                .get(alias_of.as_str())
                .copied()
                .unwrap_or_default();
            assert_eq!(
                canonical_route,
                entry.route.route.as_str(),
                "alias {} and canonical {alias_of} must share one route",
                entry.path
            );
        }
        let languages: BTreeSet<&str> = entry.languages.keys().map(String::as_str).collect();
        assert_eq!(
            languages, expected_languages,
            "language mapping: {}",
            entry.path
        );
        for language in entry.languages.values() {
            assert!(
                !language.target.is_empty(),
                "empty language target: {}",
                entry.path
            );
            assert!(
                !language.contract_test.is_empty(),
                "missing language contract test: {}",
                entry.path
            );
        }
        if entry.features.is_empty() && !entry.full_only {
            assert_eq!(entry.feature_semantics, FeatureSemantics::Default);
        }
        match entry.feature_semantics {
            FeatureSemantics::Default => assert!(entry.features.is_empty()),
            FeatureSemantics::AnyOf => assert!(!entry.features.is_empty()),
            FeatureSemantics::AllOf => {
                assert!(entry.full_only);
                assert!(
                    entry.features.len() >= 2,
                    "full-only entry lacks an AND condition: {}",
                    entry.path
                );
            }
        }
        classes.insert(entry.classification);
        relationships.insert(entry.acp_relationship);
    }
    for class in [
        SemanticClass::WireValue,
        SemanticClass::Operation,
        SemanticClass::Handle,
        SemanticClass::Stream,
        SemanticClass::Extension,
        SemanticClass::LanguageIntrinsic,
    ] {
        assert!(classes.contains(&class), "missing semantic class {class:?}");
    }
    for relationship in [
        AcpRelationship::StandardProjection,
        AcpRelationship::EchoExtension,
        AcpRelationship::LanguageIntrinsic,
    ] {
        assert!(
            relationships.contains(&relationship),
            "missing ACP relationship {relationship:?}"
        );
    }
    Ok(())
}

#[test]
fn known_facade_semantics_are_classified_correctly() -> TestResult {
    let manifest = manifest()?;
    assert_eq!(
        find_entry(&manifest, "echo_agent::agent::Agent")?.classification,
        SemanticClass::Extension
    );
    assert_eq!(
        find_entry(&manifest, "echo_agent::agent::AgentHandle")?.classification,
        SemanticClass::Handle
    );
    assert_eq!(
        find_entry(&manifest, "echo_agent::llm::LlmClient")?.classification,
        SemanticClass::Extension
    );
    assert_eq!(
        find_entry(&manifest, "echo_agent::agent::ReactAgentBuilder")?.classification,
        SemanticClass::LanguageIntrinsic
    );
    assert_eq!(
        find_entry(&manifest, "echo_agent::agent::CancellationToken")?.classification,
        SemanticClass::Handle
    );
    assert_eq!(
        find_entry(&manifest, "echo_agent::agent::AgentRunSnapshot::llm_client")?.classification,
        SemanticClass::LanguageIntrinsic
    );
    assert_eq!(
        find_entry(&manifest, "echo_agent::agent::AgentRunSnapshot")?.classification,
        SemanticClass::Handle
    );
    for resource in [
        "echo_agent::agent::subagent::SubagentExecutor",
        "echo_agent::intent::IntentRouter",
        "echo_agent::agent::react::run::pipeline::ToolExecutionPipeline",
    ] {
        assert_eq!(
            find_entry(&manifest, resource)?.classification,
            SemanticClass::Handle,
            "resource {resource} must remain opaque"
        );
    }
    assert_eq!(
        find_entry(
            &manifest,
            "echo_agent::agent::subagent::SharedIsolationProvider"
        )?
        .classification,
        SemanticClass::Extension
    );
    for callback in [
        "echo_agent::tools::SubagentUplinkFn",
        "echo_agent::scheduler::FireFn",
    ] {
        assert_eq!(
            find_entry(&manifest, callback)?.classification,
            SemanticClass::LanguageIntrinsic,
            "callback alias {callback} must not be a wire value"
        );
    }
    assert_eq!(
        find_entry(&manifest, "echo_agent::evolution::PromptInjectionDetector")?.acp_relationship,
        AcpRelationship::EchoExtension
    );
    assert_eq!(
        find_entry(&manifest, "echo_agent::agent::AgentEvent")?.acp_relationship,
        AcpRelationship::StandardProjection
    );
    assert_eq!(
        find_entry(&manifest, "echo_agent::llm::types::LinkedResource")?.acp_relationship,
        AcpRelationship::StandardProjection
    );
    let canonical_resource = find_entry(&manifest, "echo_agent::llm::types::LinkedResource")?;
    let prelude_resource = find_entry(&manifest, "echo_agent::prelude::LinkedResource")?;
    assert_eq!(
        prelude_resource.acp_relationship,
        AcpRelationship::StandardProjection
    );
    assert_eq!(prelude_resource.route, canonical_resource.route);
    assert_eq!(
        prelude_resource.alias_of.as_deref(),
        Some("echo_agent::llm::types::LinkedResource")
    );
    assert!(canonical_resource.canonical);
    for field in [
        "annotations",
        "description",
        "mime_type",
        "name",
        "size",
        "title",
        "uri",
        "meta",
    ] {
        let canonical = find_entry(
            &manifest,
            &format!("echo_agent::llm::types::LinkedResource::{field}"),
        )?;
        let prelude = find_entry(
            &manifest,
            &format!("echo_agent::prelude::LinkedResource::{field}"),
        )?;
        assert_eq!(prelude.acp_relationship, canonical.acp_relationship);
        assert_eq!(prelude.route, canonical.route);
    }
    assert_eq!(
        find_entry(&manifest, "echo_agent::acp::AcpAgentAdapter")?.acp_relationship,
        AcpRelationship::LanguageIntrinsic
    );
    assert_eq!(
        find_entry(&manifest, "echo_agent::acp::AcpAdapterConfig")?.acp_relationship,
        AcpRelationship::LanguageIntrinsic
    );
    assert_eq!(
        find_entry(&manifest, "echo_agent::acp::AcpSessionFactory")?.acp_relationship,
        AcpRelationship::LanguageIntrinsic
    );
    assert_eq!(
        find_entry(&manifest, "echo_agent::acp::AcpSessionContext")?.acp_relationship,
        AcpRelationship::StandardProjection
    );
    assert!(manifest.entries.iter().any(|entry| {
        entry.path.ends_with("RuntimeTaskService") && entry.classification == SemanticClass::Handle
    }));
    assert!(
        manifest
            .entries
            .iter()
            .any(|entry| entry.path.ends_with("TurnReceipt"))
    );
    assert!(manifest.entries.iter().any(|entry| {
        entry.path.ends_with("FileConversationStore")
            && entry.classification == SemanticClass::Handle
    }));
    assert!(
        manifest
            .entries
            .iter()
            .any(|entry| { entry.path.ends_with("EventJournal") && entry.kind == ItemKind::Trait })
    );
    assert!(manifest.entries.iter().any(|entry| {
        entry.kind == ItemKind::TraitImpl && entry.path.contains("ReactAgent::impl<Agent>")
    }));
    assert!(manifest.entries.iter().any(|entry| {
        entry.kind == ItemKind::TraitImpl && entry.path.contains("ReactAgentBuilder::impl<Default>")
    }));
    assert!(manifest.entries.iter().any(|entry| {
        entry.path.ends_with("AgentEvent::ThinkEnd::prompt_tokens")
            && entry.kind == ItemKind::StructField
    }));
    assert!(
        find_entry(&manifest, "echo_agent::agent::Agent")?
            .features
            .is_empty()
    );
    Ok(())
}

#[test]
fn manifest_and_snapshot_agree_on_profiles() -> TestResult {
    let snapshot = read("contracts/sdk/public-api.txt")?;
    let snapshot_profiles = snapshot
        .lines()
        .find_map(|line| line.strip_prefix("# profiles: "))
        .ok_or("snapshot profiles header missing")?;
    assert_eq!(
        snapshot_profiles,
        manifest()?.generated.profiles.join(", "),
        "profile lists diverged"
    );
    Ok(())
}

#[test]
fn facade_route_table_is_mechanically_closed() -> TestResult {
    assert!(
        validate_facade_route_table().is_empty(),
        "route table violations: {:?}",
        validate_facade_route_table()
    );
    // Every source-routed family must own at least one canonical manifest
    // item (aliases do not count — one handler per route needs one real
    // item to serve).
    let manifest = manifest()?;
    let mut items_by_family: std::collections::BTreeMap<&str, usize> =
        std::collections::BTreeMap::new();
    // One operation identity may legitimately carry several signature
    // variants (cfg-shaped re-exports); the exact (operation, signature)
    // pair is what a request must match, so that pair must be unique.
    let mut invoke_signatures: BTreeSet<(&str, &str)> = BTreeSet::new();
    for entry in &manifest.entries {
        if let Some(family) = entry.route.family.as_deref()
            && entry.canonical
        {
            *items_by_family.entry(family).or_insert(0) += 1;
        }
        if let Some(operation) = entry.route.operation.as_deref()
            && entry.alias_of.is_none()
        {
            for signature in &entry.signatures {
                assert!(
                    invoke_signatures.insert((operation, signature.digest.as_str())),
                    "duplicate invoke operation signature {operation} {}",
                    signature.digest
                );
            }
        }
    }
    for descriptor in FACADE_FAMILIES {
        if !descriptor.family.is_source_routed() {
            continue;
        }
        let family = descriptor.family.as_str();
        let items = items_by_family.get(family).copied().unwrap_or(0);
        assert!(
            items > 0,
            "source-routed family {family} has no canonical manifest item"
        );
    }
    Ok(())
}

#[test]
fn facade_operation_catalog_artifact_matches_manifest() -> TestResult {
    let manifest = manifest()?;
    let catalog: serde_json::Value =
        serde_json::from_str(&read("contracts/sdk/facade-operation-catalog.json")?)?;
    let total_items = catalog
        .get("total_items")
        .and_then(|v| v.as_u64())
        .ok_or("facade catalog missing total_items")?;
    assert_eq!(
        total_items,
        manifest.entries.len() as u64,
        "facade catalog item total drifted from the parity manifest"
    );
    let routes = catalog
        .get("routes")
        .and_then(|v| v.as_array())
        .ok_or("facade catalog missing routes")?;
    let mut route_ids: Vec<&str> = routes
        .iter()
        .filter_map(|route| route.get("route").and_then(|v| v.as_str()))
        .collect();
    assert!(
        route_ids.iter().all(|id| !id.contains('*')),
        "wildcard route in the generated catalog"
    );
    route_ids.sort_unstable();
    route_ids.dedup();
    let manifest_routes: BTreeSet<&str> = manifest
        .entries
        .iter()
        .map(|e| e.route.route.as_str())
        .collect();
    for id in route_ids {
        assert!(
            manifest_routes.contains(id),
            "generated catalog route {id} is absent from the manifest"
        );
    }
    Ok(())
}
