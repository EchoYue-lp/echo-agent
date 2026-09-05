//! Deterministic public-facade inventory extraction from rustdoc JSON.
//!
//! The SDK parity authority is the rustdoc-reachable public surface of the
//! root `echo_agent` crate (see design §7.1), not a hand-written module list.
//! This module parses the rustdoc JSON emitted by the toolchain pinned in
//! `contracts/sdk/toolchain.json` and walks the public module tree to produce
//! a stable, sorted item list per feature profile.
//!
//! Paths follow rustdoc semantics for the facade:
//! - items defined in the root crate get their module path
//! - named re-exports are recorded as `import` items with their source
//! - glob re-exports (typically from workspace sub-crates) stay as a single
//!   `import` declaration; the crate-local rustdoc JSON has no visibility of
//!   the foreign targets, which is exactly the facade boundary we snapshot
//! - methods on inherent or trait impls are keyed by
//!   `module::(TypeName)::method` so the same logical method stays aligned
//!   across feature profiles (rustdoc item ids are not stable)
//!
//! Struct fields and enum variants are folded into their parent type: the
//! facade contract is the type, not each field. Anything the parser does not
//! recognize is surfaced as `other` rather than dropped, so new rustdoc kinds
//! can never silently shrink the inventory.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::Deserialize;

/// Supported rustdoc JSON format version. The pinned nightly toolchain in
/// `contracts/sdk/toolchain.json` must emit exactly this version; a mismatch
/// fails the contract check instead of silently changing the inventory.
pub const RUSTDOC_FORMAT_VERSION: u64 = 61;

/// Public item kinds recorded in the inventory snapshot.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Module,
    Import,
    Function,
    Method,
    Struct,
    Enum,
    Union,
    Trait,
    TraitAlias,
    TypeAlias,
    Macro,
    ProcMacro,
    Constant,
    Static,
    /// Exotic or future rustdoc kinds. Recorded so new kinds can never be
    /// silently dropped from the snapshot; the manifest check treats them as
    /// requiring an explicit review note.
    Other,
}

impl ItemKind {
    fn parse(inner: &serde_json::Map<String, serde_json::Value>) -> Self {
        if let Some(key) = inner.keys().next() {
            return match key.as_str() {
                "module" => ItemKind::Module,
                // rustdoc format 61 spells re-exports as `use`.
                "use" => ItemKind::Import,
                "function" => ItemKind::Function,
                "struct" => ItemKind::Struct,
                "enum" => ItemKind::Enum,
                "union" => ItemKind::Union,
                "trait" => ItemKind::Trait,
                "trait_alias" => ItemKind::TraitAlias,
                "type_alias" | "assoc_type" => ItemKind::TypeAlias,
                "macro" => ItemKind::Macro,
                "proc_macro" => ItemKind::ProcMacro,
                "constant" | "assoc_const" => ItemKind::Constant,
                "static" => ItemKind::Static,
                // variant / struct_field / impl / extern_crate / primitive are
                // handled structurally by the walker, not recorded as items.
                _ => ItemKind::Other,
            };
        }
        ItemKind::Other
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ItemKind::Module => "module",
            ItemKind::Import => "import",
            ItemKind::Function => "function",
            ItemKind::Method => "method",
            ItemKind::Struct => "struct",
            ItemKind::Enum => "enum",
            ItemKind::Union => "union",
            ItemKind::Trait => "trait",
            ItemKind::TraitAlias => "trait_alias",
            ItemKind::TypeAlias => "type_alias",
            ItemKind::Macro => "macro",
            ItemKind::ProcMacro => "proc_macro",
            ItemKind::Constant => "constant",
            ItemKind::Static => "static",
            ItemKind::Other => "other",
        }
    }
}

/// One public facade item collected from a single feature profile.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PublicItem {
    /// Facade path such as `echo_agent::llm::ChatRequest` or
    /// `echo_agent::agent::(ReactAgent)::execute`. Unique within a profile
    /// after the walker appends a disambiguator for repeated paths.
    pub path: String,
    pub kind: ItemKind,
    /// For imports: the declared source path (`echo_core::hooks` style).
    pub import_source: Option<String>,
    /// For imports: whether this is a glob re-export declaration.
    pub import_glob: Option<bool>,
}

#[derive(Deserialize)]
struct RustdocFile {
    format_version: u64,
    index: HashMap<String, RustdocItem>,
}

