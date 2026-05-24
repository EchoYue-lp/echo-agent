//! `#[derive(Tool)]` — Generate `Tool` trait impl from a struct definition.
//!
//! ## Usage
//!
//! ```rust,ignore
//! #[derive(Tool)]
//! #[tool(name = "read_file", description = "Read a file", risk_level = "ReadOnly")]
//! struct ReadFileTool {
//!     #[tool_param(skip)]
//!     base_dir: PathBuf,
//!
//!     #[tool_param(description = "File path")]
//!     path: String,
//!
//!     #[tool_param(description = "Start line")]
//!     start_line: Option<usize>,
//! }
//!
//! impl ToolRunner<ReadFileToolParams> for ReadFileTool {
//!     async fn run(&self, params: ReadFileToolParams) -> Result<ToolResult> {
//!         // ... business logic ...
//!     }
//! }
//! ```
//!
//! The macro generates:
//! - `ReadFileToolParams` struct with `Deserialize + JsonSchema`
//! - Full `impl Tool for ReadFileTool` (name, description, parameters, execute, etc.)

use proc_macro_crate;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Attribute, Data, DeriveInput, Fields, Ident, LitStr, Token};

/// Resolve the echo-agent (or echo-core) crate path for code generation.
/// Tries `echo_core` first (for crates that only depend on echo-core),
/// then falls back to `echo_agent` (the facade crate).
fn resolve_echo_crate_path() -> syn::Result<syn::Path> {
    match proc_macro_crate::crate_name("echo_core") {
        Ok(proc_macro_crate::FoundCrate::Itself) => Ok(syn::parse_quote!(::echo_core)),
        Ok(proc_macro_crate::FoundCrate::Name(name)) => {
            let ident = syn::Ident::new(&name, proc_macro2::Span::call_site());
            Ok(syn::parse_quote!(::#ident))
        }
        Err(_) => {
            // Fallback to echo_agent (facade)
            match proc_macro_crate::crate_name("echo_agent") {
                Ok(proc_macro_crate::FoundCrate::Itself) => Ok(syn::parse_quote!(::echo_agent)),
                Ok(proc_macro_crate::FoundCrate::Name(name)) => {
                    let ident = syn::Ident::new(&name, proc_macro2::Span::call_site());
                    Ok(syn::parse_quote!(::#ident))
                }
                Err(e) => Err(syn::Error::new(
                    proc_macro2::Span::call_site(),
                    format!(
                        "Cannot find `echo_core` or `echo_agent` in dependencies: {}",
                        e
                    ),
                )),
            }
        }
    }
}

// ── Parsed attributes ──────────────────────────────────────────────────────────

/// Parsed `#[tool(...)]` struct-level attributes
struct ToolStructAttrs {
    name: String,
    description: String,
    risk_level: Option<String>,
    permissions: Vec<Ident>,
}

/// Parsed `#[tool_param(...)]` field-level attributes
struct ToolParamAttrs {
    skip: bool,
    description: Option<String>,
}

// ── Parsing helpers ────────────────────────────────────────────────────────────

impl syn::parse::Parse for ToolStructAttrs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut name: Option<String> = None;
        let mut description: Option<String> = None;
        let mut risk_level: Option<String> = None;
        let mut permissions: Vec<Ident> = Vec::new();

        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            if ident == "name" {
                let _eq: Token![=] = input.parse()?;
                let value: LitStr = input.parse()?;
                name = Some(value.value());
            } else if ident == "description" {
                let _eq: Token![=] = input.parse()?;
                let value: LitStr = input.parse()?;
                description = Some(value.value());
            } else if ident == "risk_level" {
                let _eq: Token![=] = input.parse()?;
                let value: LitStr = input.parse()?;
                risk_level = Some(value.value());
            } else if ident == "permissions" {
                let _eq: Token![=] = input.parse()?;
                let content;
                syn::bracketed!(content in input);
                while !content.is_empty() {
                    let perm: Ident = content.parse()?;
                    permissions.push(perm);
                    if !content.is_empty() {
                        let _comma: Token![,] = content.parse()?;
                    }
                }
            } else {
                return Err(syn::Error::new_spanned(
                    ident,
                    "unknown attribute, expected `name`, `description`, `risk_level`, or `permissions`",
                ));
            }
            if !input.is_empty() {
                let _comma: Token![,] = input.parse()?;
            }
        }

        Ok(ToolStructAttrs {
            name: name.ok_or_else(|| {
                syn::Error::new(
                    proc_macro2::Span::call_site(),
                    "#[tool] requires `name = \"...\"`",
                )
            })?,
            description: description.ok_or_else(|| {
                syn::Error::new(
                    proc_macro2::Span::call_site(),
                    "#[tool] requires `description = \"...\"`",
                )
            })?,
            risk_level,
            permissions,
        })
    }
}

/// Parse `#[tool_param(skip)]` or `#[tool_param(description = "...")]` from a field.
fn parse_tool_param_attrs(attrs: &[Attribute]) -> ToolParamAttrs {
    let mut skip = false;
    let mut description: Option<String> = None;

    for attr in attrs {
        if !attr.path().is_ident("tool_param") {
            continue;
        }
        if let Ok(parsed) = attr.parse_args::<ToolParamRaw>() {
            skip = skip || parsed.skip;
            if parsed.description.is_some() {
                description = parsed.description;
            }
        }
    }

    // Also check for doc comments on the field
    if description.is_none() {
        description = super::extract_doc_comments(attrs);
    }

    ToolParamAttrs { skip, description }
}

/// Raw parse of tool_param args (supports both `skip` and `description = "..."`)
struct ToolParamRaw {
    skip: bool,
    description: Option<String>,
}

impl syn::parse::Parse for ToolParamRaw {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut skip = false;
        let mut description: Option<String> = None;

        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            if ident == "skip" {
                skip = true;
            } else if ident == "description" {
                let _eq: Token![=] = input.parse()?;
                let value: LitStr = input.parse()?;
                description = Some(value.value());
            } else {
                return Err(syn::Error::new_spanned(
                    ident,
                    "expected `skip` or `description`",
                ));
            }
            if !input.is_empty() {
                let _comma: Token![,] = input.parse()?;
            }
        }
        Ok(ToolParamRaw { skip, description })
    }
}

