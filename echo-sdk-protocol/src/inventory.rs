//! Deterministic inventory of the public `echo_agent` facade.
//!
//! Rustdoc JSON for a facade crate does not contain the children of a glob
//! re-export from another crate. The contract generator therefore supplies
//! rustdoc JSON for every workspace crate resolved in the same Cargo feature
//! profile. This module follows those imports across documents and records the
//! actual public item, its members, fields, variants and a stable API-shape
//! digest. An unresolved glob or an unknown rustdoc item kind fails closed.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Rustdoc JSON format emitted by the toolchain pinned in
/// `contracts/sdk/toolchain.json`.
pub const RUSTDOC_FORMAT_VERSION: u64 = 61;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Module,
    Function,
    Method,
    Struct,
    StructField,
    Enum,
    Variant,
    Union,
    Trait,
    TraitImpl,
    TraitAlias,
    TypeAlias,
    Macro,
    ProcMacro,
    Constant,
    Static,
    ExternType,
    Primitive,
}

impl ItemKind {
    fn parse(inner: &serde_json::Map<String, serde_json::Value>) -> Result<Self, InventoryError> {
        let Some(key) = inner.keys().next() else {
            return Err(InventoryError::UnsupportedItemKind("<empty>".to_string()));
        };
        match key.as_str() {
            "module" => Ok(Self::Module),
            "function" => Ok(Self::Function),
            "struct" => Ok(Self::Struct),
            "struct_field" => Ok(Self::StructField),
            "enum" => Ok(Self::Enum),
            "variant" => Ok(Self::Variant),
            "union" => Ok(Self::Union),
            "trait" => Ok(Self::Trait),
            "trait_alias" => Ok(Self::TraitAlias),
            "type_alias" | "assoc_type" => Ok(Self::TypeAlias),
            "macro" => Ok(Self::Macro),
            "proc_macro" => Ok(Self::ProcMacro),
            "constant" | "assoc_const" => Ok(Self::Constant),
            "static" => Ok(Self::Static),
            "extern_type" => Ok(Self::ExternType),
            "primitive" => Ok(Self::Primitive),
            other => Err(InventoryError::UnsupportedItemKind(other.to_string())),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Module => "module",
            Self::Function => "function",
            Self::Method => "method",
            Self::Struct => "struct",
            Self::StructField => "struct_field",
            Self::Enum => "enum",
            Self::Variant => "variant",
            Self::Union => "union",
            Self::Trait => "trait",
            Self::TraitImpl => "trait_impl",
            Self::TraitAlias => "trait_alias",
            Self::TypeAlias => "type_alias",
            Self::Macro => "macro",
            Self::ProcMacro => "proc_macro",
            Self::Constant => "constant",
            Self::Static => "static",
            Self::ExternType => "extern_type",
            Self::Primitive => "primitive",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PublicItem {
    pub path: String,
    pub kind: ItemKind,
    /// Canonical rustdoc shape with unstable item ids and child-id lists removed.
    pub api_shape: String,
    pub api_shape_digest: String,
    /// Canonical source path for re-exported items.
    pub source_path: Option<String>,
    pub required_features: BTreeSet<String>,
    pub automatically_derived: bool,
}

#[derive(Debug, Deserialize)]
struct RustdocFile {
    format_version: u64,
    #[serde(default)]
    root: Option<u64>,
    index: HashMap<String, RustdocItem>,
    #[serde(default)]
    paths: HashMap<String, RustdocPath>,
}

#[derive(Debug, Deserialize)]
struct RustdocPath {
    path: Vec<String>,
    kind: String,
}

#[derive(Debug, Deserialize)]
struct RustdocItem {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    visibility: serde_json::Value,
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

    fn is_default_visibility(&self) -> bool {
        self.visibility == "default"
    }

    fn is_doc_hidden(&self) -> bool {
        fn mentions_hidden(value: &serde_json::Value) -> bool {
            match value {
                serde_json::Value::String(text) => text.contains("doc(hidden)"),
                serde_json::Value::Array(values) => values.iter().any(mentions_hidden),
                serde_json::Value::Object(values) => values.values().any(mentions_hidden),
                _ => false,
            }
        }
        self.attrs.iter().any(mentions_hidden)
    }

    fn is_automatically_derived(&self) -> bool {
        self.attrs
            .iter()
            .any(|attribute| attribute.to_string().contains("automatically_derived"))
    }

    fn inner_map(&self) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.inner.as_ref()?.as_object()
    }

    fn child_ids(&self) -> Vec<String> {
        let Some(inner) = self.inner_map() else {
            return Vec::new();
        };
        let mut ids = Vec::new();
        for key in ["module", "trait", "impl"] {
            for id in inner
                .get(key)
                .and_then(|value| value.get("items"))
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(id) = id.as_u64() {
                    ids.push(id.to_string());
                }
            }
        }
        ids
    }

    fn impl_ids(&self) -> Vec<String> {
        let Some(inner) = self.inner_map() else {
            return Vec::new();
        };
        let mut ids = Vec::new();
        for key in ["struct", "enum", "union"] {
            for id in inner
                .get(key)
                .and_then(|value| value.get("impls"))
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(id) = id.as_u64() {
                    ids.push(id.to_string());
                }
            }
        }
        ids
    }
}

#[derive(Debug)]
pub enum InventoryError {
    MalformedJson(String),
    UnsupportedFormatVersion { found: u64, expected: u64 },
    MissingRootModule(String),
    MissingItem { document: String, id: String },
    MissingDependencyDocument(String),
    UnresolvedGlob(String),
    UnsupportedItemKind(String),
    ConflictingItemKind(String),
}

impl std::fmt::Display for InventoryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MalformedJson(message) => write!(formatter, "malformed rustdoc JSON: {message}"),
            Self::UnsupportedFormatVersion { found, expected } => write!(
                formatter,
                "rustdoc JSON format version {found} does not match pinned {expected}"
            ),
            Self::MissingRootModule(document) => {
                write!(formatter, "rustdoc JSON for {document} has no root module")
            }
            Self::MissingItem { document, id } => {
                write!(
                    formatter,
                    "rustdoc JSON for {document} is missing item {id}"
                )
            }
            Self::MissingDependencyDocument(name) => {
                write!(
                    formatter,
                    "missing rustdoc JSON for re-export source crate {name}"
                )
            }
            Self::UnresolvedGlob(source) => {
                write!(formatter, "cannot expand public glob re-export {source}")
            }
            Self::UnsupportedItemKind(kind) => {
                write!(formatter, "unsupported public rustdoc item kind {kind}")
            }
            Self::ConflictingItemKind(path) => {
                write!(
                    formatter,
                    "public facade identity {path} changes item kind across profiles"
                )
            }
        }
    }
}

impl std::error::Error for InventoryError {}

struct DocumentGraph {
    documents: BTreeMap<String, RustdocFile>,
    ids_by_path: BTreeMap<String, HashMap<Vec<String>, String>>,
}

impl DocumentGraph {
    fn new(
        root_json: &str,
        dependencies: &BTreeMap<String, String>,
    ) -> Result<Self, InventoryError> {
        let mut documents = BTreeMap::new();
        let root = parse_document(root_json)?;
        let root_name = document_name(&root, "echo_agent")?;
        documents.insert(root_name, root);
        for (declared_name, json) in dependencies {
            let document = parse_document(json)?;
            let actual_name = document_name(&document, declared_name)?;
            documents.insert(actual_name, document);
        }
        let ids_by_path = documents
            .iter()
            .map(|(name, document)| {
                let paths = document
                    .paths
                    .iter()
                    .map(|(id, path)| (path.path.clone(), id.clone()))
                    .collect();
                (name.clone(), paths)
            })
            .collect();
        Ok(Self {
            documents,
            ids_by_path,
        })
    }

    fn root(&self, document: &str) -> Result<(String, String), InventoryError> {
        let file = self
            .documents
            .get(document)
            .ok_or_else(|| InventoryError::MissingDependencyDocument(document.to_string()))?;
        let id =
            root_id(file).ok_or_else(|| InventoryError::MissingRootModule(document.to_string()))?;
        let name = file
            .index
            .get(&id)
            .and_then(|item| item.name.clone())
            .unwrap_or_else(|| document.to_string());
        Ok((id, name))
    }

    fn item(&self, document: &str, id: &str) -> Result<&RustdocItem, InventoryError> {
        self.documents
            .get(document)
            .and_then(|file| file.index.get(id))
            .ok_or_else(|| InventoryError::MissingItem {
                document: document.to_string(),
                id: id.to_string(),
            })
    }

    fn path_summary(&self, document: &str, id: &str) -> Option<&RustdocPath> {
        self.documents.get(document)?.paths.get(id)
    }

    fn resolve_target(&self, document: &str, id: &str) -> Option<(String, String)> {
        if self
            .documents
            .get(document)
            .is_some_and(|file| file.index.contains_key(id))
        {
            return Some((document.to_string(), id.to_string()));
        }
        let summary = self.path_summary(document, id)?;
        let crate_name = summary.path.first()?.clone();
        let target_id = self
            .ids_by_path
            .get(&crate_name)?
            .get(&summary.path)?
            .clone();
        self.documents
            .get(&crate_name)
            .is_some_and(|file| file.index.contains_key(&target_id))
            .then_some((crate_name, target_id))
    }
}

fn parse_document(json: &str) -> Result<RustdocFile, InventoryError> {
    let document: RustdocFile = serde_json::from_str(json)
        .map_err(|error| InventoryError::MalformedJson(error.to_string()))?;
    if document.format_version != RUSTDOC_FORMAT_VERSION {
        return Err(InventoryError::UnsupportedFormatVersion {
            found: document.format_version,
            expected: RUSTDOC_FORMAT_VERSION,
        });
    }
    Ok(document)
}

fn root_id(document: &RustdocFile) -> Option<String> {
    if let Some(id) = document.root {
        return Some(id.to_string());
    }
    let mut nested = HashSet::new();
    for item in document.index.values() {
        nested.extend(item.child_ids());
    }
    document.index.iter().find_map(|(id, item)| {
        (item.crate_id == 0
            && !nested.contains(id)
            && item
                .inner_map()
                .is_some_and(|inner| inner.contains_key("module")))
        .then(|| id.clone())
    })
}

fn document_name(document: &RustdocFile, fallback: &str) -> Result<String, InventoryError> {
    let id =
        root_id(document).ok_or_else(|| InventoryError::MissingRootModule(fallback.to_string()))?;
    Ok(document
        .index
        .get(&id)
        .and_then(|item| item.name.clone())
        .unwrap_or_else(|| fallback.to_string()))
}

pub fn extract_public_items(json: &str) -> Result<Vec<PublicItem>, InventoryError> {
    extract_public_items_with_dependencies(json, &BTreeMap::new())
}

pub fn extract_public_items_with_dependencies(
    root_json: &str,
    dependencies: &BTreeMap<String, String>,
) -> Result<Vec<PublicItem>, InventoryError> {
    let graph = DocumentGraph::new(root_json, dependencies)?;
    let root_document = graph
        .documents
        .keys()
        .find(|name| name.as_str() == "echo_agent")
        .cloned()
        .or_else(|| graph.documents.keys().next().cloned())
        .ok_or_else(|| InventoryError::MissingRootModule("echo_agent".to_string()))?;
    let (root_id, root_name) = graph.root(&root_document)?;
    let mut state = WalkState::default();
    walk_module(
        &graph,
        &root_document,
        &root_id,
        &root_name,
        &BTreeSet::new(),
        &mut state,
    )?;
    Ok(state.finish())
}

#[derive(Default)]
struct WalkState {
    items: BTreeMap<String, BTreeMap<String, PublicItem>>,
    visited_modules: HashSet<(String, String, String)>,
}

impl WalkState {
    fn push(
        &mut self,
        path: String,
        kind: ItemKind,
        item: &RustdocItem,
        source_path: Option<String>,
        required_features: &BTreeSet<String>,
    ) {
        let api_shape = item_shape(item);
        let api_shape_digest = digest(api_shape.as_bytes());
        self.items
            .entry(path.clone())
            .or_default()
            .entry(api_shape_digest.clone())
            .and_modify(|existing| {
                existing
                    .required_features
                    .extend(required_features.iter().cloned());
            })
            .or_insert(PublicItem {
                path,
                kind,
                api_shape,
                api_shape_digest,
                source_path,
                required_features: required_features.clone(),
                automatically_derived: item.is_automatically_derived(),
            });
    }

    fn push_summary(
        &mut self,
        path: String,
        kind: ItemKind,
        source_path: String,
        required_features: &BTreeSet<String>,
    ) {
        let api_shape = format!("{{\"external_source\":{}}}", json_string(&source_path));
        let api_shape_digest = digest(api_shape.as_bytes());
        self.items
            .entry(path.clone())
            .or_default()
            .entry(api_shape_digest.clone())
            .and_modify(|existing| {
                existing
                    .required_features
                    .extend(required_features.iter().cloned());
            })
            .or_insert(PublicItem {
                path,
                kind,
                api_shape,
                api_shape_digest,
                source_path: Some(source_path),
                required_features: required_features.clone(),
                automatically_derived: false,
            });
    }

    fn finish(self) -> Vec<PublicItem> {
        let mut result = Vec::new();
        for (path, variants) in self.items {
            if variants.len() == 1 {
                result.extend(variants.into_values());
                continue;
            }
            for (_, mut item) in variants {
                let suffix: String = item
                    .api_shape_digest
                    .chars()
                    .filter(|character| character.is_ascii_hexdigit())
                    .take(12)
                    .collect();
                item.path = format!("{path}#{suffix}");
                result.push(item);
            }
        }
        result.sort();
        result
    }
}

fn walk_module(
    graph: &DocumentGraph,
    document: &str,
    module_id: &str,
    namespace: &str,
    inherited_features: &BTreeSet<String>,
    state: &mut WalkState,
) -> Result<(), InventoryError> {
    let visit = (
        document.to_string(),
        module_id.to_string(),
        namespace.to_string(),
    );
    if !state.visited_modules.insert(visit) {
        return Ok(());
    }
    let module = graph.item(document, module_id)?;
    for child_id in module.child_ids() {
        let child = graph.item(document, &child_id)?;
        if child.is_doc_hidden() || !child.is_public() {
            continue;
        }
        walk_item(
            graph,
            document,
            &child_id,
            namespace,
            None,
            inherited_features,
            state,
        )?;
    }
    Ok(())
}

fn walk_item(
    graph: &DocumentGraph,
    document: &str,
    item_id: &str,
    namespace: &str,
    alias: Option<&str>,
    inherited_features: &BTreeSet<String>,
    state: &mut WalkState,
) -> Result<(), InventoryError> {
    let item = graph.item(document, item_id)?;
    let inner = item
        .inner_map()
        .ok_or_else(|| InventoryError::UnsupportedItemKind("<missing inner>".to_string()))?;
    if inner.contains_key("use") {
        return walk_import(graph, document, item, namespace, inherited_features, state);
    }
    if inner.contains_key("impl") || inner.contains_key("extern_crate") {
        return Ok(());
    }
    let kind = ItemKind::parse(inner)?;
    let name = alias
        .map(str::to_string)
        .or_else(|| item.name.clone())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| InventoryError::UnsupportedItemKind(format!("unnamed {}", kind.as_str())))?;
    let path = namespace_path(namespace, &name);
    let required_features = combined_features(inherited_features, item);
    let source_path = graph
        .path_summary(document, item_id)
        .map(|summary| summary.path.join("::"));
    state.push(
        path.clone(),
        kind,
        item,
        source_path.clone(),
        &required_features,
    );

    match kind {
        ItemKind::Module => {
            walk_module(graph, document, item_id, &path, &required_features, state)?
        }
        ItemKind::Trait => walk_members(
            graph,
            item,
            WalkLocation {
                document,
                path: &path,
                source_path: source_path.as_deref(),
                required_features: &required_features,
            },
            true,
            state,
        )?,
        ItemKind::Struct | ItemKind::Enum | ItemKind::Union => {
            walk_fields_and_variants(
                graph,
                document,
                item,
                &path,
                source_path.as_deref(),
                &required_features,
                state,
            )?;
            walk_impls(
                graph,
                document,
                item,
                &path,
                source_path.as_deref(),
                &required_features,
                state,
            )?;
        }
        _ => {}
    }
    Ok(())
}