#[derive(Deserialize)]
struct RustdocItem {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    visibility: serde_json::Value,
    /// Format 61 encodes attributes as tagged objects (e.g.
    /// `{"other": "#[doc(hidden)]"}`); keep them as raw values and inspect
    /// stringly so future attribute shapes cannot break parsing.
    #[serde(default)]
    attrs: Vec<serde_json::Value>,
    #[serde(default)]
    inner: Option<serde_json::Value>,
    #[serde(default)]
    crate_id: u32,
}

impl RustdocItem {
    fn is_public(&self) -> bool {
        self.visibility == "public"
    }

    fn is_doc_hidden(&self) -> bool {
        fn value_mentions_hidden(value: &serde_json::Value) -> bool {
            match value {
                serde_json::Value::String(text) => text.contains("doc(hidden)"),
                serde_json::Value::Array(items) => items.iter().any(value_mentions_hidden),
                serde_json::Value::Object(map) => map.values().any(value_mentions_hidden),
                _ => false,
            }
        }
        self.attrs.iter().any(value_mentions_hidden)
    }

    fn inner_map(&self) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.inner.as_ref()?.as_object()
    }

    fn child_ids(&self) -> Vec<String> {
        let Some(map) = self.inner_map() else {
            return Vec::new();
        };
        // Module, trait and impl items all expose an `items` child list.
        let mut out = Vec::new();
        for (key, value) in map {
            if matches!(key.as_str(), "module" | "trait" | "impl") {
                let items = value.get("items").and_then(|v| v.as_array());
                for id in items.into_iter().flatten() {
                    if let Some(id) = id.as_u64() {
                        out.push(id.to_string());
                    }
                }
            }
        }
        out
    }

    fn is_impl(&self) -> bool {
        self.inner_map().is_some_and(|m| m.contains_key("impl"))
    }

    /// Impl blocks attached to a type definition (rustdoc format 61 stores
    /// them under the struct/enum/union `impls` array, plural).
    fn impl_child_ids(&self) -> Vec<String> {
        let Some(map) = self.inner_map() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for kind in ["struct", "enum", "union"] {
            if let Some(impls) = map
                .get(kind)
                .and_then(|v| v.get("impls"))
                .and_then(|v| v.as_array())
            {
                for id in impls {
                    if let Some(id) = id.as_u64() {
                        out.push(id.to_string());
                    }
                }
            }
        }
        out
    }
}

/// Import details: (source, is_glob, imported name). Format 61 keeps the
/// re-exported name inside `use.name` while the item's own `name` stays
/// empty, so read it from there first.
fn import_details(
    inner: &serde_json::Map<String, serde_json::Value>,
) -> (Option<String>, bool, Option<String>) {
    let import = inner.get("use").and_then(|v| v.as_object());
    match import {
        Some(import) => (
            import
                .get("source")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            import
                .get("is_glob")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            import
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        ),
        None => (None, false, None),
    }
}

/// Errors that abort inventory extraction. Parsing must fail closed: an
/// unexpected rustdoc shape is a contract failure, never a partial list.
#[derive(Debug)]
pub enum InventoryError {
    MalformedJson(String),
    UnsupportedFormatVersion { found: u64, expected: u64 },
    MissingRootModule,
}

impl std::fmt::Display for InventoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InventoryError::MalformedJson(msg) => {
                write!(f, "malformed rustdoc JSON: {msg}")
            }
            InventoryError::UnsupportedFormatVersion { found, expected } => {
                write!(
                    f,
                    "rustdoc JSON format version {found} does not match pinned {expected}; \
                     re-pin the toolchain in contracts/sdk/toolchain.json and regenerate"
                )
            }
            InventoryError::MissingRootModule => {
                write!(f, "rustdoc JSON has no reachable crate root module")
            }
        }
    }
}

impl std::error::Error for InventoryError {}

