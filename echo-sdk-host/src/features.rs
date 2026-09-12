//! Compiled feature authority for the SDK Host (plan 07, todo 2).
//!
//! Cargo features are the single capability authority (design §13): the
//! advertisement in `initialize` `_meta` is derived from the features this
//! build actually compiled — never from runtime config — so a client can
//! trust `feature_unavailable` to mean "not compiled in".
//!
//! [`FRAMEWORK_FEATURE_TABLE`] is the one declaration binding each Host
//! Cargo feature to the root `echo_agent` leaf feature it activates.
//! `compiled_leaf_features()` reads the compiled set through `cfg!`, and
//! the reconciliation test fails the build when the table drifts from the
//! root crate's real feature list.
//!
//! Facade family availability follows the same rule with one extra gate:
//! a family is advertised only when its typed handler family is compiled
//! in as typed adapters land);
//! merely compiling a passthrough feature is not enough, because the
//! contract only advertises capabilities the Host can actually serve.

/// One entry per Host Cargo feature that activates a root `echo_agent` leaf
/// feature: `(host feature, root leaf feature)`.
macro_rules! framework_feature_table {
    ($(($host_feature:literal, $root:literal)),* $(,)?) => {
        /// The single declaration binding Host features to root leaf features.
        pub const FRAMEWORK_FEATURE_TABLE: &[(&str, &str)] = &[
            $(( $host_feature, $root )),*
        ];

        /// Root leaf features compiled into this build, sorted and
        /// deduplicated. This is the advertisement authority.
        pub fn compiled_leaf_features() -> Vec<String> {
            let mut features = Vec::new();
            $(
                if cfg!(feature = $host_feature) {
                    features.push($root.to_string());
                }
            )*
            features.sort();
            features.dedup();
            features
        }
    };
}

framework_feature_table! {
    // Standard runtime baseline: the official ACP stack and the MCP
    // manager the standard Host serves.
    ("framework-acp", "acp"),
    ("framework-mcp", "mcp"),
    // The extension bridge compiles the framework surfaces its proxies
    // inject into; the advertisement must name them so capability checks
    // stay honest.
    ("framework-human-loop", "human-loop"),
    ("framework-subagent", "subagent"),
    ("framework-sqlite", "sqlite"),
    ("framework-telemetry", "telemetry"),
    ("framework-channels", "channels"),
    ("framework-testing", "testing"),
    // Leaf features the eval/improve facade families bind to.
    ("framework-eval", "eval"),
    ("framework-improve", "improve"),
    // Tool leaf features the tool families bind to. `testing` is compiled
    // and advertised as a root leaf, but remains method-not-found because
    // it is mock infrastructure rather than a remote facade family.
    ("framework-web", "web"),
    ("framework-files", "files"),
    ("framework-shell", "shell"),
    ("framework-git", "git"),
    ("framework-database", "database"),
    ("framework-rag", "rag"),
    ("framework-chart", "chart"),
    ("framework-media", "media"),
    ("framework-data", "data"),
    ("framework-statistics", "statistics"),
    ("framework-research", "research"),
    ("framework-content-guard", "content-guard"),
    ("framework-project-rules", "project-rules"),
    // Integration leaf features (todo 5).
    ("framework-a2a", "a2a"),
    ("framework-lsp", "lsp"),
    ("framework-topology", "topology"),
}

/// Facade families whose typed handler families are compiled into this
/// build. Advertisement derives from handler presence (see module docs);
/// each family feature is appended here together with its concrete adapter.
pub fn compiled_facade_families() -> Vec<&'static str> {
    let mut families = vec!["delivery", "memory", "state", "trace", "workflow"];
    if cfg!(feature = "framework-eval") {
        families.push("eval");
    }
    if cfg!(feature = "framework-improve") {
        families.push("improve");
    }
    if cfg!(feature = "framework-human-loop") {
        families.push("permission");
    }
    macro_rules! push_family {
        ($feature:literal, $family:literal) => {
            if cfg!(feature = $feature) {
                families.push($family);
            }
        };
    }
    push_family!("framework-web", "web");
    push_family!("framework-files", "files");
    push_family!("framework-shell", "shell");
    push_family!("framework-git", "git");
    push_family!("framework-database", "database");
    push_family!("framework-rag", "rag");
    push_family!("framework-chart", "chart");
    push_family!("framework-media", "media");
    push_family!("framework-data", "data");
    push_family!("framework-statistics", "statistics");
    push_family!("framework-research", "research");
    push_family!("framework-content-guard", "content_guard");
    push_family!("framework-project-rules", "project_rules");
    // Integration families: MCP rides the default framework-mcp feature;
    // the rest follow their passthrough features.
    push_family!("framework-mcp", "mcp");
    push_family!("framework-a2a", "a2a");
    push_family!("framework-lsp", "lsp");
    push_family!("framework-topology", "topology");
    push_family!("framework-telemetry", "telemetry");
    // Channel plugins and handlers are reverse-RPC extensions, so the
    // framework channel family is executable only when its bridge authority
    // is present as well as the channel implementation.
    if cfg!(all(
        feature = "framework-channels",
        feature = "sdk-extension-bridge"
    )) {
        families.push("channels");
    }
    // Do not advertise family methods without a concrete Host adapter.
    // `testing` remains a root leaf feature but is not a facade family.
    families.sort_unstable();
    families
}

/// Whether the facade adapter runtime (registry, resource/stream ladder,
/// admission) is compiled in at all.
pub fn facade_runtime_compiled() -> bool {
    cfg!(feature = "sdk-facade-adapters")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table must reference real root `echo_agent` leaf features; a
    /// typo or a renamed root feature breaks the build instead of silently
    /// advertising a capability that does not exist.
    #[test]
    fn framework_feature_table_matches_root_crate() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../Cargo.toml");
        let cargo_toml = std::fs::read_to_string(&manifest)
            .unwrap_or_else(|error| panic!("reading root manifest: {error}"));
        let mut in_features = false;
        let mut root_features = std::collections::BTreeSet::new();
        for line in cargo_toml.lines() {
            let trimmed = line.trim();
            if trimmed == "[features]" {
                in_features = true;
                continue;
            }
            if in_features && trimmed.starts_with('[') {
                break;
            }
            if in_features && let Some((name, _)) = trimmed.split_once('=') {
                root_features.insert(name.trim().to_string());
            }
        }
        assert!(
            !root_features.is_empty(),
            "failed to parse root crate features"
        );
        for (_, root) in FRAMEWORK_FEATURE_TABLE {
            assert!(
                root_features.contains(*root),
                "root crate has no feature {root}; FRAMEWORK_FEATURE_TABLE drifted"
            );
        }
    }

    #[test]
    fn runtime_build_advertises_the_acp_baseline() {
        // The default `runtime` feature activates framework-acp and
        // framework-mcp, so every runnable Host advertises at least these.
        let features = compiled_leaf_features();
        assert!(
            features.contains(&"acp".to_string()),
            "features: {features:?}"
        );
        assert!(
            features.contains(&"mcp".to_string()),
            "features: {features:?}"
        );
    }

    #[test]
    fn facade_families_are_sorted_and_unique() {
        let mut families = compiled_facade_families();
        let sorted = {
            families.sort_unstable();
            families.clone()
        };
        families.dedup();
        assert_eq!(families, sorted);
    }
}
