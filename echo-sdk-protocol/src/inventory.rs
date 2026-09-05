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
pub struct AdapterObligation {
    pub operation: String,
    pub mapping: String,
    pub validation: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManifestSignature {
    pub digest: String,
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
    pub adapter: AdapterObligation,
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
    let relationship = if class == SemanticClass::LanguageIntrinsic {
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

fn adapter_for(
    entry: &InventoryEntry,
    class: SemanticClass,
    relationship: AcpRelationship,
    semantic_rule: &str,
) -> AdapterObligation {
    let operation = if relationship == AcpRelationship::LanguageIntrinsic {
        "language:facade".to_string()
    } else if relationship == AcpRelationship::StandardProjection {
        "acp:v1+_echo_agent/facade/invoke".to_string()
    } else if class == SemanticClass::Extension {
        "_echo_agent/extension/register+invoke".to_string()
    } else if entry.path.contains("::tasks::") {
        "_echo_agent/task/*".to_string()
    } else if entry.path.contains("::subagent::") {
        "_echo_agent/subagent/*".to_string()
    } else if entry.path.contains("::memory::") || entry.path.contains("::compression::") {
        "_echo_agent/memory/op".to_string()
    } else if entry.path.contains("::workflow::") {
        "_echo_agent/workflow/op".to_string()
    } else if class == SemanticClass::Handle {
        "_echo_agent/agent/*".to_string()
    } else {
        "_echo_agent/facade/invoke".to_string()
    };
    AdapterObligation {
        operation,
        mapping: format!(
            "{} via {}; Rust remains authoritative",
            relationship.as_str(),
            semantic_rule
        ),
        validation: vec![
            "echo-sdk-protocol/tests/facade_inventory.rs#known_facade_semantics_are_classified_correctly".to_string(),
            "echo-sdk-protocol/tests/extension_contract.rs".to_string(),
        ],
    }
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
                adapter: adapter_for(entry, classification, acp_relationship, semantic_rule),
                languages: LANGUAGES
                    .iter()
                    .map(|language| {
                        (
                            (*language).to_string(),
                            LanguageStatusRecord {
                                status: LanguageImplementationStatus::NotImplemented,
                                target: language_target(classification).to_string(),
                                contract_test: format!(
                                    "sdk-parity/{language}/{}",
                                    classification.as_str()
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