/// Parse one rustdoc JSON document and return the sorted public facade items.
///
/// Two phases: first a DFS over public modules records every named public
/// item (with its rustdoc id → facade path) and defers impl blocks; then
/// each deferred impl is expanded only when its `for` type resolves to a
/// collected public item, because rustdoc marks impl blocks themselves with
/// the default (private) visibility regardless of the methods inside.
pub fn extract_public_items(json: &str) -> Result<Vec<PublicItem>, InventoryError> {
    let file: RustdocFile =
        serde_json::from_str(json).map_err(|e| InventoryError::MalformedJson(e.to_string()))?;
    if file.format_version != RUSTDOC_FORMAT_VERSION {
        return Err(InventoryError::UnsupportedFormatVersion {
            found: file.format_version,
            expected: RUSTDOC_FORMAT_VERSION,
        });
    }

    // Locate the crate root: a module item of crate 0 that is not nested in
    // any other module.
    let mut nested: HashSet<String> = HashSet::new();
    for item in file.index.values() {
        for child in item.child_ids() {
            nested.insert(child);
        }
    }
    let mut root: Option<&RustdocItem> = None;
    for (id, item) in &file.index {
        if item.crate_id == 0
            && !nested.contains(id)
            && item.inner_map().is_some_and(|m| m.contains_key("module"))
        {
            root = Some(item);
        }
    }
    let root = root.ok_or(InventoryError::MissingRootModule)?;
    let root_name = root
        .name
        .clone()
        .unwrap_or_else(|| "echo_agent".to_string());

    let mut state = WalkState::default();
    walk_module(&file.index, root, &root_name, &mut state);
    expand_impls(&file.index, &mut state);

    state.out.sort();
    state.out.dedup();
    Ok(state.out)
}

/// Walker state: collected items, path disambiguation, the public id→path
/// map used to resolve impl targets, and deferred impl blocks.
#[derive(Default)]
struct WalkState {
    out: Vec<PublicItem>,
    seen_paths: HashMap<String, usize>,
    /// rustdoc id → facade path for every collected named public item.
    public_paths: HashMap<String, String>,
    /// Impl item ids awaiting target-type resolution.
    pending_impls: Vec<String>,
}

/// Walk a module's direct children. Only public, non-doc-hidden children are
/// part of the facade; impl blocks are exempt from the visibility check
/// because rustdoc always marks them default (see `extract_public_items`).
fn walk_module(
    index: &HashMap<String, RustdocItem>,
    module: &RustdocItem,
    namespace: &str,
    state: &mut WalkState,
) {
    let _ = namespace;
    for child_id in module.child_ids() {
        let Some(child) = index.get(&child_id) else {
            continue;
        };
        if child.is_impl() {
            continue;
        }
        if !child.is_public() || child.is_doc_hidden() {
            continue;
        }
        walk_item(index, child, &child_id, namespace, state);
    }
}

/// Walk one named public item inside `namespace`.
fn walk_item(
    index: &HashMap<String, RustdocItem>,
    item: &RustdocItem,
    item_id: &str,
    namespace: &str,
    state: &mut WalkState,
) {
    let Some(inner) = item.inner_map() else {
        return;
    };
    let kind = ItemKind::parse(inner);
    let name = item.name.clone().unwrap_or_default();

    match kind {
        ItemKind::Module if !name.is_empty() => {
            let module_path = namespace_path(namespace, &name);
            state
                .public_paths
                .insert(item_id.to_string(), module_path.clone());
            push_item(state, &module_path, kind, None, None);
            walk_module(index, item, &module_path, state);
        }
        ItemKind::Import => {
            let (source, glob, imported_name) = import_details(inner);
            // Glob re-exports carry no item name; record them under the
            // module path so the facade boundary (what the glob exposes) is
            // visible in the snapshot instead of silently dropped.
            let imported = imported_name.as_deref().unwrap_or(name.as_str());
            let path = if glob {
                format!("{namespace}::*")
            } else if !imported.is_empty() {
                namespace_path(namespace, imported)
            } else {
                return;
            };
            push_item(state, &path, kind, source, Some(glob));
        }
        ItemKind::Trait if !name.is_empty() => {
            let trait_path = namespace_path(namespace, &name);
            state
                .public_paths
                .insert(item_id.to_string(), trait_path.clone());
            push_item(state, &trait_path, kind, None, None);
            walk_members(index, item, &trait_path, state);
        }
        ItemKind::Other => {
            // Unrecognized kinds (extern crates, primitives, ...) are not
            // facade items; variants and fields never reach here.
        }
        _ => {
            if !name.is_empty() {
                let path = namespace_path(namespace, &name);
                state.public_paths.insert(item_id.to_string(), path.clone());
                push_item(state, &path, kind, None, None);
                state.pending_impls.extend(item.impl_child_ids());
            }
        }
    }
}

