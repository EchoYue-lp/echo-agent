//! Artifact-level facade inventory and parity-manifest checks.

use std::collections::BTreeSet;
use std::path::PathBuf;

use echo_sdk_protocol::facade::{FACADE_FAMILIES, validate_facade_route_table};
use echo_sdk_protocol::inventory::{
    AcpRelationship, FeatureSemantics, ItemKind, ManifestEntry, ParityManifest, SemanticClass,
};
use sha2::{Digest, Sha256};

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
fn language_mapping_status_matches_the_route_boundary() -> TestResult {
    let manifest = manifest()?;
    assert!(
        !manifest.entries.is_empty(),
        "facade manifest must not be empty"
    );
    for entry in manifest.entries.iter().filter(|entry| entry.canonical) {
        for (language, mapping) in &entry.languages {
            if entry.route.surface != "intrinsic" {
                assert_eq!(
                    mapping.status,
                    echo_sdk_protocol::inventory::LanguageImplementationStatus::Done,
                    "{language}: {}",
                    entry.path
                );
            }
            assert!(
                mapping.contract_test.starts_with("sdk-parity/"),
                "{} has no language parity contract test",
                entry.path
            );
        }
    }
    Ok(())
}

#[test]
fn local_tool_value_intrinsics_have_language_behavior_evidence() -> TestResult {
    let manifest = manifest()?;
    let entries: Vec<_> = manifest
        .entries
        .iter()
        .filter(|entry| {
            entry.canonical && entry.route.route == "intrinsic:language-local-wire-helper"
        })
        .collect();
    assert_eq!(entries.len(), 28, "local tool-value intrinsic set drifted");
    for entry in entries {
        for (language, mapping) in &entry.languages {
            assert_eq!(
                mapping.status,
                echo_sdk_protocol::inventory::LanguageImplementationStatus::Done,
                "{language}: {}",
                entry.path
            );
            assert!(
                mapping.contract_test.ends_with("/local_tool_values"),
                "{language}: {} has the wrong contract test",
                entry.path
            );
        }
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
    // Every family actually referenced by a canonical manifest item must
    // have a registered descriptor. A descriptor may legitimately have no
    // current root item: protocol-native tool families still own closed
    // operations after their Rust construction types become language-local.
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
    let registered_families: BTreeSet<&str> = FACADE_FAMILIES
        .iter()
        .map(|descriptor| descriptor.family.as_str())
        .collect();
    for family in items_by_family.keys() {
        assert!(
            registered_families.contains(family),
            "canonical manifest family {family} has no route descriptor"
        );
    }
    Ok(())
}

#[test]
fn intrinsic_routes_are_an_explicit_frozen_snapshot() -> TestResult {
    const EXPECTED: &str = "32942a67d6f4bd225c7ce6ed0ce55aed284954145ee88e2f84d7fa1e69921501";
    let mut routes = manifest()?
        .entries
        .into_iter()
        .filter(|entry| entry.canonical && entry.route.route.starts_with("intrinsic:"))
        .map(|entry| {
            format!(
                "{}\t{}\t{}",
                entry.kind.as_str(),
                entry.source_paths.into_iter().collect::<Vec<_>>().join(","),
                entry.route.route
            )
        })
        .collect::<Vec<_>>();
    routes.sort();
    let mut frozen = routes.join("\n");
    frozen.push('\n');
    let digest = format!("{:x}", Sha256::digest(frozen.as_bytes()));
    assert_eq!(
        digest, EXPECTED,
        "intrinsic facade membership changed; review each new/removed identity and update the frozen snapshot deliberately"
    );
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
    for descriptor in FACADE_FAMILIES
        .iter()
        .filter(|descriptor| !descriptor.family.operations().is_empty())
    {
        let route_id = format!("family:{}", descriptor.family.as_str());
        assert!(
            routes.iter().any(|route| {
                route.get("route").and_then(serde_json::Value::as_str) == Some(route_id.as_str())
            }),
            "closed family operations have no executable catalog route: {}",
            descriptor.family.as_str()
        );
    }
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
        if !manifest_routes.contains(id)
            && let Some(family) = id.strip_prefix("family:")
        {
            let route = routes
                .iter()
                .find(|route| route.get("route").and_then(|value| value.as_str()) == Some(id))
                .ok_or("generated catalog is missing a synthetic family route")?;
            let descriptor = FACADE_FAMILIES
                .iter()
                .find(|descriptor| descriptor.family.as_str() == family)
                .ok_or("synthetic family route has no descriptor")?;
            let operations: BTreeSet<&str> = route
                .get("operation_signatures")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|entry| entry.get("operation").and_then(serde_json::Value::as_str))
                .collect();
            let expected: BTreeSet<&str> = descriptor.family.operations().iter().copied().collect();
            assert_eq!(
                operations, expected,
                "protocol-owned family operations drifted for {family}"
            );
            assert_eq!(
                route.get("method").and_then(serde_json::Value::as_str),
                descriptor.methods.first().copied(),
                "synthetic family route method drifted for {family}"
            );
            assert_eq!(
                route.get("items").and_then(serde_json::Value::as_u64),
                Some(0),
                "synthetic family route must not fabricate manifest items"
            );
            continue;
        }
        assert!(
            manifest_routes.contains(id),
            "generated catalog route {id} is absent from the manifest"
        );
    }
    for route in routes {
        if matches!(
            route.get("surface").and_then(|value| value.as_str()),
            Some("invoke") | Some("family")
        ) {
            assert_eq!(
                route
                    .get("input")
                    .and_then(|value| value.get("encoding"))
                    .and_then(|value| value.as_str()),
                Some("wire_value_array"),
                "operation route is missing its wire input description"
            );
            assert_eq!(
                route
                    .get("result")
                    .and_then(|value| value.get("encoding"))
                    .and_then(|value| value.as_str()),
                Some("wire_value"),
                "operation route is missing its wire result description"
            );
        }
    }
    Ok(())
}

