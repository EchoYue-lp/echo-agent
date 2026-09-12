//! Compiled facade operation registry (plan 07, todo 2).
//!
//! The generated `contracts/sdk/facade-operation-catalog.json` (plan 07
//! todo 1) is the executable route authority: every remote operation, its
//! family and its required root leaf feature. The Host embeds the
//! committed artifact at compile time — the same bytes the contract drift
//! gate verifies — so runtime admission never guesses execution semantics
//! from paths; it resolves the exact operation identity through this
//! registry.
//!
//! The registry owns *addressing* only: which family a method or operation
//! belongs to and which feature gates it. Business state stays with the
//! Rust framework services (design §10.4); family and source-operation
//! handlers dispatch through the compiled facade family set and the explicit
//! source-operation adapter.

use std::collections::HashMap;
use std::sync::OnceLock;

/// The committed canonical operation catalog, embedded verbatim. The
/// contract drift check (`scripts/check-sdk-contracts.sh`) guarantees this
/// copy matches what the current sources regenerate.
const EMBEDDED_CATALOG_JSON: &str =
    include_str!("../../../../contracts/sdk/facade-operation-catalog.json");

/// Summary of one canonical invoke route, resolved by exact operation
/// identity (never a wildcard).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InvokeRouteSummary {
    pub family: String,
    pub handler_operation: Option<String>,
    pub required_feature: Option<String>,
    pub required_features: Vec<String>,
    pub feature_semantics: String,
    pub signature_digests: Vec<String>,
}

/// Summary of one family method binding (`method -> family`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FamilyMethodSummary {
    pub family: String,
    pub required_feature: Option<String>,
    /// Feature requirement of the family surface as frozen in the catalog
    /// (all-of/any-of semantics); enforced with the same rules as invoke
    /// routes so advertisement, preflight and handler share one authority.
    pub required_features: Vec<String>,
    pub feature_semantics: String,
    pub operations: Vec<String>,
    pub operation_signatures: HashMap<String, Vec<String>>,
}

pub(crate) struct CompiledOperationCatalog {
    family_methods: HashMap<String, FamilyMethodSummary>,
    invoke_routes: HashMap<String, InvokeRouteSummary>,
    /// Operation → family binding for the generic invoke surface: a family
    /// operation identity may be invoked through `_echo_agent/facade/invoke`
    /// and dispatches to the same family handler as the `<family>/op`
    /// method.
    operation_family: HashMap<String, String>,
}