fn walk_import(
    graph: &DocumentGraph,
    document: &str,
    item: &RustdocItem,
    namespace: &str,
    inherited_features: &BTreeSet<String>,
    state: &mut WalkState,
) -> Result<(), InventoryError> {
    let required_features = combined_features(inherited_features, item);
    let import = item
        .inner_map()
        .and_then(|inner| inner.get("use"))
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| InventoryError::UnsupportedItemKind("malformed use".to_string()))?;
    let source = import
        .get("source")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<unknown>");
    let is_glob = import
        .get("is_glob")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let target_id = import
        .get("id")
        .and_then(serde_json::Value::as_u64)
        .map(|id| id.to_string());
    let Some(target_id) = target_id else {
        return if is_glob {
            Err(InventoryError::UnresolvedGlob(source.to_string()))
        } else {
            let name = import
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(source);
            state.push_summary(
                namespace_path(namespace, name),
                ItemKind::TypeAlias,
                source.to_string(),
                &required_features,
            );
            Ok(())
        };
    };

    if let Some((target_document, resolved_id)) = graph.resolve_target(document, &target_id) {
        if is_glob {
            let target = graph.item(&target_document, &resolved_id)?;
            let kind = target
                .inner_map()
                .map(ItemKind::parse)
                .transpose()?
                .ok_or_else(|| InventoryError::UnresolvedGlob(source.to_string()))?;
            if kind != ItemKind::Module {
                return Err(InventoryError::UnresolvedGlob(source.to_string()));
            }
            let target_features = combined_features(&required_features, target);
            return walk_module(
                graph,
                &target_document,
                &resolved_id,
                namespace,
                &target_features,
                state,
            );
        }
        let alias = import
            .get("name")
            .and_then(serde_json::Value::as_str)
            .or_else(|| source.rsplit("::").next());
        return walk_item(
            graph,
            &target_document,
            &resolved_id,
            namespace,
            alias,
            &required_features,
            state,
        );
    }

    let summary =
        graph
            .path_summary(document, &target_id)
            .ok_or_else(|| InventoryError::MissingItem {
                document: document.to_string(),
                id: target_id.clone(),
            })?;
    if is_glob {
        return Err(InventoryError::UnresolvedGlob(source.to_string()));
    }
    let kind = kind_from_summary(&summary.kind)?;
    let alias = import
        .get("name")
        .and_then(serde_json::Value::as_str)
        .or_else(|| summary.path.last().map(String::as_str))
        .unwrap_or(source);
    state.push_summary(
        namespace_path(namespace, alias),
        kind,
        summary.path.join("::"),
        &required_features,
    );
    Ok(())
}

fn kind_from_summary(kind: &str) -> Result<ItemKind, InventoryError> {
    match kind {
        "module" => Ok(ItemKind::Module),
        "function" => Ok(ItemKind::Function),
        "struct" => Ok(ItemKind::Struct),
        "enum" => Ok(ItemKind::Enum),
        "union" => Ok(ItemKind::Union),
        "trait" => Ok(ItemKind::Trait),
        "trait_alias" => Ok(ItemKind::TraitAlias),
        "type_alias" => Ok(ItemKind::TypeAlias),
        "macro" => Ok(ItemKind::Macro),
        "proc_macro" | "proc_attribute" | "proc_derive" => Ok(ItemKind::ProcMacro),
        "constant" => Ok(ItemKind::Constant),
        "static" => Ok(ItemKind::Static),
        "extern_type" => Ok(ItemKind::ExternType),
        "primitive" => Ok(ItemKind::Primitive),
        other => Err(InventoryError::UnsupportedItemKind(other.to_string())),
    }
}

#[derive(Clone, Copy)]
struct WalkLocation<'a> {
    document: &'a str,
    path: &'a str,
    source_path: Option<&'a str>,
    required_features: &'a BTreeSet<String>,
}

fn walk_members(
    graph: &DocumentGraph,
    container: &RustdocItem,
    location: WalkLocation<'_>,
    trait_members: bool,
    state: &mut WalkState,
) -> Result<(), InventoryError> {
    for child_id in container.child_ids() {
        let child = graph.item(location.document, &child_id)?;
        if child.is_doc_hidden()
            || (!child.is_public() && !(trait_members && child.is_default_visibility()))
        {
            continue;
        }
        let inner = child.inner_map().ok_or_else(|| {
            InventoryError::UnsupportedItemKind("member without inner".to_string())
        })?;
        let mut kind = ItemKind::parse(inner)?;
        if kind == ItemKind::Function {
            kind = ItemKind::Method;
        }
        let name = child
            .name
            .clone()
            .ok_or_else(|| InventoryError::UnsupportedItemKind("unnamed member".to_string()))?;
        let required_features = combined_features(location.required_features, child);
        let source_path = graph
            .path_summary(location.document, &child_id)
            .map(|summary| summary.path.join("::"))
            .or_else(|| {
                location
                    .source_path
                    .map(|source| namespace_path(source, &name))
            });
        state.push(
            namespace_path(location.path, &name),
            kind,
            child,
            source_path,
            &required_features,
        );
    }
    Ok(())
}

fn walk_impls(
    graph: &DocumentGraph,
    document: &str,
    item: &RustdocItem,
    path: &str,
    source_path: Option<&str>,
    inherited_features: &BTreeSet<String>,
    state: &mut WalkState,
) -> Result<(), InventoryError> {
    for impl_id in item.impl_ids() {
        let implementation = graph.item(document, &impl_id)?;
        let Some(details) = implementation
            .inner_map()
            .and_then(|inner| inner.get("impl"))
            .and_then(serde_json::Value::as_object)
        else {
            return Err(InventoryError::UnsupportedItemKind(
                "malformed impl".to_string(),
            ));
        };
        let required_features = combined_features(inherited_features, implementation);
        if let Some(trait_path) = details
            .get("trait")
            .filter(|value| !value.is_null())
            .and_then(|value| value.get("path"))
            .and_then(serde_json::Value::as_str)
        {
            let is_synthetic = details
                .get("is_synthetic")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let is_blanket = details
                .get("blanket_impl")
                .is_some_and(|value| !value.is_null());
            if !is_synthetic && !is_blanket {
                state.push(
                    format!("{path}::impl<{trait_path}>"),
                    ItemKind::TraitImpl,
                    implementation,
                    source_path.map(|source| format!("{source}::impl<{trait_path}>")),
                    &required_features,
                );
            }
            continue;
        }
        walk_members(
            graph,
            implementation,
            WalkLocation {
                document,
                path,
                source_path,
                required_features: &required_features,
            },
            false,
            state,
        )?;
    }
    Ok(())
}

fn walk_fields_and_variants(
    graph: &DocumentGraph,
    document: &str,
    item: &RustdocItem,
    path: &str,
    source_path: Option<&str>,
    inherited_features: &BTreeSet<String>,
    state: &mut WalkState,
) -> Result<(), InventoryError> {
    let Some(inner) = item.inner_map() else {
        return Ok(());
    };
    if let Some(struct_value) = inner.get("struct") {
        walk_field_container(
            graph,
            struct_value.get("kind"),
            WalkLocation {
                document,
                path,
                source_path,
                required_features: inherited_features,
            },
            false,
            state,
        )?;
    }
    if let Some(union_value) = inner.get("union") {
        walk_id_array(
            graph,
            union_value.get("fields"),
            WalkLocation {
                document,
                path,
                source_path,
                required_features: inherited_features,
            },
            ItemKind::StructField,
            true,
            state,
        )?;
    }
    if let Some(enum_value) = inner.get("enum") {
        let variants = enum_value
            .get("variants")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten();
        for variant_id in variants {
            let Some(variant_id) = variant_id.as_u64().map(|id| id.to_string()) else {
                continue;
            };
            let variant = graph.item(document, &variant_id)?;
            let name = variant.name.clone().ok_or_else(|| {
                InventoryError::UnsupportedItemKind("unnamed variant".to_string())
            })?;
            let variant_path = namespace_path(path, &name);
            let required_features = combined_features(inherited_features, variant);
            let variant_source_path = graph
                .path_summary(document, &variant_id)
                .map(|summary| summary.path.join("::"))
                .or_else(|| source_path.map(|source| namespace_path(source, &name)));
            state.push(
                variant_path.clone(),
                ItemKind::Variant,
                variant,
                variant_source_path.clone(),
                &required_features,
            );
            let kind = variant
                .inner_map()
                .and_then(|value| value.get("variant"))
                .and_then(|value| value.get("kind"));
            walk_field_container(
                graph,
                kind,
                WalkLocation {
                    document,
                    path: &variant_path,
                    source_path: variant_source_path.as_deref(),
                    required_features: &required_features,
                },
                true,
                state,
            )?;
        }
    }
    Ok(())
}

fn walk_field_container(
    graph: &DocumentGraph,
    kind: Option<&serde_json::Value>,
    location: WalkLocation<'_>,
    enum_fields_are_public: bool,
    state: &mut WalkState,
) -> Result<(), InventoryError> {
    let Some(kind) = kind.and_then(serde_json::Value::as_object) else {
        return Ok(());
    };
    for key in ["plain", "tuple", "struct"] {
        let fields = if matches!(key, "plain" | "struct") {
            kind.get(key).and_then(|value| value.get("fields"))
        } else {
            kind.get(key)
        };
        walk_id_array(
            graph,
            fields,
            location,
            ItemKind::StructField,
            enum_fields_are_public,
            state,
        )?;
    }
    Ok(())
}

fn walk_id_array(
    graph: &DocumentGraph,
    ids: Option<&serde_json::Value>,
    location: WalkLocation<'_>,
    kind: ItemKind,
    default_is_public: bool,
    state: &mut WalkState,
) -> Result<(), InventoryError> {
    for (position, id) in ids
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let Some(id) = id.as_u64().map(|value| value.to_string()) else {
            continue;
        };
        let field = graph.item(location.document, &id)?;
        if field.is_doc_hidden()
            || (!field.is_public() && !(default_is_public && field.is_default_visibility()))
        {
            continue;
        }
        let name = field.name.clone().unwrap_or_else(|| position.to_string());
        let required_features = combined_features(location.required_features, field);
        let field_source_path = graph
            .path_summary(location.document, &id)
            .map(|summary| summary.path.join("::"))
            .or_else(|| {
                location
                    .source_path
                    .map(|source| namespace_path(source, &name))
            });
        state.push(
            namespace_path(location.path, &name),
            kind,
            field,
            field_source_path,
            &required_features,
        );
    }
    Ok(())
}

fn combined_features(inherited: &BTreeSet<String>, item: &RustdocItem) -> BTreeSet<String> {
    let mut features = inherited.clone();
    for attribute in &item.attrs {
        collect_feature_names(attribute, &mut features);
    }
    features
}

fn collect_feature_names(value: &serde_json::Value, features: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::String(text) => {
            for segment in text.split("name: \"feature\"").skip(1) {
                if let Some(feature) = segment
                    .split("value: Some(\"")
                    .nth(1)
                    .and_then(|rest| rest.split('"').next())
                    .filter(|feature| !feature.is_empty())
                {
                    features.insert(feature.to_string());
                }
            }
            for segment in text.split("feature = \"").skip(1) {
                if let Some(feature) = segment
                    .split('"')
                    .next()
                    .filter(|feature| !feature.is_empty())
                {
                    features.insert(feature.to_string());
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_feature_names(value, features);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values() {
                collect_feature_names(value, features);
            }
        }
        _ => {}
    }
}

fn item_shape(item: &RustdocItem) -> String {
    let attrs: Vec<&serde_json::Value> = item
        .attrs
        .iter()
        .filter(|attribute| {
            let rendered = attribute.to_string();
            [
                "serde(",
                "repr(",
                "non_exhaustive",
                "must_use",
                "echo_sdk_behavior_digest",
            ]
            .iter()
            .any(|marker| rendered.contains(marker))
        })
        .collect();
    let mut value = serde_json::json!({
        "attrs": attrs,
        "inner": item.inner,
    });
    remove_structural_child_ids(&mut value);
    normalize_shape(&mut value);
    serde_json::to_string(&value).unwrap_or_default()
}

fn remove_structural_child_ids(value: &mut serde_json::Value) {
    for pointer in [
        "/inner/struct/kind/tuple",
        "/inner/variant/kind/tuple",
        "/inner/variant/kind/struct/fields",
    ] {
        if let Some(target) = value.pointer_mut(pointer) {
            *target = serde_json::Value::Array(Vec::new());
        }
    }
}

fn normalize_shape(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for unstable in [
                "id",
                "impls",
                "items",
                "fields",
                "variants",
                "implementations",
                "span",
                "default_unstable",
                "is_stripped",
                "has_stripped_fields",
            ] {
                map.remove(unstable);
            }
            if let Some(inputs) = map
                .get_mut("inputs")
                .and_then(serde_json::Value::as_array_mut)
            {
                for input in inputs {
                    if let Some(pair) = input.as_array_mut().filter(|pair| pair.len() == 2)
                        && let Some(parameter_type) = pair.pop()
                    {
                        pair.clear();
                        pair.push(parameter_type);
                    }
                }
            }
            for child in map.values_mut() {
                normalize_shape(child);
            }
        }
        serde_json::Value::Array(values) => {
            for child in values {
                normalize_shape(child);
            }
        }
        _ => {}
    }
}

fn namespace_path(namespace: &str, name: &str) -> String {
    if namespace.is_empty() {
        name.to_string()
    } else {
        format!("{namespace}::{name}")
    }
}