/// Plan 08: the extension obligation set is closed. Every
/// bridge-routed manifest item carries a typed kind the Host actually has
/// a proxy for, every typed kind is reachable from at least one canonical
/// trait identity, and the pre-closure `bridge:pending` middle state no
/// longer exists anywhere in the contract.
#[test]
fn extension_obligations_form_a_closed_bridge_set() -> TestResult {
    use echo_sdk_protocol::facade::typed_bridge_kinds;
    use echo_sdk_protocol::methods::ExtensionKind;

    let manifest = manifest()?;
    let typed: BTreeSet<String> = typed_bridge_kinds()
        .into_iter()
        .map(|kind| kind.as_str().to_string())
        .collect();
    // Hook registers by event descriptor, not by trait identity; it is the
    // only descriptor-routed kind. Everything else must come from the table.
    let mut expected = typed.clone();
    expected.insert(ExtensionKind::Hook.as_str().to_string());
    let all_kinds: BTreeSet<String> = [
        ExtensionKind::Tool,
        ExtensionKind::LlmClient,
        ExtensionKind::Store,
        ExtensionKind::HumanLoopProvider,
        ExtensionKind::Hook,
        ExtensionKind::AgentCallback,
        ExtensionKind::InterventionCallback,
        ExtensionKind::AgentFactory,
        ExtensionKind::CustomAgent,
        ExtensionKind::Critic,
        ExtensionKind::ChannelPlugin,
        ExtensionKind::ChannelMessageHandler,
        ExtensionKind::ContextCompressor,
        ExtensionKind::AgentComponent,
    ]
    .into_iter()
    .map(|kind| kind.as_str().to_string())
    .collect();
    assert_eq!(
        expected, all_kinds,
        "typed trait kinds + descriptor-routed Hook must cover every wire kind"
    );

    let mut routed_kinds: BTreeSet<String> = BTreeSet::new();
    for entry in &manifest.entries {
        let route = entry.route.route.as_str();
        assert!(
            route != "bridge:pending",
            "pending bridge route reopened on {}: {}",
            entry.path,
            route
        );
        if let Some(kind) = route.strip_prefix("bridge:")
            && entry.canonical
        {
            assert!(
                all_kinds.contains(kind),
                "unknown bridge kind '{kind}' on {}",
                entry.path
            );
            assert!(
                typed.contains(kind),
                "kind {} has no typed trait identity and is not Hook",
                kind
            );
            routed_kinds.insert(kind.to_string());
        }
    }
    assert_eq!(
        routed_kinds, typed,
        "every typed trait kind must be routed by at least one canonical item"
    );
    Ok(())
}

