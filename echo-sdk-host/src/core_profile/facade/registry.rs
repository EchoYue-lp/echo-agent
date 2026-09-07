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
//! Rust framework services (design §10.4); family handlers (todos 3–5)
//! dispatch through [`crate::features::compiled_facade_families`].

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
    pub required_feature: Option<String>,
}

/// Summary of one family method binding (`method -> family`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FamilyMethodSummary {
    pub family: String,
    pub required_feature: Option<String>,
}

pub(crate) struct CompiledOperationCatalog {
    family_methods: HashMap<String, FamilyMethodSummary>,
    invoke_routes: HashMap<String, InvokeRouteSummary>,
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
        for family in families {
            let name = family
                .get("family")
                .and_then(|value| value.as_str())
                .ok_or("embedded facade catalog family without a name")?;
            let required_feature = family
                .get("required_feature")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            let summary = FamilyMethodSummary {
                family: name.to_string(),
                required_feature,
            };
            for method in family
                .get("methods")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
                .filter_map(|value| value.as_str())
            {
                family_methods.insert(method.to_string(), summary.clone());
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
            invoke_routes.insert(
                operation.to_string(),
                InvokeRouteSummary {
                    family,
                    required_feature,
                },
            );
        }
        if family_methods.is_empty() || invoke_routes.is_empty() {
            return Err("embedded facade catalog is missing its route tables".to_string());
        }
        Ok(Self {
            family_methods,
            invoke_routes,
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
        assert!(catalog.invoke_route("_echo_agent/task/*").is_none());
        assert!(catalog.invoke_route("totally::unknown::op").is_none());
    }
}