fn digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{:x}", hasher.finalize())
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FeatureProfile {
    pub name: String,
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

pub fn profiles_for_leaf_features(leaf_features: &[String]) -> Vec<FeatureProfile> {
    let mut profiles = vec![
        FeatureProfile::default_profile(),
        FeatureProfile::full_profile(),
    ];
    let mut leaves = leaf_features.to_vec();
    leaves.sort();
    leaves.dedup();
    profiles.extend(leaves.iter().map(|feature| FeatureProfile::leaf(feature)));
    profiles
}

#[derive(Debug, Clone, Serialize)]
pub struct InventorySignature {
    pub digest: String,
    pub shape: String,
    pub profiles: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InventoryEntry {
    pub path: String,
    pub kind: ItemKind,
    pub source_paths: BTreeSet<String>,
    pub signatures: BTreeMap<String, InventorySignature>,
    pub profiles: BTreeSet<String>,
    pub declared_feature_requirements: BTreeSet<String>,
    pub automatically_derived: bool,
}

pub fn merge_profiles(
    per_profile: &BTreeMap<String, Vec<PublicItem>>,
) -> Result<Vec<InventoryEntry>, InventoryError> {
    let mut merged: BTreeMap<String, InventoryEntry> = BTreeMap::new();
    for (profile, items) in per_profile {
        for item in items {
            let entry = merged
                .entry(item.path.clone())
                .or_insert_with(|| InventoryEntry {
                    path: item.path.clone(),
                    kind: item.kind,
                    source_paths: BTreeSet::new(),
                    signatures: BTreeMap::new(),
                    profiles: BTreeSet::new(),
                    declared_feature_requirements: BTreeSet::new(),
                    automatically_derived: item.automatically_derived,
                });
            if entry.kind != item.kind {
                return Err(InventoryError::ConflictingItemKind(item.path.clone()));
            }
            entry.automatically_derived &= item.automatically_derived;
            if let Some(source) = &item.source_path {
                entry.source_paths.insert(source.clone());
            }
            entry.profiles.insert(profile.clone());
            entry
                .declared_feature_requirements
                .extend(item.required_features.iter().cloned());
            entry
                .signatures
                .entry(item.api_shape_digest.clone())
                .or_insert_with(|| InventorySignature {
                    digest: item.api_shape_digest.clone(),
                    shape: item.api_shape.clone(),
                    profiles: BTreeSet::new(),
                })
                .profiles
                .insert(profile.clone());
        }
    }
    Ok(merged.into_values().collect())
}

pub fn render_public_api_snapshot(
    profiles: &[FeatureProfile],
    merged: &[InventoryEntry],
) -> String {
    let mut output = String::new();
    output.push_str("# Generated by echo-sdk-protocol; do not edit.\n");
    output.push_str("# Every line records a facade identity plus its stable rustdoc API shape.\n");
    output.push_str(&format!(
        "# rustdoc JSON format version: {RUSTDOC_FORMAT_VERSION}\n"
    ));
    let profile_names: Vec<&str> = profiles
        .iter()
        .map(|profile| profile.name.as_str())
        .collect();
    output.push_str(&format!("# profiles: {}\n\n", profile_names.join(", ")));
    let mut rendered_items = 0usize;
    for entry in merged {
        if entry.kind == ItemKind::TraitImpl && entry.automatically_derived {
            continue;
        }
        rendered_items = rendered_items.saturating_add(1);
        for signature in entry.signatures.values() {
            let profiles: Vec<&str> = signature.profiles.iter().map(String::as_str).collect();
            output.push_str(&format!(
                "{:<14} {}  {}  [{}]  {}\n",
                entry.kind.as_str(),
                entry.path,
                signature.digest,
                profiles.join(","),
                signature.shape
            ));
        }
    }
    output.push_str(&format!("\n# total items: {rendered_items}\n"));
    output
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
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
            Self::WireValue => "wire_value",
            Self::Operation => "operation",
            Self::Handle => "handle",
            Self::Stream => "stream",
            Self::Extension => "extension",
            Self::LanguageIntrinsic => "language_intrinsic",
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
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
            Self::Standard => "standard",
            Self::StandardProjection => "standard_projection",
            Self::EchoExtension => "echo_extension",
            Self::LanguageIntrinsic => "language_intrinsic",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LanguageImplementationStatus {
    NotImplemented,
    InProgress,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FeatureSemantics {
    Default,
    AnyOf,
    AllOf,
}

impl LanguageImplementationStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotImplemented => "not_implemented",
            Self::InProgress => "in_progress",
            Self::Done => "done",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LanguageStatusRecord {
    pub status: LanguageImplementationStatus,
    pub target: String,
    pub contract_test: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManifestSignature {
    pub digest: String,
}

/// Canonical adapter obligation of one facade item: exactly one route from
/// `facade.rs`, the wire surface it rides, and the real validation
/// references. Route ids never contain wildcards; re-export aliases share
/// the source identity and therefore the same route (see
/// `ManifestEntry::alias_of`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RouteObligation {
    /// Canonical route id (`family:memory`, `core:task`,
    /// `source:<source identity>`, `bridge:tool`, `value`,
    /// `intrinsic:<reason>`, `standard:<acp method>`).
    pub route: String,
    /// Wire surface kind: standard/core/family/bridge/invoke/value/intrinsic.
    pub surface: String,
    /// Adapter family name, when the route belongs to one.
    pub family: Option<String>,
    /// Primary wire method owned by the route, when applicable.
    pub method: Option<String>,
    /// Exact operation identity (generic invoke routes only).
    pub operation: Option<String>,
    /// Closed family operation the Host executes after admitting an exact
    /// source identity. Present only for operation-level family adapters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handler_operation: Option<String>,
    /// Root leaf feature required by the route's family.
    pub required_feature: Option<String>,
    /// Complete feature requirement inherited from the canonical inventory
    /// item. `AllOf` is used for root `full` entries; `AnyOf` is used when an
    /// item exists in one of several feature profiles.
    pub required_features: Vec<String>,
    pub feature_semantics: FeatureSemantics,
    /// Exact signature digests accepted for an executable invoke route.
    /// Family/value routes may legitimately carry no operation signature.
    #[serde(default)]
    pub signatures: Vec<ManifestSignature>,
    pub mapping: String,
    pub validation: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManifestEntry {
    pub path: String,
    pub kind: ItemKind,
    pub source_paths: BTreeSet<String>,
    pub features: BTreeSet<String>,
    pub full_only: bool,
    pub feature_semantics: FeatureSemantics,
    pub signatures: Vec<ManifestSignature>,
    pub classification: SemanticClass,
    pub acp_relationship: AcpRelationship,
    pub semantic_rule: String,
    pub derived_traits: BTreeSet<String>,
    pub route: RouteObligation,
    /// Whether this facade path is the canonical member of its alias group
    /// (same canonical source identity). Aliases carry `alias_of` instead.
    pub canonical: bool,
    /// Canonical facade path this entry re-exports; aliases share the
    /// source identity, signature, feature and handler of the canonical
    /// member, so the Host registers exactly one handler per route.
    pub alias_of: Option<String>,
    pub languages: BTreeMap<String, LanguageStatusRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManifestGenerated {
    pub rustdoc_format_version: u64,
    pub profiles: Vec<String>,
    pub inventory_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ParityManifest {
    pub schema_version: u32,
    pub extension_protocol_version: u32,
    pub generated: ManifestGenerated,
    pub entries: Vec<ManifestEntry>,
}

pub const LANGUAGES: &[&str] = &["typescript", "python", "java"];

pub fn features_of_entry(entry: &InventoryEntry) -> BTreeSet<String> {
    if entry.profiles.contains("default") {
        return BTreeSet::new();
    }
    let leaf_features: BTreeSet<String> = entry
        .profiles
        .iter()
        .filter_map(|profile| profile.strip_prefix("feature:").map(str::to_string))
        .collect();
    if leaf_features.is_empty() && entry.profiles.contains("full") {
        entry.declared_feature_requirements.clone()
    } else {
        leaf_features
    }
}

pub fn classify_entry(
    entry: &InventoryEntry,
    serializable_types: &BTreeSet<String>,
    public_value_types: &BTreeSet<String>,
    process_local_types: &BTreeSet<String>,
) -> (SemanticClass, AcpRelationship, &'static str) {
    let shape = entry
        .signatures
        .values()
        .map(|signature| signature.shape.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let last = entry
        .path
        .rsplit("::")
        .next()
        .unwrap_or(entry.path.as_str());
    let is_stream = last.ends_with("Stream")
        || shape.contains("Stream<")
        || shape.contains("BoxStream")
        || shape.contains("Receiver")
        || shape.contains("\"path\":\"Stream\"");
    let is_builder = last.ends_with("Builder")
        || last.ends_with("Factory")
        || last.starts_with("Fn") && last.ends_with("Factory");
    let has_process_local_shape = shape_is_process_local(&shape);
    let is_handle = last.ends_with("Handle")
        || last.ends_with("Registry")
        || last.ends_with("Manager")
        || last.ends_with("Service")
        || last.ends_with("Store")
        || last.ends_with("Client")
        || last.ends_with("Pool")
        || last.ends_with("Bus")
        || last.ends_with("Connection")
        || matches!(
            last,
            "ReactAgent" | "AgentPool" | "Conversation" | "Session" | "CancellationToken"
        );

    let (class, rule) = match entry.kind {
        ItemKind::Module | ItemKind::Macro | ItemKind::ProcMacro | ItemKind::Primitive => {
            (SemanticClass::LanguageIntrinsic, "rust-language-surface")
        }
        ItemKind::Trait | ItemKind::TraitAlias => {
            (SemanticClass::Extension, "consumer-implemented-trait")
        }
        ItemKind::TraitImpl => (
            SemanticClass::LanguageIntrinsic,
            "rust-trait-implementation",
        ),
        ItemKind::StructField if has_process_local_shape => {
            (SemanticClass::LanguageIntrinsic, "process-local-field")
        }
        ItemKind::TypeAlias if is_stream => (SemanticClass::Stream, "async-stream-signature"),
        ItemKind::TypeAlias
            if shape.contains("function_pointer")
                || shape.contains("\"path\":\"Fn")
                || shape.contains("BoxFuture") =>
        {
            (SemanticClass::LanguageIntrinsic, "callback-type-alias")
        }
        ItemKind::TypeAlias if shape.contains("dyn_trait") => {
            (SemanticClass::Extension, "trait-object-alias")
        }
        ItemKind::TypeAlias if has_process_local_shape => {
            (SemanticClass::Handle, "process-local-type-alias")
        }
        _ if is_builder => (SemanticClass::LanguageIntrinsic, "builder-or-factory"),
        ItemKind::Function | ItemKind::Method if is_stream => {
            (SemanticClass::Stream, "async-stream-signature")
        }
        ItemKind::Function | ItemKind::Method => (SemanticClass::Operation, "callable-operation"),
        _ if is_handle => (SemanticClass::Handle, "long-lived-resource"),
        ItemKind::Struct | ItemKind::Enum | ItemKind::Union
            if process_local_types.contains(&entry.path) =>
        {
            (SemanticClass::Handle, "contains-process-local-state")
        }
        ItemKind::Struct | ItemKind::Enum | ItemKind::Union
            if serializable_types.contains(&entry.path)
                || public_value_types.contains(&entry.path) =>
        {
            (SemanticClass::WireValue, "verified-value-shape")
        }
        ItemKind::Struct | ItemKind::Enum | ItemKind::Union => {
            (SemanticClass::Handle, "opaque-nonwire-type")
        }
        _ => (SemanticClass::WireValue, "serializable-value"),
    };
    const ACP_PROJECTION_SOURCES: &[&str] = &[
        "echo_core::agent::AgentEvent",
        "echo_core::agent::event_envelope::EventEnvelope",
        "echo_core::llm::types::ContentPart",
        "echo_core::llm::types::LinkedResource",
        "echo_core::llm::types::Message",
        "echo_core::llm::types::MessageContent",
        "echo_core::llm::types::Role",
        "echo_core::llm::types::ToolCall",
    ];
    let standard_projection = entry.source_paths.iter().any(|source| {
        ACP_PROJECTION_SOURCES.iter().any(|prefix| {
            source == prefix
                || source
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with("::"))
        })
    });
    let acp_adapter_item = entry.source_paths.iter().any(|source| {
        source == "echo_agent::acp"
            || source
                .strip_prefix("echo_agent::acp::")
                .is_some_and(|rest| !rest.is_empty())
    });
    let acp_session_context = entry.path == "echo_agent::acp::AcpSessionContext"
        || entry
            .path
            .strip_prefix("echo_agent::acp::AcpSessionContext::")
            .is_some_and(|rest| !rest.is_empty());
    let relationship = if class == SemanticClass::LanguageIntrinsic {
        AcpRelationship::LanguageIntrinsic
    } else if acp_session_context {
        AcpRelationship::StandardProjection
    } else if acp_adapter_item {
        AcpRelationship::LanguageIntrinsic
    } else if standard_projection {
        AcpRelationship::StandardProjection
    } else {
        AcpRelationship::EchoExtension
    };
    (class, relationship, rule)
}

fn shape_is_process_local(shape: &str) -> bool {
    [
        "Arc<",
        "Mutex<",
        "RwLock<",
        "Instant",
        "dyn ",
        "CancellationToken",
        "Fn(",
        "Future<",
        "Pin<",
        "\"path\":\"Arc\"",
        "\"path\":\"Mutex\"",
        "\"path\":\"RwLock\"",
        "\"path\":\"Instant\"",
        "dyn_trait",
        "function_pointer",
        "impl_trait",
    ]
    .iter()
    .any(|marker| shape.contains(marker))
}

fn route_obligation_for(
    entry: &InventoryEntry,
    class: SemanticClass,
    relationship: AcpRelationship,
    semantic_rule: &'static str,
) -> RouteObligation {
    let route = crate::facade::resolve_route(entry, class, relationship, semantic_rule);
    let family = route.family();
    let mapping = if route.surface() == "intrinsic" {
        format!(
            "{} via {}; Rust remains authoritative",
            relationship.as_str(),
            route.route_id()
        )
    } else {
        format!(
            "{} via {}; Rust remains authoritative",
            relationship.as_str(),
            semantic_rule
        )
    };
    RouteObligation {
        route: route.route_id(),
        surface: route.surface().to_string(),
        family: family.map(|family| family.as_str().to_string()),
        method: route.method().map(str::to_string),
        operation: route.operation().map(str::to_string),
        handler_operation: route.handler_operation().map(str::to_string),
        required_feature: family
            .and_then(|family| family.required_feature())
            .map(str::to_string),
        required_features: features_of_entry(entry).into_iter().collect(),
        feature_semantics: entry_feature_semantics(entry),
        signatures: entry
            .signatures
            .values()
            .map(|signature| ManifestSignature {
                digest: signature.digest.clone(),
            })
            .collect(),
        mapping,
        validation: family
            .map(|family| {
                family
                    .validation()
                    .iter()
                    .map(|reference| reference.to_string())
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn entry_feature_semantics(entry: &InventoryEntry) -> FeatureSemantics {
    if entry.profiles.contains("default") {
        FeatureSemantics::Default
    } else if entry.profiles.contains("full")
        && entry
            .profiles
            .iter()
            .all(|profile| !profile.starts_with("feature:"))
    {
        FeatureSemantics::AllOf
    } else {
        FeatureSemantics::AnyOf
    }
}

/// Facade namespaces that only re-export items defined elsewhere; when an
/// alias group has members outside them, the canonical member never comes
/// from a re-export namespace.
fn is_reexport_namespace(path: &str) -> bool {
    path.starts_with("echo_agent::prelude::") || path.starts_with("echo_agent::advanced::")
}

fn language_target(class: SemanticClass) -> &'static str {
    match class {
        SemanticClass::WireValue => "generated_or_lossless_value",
        SemanticClass::Operation => "idiomatic_method",
        SemanticClass::Handle => "opaque_lifecycle_handle",
        SemanticClass::Stream => "native_async_stream",
        SemanticClass::Extension => "callback_interface",
        SemanticClass::LanguageIntrinsic => "native_language_construct",
    }
}

/// Resolve language coverage from the actual route boundary. Executable and
/// serializable routes are covered by the shared catalog/WireValue adapters;
/// only the explicitly implemented pure helpers cross the intrinsic boundary.
/// Other process-local Rust mechanisms remain `not_implemented` until a real
/// language-native implementation and behavior evidence exists.
fn language_status_for(
    identity: &str,
    classification: SemanticClass,
    route: &RouteObligation,
) -> (LanguageImplementationStatus, &'static str) {
    const LOCAL_TOOL_VALUE_IDENTITIES: &[&str] = &[
        "echo_core::tools::ToolCallParams",
        "echo_core::tools::ToolCallParams::from_params",
        "echo_core::tools::ToolCallParams::from_value",
        "echo_core::tools::ToolCallParams::get",
        "echo_core::tools::ToolCallParams::get_bool",
        "echo_core::tools::ToolCallParams::get_number",
        "echo_core::tools::ToolCallParams::get_str",
        "echo_core::tools::ToolCallParams::has",
        "echo_core::tools::ToolCallParams::is_empty",
        "echo_core::tools::ToolCallParams::len",
        "echo_core::tools::ToolCallParams::validate_required",
        "echo_core::tools::ToolResult",
        "echo_core::tools::ToolResult::error",
        "echo_core::tools::ToolResult::failure",
        "echo_core::tools::ToolResult::invalid_arguments",
        "echo_core::tools::ToolResult::success",
        "echo_core::tools::ToolResult::success_json",
        "echo_core::tools::ToolResult::success_with_kind",
        "echo_core::tools::ToolResult::with_artifact",
        "echo_core::tools::ToolResult::with_data",
        "echo_core::tools::ToolResult::with_error",
        "echo_core::tools::ToolResult::with_failure",
        "echo_core::tools::ToolResult::with_meta",
        "echo_core::tools::ToolResult::with_metadata",
        "echo_core::tools::ToolResult::with_mime_type",
        "echo_core::tools::ToolResult::with_model_content",
        "echo_core::tools::ToolResult::with_output",
        "echo_core::tools::ToolResult::with_truncated",
    ];
    const A2A_TASK_STATE_IDENTITIES: &[&str] = &[
        "echo_agent::a2a::types::TaskState",
        "echo_agent::a2a::types::TaskState::Canceled",
        "echo_agent::a2a::types::TaskState::Completed",
        "echo_agent::a2a::types::TaskState::Failed",
        "echo_agent::a2a::types::TaskState::InputRequired",
        "echo_agent::a2a::types::TaskState::Submitted",
        "echo_agent::a2a::types::TaskState::Working",
        "echo_agent::a2a::types::TaskState::can_transition_to",
        "echo_agent::a2a::types::TaskState::impl<Display>",
        "echo_agent::a2a::types::TaskState::is_terminal",
    ];
    const A2A_VALUE_IDENTITIES: &[&str] = &[
        "echo_agent::a2a::types::A2AMessage",
        "echo_agent::a2a::types::A2AMessage::agent_text",
        "echo_agent::a2a::types::A2AMessage::text_content",
        "echo_agent::a2a::types::A2AMessage::user_text",
        "echo_agent::a2a::types::A2ATaskStatus",
        "echo_agent::a2a::types::A2ATaskStatus::new",
        "echo_agent::a2a::types::A2ATaskStatus::with_message",
        "echo_agent::a2a::types::AgentProvider",
        "echo_agent::a2a::types::AgentProvider::new",
        "echo_agent::a2a::types::AgentProvider::with_url",
        "echo_agent::a2a::types::AgentSkill",
        "echo_agent::a2a::types::AgentSkill::new",
        "echo_agent::a2a::types::AgentSkill::with_examples",
        "echo_agent::a2a::types::AgentSkill::with_tags",
    ];
    const A2A_AGENT_CARD_IDENTITIES: &[&str] = &[
        "echo_agent::a2a::types::AgentCard",
        "echo_agent::a2a::types::AgentCard::builder",
        "echo_agent::a2a::types::AgentCardBuilder",
        "echo_agent::a2a::types::AgentCardBuilder::authentication",
        "echo_agent::a2a::types::AgentCardBuilder::build",
        "echo_agent::a2a::types::AgentCardBuilder::description",
        "echo_agent::a2a::types::AgentCardBuilder::input_modes",
        "echo_agent::a2a::types::AgentCardBuilder::output_modes",
        "echo_agent::a2a::types::AgentCardBuilder::provider",
        "echo_agent::a2a::types::AgentCardBuilder::push_notifications",
        "echo_agent::a2a::types::AgentCardBuilder::skill",
        "echo_agent::a2a::types::AgentCardBuilder::skills",
        "echo_agent::a2a::types::AgentCardBuilder::streaming",
        "echo_agent::a2a::types::AgentCardBuilder::version",
    ];
    const A2A_WIRE_VALUE_IDENTITIES: &[&str] = &[
        "echo_agent::a2a::types::A2AArtifact",
        "echo_agent::a2a::types::A2AArtifact::append",
        "echo_agent::a2a::types::A2AArtifact::index",
        "echo_agent::a2a::types::A2AArtifact::name",
        "echo_agent::a2a::types::A2AArtifact::parts",
        "echo_agent::a2a::types::A2AError",
        "echo_agent::a2a::types::A2AError::code",
        "echo_agent::a2a::types::A2AError::message",
    ];
    const A2A_STREAM_VALUE_IDENTITIES: &[&str] = &[
        "echo_agent::a2a::types::A2AStreamEvent",
        "echo_agent::a2a::types::A2AStreamEvent::ArtifactUpdate",
        "echo_agent::a2a::types::A2AStreamEvent::ArtifactUpdate::0",
        "echo_agent::a2a::types::A2AStreamEvent::StatusUpdate",
        "echo_agent::a2a::types::A2AStreamEvent::StatusUpdate::0",
        "echo_agent::a2a::types::A2AStreamResponse",
        "echo_agent::a2a::types::A2AStreamResponse::error",
        "echo_agent::a2a::types::A2AStreamResponse::id",
        "echo_agent::a2a::types::A2AStreamResponse::jsonrpc",
        "echo_agent::a2a::types::A2AStreamResponse::result",
        "echo_agent::a2a::types::TaskArtifactUpdateEvent",
        "echo_agent::a2a::types::TaskArtifactUpdateEvent::artifact",
        "echo_agent::a2a::types::TaskArtifactUpdateEvent::is_final",
        "echo_agent::a2a::types::TaskArtifactUpdateEvent::task_id",
        "echo_agent::a2a::types::TaskStatusUpdateEvent",
        "echo_agent::a2a::types::TaskStatusUpdateEvent::is_final",
        "echo_agent::a2a::types::TaskStatusUpdateEvent::status",
        "echo_agent::a2a::types::TaskStatusUpdateEvent::task_id",
    ];
    const A2A_TASK_ENVELOPE_IDENTITIES: &[&str] = &[
        "echo_agent::a2a::types::A2ATask",
        "echo_agent::a2a::types::A2ATaskParams",
        "echo_agent::a2a::types::A2ATaskParams::id",
        "echo_agent::a2a::types::A2ATaskParams::message",
        "echo_agent::a2a::types::A2ATaskParams::session_id",
        "echo_agent::a2a::types::A2ATaskRequest",
        "echo_agent::a2a::types::A2ATaskRequest::id",
        "echo_agent::a2a::types::A2ATaskRequest::jsonrpc",
        "echo_agent::a2a::types::A2ATaskRequest::method",
        "echo_agent::a2a::types::A2ATaskRequest::params",
        "echo_agent::a2a::types::A2ATaskResponse",
        "echo_agent::a2a::types::A2ATaskResponse::error",
        "echo_agent::a2a::types::A2ATaskResponse::id",
        "echo_agent::a2a::types::A2ATaskResponse::jsonrpc",
        "echo_agent::a2a::types::A2ATaskResponse::result",
    ];
    const THINKING_LEVEL_IDENTITIES: &[&str] = &[
        "echo_core::llm::thinking::ThinkingLevel",
        "echo_core::llm::thinking::ThinkingLevel::High",
        "echo_core::llm::thinking::ThinkingLevel::Low",
        "echo_core::llm::thinking::ThinkingLevel::Max",
        "echo_core::llm::thinking::ThinkingLevel::Medium",
        "echo_core::llm::thinking::ThinkingLevel::Minimal",
        "echo_core::llm::thinking::ThinkingLevel::None",
        "echo_core::llm::thinking::ThinkingLevel::Xhigh",
        "echo_core::llm::thinking::ThinkingLevel::parse",
        "echo_core::llm::ThinkingLevel",
        "echo_core::llm::ThinkingLevel::High",
        "echo_core::llm::ThinkingLevel::Low",
        "echo_core::llm::ThinkingLevel::Max",
        "echo_core::llm::ThinkingLevel::Medium",
        "echo_core::llm::ThinkingLevel::Minimal",
        "echo_core::llm::ThinkingLevel::None",
        "echo_core::llm::ThinkingLevel::Xhigh",
        "echo_core::llm::ThinkingLevel::parse",
        "echo_agent::llm::ThinkingLevel",
        "echo_agent::llm::ThinkingLevel::High",
        "echo_agent::llm::ThinkingLevel::Low",
        "echo_agent::llm::ThinkingLevel::Max",
        "echo_agent::llm::ThinkingLevel::Medium",
        "echo_agent::llm::ThinkingLevel::Minimal",
        "echo_agent::llm::ThinkingLevel::None",
        "echo_agent::llm::ThinkingLevel::Xhigh",
        "echo_agent::llm::ThinkingLevel::parse",
    ];
    const STEERING_VALUE_IDENTITIES: &[&str] = &[
        "echo_core::agent::AgentSteerState",
        "echo_core::agent::AgentSteerState::Accepted",
        "echo_core::agent::AgentSteerState::Drained",
        "echo_core::agent::AgentSteerState::TurnSettled",
        "echo_core::agent::AgentSteerState::phase",
        "echo_core::agent::AgentSteerState::was_drained",
        "echo_core::agent::AgentSteerTurnOutcome",
        "echo_core::agent::AgentSteerTurnOutcome::Cancelled",
        "echo_core::agent::AgentSteerTurnOutcome::Completed",
        "echo_core::agent::AgentSteerTurnOutcome::Dropped",
        "echo_core::agent::AgentSteerTurnOutcome::Failed",
        "echo_core::agent::AgentSteerTurnOutcome::as_str",
        "echo_core::agent::AgentSteerTurnOutcome::parse",
        "echo_agent::agent::AgentSteerState",
        "echo_agent::agent::AgentSteerState::Accepted",
        "echo_agent::agent::AgentSteerState::Drained",
        "echo_agent::agent::AgentSteerState::TurnSettled",
        "echo_agent::agent::AgentSteerState::phase",
        "echo_agent::agent::AgentSteerState::was_drained",
        "echo_agent::agent::AgentSteerTurnOutcome",
        "echo_agent::agent::AgentSteerTurnOutcome::Cancelled",
        "echo_agent::agent::AgentSteerTurnOutcome::Completed",
        "echo_agent::agent::AgentSteerTurnOutcome::Dropped",
        "echo_agent::agent::AgentSteerTurnOutcome::Failed",
        "echo_agent::agent::AgentSteerTurnOutcome::as_str",
        "echo_agent::agent::AgentSteerTurnOutcome::parse",
    ];
    const SUBAGENT_VALUE_IDENTITIES: &[&str] = &[
        "echo_agent::agent::subagent::SubagentCommandPhase",
        "echo_agent::agent::subagent::SubagentCommandPhase::Drained",
        "echo_agent::agent::subagent::SubagentCommandPhase::MailboxAccepted",
        "echo_agent::agent::subagent::SubagentCommandPhase::Persisted",
        "echo_agent::agent::subagent::SubagentCommandPhase::TurnSettled",
        "echo_agent::agent::subagent::SubagentCommandPhase::as_str",
        "echo_agent::agent::subagent::SubagentCommandPhase::parse",
        "echo_agent::agent::subagent::SubagentStatus",
        "echo_agent::agent::subagent::SubagentStatus::Cancelled",
        "echo_agent::agent::subagent::SubagentStatus::Completed",
        "echo_agent::agent::subagent::SubagentStatus::Failed",
        "echo_agent::agent::subagent::SubagentStatus::Running",
        "echo_agent::agent::subagent::SubagentStatus::TimedOut",
        "echo_agent::agent::subagent::SubagentStatus::as_str",
        "echo_agent::agent::subagent::SubagentStatus::impl<FromStr>",
        "echo_agent::agent::subagent::control::SubagentCommandPhase",
        "echo_agent::agent::subagent::control::SubagentCommandPhase::Drained",
        "echo_agent::agent::subagent::control::SubagentCommandPhase::MailboxAccepted",
        "echo_agent::agent::subagent::control::SubagentCommandPhase::Persisted",
        "echo_agent::agent::subagent::control::SubagentCommandPhase::TurnSettled",
        "echo_agent::agent::subagent::control::SubagentCommandPhase::as_str",
        "echo_agent::agent::subagent::control::SubagentCommandPhase::parse",
        "echo_agent::agent::subagent::types::SubagentStatus",
        "echo_agent::agent::subagent::types::SubagentStatus::Cancelled",
        "echo_agent::agent::subagent::types::SubagentStatus::Completed",
        "echo_agent::agent::subagent::types::SubagentStatus::Failed",
        "echo_agent::agent::subagent::types::SubagentStatus::Running",
        "echo_agent::agent::subagent::types::SubagentStatus::TimedOut",
        "echo_agent::agent::subagent::types::SubagentStatus::as_str",
        "echo_agent::agent::subagent::types::SubagentStatus::impl<FromStr>",
    ];
    const CONTENT_GUARD_VALUE_IDENTITIES: &[&str] = &[
        "echo_core::guard::content::ContentGuardResult",
        "echo_core::guard::content::ContentGuardResult::Detected",
        "echo_core::guard::content::ContentGuardResult::Pass",
        "echo_core::guard::content::ContentGuardResult::Redacted",
        "echo_core::guard::content::ContentGuardResult::Rejected",
        "echo_core::guard::content::ContentGuardResult::is_rejected",
        "echo_agent::guard::content::ContentGuardResult",
        "echo_agent::guard::content::ContentGuardResult::Detected",
        "echo_agent::guard::content::ContentGuardResult::Pass",
        "echo_agent::guard::content::ContentGuardResult::Redacted",
        "echo_agent::guard::content::ContentGuardResult::Rejected",
        "echo_agent::guard::content::ContentGuardResult::is_rejected",
    ];
    const GUARD_VALUE_IDENTITIES: &[&str] = &[
        "echo_core::guard::GuardResult",
        "echo_core::guard::GuardResult::Block",
        "echo_core::guard::GuardResult::Pass",
        "echo_core::guard::GuardResult::Transform",
        "echo_core::guard::GuardResult::Warn",
        "echo_core::guard::GuardResult::is_blocked",
        "echo_agent::guard::GuardResult",
        "echo_agent::guard::GuardResult::Block",
        "echo_agent::guard::GuardResult::Pass",
        "echo_agent::guard::GuardResult::Transform",
        "echo_agent::guard::GuardResult::Warn",
        "echo_agent::guard::GuardResult::is_blocked",
    ];
    const DELIVERY_VALUE_IDENTITIES: &[&str] = &[
        "echo_state::delivery::DeliveryOutcome",
        "echo_state::delivery::DeliveryOutcome::Cancelled",
        "echo_state::delivery::DeliveryOutcome::Completed",
        "echo_state::delivery::DeliveryOutcome::Dropped",
        "echo_state::delivery::DeliveryOutcome::Failed",
        "echo_state::delivery::DeliveryOutcome::OutcomeUnknown",
        "echo_state::delivery::DeliveryOutcome::as_str",
        "echo_state::delivery::DeliveryPhase",
        "echo_state::delivery::DeliveryPhase::Claimed",
        "echo_state::delivery::DeliveryPhase::Deferred",
        "echo_state::delivery::DeliveryPhase::Drained",
        "echo_state::delivery::DeliveryPhase::EffectStarted",
        "echo_state::delivery::DeliveryPhase::MailboxAccepted",
        "echo_state::delivery::DeliveryPhase::Persisted",
        "echo_state::delivery::DeliveryPhase::TurnSettled",
        "echo_state::delivery::DeliveryPhase::as_str",
        "echo_agent::delivery::DeliveryOutcome",
        "echo_agent::delivery::DeliveryOutcome::Cancelled",
        "echo_agent::delivery::DeliveryOutcome::Completed",
        "echo_agent::delivery::DeliveryOutcome::Dropped",
        "echo_agent::delivery::DeliveryOutcome::Failed",
        "echo_agent::delivery::DeliveryOutcome::OutcomeUnknown",
        "echo_agent::delivery::DeliveryOutcome::as_str",
        "echo_agent::delivery::DeliveryPhase",
        "echo_agent::delivery::DeliveryPhase::Claimed",
        "echo_agent::delivery::DeliveryPhase::Deferred",
        "echo_agent::delivery::DeliveryPhase::Drained",
        "echo_agent::delivery::DeliveryPhase::EffectStarted",
        "echo_agent::delivery::DeliveryPhase::MailboxAccepted",
        "echo_agent::delivery::DeliveryPhase::Persisted",
        "echo_agent::delivery::DeliveryPhase::TurnSettled",
        "echo_agent::delivery::DeliveryPhase::as_str",
    ];
    const SUBAGENT_STOP_VALUE_IDENTITIES: &[&str] = &[
        "echo_core::hooks::types::SubagentStopStatus",
        "echo_core::hooks::types::SubagentStopStatus::Cancelled",
        "echo_core::hooks::types::SubagentStopStatus::Completed",
        "echo_core::hooks::types::SubagentStopStatus::Failed",
        "echo_core::hooks::types::SubagentStopStatus::TimedOut",
        "echo_core::hooks::types::SubagentStopStatus::as_str",
        "echo_agent::hooks::SubagentStopStatus",
        "echo_agent::hooks::SubagentStopStatus::Cancelled",
        "echo_agent::hooks::SubagentStopStatus::Completed",
        "echo_agent::hooks::SubagentStopStatus::Failed",
        "echo_agent::hooks::SubagentStopStatus::TimedOut",
        "echo_agent::hooks::SubagentStopStatus::as_str",
    ];
    const TASK_TERMINAL_VALUE_IDENTITIES: &[&str] = &[
        "echo_core::hooks::types::TaskTerminalStatus",
        "echo_core::hooks::types::TaskTerminalStatus::Cancelled",
        "echo_core::hooks::types::TaskTerminalStatus::Completed",
        "echo_core::hooks::types::TaskTerminalStatus::Failed",
        "echo_core::hooks::types::TaskTerminalStatus::Skipped",
        "echo_core::hooks::types::TaskTerminalStatus::TimedOut",
        "echo_core::hooks::types::TaskTerminalStatus::as_str",
        "echo_agent::hooks::TaskTerminalStatus",
        "echo_agent::hooks::TaskTerminalStatus::Cancelled",
        "echo_agent::hooks::TaskTerminalStatus::Completed",
        "echo_agent::hooks::TaskTerminalStatus::Failed",
        "echo_agent::hooks::TaskTerminalStatus::Skipped",
        "echo_agent::hooks::TaskTerminalStatus::TimedOut",
        "echo_agent::hooks::TaskTerminalStatus::as_str",
    ];
    const PERMISSION_RULE_SOURCE_IDENTITIES: &[&str] = &[
        "echo_core::tools::permission::RuleSource",
        "echo_core::tools::permission::RuleSource::CliArg",
        "echo_core::tools::permission::RuleSource::Default",
        "echo_core::tools::permission::RuleSource::LocalSettings",
        "echo_core::tools::permission::RuleSource::Managed",
        "echo_core::tools::permission::RuleSource::ProjectSettings",
        "echo_core::tools::permission::RuleSource::Session",
        "echo_core::tools::permission::RuleSource::UserSettings",
        "echo_core::tools::permission::RuleSource::impl<Display>",
        "echo_core::tools::permission::RuleSource::impl<FromStr>",
        "echo_agent::tools::permission::RuleSource",
        "echo_agent::tools::permission::RuleSource::CliArg",
        "echo_agent::tools::permission::RuleSource::Default",
        "echo_agent::tools::permission::RuleSource::LocalSettings",
        "echo_agent::tools::permission::RuleSource::Managed",
        "echo_agent::tools::permission::RuleSource::ProjectSettings",
        "echo_agent::tools::permission::RuleSource::Session",
        "echo_agent::tools::permission::RuleSource::UserSettings",
        "echo_agent::tools::permission::RuleSource::impl<Display>",
        "echo_agent::tools::permission::RuleSource::impl<FromStr>",
    ];
    const PERMISSION_RULE_BEHAVIOR_IDENTITIES: &[&str] = &[
        "echo_core::tools::permission::RuleBehavior",
        "echo_core::tools::permission::RuleBehavior::Allow",
        "echo_core::tools::permission::RuleBehavior::Ask",
        "echo_core::tools::permission::RuleBehavior::Deny",
        "echo_core::tools::permission::RuleBehavior::impl<FromStr>",
        "echo_core::tools::permission::RuleBehavior::to_decision",
        "echo_agent::tools::permission::RuleBehavior",
        "echo_agent::tools::permission::RuleBehavior::Allow",
        "echo_agent::tools::permission::RuleBehavior::Ask",
        "echo_agent::tools::permission::RuleBehavior::Deny",
        "echo_agent::tools::permission::RuleBehavior::impl<FromStr>",
        "echo_agent::tools::permission::RuleBehavior::to_decision",
    ];
    const PERMISSION_MODE_HELPER_IDENTITIES: &[&str] = &[
        "echo_core::tools::permission::PermissionMode::allows_write",
        "echo_core::tools::permission::PermissionMode::id",
        "echo_core::tools::permission::PermissionMode::impl<Display>",
        "echo_core::tools::permission::PermissionMode::impl<FromStr>",
        "echo_core::tools::permission::PermissionMode::requires_interaction",
        "echo_core::tools::permission::PermissionMode::uses_classifier",
        "echo_agent::tools::permission::PermissionMode::allows_write",
        "echo_agent::tools::permission::PermissionMode::id",
        "echo_agent::tools::permission::PermissionMode::impl<Display>",
        "echo_agent::tools::permission::PermissionMode::impl<FromStr>",
        "echo_agent::tools::permission::PermissionMode::requires_interaction",
        "echo_agent::tools::permission::PermissionMode::uses_classifier",
    ];
    const PERMISSION_RULE_MATCHER_IDENTITIES: &[&str] = &[
        "echo_core::tools::permission::RuleMatcher",
        "echo_core::tools::permission::RuleMatcher::All",
        "echo_core::tools::permission::RuleMatcher::Pattern",
        "echo_core::tools::permission::RuleMatcher::Permission",
        "echo_core::tools::permission::RuleMatcher::Tool",
        "echo_core::tools::permission::RuleMatcher::impl<Display>",
        "echo_core::tools::permission::RuleMatcher::impl<FromStr>",
        "echo_core::tools::permission::RuleMatcher::matches",
        "echo_core::tools::permission::RuleMatcher::matches_matcher_str",
        "echo_agent::tools::permission::RuleMatcher",
        "echo_agent::tools::permission::RuleMatcher::All",
        "echo_agent::tools::permission::RuleMatcher::Pattern",
        "echo_agent::tools::permission::RuleMatcher::Permission",
        "echo_agent::tools::permission::RuleMatcher::Tool",
        "echo_agent::tools::permission::RuleMatcher::impl<Display>",
        "echo_agent::tools::permission::RuleMatcher::impl<FromStr>",
        "echo_agent::tools::permission::RuleMatcher::matches",
        "echo_agent::tools::permission::RuleMatcher::matches_matcher_str",
    ];
    const COMMAND_CELL_PHASE_IDENTITIES: &[&str] = &[
        "echo_core::tools::cell::CommandCellPhase",
        "echo_core::tools::cell::CommandCellPhase::Cancelled",
        "echo_core::tools::cell::CommandCellPhase::Failed",
        "echo_core::tools::cell::CommandCellPhase::LaunchFailed",
        "echo_core::tools::cell::CommandCellPhase::Prepared",
        "echo_core::tools::cell::CommandCellPhase::Queued",
        "echo_core::tools::cell::CommandCellPhase::Running",
        "echo_core::tools::cell::CommandCellPhase::Succeeded",
        "echo_core::tools::cell::CommandCellPhase::as_str",
        "echo_core::tools::cell::CommandCellPhase::is_terminal",
        "echo_agent::tools::cell::CommandCellPhase",
        "echo_agent::tools::cell::CommandCellPhase::Cancelled",
        "echo_agent::tools::cell::CommandCellPhase::Failed",
        "echo_agent::tools::cell::CommandCellPhase::LaunchFailed",
        "echo_agent::tools::cell::CommandCellPhase::Prepared",
        "echo_agent::tools::cell::CommandCellPhase::Queued",
        "echo_agent::tools::cell::CommandCellPhase::Running",
        "echo_agent::tools::cell::CommandCellPhase::Succeeded",
        "echo_agent::tools::cell::CommandCellPhase::as_str",
        "echo_agent::tools::cell::CommandCellPhase::is_terminal",
    ];
    const COMMAND_CELL_STATUS_IDENTITIES: &[&str] = &[
        "echo_core::tools::cell::CommandCellTerminalCause",
        "echo_core::tools::cell::CommandCellTerminalCause::Cancelled",
        "echo_core::tools::cell::CommandCellTerminalCause::Exited",
        "echo_core::tools::cell::CommandCellTerminalCause::LaunchFailed",
        "echo_core::tools::cell::CommandCellTerminalCause::OutputDrainFailed",
        "echo_core::tools::cell::CommandCellTerminalCause::TimedOut",
        "echo_core::tools::cell::CommandCellTerminalCause::WaitFailed",
        "echo_core::tools::cell::CommandCellTerminalCause::as_str",
        "echo_core::tools::cell::CommandCellArtifactStatus",
        "echo_core::tools::cell::CommandCellArtifactStatus::Available",
        "echo_core::tools::cell::CommandCellArtifactStatus::BelowThreshold",
        "echo_core::tools::cell::CommandCellArtifactStatus::Failed",
        "echo_core::tools::cell::CommandCellArtifactStatus::NotRequested",
        "echo_core::tools::cell::CommandCellArtifactStatus::Writing",
        "echo_core::tools::cell::CommandCellArtifactStatus::as_str",
        "echo_agent::tools::cell::CommandCellTerminalCause",
        "echo_agent::tools::cell::CommandCellTerminalCause::Cancelled",
        "echo_agent::tools::cell::CommandCellTerminalCause::Exited",
        "echo_agent::tools::cell::CommandCellTerminalCause::LaunchFailed",
        "echo_agent::tools::cell::CommandCellTerminalCause::OutputDrainFailed",
        "echo_agent::tools::cell::CommandCellTerminalCause::TimedOut",
        "echo_agent::tools::cell::CommandCellTerminalCause::WaitFailed",
        "echo_agent::tools::cell::CommandCellTerminalCause::as_str",
        "echo_agent::tools::cell::CommandCellArtifactStatus",
        "echo_agent::tools::cell::CommandCellArtifactStatus::Available",
        "echo_agent::tools::cell::CommandCellArtifactStatus::BelowThreshold",
        "echo_agent::tools::cell::CommandCellArtifactStatus::Failed",
        "echo_agent::tools::cell::CommandCellArtifactStatus::NotRequested",
        "echo_agent::tools::cell::CommandCellArtifactStatus::Writing",
        "echo_agent::tools::cell::CommandCellArtifactStatus::as_str",
    ];
    const TEAM_STRATEGY_VALUE_IDENTITIES: &[&str] = &[
        "echo_agent::agent::subagent::team::TeamStrategy",
        "echo_agent::agent::subagent::team::TeamStrategy::Debate",
        "echo_agent::agent::subagent::team::TeamStrategy::ManagerSubagent",
        "echo_agent::agent::subagent::team::TeamStrategy::Pipeline",
        "echo_agent::agent::subagent::team::TeamStrategy::Swarm",
        "echo_agent::agent::subagent::team::TeamStrategy::description",
        "echo_agent::agent::subagent::team::TeamStrategy::name",
    ];
    const ACP_VALUE_IDENTITIES: &[&str] = &[
        "echo_agent::acp::runtime::AcpLedgerLimits",
        "echo_agent::acp::runtime::AcpLedgerLimits::impl<Default>",
        "echo_agent::acp::runtime::AcpLedgerLimits::max_bytes",
        "echo_agent::acp::runtime::AcpLedgerLimits::max_events",
        "echo_agent::acp::runtime::ConnectionMode",
        "echo_agent::acp::runtime::ConnectionMode::Extended",
        "echo_agent::acp::runtime::ConnectionMode::Standard",
        "echo_agent::acp::extension::ExtensionSettlement",
        "echo_agent::acp::extension::ExtensionSettlement::Answered",
        "echo_agent::acp::extension::ExtensionSettlement::Cancelled",
        "echo_agent::acp::extension::ExtensionSettlement::Disconnected",
        "echo_agent::acp::extension::ExtensionSettlement::TimedOut",
        "echo_agent::acp::extension::ExtensionSettlement::is_answered",
    ];
    const ACP_CONFIG_VALUE_IDENTITIES: &[&str] = &[
        "echo_agent::acp::adapter::AcpAdapterConfig",
        "echo_agent::acp::adapter::AcpAdapterConfig::impl<Default>",
        "echo_agent::acp::adapter::AcpAdapterConfig::max_extension_concurrency",
        "echo_agent::acp::adapter::AcpAdapterConfig::max_prompt_chars",
        "echo_agent::acp::adapter::AcpAdapterConfig::max_sessions",
        "echo_agent::acp::adapter::AcpAdapterConfig::max_total_update_chars",
        "echo_agent::acp::adapter::AcpAdapterConfig::max_update_chars",
        "echo_agent::acp::adapter::AcpAdapterConfig::max_updates_per_turn",
        "echo_agent::acp::adapter::AcpAdapterConfig::name",
        "echo_agent::acp::adapter::AcpAdapterConfig::shutdown_timeout",
        "echo_agent::acp::adapter::AcpAdapterConfig::title",
        "echo_agent::acp::adapter::AcpAdapterConfig::validate",
        "echo_agent::acp::adapter::AcpAdapterConfig::version",
    ];
    const ACP_LEASE_VALUE_IDENTITIES: &[&str] = &[
        "echo_agent::acp::extension::ExtensionLeaseError",
        "echo_agent::acp::extension::ExtensionLeaseError::AdmissionClosed",
        "echo_agent::acp::extension::ExtensionLeaseError::ConcurrencyLimit",
        "echo_agent::acp::extension::ExtensionLeaseError::ExclusiveConflict",
        "echo_agent::acp::extension::ExtensionLeaseError::impl<Display>",
    ];
    const JWT_VALUE_IDENTITIES: &[&str] = &[
        "echo_agent::a2a::auth::JwtClaims::subject",
        "echo_agent::a2a::auth::JwtConfig",
        "echo_agent::a2a::auth::JwtConfig::disabled",
        "echo_agent::a2a::auth::JwtConfig::hs256",
        "echo_agent::a2a::auth::JwtConfig::impl<Debug>",
        "echo_agent::a2a::auth::JwtConfig::is_enabled",
        "echo_agent::a2a::auth::JwtConfig::rs256",
        "echo_agent::a2a::auth::JwtConfig::with_audience",
        "echo_agent::a2a::auth::JwtConfig::with_issuer",
    ];
    const DEPENDENCY_VALUE_IDENTITIES: &[&str] = &[
        "echo_execution::skills::dependency_probe::DepKind",
        "echo_execution::skills::dependency_probe::DepKind::Binary",
        "echo_execution::skills::dependency_probe::DepKind::NodeModule",
        "echo_execution::skills::dependency_probe::DepKind::PythonPkg",
        "echo_execution::skills::external::prompt_exec::SkillSource",
        "echo_execution::skills::external::prompt_exec::SkillSource::Local",
        "echo_execution::skills::external::prompt_exec::SkillSource::Mcp",
    ];
    const CONTEXT_INHERITANCE_IDENTITIES: &[&str] = &[
        "echo_agent::agent::subagent::context::ContextInheritance",
        "echo_agent::agent::subagent::context::ContextInheritance::for_mode",
        "echo_agent::agent::subagent::context::ContextInheritance::fork_default",
        "echo_agent::agent::subagent::context::ContextInheritance::fresh_default",
        "echo_agent::agent::subagent::context::ContextInheritance::impl<Default>",
        "echo_agent::agent::subagent::context::ContextInheritance::inherit_history",
        "echo_agent::agent::subagent::context::ContextInheritance::inherit_memory",
        "echo_agent::agent::subagent::context::ContextInheritance::inherit_tools",
        "echo_agent::agent::subagent::context::ContextInheritance::inject_metadata",
        "echo_agent::agent::subagent::context::ContextInheritance::sync_default",
        "echo_agent::agent::subagent::context::ContextInheritance::teammate_default",
    ];
    const OBSERVED_ISOLATION_IDENTITIES: &[&str] = &[
        "echo_agent::agent::subagent::types::ObservedIsolation",
        "echo_agent::agent::subagent::types::ObservedIsolation::as_str",
        "echo_agent::agent::subagent::types::ObservedIsolation::impl<Default>",
        "echo_agent::agent::subagent::types::ObservedIsolation::new",
    ];
    const SEGMENT_RANGE_IDENTITIES: &[&str] = &[
        "echo_core::llm::cache::layout::SegmentRange",
        "echo_core::llm::cache::layout::SegmentRange::is_empty",
        "echo_core::llm::cache::layout::SegmentRange::len",
    ];
    const PROMPT_DIAGNOSTICS_IDENTITIES: &[&str] = &[
        "echo_agent::agent::subagent::prompt::PromptDiagnostics",
        "echo_agent::agent::subagent::prompt::PromptDiagnostics::count",
        "echo_agent::agent::subagent::prompt::PromptDiagnostics::record",
    ];
    const SUBAGENT_COMMAND_IDENTITY_IDENTITIES: &[&str] = &[
        "echo_agent::agent::subagent::control::SubagentCommandIdentity",
        "echo_agent::agent::subagent::control::SubagentCommandIdentity::attempt_identity",
        "echo_agent::agent::subagent::control::SubagentCommandIdentity::new",
        "echo_agent::agent::subagent::control::SubagentCommandIdentity::validate",
        "echo_agent::agent::subagent::control::SubagentAttemptIdentity",
        "echo_agent::agent::subagent::control::SubagentAttemptIdentity::new",
    ];
    const SUBAGENT_USAGE_IDENTITIES: &[&str] = &[
        "echo_agent::agent::subagent::usage::LlmUsageStats",
        "echo_agent::agent::subagent::usage::LlmUsageStats::record",
        "echo_agent::agent::subagent::usage::LlmUsageStats::to_payload",
    ];
    const TOOL_OUTPUT_ARTIFACT_CONFIG_IDENTITIES: &[&str] = &[
        "echo_core::tools::artifact::ToolOutputArtifactConfig",
        "echo_core::tools::artifact::ToolOutputArtifactConfig::impl<Default>",
        "echo_core::tools::artifact::ToolOutputArtifactConfig::max_age_secs",
        "echo_core::tools::artifact::ToolOutputArtifactConfig::new",
        "echo_core::tools::artifact::ToolOutputArtifactConfig::threshold_bytes",
    ];
    const SKILL_VALIDATION_IDENTITIES: &[&str] = &[
        "echo_execution::skills::external::validate::SkillValidationReport",
        "echo_execution::skills::external::validate::SkillValidationReport::is_valid",
    ];
    const SKILL_CONTENT_IDENTITIES: &[&str] = &[
        "echo_execution::skills::external::types::SkillContent",
        "echo_execution::skills::external::types::SkillContent::to_prompt_block",
    ];
    const JSONRPC_VALUE_IDENTITIES: &[&str] = &[
        "echo_integration::mcp::types::JsonRpcNotification",
        "echo_integration::mcp::types::JsonRpcNotification::new",
        "echo_integration::mcp::types::JsonRpcRequest",
        "echo_integration::mcp::types::JsonRpcRequest::new",
    ];
    const SUBAGENT_CONTEXT_IDENTITIES: &[&str] = &[
        "echo_agent::agent::subagent::context::SubagentContext",
        "echo_agent::agent::subagent::context::SubagentContext::empty",
        "echo_agent::agent::subagent::context::SubagentContext::has_content",
    ];
    const USAGE_IDENTITIES: &[&str] = &[
        "echo_core::llm::types::Usage",
        "echo_core::llm::types::Usage::cache_creation_prompt_tokens",
        "echo_core::llm::types::Usage::cache_hit_rate",
        "echo_core::llm::types::Usage::cached_prompt_tokens",
        "echo_core::llm::types::Usage::effective_prompt_tokens",
        "echo_core::llm::types::Usage::effective_total_tokens",
    ];
    const HOOK_ACTION_IDENTITIES: &[&str] = &[
        "echo_execution::skills::hooks::HookAction",
        "echo_execution::skills::hooks::HookAction::ActivateSkill",
        "echo_execution::skills::hooks::HookAction::ActivateSkill::reason",
        "echo_execution::skills::hooks::HookAction::ActivateSkill::skill",
        "echo_execution::skills::hooks::HookAction::Command",
        "echo_execution::skills::hooks::HookAction::Command::command",
        "echo_execution::skills::hooks::HookAction::Command::shell",
        "echo_execution::skills::hooks::HookAction::Command::timeout",
        "echo_execution::skills::hooks::HookAction::Http",
        "echo_execution::skills::hooks::HookAction::Http::headers",
        "echo_execution::skills::hooks::HookAction::Http::method",
        "echo_execution::skills::hooks::HookAction::Http::timeout",
        "echo_execution::skills::hooks::HookAction::Http::url",
        "echo_execution::skills::hooks::HookAction::McpTool",
        "echo_execution::skills::hooks::HookAction::McpTool::arguments",
        "echo_execution::skills::hooks::HookAction::McpTool::server",
        "echo_execution::skills::hooks::HookAction::McpTool::timeout",
        "echo_execution::skills::hooks::HookAction::McpTool::tool",
        "echo_execution::skills::hooks::HookAction::Permission",
        "echo_execution::skills::hooks::HookAction::Permission::decision",
        "echo_execution::skills::hooks::HookAction::Permission::reason",
        "echo_execution::skills::hooks::HookAction::Permission::suggestions",
        "echo_execution::skills::hooks::HookAction::Prompt",
        "echo_execution::skills::hooks::HookAction::Prompt::prompt",
        "echo_execution::skills::hooks::HookAction::Subagent",
        "echo_execution::skills::hooks::HookAction::Subagent::name",
        "echo_execution::skills::hooks::HookAction::Subagent::task",
        "echo_execution::skills::hooks::HookAction::Subagent::timeout",
        "echo_execution::skills::hooks::HookAction::kind",
        "echo_execution::skills::hooks::HookAction::validate",
    ];
    const PAGE_INFO_IDENTITIES: &[&str] = &[
        "echo_core::tools::pagination::PageInfo",
        "echo_core::tools::pagination::PageInfo::apply_to",
    ];
    const HOOK_EVENT_IDENTITIES: &[&str] = &[
        "echo_core::hooks::types::HookEvent",
        "echo_core::hooks::types::HookEvent::ALL",
        "echo_core::hooks::types::HookEvent::ConfigChange",
        "echo_core::hooks::types::HookEvent::InstructionsLoaded",
        "echo_core::hooks::types::HookEvent::MemoryLayerChange",
        "echo_core::hooks::types::HookEvent::Notification",
        "echo_core::hooks::types::HookEvent::PermissionDenied",
        "echo_core::hooks::types::HookEvent::PermissionRequest",
        "echo_core::hooks::types::HookEvent::PluginDisabled",
        "echo_core::hooks::types::HookEvent::PluginLoaded",
        "echo_core::hooks::types::HookEvent::PostCompact",
        "echo_core::hooks::types::HookEvent::PostMemoryWrite",
        "echo_core::hooks::types::HookEvent::PostToolBatch",
        "echo_core::hooks::types::HookEvent::PostToolUse",
        "echo_core::hooks::types::HookEvent::PostToolUseFailure",
        "echo_core::hooks::types::HookEvent::PreCompact",
        "echo_core::hooks::types::HookEvent::PreToolUse",
        "echo_core::hooks::types::HookEvent::RulePromoted",
        "echo_core::hooks::types::HookEvent::SessionEnd",
        "echo_core::hooks::types::HookEvent::SessionStart",
        "echo_core::hooks::types::HookEvent::SkillCandidateDetected",
        "echo_core::hooks::types::HookEvent::SkillHealthCheck",
        "echo_core::hooks::types::HookEvent::SkillLifecycleTransition",
        "echo_core::hooks::types::HookEvent::SkillMergeApplied",
        "echo_core::hooks::types::HookEvent::SkillPatchApplied",
        "echo_core::hooks::types::HookEvent::Stop",
        "echo_core::hooks::types::HookEvent::StopFailure",
        "echo_core::hooks::types::HookEvent::SubagentStart",
        "echo_core::hooks::types::HookEvent::SubagentStop",
        "echo_core::hooks::types::HookEvent::TaskCompleted",
        "echo_core::hooks::types::HookEvent::TaskCreated",
        "echo_core::hooks::types::HookEvent::TaskStarted",
        "echo_core::hooks::types::HookEvent::UserPromptSubmit",
        "echo_core::hooks::types::HookEvent::as_str",
        "echo_core::hooks::types::HookEvent::category",
        "echo_core::hooks::types::HookEvent::from_name",
        "echo_core::hooks::types::HookEvent::is_tool_event",
        "echo_core::hooks::types::HookEvent::supports_matcher",
        "echo_core::hooks::types::HookEventCategory",
        "echo_core::hooks::types::HookEventCategory::Error",
        "echo_core::hooks::types::HookEventCategory::Evolution",
        "echo_core::hooks::types::HookEventCategory::Lifecycle",
        "echo_core::hooks::types::HookEventCategory::Subagent",
        "echo_core::hooks::types::HookEventCategory::Task",
        "echo_core::hooks::types::HookEventCategory::Tool",
    ];
    const EVENT_IDENTITY_IDENTITIES: &[&str] = &[
        "echo_core::agent::event_envelope::EventId",
        "echo_core::agent::event_envelope::EventId::as_str",
        "echo_core::agent::event_envelope::EventId::impl<AsRef>",
        "echo_core::agent::event_envelope::EventId::impl<Display>",
        "echo_core::agent::event_envelope::EventId::new",
        "echo_core::agent::event_envelope::EventIdentity",
        "echo_core::agent::event_envelope::EventIdentity::conversation_id",
        "echo_core::agent::event_envelope::EventIdentity::execution_id",
        "echo_core::agent::event_envelope::EventIdentity::for_chat",
        "echo_core::agent::event_envelope::EventIdentity::for_run",
        "echo_core::agent::event_envelope::EventIdentity::from_invocation",
        "echo_core::agent::event_envelope::EventIdentity::from_runtime_context",
        "echo_core::agent::event_envelope::EventIdentity::message_id",
        "echo_core::agent::event_envelope::EventIdentity::new",
        "echo_core::agent::event_envelope::EventIdentity::parent_event_id",
        "echo_core::agent::event_envelope::EventIdentity::run_id",
        "echo_core::agent::event_envelope::EventIdentity::stream_id",
        "echo_core::agent::event_envelope::EventIdentity::turn_id",
        "echo_core::agent::event_envelope::EventIdentity::validate",
        "echo_core::agent::event_envelope::EventIdentity::with_conversation_id",
        "echo_core::agent::event_envelope::EventIdentity::with_execution_id",
        "echo_core::agent::event_envelope::EventIdentity::with_message_id",
        "echo_core::agent::event_envelope::EventIdentity::with_parent_event_id",
        "echo_core::agent::event_envelope::EventIdentity::with_run_id",
        "echo_core::agent::event_envelope::StreamId",
        "echo_core::agent::event_envelope::StreamId::as_str",
        "echo_core::agent::event_envelope::StreamId::impl<AsRef>",
        "echo_core::agent::event_envelope::StreamId::impl<Display>",
        "echo_core::agent::event_envelope::StreamId::new",
    ];
    const INTERVENTION_RESULT_IDENTITIES: &[&str] = &[
        "echo_core::agent::intervention::InterventionResult::allow",
        "echo_core::agent::intervention::InterventionResult::block",
        "echo_core::agent::intervention::InterventionResult::cancel",
        "echo_core::agent::intervention::InterventionResult::inject",
        "echo_core::agent::intervention::InterventionResult::modify_args",
    ];
    const TOKEN_BUDGET_IDENTITIES: &[&str] = &[
        "echo_core::budget::TokenAllocation::needs_compression",
        "echo_core::budget::TokenAllocation::ok",
        "echo_core::budget::TokenBudget",
        "echo_core::budget::TokenBudget::allocate",
        "echo_core::budget::TokenBudget::conversation_budget",
        "echo_core::budget::TokenBudget::impl<Default>",
        "echo_core::budget::TokenBudget::new",
        "echo_core::budget::TokenBudget::output_budget",
        "echo_core::budget::TokenBudget::report",
        "echo_core::budget::TokenBudget::safety_budget",
        "echo_core::budget::TokenBudget::system_prompt_budget",
        "echo_core::budget::TokenBudget::tool_definitions_budget",
        "echo_core::budget::TokenBudget::total_window",
        "echo_core::budget::TokenBudget::with_allocations",
        "echo_core::budget::TokenBudgetConfig",
        "echo_core::budget::TokenBudgetConfig::build",
        "echo_core::budget::TokenBudgetConfig::disabled",
        "echo_core::budget::TokenBudgetConfig::enabled",
        "echo_core::budget::TokenBudgetConfig::impl<Default>",
        "echo_core::budget::TokenBudgetConfig::with_total_window",
        "echo_core::llm::LlmTimeouts",
        "echo_core::llm::LlmTimeouts::first_chunk_timeout",
        "echo_core::llm::LlmTimeouts::idle_timeout",
        "echo_core::llm::LlmTimeouts::impl<Default>",
        "echo_core::llm::LlmTimeouts::overall_timeout",
        "echo_core::llm::LlmTimeouts::request_timeout",
        "echo_core::llm::LlmTimeouts::with_first_chunk_timeout",
        "echo_core::llm::LlmTimeouts::with_idle_timeout",
        "echo_core::llm::LlmTimeouts::with_overall_timeout",
        "echo_core::llm::LlmTimeouts::with_request_timeout",
        "echo_core::llm::LlmTimeouts::without_first_chunk_timeout",
        "echo_core::llm::LlmTimeouts::without_idle_timeout",
        "echo_core::llm::LlmTimeouts::without_overall_timeout",
        "echo_core::llm::LlmTimeouts::without_request_timeout",
    ];
    const EXECUTION_USAGE_IDENTITIES: &[&str] =
        &["echo_core::agent::ExecutionUsage::duration_millis"];
    const TURN_MODE_IDENTITIES: &[&str] = &[
        "echo_orchestration::runtime::turn_driver::TurnMode",
        "echo_orchestration::runtime::turn_driver::TurnMode::Chat",
        "echo_orchestration::runtime::turn_driver::TurnMode::Execute",
    ];
    const RETRY_POLICY_IDENTITIES: &[&str] = &[
        "echo_core::retry::RetryPolicy",
        "echo_core::retry::RetryPolicy::delay_for",
        "echo_core::retry::RetryPolicy::impl<Default>",
        "echo_core::retry::RetryPolicy::jitter",
        "echo_core::retry::RetryPolicy::max_delay",
        "echo_core::retry::RetryPolicy::new",
        "echo_core::retry::RetryPolicy::no_retry",
    ];
    const THINKING_CONFIG_IDENTITIES: &[&str] = &[
        "echo_core::llm::thinking::ThinkingConfig",
        "echo_core::llm::thinking::ThinkingConfig::BudgetTokens",
        "echo_core::llm::thinking::ThinkingConfig::BudgetTokens::0",
        "echo_core::llm::thinking::ThinkingConfig::Disabled",
        "echo_core::llm::thinking::ThinkingConfig::Level",
        "echo_core::llm::thinking::ThinkingConfig::Level::0",
        "echo_core::llm::thinking::ThinkingConfig::medium",
        "echo_core::llm::thinking::ThinkingConfig::parse_spec",
        "echo_core::llm::thinking::ThinkingConfig::to_anthropic_budget",
        "echo_core::llm::thinking::ThinkingConfig::to_anthropic_effort",
        "echo_core::llm::thinking::ThinkingConfig::to_enable_thinking",
        "echo_core::llm::thinking::ThinkingConfig::to_glm_reasoning_effort",
        "echo_core::llm::thinking::ThinkingConfig::to_glm_thinking_type",
        "echo_core::llm::thinking::ThinkingConfig::to_reasoning_effort",
    ];
    const TIME_IDENTITIES: &[&str] = &[
        "echo_core::utils::time::local_rfc3339::deserialize",
        "echo_core::utils::time::local_rfc3339::serialize",
        "echo_core::utils::time::now_local",
        "echo_core::utils::time::now_millis",
        "echo_core::utils::time::now_secs",
        "echo_core::utils::time::option_local_rfc3339::deserialize",
        "echo_core::utils::time::option_local_rfc3339::serialize",
        "echo_core::utils::time::to_local",
    ];
    const THINKING_PROTOCOL_IDENTITIES: &[&str] = &[
        "echo_core::llm::thinking::ThinkingProtocol",
        "echo_core::llm::thinking::ThinkingProtocol::AnthropicAdaptive",
        "echo_core::llm::thinking::ThinkingProtocol::AnthropicEffort",
        "echo_core::llm::thinking::ThinkingProtocol::AnthropicThinkingBudget",
        "echo_core::llm::thinking::ThinkingProtocol::DeepseekReasoningEffort",
        "echo_core::llm::thinking::ThinkingProtocol::EnableThinkingFlag",
        "echo_core::llm::thinking::ThinkingProtocol::GlmReasoningEffort",
        "echo_core::llm::thinking::ThinkingProtocol::ModelManaged",
        "echo_core::llm::thinking::ThinkingProtocol::None",
        "echo_core::llm::thinking::ThinkingProtocol::OllamaThink",
        "echo_core::llm::thinking::ThinkingProtocol::OpenaiReasoningEffort",
        "echo_core::llm::thinking::ThinkingProtocol::ThinkingType",
        "echo_core::llm::thinking::ThinkingProtocol::emits_field",
    ];
    const SANDBOX_RESOURCE_IDENTITIES: &[&str] = &[
        "echo_core::sandbox::ResourceLimits",
        "echo_core::sandbox::ResourceLimits::impl<Default>",
        "echo_core::sandbox::ResourceLimits::strict",
        "echo_core::sandbox::ResourceLimits::unrestricted",
    ];
    const PROVIDER_CAPABILITIES_IDENTITIES: &[&str] = &[
        "echo_core::llm::capabilities::ProviderCapabilities::anthropic",
        "echo_core::llm::capabilities::ProviderCapabilities::from_provider_name",
        "echo_core::llm::capabilities::ProviderCapabilities::ollama",
        "echo_core::llm::capabilities::ProviderCapabilities::openai_compatible",
    ];
    const THINKING_PROFILE_IDENTITIES: &[&str] = &[
        "echo_core::llm::capabilities::ThinkingProfile",
        "echo_core::llm::capabilities::ThinkingProfile::new",
        "echo_core::llm::capabilities::ThinkingProfile::supports_manual_control",
        "echo_core::llm::capabilities::ThinkingProfile::unknown",
        "echo_core::llm::capabilities::resolve_thinking_profile",
    ];
    const MODEL_PROFILE_IDENTITIES: &[&str] = &[
        "echo_core::llm::capabilities::ModelProfile",
        "echo_core::llm::capabilities::ModelProfile::capabilities",
        "echo_core::llm::capabilities::ModelProfile::context_window",
        "echo_core::llm::capabilities::ModelProfile::excluded_tools",
        "echo_core::llm::capabilities::ModelProfile::from_provider_name",
        "echo_core::llm::capabilities::ModelProfile::max_output_tokens",
        "echo_core::llm::capabilities::ModelProfile::model_name",
        "echo_core::llm::capabilities::ModelProfile::new",
        "echo_core::llm::capabilities::ModelProfile::prompt_suffix",
        "echo_core::llm::capabilities::ModelProfile::provider",
        "echo_core::llm::capabilities::ModelProfile::supports_images",
        "echo_core::llm::capabilities::ModelProfile::supports_parallel_tool_calls",
        "echo_core::llm::capabilities::ModelProfile::supports_reasoning",
        "echo_core::llm::capabilities::ModelProfile::supports_streaming",
        "echo_core::llm::capabilities::ModelProfile::supports_tool_choice_none",
        "echo_core::llm::capabilities::ModelProfile::supports_tools",
        "echo_core::llm::capabilities::ModelProfile::thinking_levels",
        "echo_core::llm::capabilities::ModelProfile::thinking_protocol",
        "echo_core::llm::capabilities::ModelProfile::tokenizer_name",
        "echo_core::llm::capabilities::ModelProfileOverride",
        "echo_core::llm::capabilities::ModelProfileOverride::context_window",
        "echo_core::llm::capabilities::ModelProfileOverride::excluded_tools",
        "echo_core::llm::capabilities::ModelProfileOverride::prompt_suffix",
        "echo_core::llm::capabilities::ModelProfileOverride::supports_parallel_tool_calls",
        "echo_core::llm::capabilities::ModelProfileOverride::supports_structured_output",
        "echo_core::llm::capabilities::ModelProfileOverride::supports_tool_choice_none",
        "echo_core::llm::capabilities::ModelProfileResolver",
        "echo_core::llm::capabilities::ModelProfileResolver::new",
        "echo_core::llm::capabilities::ModelProfileResolver::register_exact",
        "echo_core::llm::capabilities::ModelProfileResolver::register_provider_default",
        "echo_core::llm::capabilities::ModelProfileResolver::resolve",
        "echo_core::llm::capabilities::infer_context_window",
    ];
    const LLM_API_PROTOCOL_IDENTITIES: &[&str] = &[
        "echo_core::llm::LlmApiProtocol",
        "echo_core::llm::LlmApiProtocol::Anthropic",
        "echo_core::llm::LlmApiProtocol::ChatCompletions",
        "echo_core::llm::LlmApiProtocol::Responses",
        "echo_core::llm::LlmApiProtocol::endpoint_path",
        "echo_core::llm::LlmApiProtocol::from_endpoint",
        "echo_core::llm::LlmApiProtocol::try_from_endpoint",
    ];
    const MODEL_INPUT_MODALITY_IDENTITIES: &[&str] = &[
        "echo_core::llm::ModelInputModality",
        "echo_core::llm::ModelInputModality::Audio",
        "echo_core::llm::ModelInputModality::Image",
        "echo_core::llm::ModelInputModality::Text",
        "echo_core::llm::ModelInputModality::Video",
        "echo_core::llm::ModelInputModality::all_supported",
        "echo_core::llm::ModelInputModality::text_only",
    ];
    const RESPONSE_FORMAT_IDENTITIES: &[&str] = &[
        "echo_core::llm::types::ResponseFormat",
        "echo_core::llm::types::ResponseFormat::JsonObject",
        "echo_core::llm::types::ResponseFormat::JsonSchema",
        "echo_core::llm::types::ResponseFormat::Text",
        "echo_core::llm::types::ResponseFormat::is_json",
        "echo_core::llm::types::ResponseFormat::json_schema",
    ];
    const PROMPT_CACHE_LAYOUT_IDENTITIES: &[&str] = &[
        "echo_core::llm::cache::layout::PromptCacheLayout",
        "echo_core::llm::cache::layout::PromptCacheLayout::from_messages",
        "echo_core::llm::cache::layout::PromptCacheLayout::segment_ranges",
    ];
    const MEMORY_SCOPE_IDENTITIES: &[&str] = &[
        "echo_core::memory::scope::MemoryScope",
        "echo_core::memory::scope::MemoryScope::Project",
        "echo_core::memory::scope::MemoryScope::Repo",
        "echo_core::memory::scope::MemoryScope::Run",
        "echo_core::memory::scope::MemoryScope::Session",
        "echo_core::memory::scope::MemoryScope::Task",
        "echo_core::memory::scope::MemoryScope::User",
        "echo_core::memory::scope::MemoryScope::all",
        "echo_core::memory::scope::MemoryScope::impl<FromStr>",
        "echo_core::memory::scope::MemoryScope::is_persistent",
        "echo_core::memory::scope::MemoryScope::name",
        "echo_core::memory::scope::MemoryScope::priority",
    ];
    const MEMORY_TYPE_IDENTITIES: &[&str] = &[
        "echo_core::memory::types::MemoryType",
        "echo_core::memory::types::MemoryType::ArchitectureDecision",
        "echo_core::memory::types::MemoryType::CommandPattern",
        "echo_core::memory::types::MemoryType::DebuggingLesson",
        "echo_core::memory::types::MemoryType::DeprecatedNote",
        "echo_core::memory::types::MemoryType::ErrorResolution",
        "echo_core::memory::types::MemoryType::ProjectFact",
        "echo_core::memory::types::MemoryType::SkillCandidate",
        "echo_core::memory::types::MemoryType::ToolUsage",
        "echo_core::memory::types::MemoryType::UserPreference",
        "echo_core::memory::types::MemoryType::WorkflowPattern",
        "echo_core::memory::types::MemoryType::default_stability",
        "echo_core::memory::types::MemoryType::is_rule_eligible",
        "echo_core::memory::types::MemoryType::is_skill_eligible",
    ];
    const MEMORY_SOURCE_IDENTITIES: &[&str] = &[
        "echo_core::memory::types::MemorySource",
        "echo_core::memory::types::MemorySource::AutoExtracted",
        "echo_core::memory::types::MemorySource::ErrorResolution",
        "echo_core::memory::types::MemorySource::ExplicitSave",
        "echo_core::memory::types::MemorySource::L3Promotion",
        "echo_core::memory::types::MemorySource::RepeatedWorkflow",
        "echo_core::memory::types::MemorySource::UserCorrection",
        "echo_core::memory::types::MemorySource::default_confidence",
        "echo_core::memory::types::MemorySource::default_recall_weight",
    ];
    let (status, suffix) = if LOCAL_TOOL_VALUE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "local_tool_values")
    } else if A2A_TASK_STATE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "a2a_task_state")
    } else if A2A_VALUE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "a2a_values")
    } else if A2A_AGENT_CARD_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "a2a_agent_card")
    } else if A2A_WIRE_VALUE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "a2a_wire_values")
    } else if A2A_STREAM_VALUE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "a2a_stream_values")
    } else if A2A_TASK_ENVELOPE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "a2a_task_envelopes")
    } else if THINKING_LEVEL_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "thinking_level")
    } else if STEERING_VALUE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "steering_values")
    } else if SUBAGENT_VALUE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "subagent_values")
    } else if CONTENT_GUARD_VALUE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "content_guard_values")
    } else if GUARD_VALUE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "guard_values")
    } else if DELIVERY_VALUE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "delivery_values")
    } else if SUBAGENT_STOP_VALUE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "subagent_stop_values")
    } else if TASK_TERMINAL_VALUE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "task_terminal_values")
    } else if PERMISSION_RULE_SOURCE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "permission_rule_values")
    } else if PERMISSION_RULE_BEHAVIOR_IDENTITIES.contains(&identity) {
        (
            LanguageImplementationStatus::Done,
            "permission_rule_behavior",
        )
    } else if PERMISSION_MODE_HELPER_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "permission_mode_values")
    } else if PERMISSION_RULE_MATCHER_IDENTITIES.contains(&identity) {
        (
            LanguageImplementationStatus::Done,
            "permission_rule_matcher",
        )
    } else if COMMAND_CELL_PHASE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "command_cell_values")
    } else if COMMAND_CELL_STATUS_IDENTITIES.contains(&identity) {
        (
            LanguageImplementationStatus::Done,
            "command_cell_status_values",
        )
    } else if TEAM_STRATEGY_VALUE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "team_strategy_values")
    } else if ACP_VALUE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "acp_values")
    } else if ACP_CONFIG_VALUE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "acp_config_values")
    } else if ACP_LEASE_VALUE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "acp_lease_values")
    } else if JWT_VALUE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "jwt_values")
    } else if DEPENDENCY_VALUE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "dependency_values")
    } else if CONTEXT_INHERITANCE_IDENTITIES.contains(&identity) {
        (
            LanguageImplementationStatus::Done,
            "context_inheritance_values",
        )
    } else if OBSERVED_ISOLATION_IDENTITIES.contains(&identity) {
        (
            LanguageImplementationStatus::Done,
            "observed_isolation_values",
        )
    } else if SEGMENT_RANGE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "segment_range_values")
    } else if PROMPT_DIAGNOSTICS_IDENTITIES.contains(&identity) {
        (
            LanguageImplementationStatus::Done,
            "prompt_diagnostics_values",
        )
    } else if SUBAGENT_COMMAND_IDENTITY_IDENTITIES.contains(&identity) {
        (
            LanguageImplementationStatus::Done,
            "subagent_command_identity_values",
        )
    } else if SUBAGENT_USAGE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "subagent_usage_values")
    } else if TOOL_OUTPUT_ARTIFACT_CONFIG_IDENTITIES.contains(&identity) {
        (
            LanguageImplementationStatus::Done,
            "tool_output_artifact_config_values",
        )
    } else if SKILL_VALIDATION_IDENTITIES.contains(&identity) {
        (
            LanguageImplementationStatus::Done,
            "skill_validation_values",
        )
    } else if SKILL_CONTENT_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "skill_content_values")
    } else if JSONRPC_VALUE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "jsonrpc_values")
    } else if SUBAGENT_CONTEXT_IDENTITIES.contains(&identity) {
        (
            LanguageImplementationStatus::Done,
            "subagent_context_values",
        )
    } else if USAGE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "usage_values")
    } else if HOOK_ACTION_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "hook_action_values")
    } else if PAGE_INFO_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "page_info_values")
    } else if HOOK_EVENT_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "hook_event_values")
    } else if EVENT_IDENTITY_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "event_identity_values")
    } else if INTERVENTION_RESULT_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "intervention_values")
    } else if TOKEN_BUDGET_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "token_budget_values")
    } else if EXECUTION_USAGE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "execution_usage_values")
    } else if TURN_MODE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "turn_mode_values")
    } else if RETRY_POLICY_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "retry_policy_values")
    } else if THINKING_CONFIG_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "thinking_config_values")
    } else if TIME_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "time_values")
    } else if THINKING_PROTOCOL_IDENTITIES.contains(&identity) {
        (
            LanguageImplementationStatus::Done,
            "thinking_protocol_values",
        )
    } else if SANDBOX_RESOURCE_IDENTITIES.contains(&identity) {
        (
            LanguageImplementationStatus::Done,
            "sandbox_resource_values",
        )
    } else if PROVIDER_CAPABILITIES_IDENTITIES.contains(&identity) {
        (
            LanguageImplementationStatus::Done,
            "provider_capabilities_values",
        )
    } else if THINKING_PROFILE_IDENTITIES.contains(&identity) {
        (
            LanguageImplementationStatus::Done,
            "thinking_profile_values",
        )
    } else if MODEL_PROFILE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "model_profile_values")
    } else if LLM_API_PROTOCOL_IDENTITIES.contains(&identity) {
        (
            LanguageImplementationStatus::Done,
            "llm_api_protocol_values",
        )
    } else if MODEL_INPUT_MODALITY_IDENTITIES.contains(&identity) {
        (
            LanguageImplementationStatus::Done,
            "model_input_modality_values",
        )
    } else if RESPONSE_FORMAT_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "response_format_values")
    } else if PROMPT_CACHE_LAYOUT_IDENTITIES.contains(&identity) {
        (
            LanguageImplementationStatus::Done,
            "prompt_cache_layout_values",
        )
    } else if MEMORY_SCOPE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "memory_scope_values")
    } else if MEMORY_TYPE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "memory_policy_values")
    } else if MEMORY_SOURCE_IDENTITIES.contains(&identity) {
        (LanguageImplementationStatus::Done, "memory_policy_values")
    } else {
        match identity {
            "echo_orchestration::runtime::turn_driver::TurnOutcome::classify" => {
                (LanguageImplementationStatus::Done, "turn_outcome")
            }
            "echo_orchestration::runtime::turn_driver::TurnOutcome::status"
            | "echo_orchestration::runtime::turn_driver::TurnReceipt::status"
            | "echo_orchestration::runtime::turn_driver::TurnReceipt::usage" => {
                (LanguageImplementationStatus::Done, "run_receipt")
            }
            "echo_core::utils::json_parse::clean_json"
            | "echo_core::utils::json_parse::extract_json_from_markdown"
            | "echo_core::utils::utf8::IncrementalUtf8Decoder"
            | "echo_core::utils::utf8::IncrementalUtf8Decoder::new"
            | "echo_core::utils::utf8::IncrementalUtf8Decoder::push"
            | "echo_core::utils::utf8::IncrementalUtf8Decoder::finish"
            | "echo_core::utils::utf8::split_utf8_chunks" => {
                (LanguageImplementationStatus::Done, "local_helpers")
            }
            _ => {
                let suffix = match classification {
                    SemanticClass::WireValue => "wire_value",
                    SemanticClass::Operation => "facade_operation",
                    SemanticClass::Handle => "facade_handle",
                    SemanticClass::Stream => "facade_stream",
                    SemanticClass::Extension => "facade_extension",
                    SemanticClass::LanguageIntrinsic => "language_intrinsic",
                };
                let status = if route.surface == "intrinsic" {
                    LanguageImplementationStatus::NotImplemented
                } else {
                    LanguageImplementationStatus::Done
                };
                (status, suffix)
            }
        }
    };
    debug_assert!(!route.route.is_empty());
    (status, suffix)
}