#[test]
fn canonical_consumer_traits_and_streams_have_executable_routes() -> TestResult {
    let manifest = manifest()?;
    for entry in manifest.entries.iter().filter(|entry| entry.canonical) {
        if entry.classification == SemanticClass::Extension {
            assert!(
                matches!(entry.route.surface.as_str(), "bridge" | "intrinsic"),
                "consumer trait {} is mislabeled as an executable family/core route: {}",
                entry.path,
                entry.route.route
            );
            if entry.route.surface == "intrinsic" {
                assert!(
                    entry.route.route.starts_with("intrinsic:process-local-"),
                    "consumer trait {} lacks explicit process-local evidence: {}",
                    entry.path,
                    entry.route.route
                );
            }
        }
        if entry.classification == SemanticClass::Stream {
            assert!(
                matches!(
                    entry.route.surface.as_str(),
                    "bridge" | "core" | "family" | "intrinsic" | "invoke"
                ),
                "stream {} has no executable or evidenced route",
                entry.path
            );
        }
    }
    let workflow = echo_sdk_protocol::facade::FacadeFamily::Workflow.operations();
    for operation in [
        "workflow.graph.run_stream",
        "workflow.stream.next",
        "workflow.stream.cancel",
        "workflow.stream.close",
    ] {
        assert!(
            workflow.contains(&operation),
            "missing workflow stream operation {operation}"
        );
    }
    let a2a = echo_sdk_protocol::facade::FacadeFamily::A2a.operations();
    for operation in [
        "a2a.task.stream.open",
        "a2a.stream.next",
        "a2a.stream.cancel",
        "a2a.stream.close",
    ] {
        assert!(
            a2a.contains(&operation),
            "missing A2A stream operation {operation}"
        );
    }
    Ok(())
}

#[test]
fn source_family_operations_are_exact_and_type_scoped() -> TestResult {
    let manifest = manifest()?;
    let mut mapped = 0_usize;
    for entry in manifest.entries.iter().filter(|entry| entry.canonical) {
        let Some(handler_operation) = entry.route.handler_operation.as_deref() else {
            continue;
        };
        mapped = mapped.saturating_add(1);
        assert_eq!(
            entry.route.surface, "invoke",
            "mapped source route must use invoke"
        );
        assert_eq!(
            entry.route.method.as_deref(),
            Some("_echo_agent/facade/invoke"),
            "mapped source route must retain the source-signature admission method"
        );
        let canonical_source = entry
            .source_paths
            .iter()
            .next()
            .cloned()
            .unwrap_or_else(|| entry.path.clone());
        assert_eq!(
            entry.route.operation.as_deref(),
            Some(canonical_source.as_str()),
            "mapped source route must preserve its exact canonical identity"
        );
        let family = entry
            .route
            .family
            .as_deref()
            .ok_or("mapped route has no family")?;
        let descriptor = FACADE_FAMILIES
            .iter()
            .find(|descriptor| descriptor.family.as_str() == family)
            .ok_or("mapped route has no family descriptor")?;
        assert!(
            descriptor.family.operations().contains(&handler_operation),
            "mapped source route {} targets an operation absent from family {family}: {handler_operation}",
            entry.path
        );
        assert!(
            !entry.path.contains("McpServerBuilder")
                && !entry.path.contains("PermissionServiceBuilder")
                && !entry.path.contains("WsClient"),
            "same-named construction or transport method was mapped as a runtime family operation: {}",
            entry.path
        );
    }
    assert!(
        mapped > 0,
        "the catalog must contain source-to-family operation mappings"
    );
    Ok(())
}

#[test]
fn canonical_intrinsic_routes_do_not_use_generic_fallback_reasons() -> TestResult {
    let manifest = manifest()?;
    let forbidden = [
        "intrinsic:callable-operation",
        "intrinsic:consumer-implemented-trait",
        "intrinsic:language-local-process-helper",
        "intrinsic:language-local-pure-helper",
        "intrinsic:language-local-react-agent-helper",
        "intrinsic:process-local-explicit-type-seam",
    ];
    for entry in manifest.entries.iter().filter(|entry| entry.canonical) {
        assert!(
            !forbidden.contains(&entry.route.route.as_str()),
            "canonical item {} uses a generic intrinsic fallback: {}",
            entry.path,
            entry.route.route
        );
    }
    Ok(())
}