/// Extract the first `#[tool(...)]` attribute from the struct, parse it, and return
/// the parsed ToolStructAttrs.
fn extract_tool_attrs(attrs: &[Attribute]) -> syn::Result<ToolStructAttrs> {
    for attr in attrs {
        if attr.path().is_ident("tool") {
            return attr.parse_args::<ToolStructAttrs>();
        }
    }
    Err(syn::Error::new(
        proc_macro2::Span::call_site(),
        "#[derive(Tool)] requires a #[tool(name = \"...\", description = \"...\")] attribute on the struct",
    ))
}

// ── Struct field info ──────────────────────────────────────────────────────────

struct ParamField {
    ident: Ident,
    ty: syn::Type,
    schemars_attr: TokenStream,
}

// ── Main entry point ───────────────────────────────────────────────────────────

pub fn derive_tool_impl(input: DeriveInput) -> syn::Result<TokenStream> {
    let echo_crate = resolve_echo_crate_path()?;
    let struct_ident = &input.ident;
    let struct_name_str = struct_ident.to_string();
    let params_ident = format_ident!("{}Params", struct_name_str);

    // Extract struct-level #[tool(...)] attributes
    let tool_attrs = extract_tool_attrs(&input.attrs)?;
    let tool_name = &tool_attrs.name;
    let tool_desc = &tool_attrs.description;

    // Parse struct fields
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            Fields::Unnamed(_) => {
                return Err(syn::Error::new_spanned(
                    &input,
                    "#[derive(Tool)] only supports named fields (struct with { })",
                ));
            }
            Fields::Unit => {
                // Unit struct: generate params as an empty struct
                return generate_unit_tool(
                    &echo_crate,
                    struct_ident,
                    params_ident,
                    tool_name,
                    tool_desc,
                    &tool_attrs,
                );
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                &input,
                "#[derive(Tool)] only supports structs",
            ));
        }
    };

    // Separate parameter fields from state fields
    let mut param_fields: Vec<ParamField> = Vec::new();
    let mut field_names: Vec<Ident> = Vec::new();

    for field in fields {
        let field_ident = field.ident.as_ref().ok_or_else(|| {
            syn::Error::new_spanned(field, "#[derive(Tool)] requires named fields")
        })?;

        let param_attrs = parse_tool_param_attrs(&field.attrs);
        if param_attrs.skip {
            continue; // Internal state, not exposed to LLM
        }

        let schemars_attr = if let Some(desc) = &param_attrs.description {
            quote! { #[schemars(description = #desc)] }
        } else {
            quote! {}
        };

        param_fields.push(ParamField {
            ident: field_ident.clone(),
            ty: field.ty.clone(),
            schemars_attr,
        });
        field_names.push(field_ident.clone());
    }

    // Generate the params struct
    let param_field_defs: Vec<TokenStream> = param_fields
        .iter()
        .map(|f| {
            let name = &f.ident;
            let ty = &f.ty;
            let schemars_attr = &f.schemars_attr;
            quote! {
                #schemars_attr
                pub #name: #ty
            }
        })
        .collect();

    // ── Risk level override ────────────────────────────────────────────────
    let risk_level_override = if let Some(level) = &tool_attrs.risk_level {
        let level_path = match level.as_str() {
            "ReadOnly" => quote! { #echo_crate::tools::ToolRiskLevel::ReadOnly },
            "Standard" => quote! { #echo_crate::tools::ToolRiskLevel::Standard },
            "Dangerous" => quote! { #echo_crate::tools::ToolRiskLevel::Dangerous },
            other => {
                return Err(syn::Error::new_spanned(
                    &input,
                    format!(
                        "risk_level must be `ReadOnly`, `Standard`, or `Dangerous`, got `{}`",
                        other
                    ),
                ));
            }
        };
        quote! {
            fn risk_level(&self) -> #echo_crate::tools::ToolRiskLevel {
                #level_path
            }
        }
    } else {
        quote! {}
    };

    // ── Permissions override ──────────────────────────────────────────────
    let permissions_override = if tool_attrs.permissions.is_empty() {
        quote! {}
    } else {
        let perms = tool_attrs.permissions.iter().map(|p| {
            quote! { #echo_crate::tools::permission::ToolPermission::#p }
        });
        quote! {
            fn permissions(&self) -> Vec<#echo_crate::tools::permission::ToolPermission> {
                vec![#(#perms),*]
            }
        }
    };

    // ── Build the generate block ──────────────────────────────────────────
    let generated = quote! {
        /// Auto-generated parameter struct for [`#struct_ident`].
        #[derive(::serde::Deserialize, ::schemars::JsonSchema)]
        #[allow(non_camel_case_types)]
        pub struct #params_ident {
            #(#param_field_defs),*
        }

        #[allow(dead_code)]
        impl #echo_crate::tools::Tool for #struct_ident {
            fn name(&self) -> &str { #tool_name }
            fn description(&self) -> &str { #tool_desc }

            fn parameters(&self) -> ::serde_json::Value {
                let schema = ::schemars::schema_for!(#params_ident);
                ::serde_json::to_value(schema).unwrap_or_default()
            }

            #risk_level_override

            #permissions_override

            fn validate_parameters<'a>(
                &'a self,
                params: &'a #echo_crate::tools::ToolParameters,
            ) -> ::futures::future::BoxFuture<'a, #echo_crate::error::Result<()>> {
                Box::pin(async move {
                    Self::deserialize_params(params)?;
                    Ok(())
                })
            }

            fn execute<'a>(
                &'a self,
                parameters: #echo_crate::tools::ToolParameters,
            ) -> ::futures::future::BoxFuture<'a, #echo_crate::error::Result<#echo_crate::tools::ToolResult>> {
                Box::pin(async move {
                    let params = #struct_ident::deserialize_params(& parameters)?;
                    <Self as #echo_crate::tools::ToolRunner<#params_ident>>::run(self, params).await
                })
            }
        }

        impl #struct_ident {
            #[doc(hidden)]
            fn deserialize_params(params: &#echo_crate::tools::ToolParameters) -> #echo_crate::error::Result<#params_ident> {
                let value = ::serde_json::Value::Object(params.iter().map(|(k, v)| (k.clone(), v.clone())).collect());
                ::serde_json::from_value(value).map_err(|e| {
                    let msg = e.to_string();
                    let field = msg
                        .strip_prefix("missing field `")
                        .and_then(|s| s.strip_suffix('`'))
                        .or_else(|| {
                            msg.split(" at `").nth(1).and_then(|s| s.strip_suffix('`'))
                        })
                        .unwrap_or("(deserialization)");
                    #echo_crate::error::ToolError::InvalidParameter {
                        name: field.to_string(),
                        message: msg,
                    }.into()
                })
            }
        }
    };

    Ok(generated)
}