pub fn manifest_entries(merged: &[InventoryEntry]) -> Vec<ManifestEntry> {
    let parent_of_impl = |path: &str| {
        path.rsplit_once("::impl<")
            .map(|(parent, _)| parent.to_string())
    };
    let serialized: BTreeSet<String> = merged
        .iter()
        .filter(|entry| {
            entry.kind == ItemKind::TraitImpl && entry.path.contains("::impl<Serialize>")
        })
        .filter_map(|entry| parent_of_impl(&entry.path))
        .collect();
    let deserialized: BTreeSet<String> = merged
        .iter()
        .filter(|entry| {
            entry.kind == ItemKind::TraitImpl && entry.path.contains("::impl<Deserialize>")
        })
        .filter_map(|entry| parent_of_impl(&entry.path))
        .collect();
    let serializable_types: BTreeSet<String> =
        serialized.intersection(&deserialized).cloned().collect();
    let mut derived_traits_by_parent: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for implementation in merged
        .iter()
        .filter(|entry| entry.kind == ItemKind::TraitImpl && entry.automatically_derived)
    {
        if let Some((parent, suffix)) = implementation.path.rsplit_once("::impl<")
            && let Some(trait_name) = suffix.split('>').next()
        {
            derived_traits_by_parent
                .entry(parent.to_string())
                .or_default()
                .insert(trait_name.to_string());
        }
    }
    let kind_by_path: BTreeMap<&str, ItemKind> = merged
        .iter()
        .map(|entry| (entry.path.as_str(), entry.kind))
        .collect();
    let mut public_value_types = BTreeSet::new();
    let mut process_local_types = BTreeSet::new();
    for field in merged
        .iter()
        .filter(|entry| entry.kind == ItemKind::StructField)
    {
        let process_local = field
            .signatures
            .values()
            .any(|signature| shape_is_process_local(&signature.shape));
        let mut current = field.path.as_str();
        while let Some((parent, _)) = current.rsplit_once("::") {
            if matches!(
                kind_by_path.get(parent),
                Some(ItemKind::Struct | ItemKind::Enum | ItemKind::Union)
            ) {
                public_value_types.insert(parent.to_string());
                if process_local {
                    process_local_types.insert(parent.to_string());
                }
                break;
            }
            current = parent;
        }
    }
    // Alias groups: facade paths sharing one canonical source identity (and
    // one signature shape) are re-exports of the same item. Each group has
    // exactly one canonical member — never from a re-export-only namespace
    // when an alternative exists — and every other member records it.
    let mut alias_groups: BTreeMap<(String, Vec<String>), Vec<String>> = BTreeMap::new();
    for entry in merged
        .iter()
        .filter(|entry| !(entry.kind == ItemKind::TraitImpl && entry.automatically_derived))
    {
        let identity = crate::facade::canonical_source_identity(entry);
        let signatures: Vec<String> = entry.signatures.keys().cloned().collect();
        alias_groups
            .entry((identity, signatures))
            .or_default()
            .push(entry.path.clone());
    }
    let mut canonical_member_of: BTreeMap<String, Option<String>> = BTreeMap::new();
    for paths in alias_groups.values() {
        let mut sorted = paths.clone();
        sorted.sort();
        let canonical = sorted
            .iter()
            .find(|path| !is_reexport_namespace(path))
            .or_else(|| sorted.first())
            .cloned();
        for path in &sorted {
            let alias_of = match &canonical {
                Some(canonical) if canonical != path => Some(canonical.clone()),
                _ => None,
            };
            canonical_member_of.insert(path.clone(), alias_of);
        }
    }
    merged
        .iter()
        .filter(|entry| !(entry.kind == ItemKind::TraitImpl && entry.automatically_derived))
        .map(|entry| {
            let (classification, acp_relationship, semantic_rule) = classify_entry(
                entry,
                &serializable_types,
                &public_value_types,
                &process_local_types,
            );
            let features = features_of_entry(entry);
            let full_only = !entry.profiles.contains("default")
                && entry
                    .profiles
                    .iter()
                    .all(|profile| !profile.starts_with("feature:"))
                && entry.profiles.contains("full");
            let feature_semantics = if entry.profiles.contains("default") {
                FeatureSemantics::Default
            } else if full_only {
                FeatureSemantics::AllOf
            } else {
                FeatureSemantics::AnyOf
            };
            let alias_of = canonical_member_of
                .get(&entry.path)
                .and_then(|alias| alias.clone());
            let route =
                route_obligation_for(entry, classification, acp_relationship, semantic_rule);
            let (language_status, language_contract_suffix) = language_status_for(
                &crate::facade::canonical_source_identity(entry),
                classification,
                &route,
            );
            ManifestEntry {
                path: entry.path.clone(),
                kind: entry.kind,
                source_paths: entry.source_paths.clone(),
                features,
                full_only,
                feature_semantics,
                signatures: entry
                    .signatures
                    .values()
                    .map(|signature| ManifestSignature {
                        digest: signature.digest.clone(),
                    })
                    .collect(),
                classification,
                acp_relationship,
                semantic_rule: semantic_rule.to_string(),
                derived_traits: derived_traits_by_parent
                    .get(&entry.path)
                    .cloned()
                    .unwrap_or_default(),
                route,
                canonical: alias_of.is_none(),
                alias_of,
                languages: LANGUAGES
                    .iter()
                    .map(|language| {
                        (
                            (*language).to_string(),
                            LanguageStatusRecord {
                                status: language_status,
                                target: language_target(classification).to_string(),
                                contract_test: format!(
                                    "sdk-parity/{language}/{language_contract_suffix}"
                                ),
                            },
                        )
                    })
                    .collect(),
            }
        })
        .collect()
}