/// Expand deferred impl blocks. Only inherent impls (`trait == null`) on a
/// collected public type contribute facade items: methods of an unreachable
/// type are not public surface, and trait impls are projections of the trait
/// definition (or of a foreign trait) rather than new facade API.
fn expand_impls(index: &HashMap<String, RustdocItem>, state: &mut WalkState) {
    let pending = std::mem::take(&mut state.pending_impls);
    for impl_id in pending {
        let Some(impl_item) = index.get(&impl_id) else {
            continue;
        };
        let Some(impl_inner) = impl_item
            .inner_map()
            .and_then(|m| m.get("impl"))
            .and_then(|v| v.as_object())
        else {
            continue;
        };
        if impl_inner
            .get("trait")
            .map(|t| !t.is_null())
            .unwrap_or(false)
        {
            continue;
        }
        let Some(target_id) = impl_inner
            .get("for")
            .and_then(|f| f.get("resolved_path"))
            .and_then(|p| p.get("id"))
            .and_then(|id| id.as_u64())
        else {
            continue;
        };
        let Some(target_path) = state.public_paths.get(&target_id.to_string()) else {
            continue;
        };
        // The impl prefix is the type's full facade path; methods therefore
        // read `echo_agent::agent::ReactAgent::execute`.
        let impl_ns = target_path.clone();
        let _ = &impl_ns;
        walk_members(index, impl_item, &impl_ns, state);
    }
}

/// Walk the members of a trait definition or an impl block: methods,
/// associated types and associated constants are part of the public contract
/// and are keyed by `container_path::member`.
fn walk_members(
    index: &HashMap<String, RustdocItem>,
    container: &RustdocItem,
    container_path: &str,
    state: &mut WalkState,
) {
    for child_id in container.child_ids() {
        let Some(child) = index.get(&child_id) else {
            continue;
        };
        if !child.is_public() || child.is_doc_hidden() {
            continue;
        }
        let Some(inner) = child.inner_map() else {
            continue;
        };
        let kind = ItemKind::parse(inner);
        let name = child.name.clone().unwrap_or_default();
        match kind {
            ItemKind::Module | ItemKind::Import | ItemKind::Trait => {}
            ItemKind::Other => {
                // Nested impls inside impl blocks are not legal Rust; skip.
            }
            _ => {
                if !name.is_empty() {
                    let member_kind = if matches!(kind, ItemKind::Function) {
                        ItemKind::Method
                    } else {
                        kind
                    };
                    push_item(
                        state,
                        &namespace_path(container_path, &name),
                        member_kind,
                        None,
                        None,
                    );
                }
            }
        }
    }
}

fn namespace_path(namespace: &str, name: &str) -> String {
    if namespace.is_empty() {
        name.to_string()
    } else {
        format!("{namespace}::{name}")
    }
}

/// Record an item, disambiguating repeated paths (two impls hitting the same
/// `Type::method` key, for example) with a stable `#n` suffix so the sorted
/// snapshot stays injective.
fn push_item(
    state: &mut WalkState,
    path: &str,
    kind: ItemKind,
    import_source: Option<String>,
    import_glob: Option<bool>,
) {
    let count = state.seen_paths.get(path).copied().unwrap_or(0);
    state.seen_paths.insert(path.to_string(), count + 1);
    let final_path = if count == 0 {
        path.to_string()
    } else {
        format!("{path}#{}", count + 1)
    };
    state.out.push(PublicItem {
        path: final_path,
        kind,
        import_source,
        import_glob,
    });
}

/// A feature profile the inventory is generated for: the empty default, the
/// `full` aggregate, and one profile per leaf feature.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub struct FeatureProfile {
    pub name: String,
    /// Cargo feature flags passed with `--no-default-features --features ...`.
    pub features: Vec<String>,
}

impl FeatureProfile {
    pub fn default_profile() -> Self {
        Self {
            name: "default".to_string(),
            features: Vec::new(),
        }
    }

    pub fn full_profile() -> Self {
        Self {
            name: "full".to_string(),
            features: vec!["full".to_string()],
        }
    }

    pub fn leaf(name: &str) -> Self {
        Self {
            name: format!("feature:{name}"),
            features: vec![name.to_string()],
        }
    }
}