// ── Unit struct special case ───────────────────────────────────────────────────

fn generate_unit_tool(
    echo_crate: &syn::Path,
    struct_ident: &Ident,
    params_ident: Ident,
    tool_name: &str,
    tool_desc: &str,
    tool_attrs: &ToolStructAttrs,
) -> syn::Result<TokenStream> {
    let risk_level_override = if let Some(level) = &tool_attrs.risk_level {
        let level_path = match level.as_str() {
            "ReadOnly" => quote! { #echo_crate::tools::ToolRiskLevel::ReadOnly },
            "Standard" => quote! { #echo_crate::tools::ToolRiskLevel::Standard },
            "Dangerous" => quote! { #echo_crate::tools::ToolRiskLevel::Dangerous },
            _ => {
                return Err(syn::Error::new(
                    proc_macro2::Span::call_site(),
                    format!("unknown risk_level: {}", level),
                ));
            }
        };
        quote! {
            fn risk_level(&self) -> #echo_crate::tools::ToolRiskLevel {
                #level_path
            }
        }
    } else {
        quote! {}
    };

    let permissions_override = if tool_attrs.permissions.is_empty() {
        quote! {}
    } else {
        let perms = tool_attrs.permissions.iter().map(|p| {
            quote! { #echo_crate::tools::permission::ToolPermission::#p }
        });
        quote! {
            fn permissions(&self) -> Vec<#echo_crate::tools::permission::ToolPermission> {
                vec![#(#perms),*]
            }
        }
    };

    Ok(quote! {
        /// Auto-generated empty parameter struct for [`#struct_ident`].
        #[derive(::serde::Deserialize, ::schemars::JsonSchema)]
        #[allow(non_camel_case_types)]
        pub struct #params_ident {}

        #[allow(dead_code)]
        #[allow(dead_code)]
        impl #echo_crate::tools::Tool for #struct_ident {
            fn name(&self) -> &str { #tool_name }
            fn description(&self) -> &str { #tool_desc }

            fn parameters(&self) -> ::serde_json::Value {
                ::serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                })
            }

            #risk_level_override

            #permissions_override

            fn execute<'a>(
                &'a self,
                _parameters: #echo_crate::tools::ToolParameters,
            ) -> ::futures::future::BoxFuture<'a, #echo_crate::error::Result<#echo_crate::tools::ToolResult>> {
                Box::pin(async move {
                    <Self as #echo_crate::tools::ToolRunner<#params_ident>>::run(self, #params_ident {}).await
                })
            }
        }
    })
}