pub fn manifest_document(
    extension_protocol_version: u32,
    profiles: &[FeatureProfile],
    merged: &[InventoryEntry],
) -> ParityManifest {
    let profile_names: Vec<String> = profiles
        .iter()
        .map(|profile| profile.name.clone())
        .collect();
    let inventory_value = serde_json::to_vec(merged).unwrap_or_default();
    ParityManifest {
        schema_version: 1,
        extension_protocol_version,
        generated: ManifestGenerated {
            rustdoc_format_version: RUSTDOC_FORMAT_VERSION,
            profiles: profile_names,
            inventory_digest: digest(&inventory_value),
        },
        entries: manifest_entries(merged),
    }
}

pub fn render_parity_manifest(
    extension_protocol_version: u32,
    profiles: &[FeatureProfile],
    merged: &[InventoryEntry],
) -> String {
    let document = manifest_document(extension_protocol_version, profiles, merged);
    let generated = serde_json::to_string_pretty(&document.generated).unwrap_or_default();
    let mut output = format!(
        "{{\n  \"schema_version\": {},\n  \"extension_protocol_version\": {},\n  \"generated\": {},\n  \"entries\": [\n",
        document.schema_version,
        document.extension_protocol_version,
        indent_json(&generated, 2)
    );
    let entry_count = document.entries.len();
    for (index, entry) in document.entries.iter().enumerate() {
        let rendered = serde_json::to_string(entry).unwrap_or_default();
        output.push_str("    ");
        output.push_str(&rendered);
        if index.saturating_add(1) < entry_count {
            output.push(',');
        }
        output.push('\n');
    }
    output.push_str("  ]\n}\n");
    output
}