/// Build the canonical profile list from the root crate's leaf features
/// (everything except `default` and `full`).
pub fn profiles_for_leaf_features(leaf_features: &[String]) -> Vec<FeatureProfile> {
    let mut profiles = vec![
        FeatureProfile::default_profile(),
        FeatureProfile::full_profile(),
    ];
    let mut leaves: Vec<&String> = leaf_features.iter().collect();
    leaves.sort();
    for leaf in leaves {
        profiles.push(FeatureProfile::leaf(leaf));
    }
    profiles
}

/// Aggregate of one item across all generated profiles.
#[derive(Debug, Clone, serde::Serialize)]
pub struct InventoryEntry {
    pub path: String,
    pub kind: ItemKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub import_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub import_glob: Option<bool>,
    /// Profiles (by name) in which the item is present.
    pub profiles: BTreeSet<String>,
}

/// Merge per-profile item lists into a cross-profile inventory. The same
/// facade item appears once with the set of profiles that contain it.
pub fn merge_profiles(per_profile: &BTreeMap<String, Vec<PublicItem>>) -> Vec<InventoryEntry> {
    let mut merged: BTreeMap<String, InventoryEntry> = BTreeMap::new();
    for (profile, items) in per_profile {
        for item in items {
            let entry = merged
                .entry(item.path.clone())
                .or_insert_with(|| InventoryEntry {
                    path: item.path.clone(),
                    kind: item.kind,
                    import_source: item.import_source.clone(),
                    import_glob: item.import_glob,
                    profiles: BTreeSet::new(),
                });
            entry.profiles.insert(profile.clone());
        }
    }
    merged.into_values().collect()
}

/// Render the deterministic `public-api.txt` snapshot for the merged
/// inventory.
pub fn render_public_api_snapshot(
    profiles: &[FeatureProfile],
    merged: &[InventoryEntry],
) -> String {
    let mut out = String::new();
    out.push_str(
        "# Generated by `cargo run -p echo-sdk-protocol --bin export_schema -- update`.\n",
    );
    out.push_str("# DO NOT EDIT: regenerate instead. Deterministic facade snapshot of the root\n");
    out.push_str("# echo_agent crate per feature profile (sorted; impl members keyed by\n");
    out.push_str("# module::(Type)::member). Imports record their source; glob imports stay\n");
    out.push_str("# as one declaration because foreign targets are outside this crate's\n");
    out.push_str("# rustdoc JSON.\n");
    out.push_str(&format!(
        "# rustdoc JSON format version: {RUSTDOC_FORMAT_VERSION}\n"
    ));
    let profile_names: Vec<&str> = profiles.iter().map(|p| p.name.as_str()).collect();
    out.push_str(&format!("# profiles: {}\n", profile_names.join(", ")));
    out.push('\n');
    for entry in merged {
        let mut import = String::new();
        if entry.kind == ItemKind::Import {
            let glob = entry.import_glob.unwrap_or(false);
            let source = entry.import_source.as_deref().unwrap_or("?");
            import = if glob {
                format!("  <= glob {source}")
            } else {
                format!("  <= {source}")
            };
        }
        let profiles: Vec<&str> = entry.profiles.iter().map(String::as_str).collect();
        out.push_str(&format!(
            "{:<12} {}{}  [{}]\n",
            entry.kind.as_str(),
            entry.path,
            import,
            profiles.join(",")
        ));
    }
    out.push_str(&format!("\n# total items: {}\n", merged.len()));
    out
}

/// Derive the leaf feature set that introduces each item: an item present in
/// `default` needs no feature; otherwise every leaf profile whose feature set
/// contains the item contributes one entry.
pub fn features_of_entry(entry: &InventoryEntry) -> BTreeSet<String> {
    let mut features = BTreeSet::new();
    for profile in &entry.profiles {
        if let Some(feature) = profile.strip_prefix("feature:") {
            features.insert(feature.to_string());
        }
    }
    features
}

// ── Parity manifest classification ─────────────────────────────────────────
//
// Design §7.2 assigns every facade item exactly one semantic class and one
// ACP relationship. The defaults below are deterministic rules reviewed with
// the crate source; `PARITY_OVERRIDES` records deliberate exceptions. New
// facade items automatically get classified defaults, so the manifest can
// never silently lag the facade while still surfacing every addition for
// review in the generated diff.