impl CompiledOperationCatalog {
    fn load() -> Result<Self, String> {
        let document: serde_json::Value = serde_json::from_str(EMBEDDED_CATALOG_JSON)
            .map_err(|error| format!("embedded facade catalog is not valid JSON: {error}"))?;
        let families = document
            .get("families")
            .and_then(|value| value.as_array())
            .ok_or("embedded facade catalog missing families")?;
        let mut family_methods = HashMap::new();
        let mut operation_family = HashMap::new();
        let mut operation_signatures_of_method: HashMap<String, HashMap<String, Vec<String>>> =
            HashMap::new();
        // Family feature requirements live on the `family`-surface routes
        // (all-of/any-of semantics); collect them before building the
        // method summaries so the admission ladder can enforce the same
        // feature rules for family methods as for invoke routes.
        let mut feature_of_method: HashMap<String, (Vec<String>, String)> = HashMap::new();
        for route in document
            .get("routes")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
        {
            if route.get("surface").and_then(|value| value.as_str()) != Some("family") {
                continue;
            }
            let Some(method) = route
                .get("method")
                .and_then(|value| value.as_str())
                .filter(|method| !method.is_empty())
            else {
                continue;
            };
            let required_features = route
                .get("required_features")
                .and_then(|value| value.as_array())
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|value| value.as_str().map(str::to_string))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let feature_semantics = route
                .get("feature_semantics")
                .and_then(|value| value.as_str())
                .unwrap_or("default")
                .to_string();
            feature_of_method.insert(method.to_string(), (required_features, feature_semantics));
            let entries = route
                .get("operation_signatures")
                .and_then(|value| value.as_array())
                .ok_or_else(|| format!("family route {method} has no operation signatures"))?;
            let mut signatures = HashMap::new();
            for entry in entries {
                let operation = entry
                    .get("operation")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| format!("family route {method} has an invalid operation"))?;
                let digests = entry
                    .get("signature_digests")
                    .and_then(|value| value.as_array())
                    .ok_or_else(|| format!("family operation {operation} has no digests"))?
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_string))
                    .collect::<Vec<_>>();
                if entry
                    .get("input")
                    .and_then(|value| value.get("encoding"))
                    .and_then(|value| value.as_str())
                    != Some("wire_value_array")
                    || entry
                        .get("result")
                        .and_then(|value| value.get("encoding"))
                        .and_then(|value| value.as_str())
                        != Some("wire_value")
                {
                    return Err(format!(
                        "family operation {operation} is missing its wire input/result description"
                    ));
                }
                signatures.insert(operation.to_string(), digests);
            }
            operation_signatures_of_method.insert(method.to_string(), signatures);
        }
        for family in families {
            let name = family
                .get("family")
                .and_then(|value| value.as_str())
                .ok_or("embedded facade catalog family without a name")?;
            let required_feature = family
                .get("required_feature")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            let operations: Vec<String> = family
                .get("operations")
                .and_then(|value| value.as_array())
                .ok_or_else(|| format!("family {name} has no operation list"))?
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect();
            for operation in &operations {
                operation_family.insert(operation.clone(), name.to_string());
            }
            let methods = family
                .get("methods")
                .and_then(|value| value.as_array())
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|value| value.as_str())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let family_summary = FamilyMethodSummary {
                family: name.to_string(),
                required_feature,
                required_features: Vec::new(),
                feature_semantics: "default".to_string(),
                operations: operations.clone(),
                operation_signatures: HashMap::new(),
            };
            for method in methods {
                let (required_features, feature_semantics) =
                    feature_of_method.remove(method).unwrap_or_default();
                // Protocol-native families (agent/session/run/task/… with no
                // `<family>/op` surface route) carry an empty signature map:
                // their typed DTOs are the contract, not family operations.
                let operation_signatures = operation_signatures_of_method
                    .remove(method)
                    .unwrap_or_default();
                family_methods.insert(
                    method.to_string(),
                    FamilyMethodSummary {
                        required_features,
                        feature_semantics,
                        operation_signatures,
                        ..family_summary.clone()
                    },
                );
            }
        }
        let routes = document
            .get("routes")
            .and_then(|value| value.as_array())
            .ok_or("embedded facade catalog missing routes")?;
        let mut invoke_routes = HashMap::new();
        for route in routes {
            if route.get("surface").and_then(|value| value.as_str()) != Some("invoke") {
                continue;
            }
            let Some(operation) = route
                .get("operation")
                .and_then(|value| value.as_str())
                .filter(|operation| !operation.is_empty())
            else {
                return Err("embedded facade catalog has an invoke route without an exact operation identity".to_string());
            };
            let family = route
                .get("family")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            if family.is_empty() {
                return Err(format!(
                    "invoke route {operation} has no family; the catalog drifted"
                ));
            }
            let required_feature = route
                .get("required_feature")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            let handler_operation = route
                .get("handler_operation")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            let required_features = route
                .get("required_features")
                .and_then(|value| value.as_array())
                .ok_or_else(|| format!("invoke route {operation} has no feature list"))?
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect::<Vec<_>>();
            let feature_semantics = route
                .get("feature_semantics")
                .and_then(|value| value.as_str())
                .ok_or_else(|| format!("invoke route {operation} has no feature semantics"))?
                .to_string();
            let signature_digests = route
                .get("signature_digests")
                .and_then(|value| value.as_array())
                .ok_or_else(|| format!("invoke route {operation} has no signature digest list"))?
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect::<Vec<_>>();
            if route
                .get("input")
                .and_then(|value| value.get("encoding"))
                .and_then(|value| value.as_str())
                != Some("wire_value_array")
                || route
                    .get("result")
                    .and_then(|value| value.get("encoding"))
                    .and_then(|value| value.as_str())
                    != Some("wire_value")
            {
                return Err(format!(
                    "invoke route {operation} is missing its wire input/result description"
                ));
            }
            invoke_routes.insert(
                operation.to_string(),
                InvokeRouteSummary {
                    family,
                    handler_operation,
                    required_feature,
                    required_features,
                    feature_semantics,
                    signature_digests,
                },
            );
        }
        if family_methods.is_empty() || invoke_routes.is_empty() {
            return Err("embedded facade catalog is missing its route tables".to_string());
        }
        Ok(Self {
            family_methods,
            invoke_routes,
            operation_family,
        })
    }

    /// The process-wide parsed catalog. A malformed embedded artifact is a
    /// build/contract failure, surfaced as a configuration error.
    pub(crate) fn global() -> Result<&'static Self, String> {
        static CATALOG: OnceLock<Result<CompiledOperationCatalog, String>> = OnceLock::new();
        CATALOG
            .get_or_init(Self::load)
            .as_ref()
            .map_err(|error| error.clone())
    }

    /// Family binding of one wire method, when the method belongs to a
    /// facade family surface.
    pub(crate) fn family_method(&self, method: &str) -> Option<&FamilyMethodSummary> {
        self.family_methods.get(method)
    }

    /// Exact-identity invoke route resolution; unknown operations return
    /// `None` and fail closed as `invalid_value`.
    pub(crate) fn invoke_route(&self, operation: &str) -> Option<&InvokeRouteSummary> {
        self.invoke_routes.get(operation)
    }

    /// Family of one closed family-operation identity, when the generic
    /// invoke surface addresses it (`None` for source identities).
    pub(crate) fn family_for_operation(&self, operation: &str) -> Option<&str> {
        self.operation_family.get(operation).map(String::as_str)
    }

    /// One deterministic sample identity from the invoke table (tests).
    #[cfg(test)]
    pub(crate) fn invoke_routes_sample(&self) -> Option<String> {
        let mut keys: Vec<&String> = self.invoke_routes.keys().collect();
        keys.sort();
        keys.first().map(|key| (*key).clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_catalog_parses_with_both_route_tables() {
        let catalog = CompiledOperationCatalog::global()
            .unwrap_or_else(|error| panic!("embedded catalog failed to parse: {error}"));
        // Family methods from the frozen catalog resolve with their feature.
        let memory = catalog
            .family_method("_echo_agent/memory/op")
            .expect("memory family method");
        assert_eq!(memory.family, "memory");
        assert_eq!(memory.required_feature, None);
        let eval = catalog
            .family_method("_echo_agent/eval/op")
            .expect("eval family method");
        assert_eq!(eval.family, "eval");
        assert_eq!(eval.required_feature.as_deref(), Some("eval"));
        // Exact invoke identities resolve; wildcards never do. Sample one
        // real identity from the catalog instead of pinning a facade path
        // that the inventory may legitimately rename.
        let sample = catalog
            .invoke_routes_sample()
            .expect("catalog carries invoke identities");
        let route = catalog
            .invoke_route(sample.as_str())
            .unwrap_or_else(|| panic!("sampled identity {sample} must resolve"));
        assert!(!route.family.is_empty());
        let source = catalog
            .invoke_route("echo_core::agent::Agent::current_run_id")
            .expect("source-operation route must resolve");
        assert_eq!(source.family, "source_operation");
        assert!(!source.signature_digests.is_empty());
        assert!(catalog.invoke_route("_echo_agent/task/*").is_none());
        assert!(catalog.invoke_route("totally::unknown::op").is_none());
    }

    #[test]
    fn family_operations_resolve_through_the_generic_invoke_binding() {
        let catalog = CompiledOperationCatalog::global()
            .unwrap_or_else(|error| panic!("embedded catalog failed to parse: {error}"));
        assert_eq!(
            catalog.family_for_operation("memory.store.put"),
            Some("memory")
        );
        assert_eq!(
            catalog.family_for_operation("workflow.graph.run"),
            Some("workflow")
        );
        assert_eq!(catalog.family_for_operation("totally::unknown::op"), None);
        assert_eq!(
            catalog.family_for_operation("echo_agent::agent::Agent"),
            None
        );
    }

    #[test]
    fn family_method_summaries_carry_frozen_feature_semantics() {
        let catalog = CompiledOperationCatalog::global()
            .unwrap_or_else(|error| panic!("embedded catalog failed to parse: {error}"));
        // Feature-free families fall back to the default (vacuous all-of).
        let memory = catalog
            .family_method("_echo_agent/memory/op")
            .expect("memory family method");
        assert!(memory.required_features.is_empty());
        assert_eq!(memory.feature_semantics, "default");
        // Feature-gated families carry the frozen any-of requirement the
        // admission ladder enforces for family methods too.
        let eval = catalog
            .family_method("_echo_agent/eval/op")
            .expect("eval family method");
        assert_eq!(eval.required_features, vec!["eval".to_string()]);
        assert_eq!(eval.feature_semantics, "any_of");
    }
}