fn indent_json(value: &str, spaces: usize) -> String {
    let indentation = " ".repeat(spaces);
    value
        .lines()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                line.to_string()
            } else {
                format!("{indentation}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn render_parity_manifest_schema() -> String {
    let schema = schemars::schema_for!(ParityManifest);
    serde_json::to_string_pretty(&schema).unwrap_or_default() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"{
      "format_version":61,"root":1,
      "index":{
        "1":{"name":"echo_agent","crate_id":0,"visibility":"public","attrs":[],"inner":{"module":{"items":[2,3,8]}}},
        "2":{"name":"audit","crate_id":0,"visibility":"public","attrs":[],"inner":{"module":{"items":[4]}}},
        "3":{"name":"hidden","crate_id":0,"visibility":"public","attrs":["#[doc(hidden)]"],"inner":{"module":{"items":[7]}}},
        "4":{"name":"ChangeLog","crate_id":0,"visibility":"public","attrs":[],"inner":{"trait":{"items":[5]}}},
        "5":{"name":"record","crate_id":0,"visibility":"default","attrs":[],"inner":{"function":{"sig":{"inputs":[],"output":null},"generics":{"params":[]}}}},
        "7":{"name":"secret","crate_id":0,"visibility":"public","attrs":[],"inner":{"function":{"sig":{}}}},
        "8":{"name":"State","crate_id":0,"visibility":"public","attrs":[],"inner":{"enum":{"variants":[9],"impls":[]}}},
        "9":{"name":"Ready","crate_id":0,"visibility":"default","attrs":[],"inner":{"variant":{"kind":"plain","discriminant":null}}}
      },"paths":{}
    }"##;

    #[test]
    fn extracts_trait_members_variants_and_skips_doc_hidden() -> Result<(), String> {
        let items = extract_public_items(SAMPLE).map_err(|error| error.to_string())?;
        let paths: Vec<&str> = items.iter().map(|item| item.path.as_str()).collect();
        assert!(paths.contains(&"echo_agent::audit::ChangeLog::record"));
        assert!(paths.contains(&"echo_agent::State::Ready"));
        assert!(!paths.iter().any(|path| path.contains("secret")));
        Ok(())
    }

    #[test]
    fn default_items_have_no_feature_condition() -> Result<(), String> {
        let item = PublicItem {
            path: "echo_agent::Agent".to_string(),
            kind: ItemKind::Trait,
            api_shape: "{}".to_string(),
            api_shape_digest: "sha256:a".to_string(),
            source_path: None,
            required_features: BTreeSet::new(),
            automatically_derived: false,
        };
        let profiles = BTreeMap::from([
            ("default".to_string(), vec![item.clone()]),
            ("full".to_string(), vec![item]),
        ]);
        let merged = merge_profiles(&profiles).map_err(|error| error.to_string())?;
        let first = merged
            .first()
            .ok_or_else(|| "expected one merged item".to_string())?;
        assert!(features_of_entry(first).is_empty());
        Ok(())
    }

    #[test]
    fn format_version_mismatch_fails_closed() -> Result<(), String> {
        let error = match extract_public_items(&SAMPLE.replace("61", "99")) {
            Ok(_) => return Err("format mismatch must fail".to_string()),
            Err(error) => error,
        };
        assert!(error.to_string().contains("99"));
        Ok(())
    }

    #[test]
    fn api_shape_ignores_rustdoc_ids_spans_and_function_bodies() -> Result<(), String> {
        let make = |implementation: u64, line: u64, has_body: bool| {
            serde_json::from_value::<RustdocItem>(serde_json::json!({
                "name": "Contract",
                "crate_id": 0,
                "visibility": "public",
                "attrs": [{"other": format!(
                    "#[attr = CfgTrace([NameValue {{ name: \"feature\", value: Some(\"eval\"), span: src/lib.rs:{line}:1 }}])]"
                )}],
                "inner": {"trait": {
                    "items": [1],
                    "implementations": [implementation],
                    "generics": {"params": [], "where_predicates": []},
                    "has_body": has_body
                }}
            }))
            .map_err(|error| error.to_string())
        };
        let first = make(7, 10, false)?;
        let second = make(99, 300, false)?;
        assert_eq!(item_shape(&first), item_shape(&second));
        let provided = make(99, 300, true)?;
        assert_ne!(item_shape(&first), item_shape(&provided));
        assert_eq!(
            combined_features(&BTreeSet::new(), &first),
            BTreeSet::from(["eval".to_string()])
        );
        Ok(())
    }
}