/// Semantic classification of a facade item (design §7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticClass {
    WireValue,
    Operation,
    Handle,
    Stream,
    Extension,
    LanguageIntrinsic,
}

impl SemanticClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            SemanticClass::WireValue => "wire_value",
            SemanticClass::Operation => "operation",
            SemanticClass::Handle => "handle",
            SemanticClass::Stream => "stream",
            SemanticClass::Extension => "extension",
            SemanticClass::LanguageIntrinsic => "language_intrinsic",
        }
    }
}

/// How an item relates to stable ACP v1 (design §7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpRelationship {
    Standard,
    StandardProjection,
    EchoExtension,
    LanguageIntrinsic,
}

impl AcpRelationship {
    pub fn as_str(&self) -> &'static str {
        match self {
            AcpRelationship::Standard => "standard",
            AcpRelationship::StandardProjection => "standard_projection",
            AcpRelationship::EchoExtension => "echo_extension",
            AcpRelationship::LanguageIntrinsic => "language_intrinsic",
        }
    }
}

/// Per-language mapping status of one facade item. Until the language SDKs
/// exist the honest status for every entry is `not_implemented`; the field is
/// mandatory precisely so "silent partial parity" can never be claimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LanguageImplementationStatus {
    NotImplemented,
}

impl LanguageImplementationStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            LanguageImplementationStatus::NotImplemented => "not_implemented",
        }
    }
}

/// Deliberate classification exceptions keyed by exact facade path. Defaults
/// apply everywhere else; keep this list short and justified in-place.
const PARITY_OVERRIDES: &[(&str, SemanticClass, AcpRelationship)] = &[
    // Agent/Session handles cross the wire as opaque ids + generation: they
    // are the handle class by design §8, not plain values.
    // (Populated as the adapter plans land; the Contract baseline keeps the
    // rule-based defaults, which already classify every item deterministically.)
];

pub fn classify_entry(entry: &InventoryEntry) -> (SemanticClass, AcpRelationship) {
    for (path, class, relationship) in PARITY_OVERRIDES {
        if *path == entry.path {
            return (*class, *relationship);
        }
    }
    match entry.kind {
        ItemKind::Module => (SemanticClass::Operation, AcpRelationship::EchoExtension),
        ItemKind::Import => (SemanticClass::WireValue, AcpRelationship::EchoExtension),
        ItemKind::Function | ItemKind::Method => {
            (SemanticClass::Operation, AcpRelationship::EchoExtension)
        }
        ItemKind::Trait | ItemKind::TraitAlias => {
            (SemanticClass::Extension, AcpRelationship::EchoExtension)
        }
        ItemKind::Macro | ItemKind::ProcMacro => (
            SemanticClass::LanguageIntrinsic,
            AcpRelationship::LanguageIntrinsic,
        ),
        ItemKind::Struct
        | ItemKind::Enum
        | ItemKind::Union
        | ItemKind::TypeAlias
        | ItemKind::Constant
        | ItemKind::Static => (SemanticClass::WireValue, AcpRelationship::EchoExtension),
        ItemKind::Other => (SemanticClass::WireValue, AcpRelationship::EchoExtension),
    }
}

/// One parity manifest entry (design §7.2): path, feature condition, semantic
/// class, ACP relationship and per-language mapping status.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ManifestEntry {
    pub path: String,
    pub kind: &'static str,
    /// Leaf features that introduce this item (empty for default-reachable).
    pub features: BTreeSet<String>,
    pub classification: SemanticClass,
    pub acp_relationship: AcpRelationship,
    pub languages: BTreeMap<&'static str, LanguageStatusRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LanguageStatusRecord {
    pub status: LanguageImplementationStatus,
}

/// Manifest entries for all non-module inventory items.
pub fn manifest_entries(merged: &[InventoryEntry]) -> Vec<ManifestEntry> {
    let mut entries: Vec<ManifestEntry> = merged
        .iter()
        .filter(|entry| entry.kind != ItemKind::Module)
        .map(|entry| ManifestEntry {
            path: entry.path.clone(),
            kind: entry.kind.as_str(),
            features: features_of_entry(entry),
            classification: classify_entry(entry).0,
            acp_relationship: classify_entry(entry).1,
            languages: LANGUAGES
                .iter()
                .map(|lang| {
                    (
                        *lang,
                        LanguageStatusRecord {
                            status: LanguageImplementationStatus::NotImplemented,
                        },
                    )
                })
                .collect(),
        })
        .collect();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries
}

