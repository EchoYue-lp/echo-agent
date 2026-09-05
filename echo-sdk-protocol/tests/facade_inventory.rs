//! Artifact-level facade inventory and parity-manifest checks.

use std::collections::BTreeSet;
use std::path::PathBuf;

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
            !entry.adapter.operation.is_empty(),
            "missing adapter: {}",
            entry.path
        );
        assert!(
            !entry.semantic_rule.is_empty(),
            "missing semantic rule: {}",
            entry.path
        );
        assert!(
            !entry.adapter.validation.is_empty(),
            "missing validation: {}",
            entry.path
        );
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
    assert_eq!(prelude_resource.adapter, canonical_resource.adapter);
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
        assert_eq!(prelude.adapter, canonical.adapter);
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