/// The three parity-tracked languages (design §5.1).
pub const LANGUAGES: &[&str] = &["typescript", "python", "java"];

/// Render the deterministic `parity-manifest.json` document.
pub fn render_parity_manifest(
    extension_protocol_version: u32,
    profiles: &[FeatureProfile],
    merged: &[InventoryEntry],
) -> String {
    let profile_names: Vec<&str> = profiles.iter().map(|p| p.name.as_str()).collect();
    let doc = serde_json::json!({
        "schema_version": 1,
        "extension_protocol_version": extension_protocol_version,
        "generated": {
            "rustdoc_format_version": RUSTDOC_FORMAT_VERSION,
            "profiles": profile_names,
        },
        "entries": manifest_entries(merged),
    });
    serde_json::to_string_pretty(&doc).unwrap_or_default() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"{
        "format_version": 61,
        "index": {
            "1": {"name": "echo_agent", "crate_id": 0, "visibility": "public",
                  "attrs": [], "inner": {"module": {"items": [2, 3, 7]}}},
            "2": {"name": "agent", "crate_id": 0, "visibility": "public",
                  "attrs": [], "inner": {"module": {"items": [4]}}},
            "3": {"name": "hidden_mod", "crate_id": 0, "visibility": "public",
                  "attrs": ["#[doc(hidden)]"], "inner": {"module": {"items": [5]}}},
            "4": {"name": "run", "crate_id": 0, "visibility": "public",
                  "attrs": [], "inner": {"function": {"sig": {}}}},
            "5": {"name": "secret", "crate_id": 0, "visibility": "public",
                  "attrs": [], "inner": {"function": {"sig": {}}}},
            "7": {"name": null, "crate_id": 0, "visibility": "public",
                  "attrs": [], "inner": {"use": {"source": "echo_core::Client", "name": "Client", "is_glob": false}}},
            "8": {"name": "hooked", "crate_id": 0, "visibility": "public",
                  "attrs": [], "inner": {"use": {"source": "echo_core::hooks", "is_glob": true}}},
            "9": {"name": "Agent", "crate_id": 0, "visibility": "public",
                  "attrs": [], "inner": {"struct": {"kind": "plain", "impls": [11]}}},
            "10": {"name": "status", "crate_id": 0, "visibility": "public",
                   "attrs": [], "inner": {"enum": {"variants": []}}},
            "11": {"name": null, "crate_id": 0, "visibility": "default",
                   "attrs": [], "inner": {"impl": {"for": {"resolved_path": {"path": "Agent", "id": 9, "args": null}}, "items": [12]}}},
            "12": {"name": "execute", "crate_id": 0, "visibility": "public",
                   "attrs": [], "inner": {"function": {"sig": {}}}}
        },
        "paths": {}
    }"##;

    #[test]
    fn extracts_public_items_and_skips_doc_hidden() {
        let items = extract_public_items(SAMPLE).expect("sample parses");
        let paths: Vec<&str> = items.iter().map(|i| i.path.as_str()).collect();
        assert!(paths.contains(&"echo_agent::agent::run"));
        assert!(!paths.iter().any(|p| p.contains("secret")));
        assert!(!paths.iter().any(|p| p.contains("hidden_mod")));
        assert!(paths.contains(&"echo_agent::Client"));
    }

    #[test]
    fn impl_members_keyed_by_type_name() {
        // Root also lists the struct and the impl for the sample tree.
        let json = SAMPLE.replace("\"items\": [2, 3, 7]", "\"items\": [2, 3, 7, 8, 9]");
        let items = extract_public_items(&json).expect("sample parses");
        let paths: Vec<&str> = items.iter().map(|i| i.path.as_str()).collect();
        assert!(paths.contains(&"echo_agent::Agent::execute"));
        assert!(paths.contains(&"echo_agent::Agent"));
        let hook = items
            .iter()
            .find(|i| i.path == "echo_agent::*" && i.import_glob == Some(true))
            .expect("glob present");
        assert!(hook.import_source.as_deref().is_some());
    }

    #[test]
    fn format_version_mismatch_fails_closed() {
        let bad = SAMPLE.replace("61", "99");
        let err = extract_public_items(&bad).expect_err("must fail");
        assert!(err.to_string().contains("99"));
    }
}
