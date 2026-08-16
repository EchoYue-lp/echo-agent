//! # echo-macros
//!
//! Procedural macros for the [echo-agent](https://crates.io/crates/echo_agent) framework.
//!
//! ## Macros
//!
//! | Macro | Generates | Description |
//! |-------|-----------|-------------|
//! | [`#[tool]`](attr.tool.html) | `Tool` impl | Auto-generates params struct, JSON Schema, and `Tool` trait impl from an async fn |
//! | [`#[callback]`](attr.callback.html) | `AgentCallback` impl | Generates lifecycle callbacks from an impl block |
//! | [`#[guard]`](attr.guard.html) | `Guard` impl | Content filtering guard from an async fn |
//! | [`#[handler]`](attr.handler.html) | `HumanLoopHandler` impl | Human-in-the-loop handler from an impl block |
//! | [`#[compressor]`](attr.compressor.html) | `ContextCompressor` impl | Context compression strategy from an async fn |
//! | [`#[permission_policy]`](attr.permission_policy.html) | `PermissionPolicy` impl | Tool permission policy from an async fn |
//! | [`#[audit_logger]`](attr.audit_logger.html) | `AuditLogger` impl | Audit logging backend from an impl block |
//! | [`#[derive(Tool)]`](derive.Tool.html) | `Tool` impl | Derive macro for structs — auto-generates params, JSON Schema, and `Tool` trait impl |
//!
//! ## Quick Example
//!
//! ```rust,ignore
//! use echo_agent::tool;
//!
//! #[tool(name = "add", description = "Add two numbers")]
//! async fn add(a: f64, b: f64) -> Result<ToolResult> {
//!     Ok(ToolResult::success(format!("{}", a + b)))
//! }
//! ```
//!
//! Most users should import these macros via `echo_agent::prelude::*` or
//! `use echo_agent::{tool, callback, guard, handler};` rather than depending
//! on `echo_macros` directly.

mod derive_tool;

use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::{
    DeriveInput, FnArg, ImplItem, ItemFn, ItemImpl, LitStr, Pat, ReturnType, parse_macro_input,
};

#[derive(Clone, Copy)]
enum MacroCrate {
    Core,
    Orchestration,
}

fn resolve_echo_crate_path(target: MacroCrate) -> syn::Result<syn::Path> {
    const CORE_CANDIDATES: &[&str] = &["echo_core", "echo_agent"];
    const ORCHESTRATION_CANDIDATES: &[&str] = &["echo_orchestration", "echo_agent"];
    let candidates = match target {
        MacroCrate::Core => CORE_CANDIDATES,
        MacroCrate::Orchestration => ORCHESTRATION_CANDIDATES,
    };
    for candidate in candidates {
        match crate_name(candidate) {
            Ok(FoundCrate::Itself) => return Ok(syn::parse_quote!(crate)),
            Ok(FoundCrate::Name(name)) => {
                let ident = syn::Ident::new(&name, Span::call_site());
                return Ok(syn::parse_quote!(::#ident));
            }
            Err(_) => {}
        }
    }
    let names = candidates.join(" or ");
    Err(syn::Error::new(
        Span::call_site(),
        format!("Cannot find {names} in dependencies"),
    ))
}

fn macro_support_crate_path(echo_crate: &syn::Path, crate_name: &str) -> LitStr {
    let path = quote!(#echo_crate).to_string().replace(' ', "");
    LitStr::new(
        &format!("{path}::__macro_support::{crate_name}"),
        Span::call_site(),
    )
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// #[derive(Tool)] — Generate Tool impl from struct definition
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Derive macro that generates a complete `Tool` trait implementation from a
/// struct definition.
///
/// # Struct Attributes
///
/// - `#[tool(name = "...", description = "...")]` — required
/// - `#[tool(risk_level = "ReadOnly|Standard|Dangerous")]` — optional
/// - `#[tool(permissions = [Read, Write, ...])]` — optional
///
/// # Field Attributes
///
/// - `#[tool_param(skip)]` — internal state, not exposed to LLM
/// - `#[tool_param(description = "...")]` — parameter description (also reads doc comments)
///
/// # Example
///
/// ```rust,ignore
/// use echo_agent::prelude::*;
///
/// #[derive(Tool)]
/// #[tool(name = "read_file", description = "Read file contents")]
/// struct ReadFileTool {
///     #[tool_param(skip)]
///     base_dir: PathBuf,
///     #[tool_param(description = "File path")]
///     path: String,
/// }
///
/// impl ToolRunner<ReadFileToolParams> for ReadFileTool {
///     async fn run(&self, params: ReadFileToolParams) -> Result<ToolResult> {
///         let content = std::fs::read_to_string(&params.path)?;
///         Ok(ToolResult::success(content))
///     }
/// }
/// ```
#[proc_macro_derive(Tool, attributes(tool, tool_param))]
pub fn derive_tool(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match derive_tool::derive_tool_impl(input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// #[tool] — Generate Tool impl from async fn
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

struct ToolAttrs {
    name: String,
    description: String,
    permissions: Vec<syn::Ident>,
}

impl syn::parse::Parse for ToolAttrs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut name: Option<String> = None;
        let mut description: Option<String> = None;
        let mut permissions: Vec<syn::Ident> = Vec::new();

        while !input.is_empty() {
            let ident: syn::Ident = input.parse()?;

            if ident == "name" {
                let _eq: syn::Token![=] = input.parse()?;
                let value: LitStr = input.parse()?;
                name = Some(value.value());
            } else if ident == "description" {
                let _eq: syn::Token![=] = input.parse()?;
                let value: LitStr = input.parse()?;
                description = Some(value.value());
            } else if ident == "permissions" {
                let _eq: syn::Token![=] = input.parse()?;
                let content;
                syn::bracketed!(content in input);
                while !content.is_empty() {
                    let perm: syn::Ident = content.parse()?;
                    permissions.push(perm);
                    if !content.is_empty() {
                        let _comma: syn::Token![,] = content.parse()?;
                    }
                }
            } else {
                return Err(syn::Error::new_spanned(
                    ident,
                    "unknown attribute, expected `name`, `description`, or `permissions`",
                ));
            }

            if !input.is_empty() {
                let _comma: syn::Token![,] = input.parse()?;
            }
        }

        let name = name.ok_or_else(|| {
            syn::Error::new(Span::call_site(), "#[tool] requires `name = \"...\"`")
        })?;
        let description = description.ok_or_else(|| {
            syn::Error::new(
                Span::call_site(),
                "#[tool] requires `description = \"...\"`",
            )
        })?;

        Ok(ToolAttrs {
            name,
            description,
            permissions,
        })
    }
}

/// Generate a `Tool` implementation from an async function, auto-creating the
/// parameter struct and JSON Schema.
///
/// # Attributes
///
/// - `name` (required): Tool name
/// - `description` (required): Tool description
/// - `permissions` (optional): Permission list, e.g. `[Execute, Network]`
///
/// # Example
///
/// ```rust,ignore
/// #[tool(name = "add", description = "Add two numbers")]
/// async fn add(
///     /// First number
///     a: f64,
///     /// Second number
///     b: f64,
/// ) -> Result<ToolResult> {
///     Ok(ToolResult::success(format!("{}", a + b)))
/// }
/// // Generates: AddParams struct + AddTool unit struct + impl Tool
/// ```
#[proc_macro_attribute]
pub fn tool(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attrs = parse_macro_input!(attr as ToolAttrs);
    let input_fn = parse_macro_input!(item as ItemFn);

    match tool_impl(attrs, input_fn) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn tool_impl(attrs: ToolAttrs, func: ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    let echo_agent = resolve_echo_crate_path(MacroCrate::Core)?;
    let serde_crate = macro_support_crate_path(&echo_agent, "serde");
    let schemars_crate = macro_support_crate_path(&echo_agent, "schemars");
    let tool_name = &attrs.name;
    let tool_desc = &attrs.description;
    let fn_name = &func.sig.ident;
    let fn_name_str = fn_name.to_string();
    let struct_name = format_ident!("{}Tool", to_pascal_case(&fn_name_str));
    let params_name = format_ident!("{}Params", to_pascal_case(&fn_name_str));

    if !func.sig.generics.params.is_empty() || func.sig.generics.where_clause.is_some() {
        return Err(syn::Error::new_spanned(
            &func.sig.generics,
            "#[tool] does not support generic functions; use a concrete wrapper function",
        ));
    }

    if let ReturnType::Default = &func.sig.output {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "#[tool] function must have a return type (e.g., Result<ToolResult>)",
        ));
    }

    let (param_fields, param_names) = extract_fn_params(&func)?;
    let body = &func.block;

    let permissions_override = if attrs.permissions.is_empty() {
        quote! {}
    } else {
        let perms = attrs.permissions.iter().map(|p| {
            quote! { #echo_agent::tools::permission::ToolPermission::#p }
        });
        quote! {
            fn permissions(&self) -> Vec<#echo_agent::tools::permission::ToolPermission> {
                vec![#(#perms),*]
            }
        }
    };

    let expanded = quote! {
        #[derive(#echo_agent::__macro_support::serde::Deserialize, #echo_agent::__macro_support::schemars::JsonSchema)]
        #[serde(crate = #serde_crate)]
        #[schemars(crate = #schemars_crate)]
        pub struct #params_name {
            #(#param_fields),*
        }

        pub struct #struct_name;

        impl #echo_agent::tools::Tool for #struct_name {
            fn name(&self) -> &str { #tool_name }
            fn description(&self) -> &str { #tool_desc }

            fn parameters(&self) -> #echo_agent::__macro_support::serde_json::Value {
                let schema = #echo_agent::__macro_support::schemars::schema_for!(#params_name);
                #echo_agent::__macro_support::serde_json::to_value(schema).unwrap_or_default()
            }

            #permissions_override

            fn validate_parameters<'a>(&'a self, params: &'a #echo_agent::tools::ToolParameters) -> #echo_agent::__macro_support::futures::future::BoxFuture<'a, #echo_agent::error::Result<()>> {
                Box::pin(async move {
                    #struct_name::deserialize_params(params)?;
                    Ok(())
                })
            }

            fn execute<'a>(&'a self, parameters: #echo_agent::tools::ToolParameters) -> #echo_agent::__macro_support::futures::future::BoxFuture<'a, #echo_agent::error::Result<#echo_agent::tools::ToolResult>> {
                Box::pin(async move {
                    let params = #struct_name::deserialize_params(&parameters)?;
                    let #params_name { #(#param_names),* } = params;
                    #body
                })
            }
        }

        impl #struct_name {
            /// Deserialize and validate tool parameters, returning a typed struct
            /// or a [`ToolError::InvalidParameter`] with extracted field name.
            fn deserialize_params(params: &#echo_agent::tools::ToolParameters) -> #echo_agent::error::Result<#params_name> {
                let value = #echo_agent::__macro_support::serde_json::Value::Object(params.iter().map(|(k, v)| (k.clone(), v.clone())).collect());
                #echo_agent::__macro_support::serde_json::from_value(value).map_err(|e| {
                    let msg = e.to_string();
                    // Extract field name from common serde error patterns:
                    // "missing field `name`" -> "name"
                    // "invalid type: ... at `field`" -> "field"
                    let field = msg
                        .strip_prefix("missing field `")
                        .and_then(|s| s.strip_suffix('`'))
                        .or_else(|| {
                            msg.split("at `").nth(1).and_then(|s| s.strip_suffix('`'))
                        })
                        .unwrap_or("(deserialization)");
                    #echo_agent::error::ToolError::InvalidParameter {
                        name: field.to_string(),
                        message: msg,
                    }.into()
                })
            }
        }
    };

    Ok(expanded)
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// #[callback] — Generate AgentCallback impl from impl block
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Generate an `AgentCallback` implementation from an impl block, overriding
/// only the methods you define.
///
/// # Example
///
/// ```rust,ignore
/// struct LogCallback;
///
/// #[callback]
/// impl LogCallback {
///     async fn on_tool_start(&self, _agent: &str, tool: &str, _args: &Value) {
///         println!("Tool started: {tool}");
///     }
///     async fn on_final_answer(&self, _agent: &str, answer: &str) {
///         println!("Final answer: {answer}");
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn callback(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemImpl);
    match callback_impl(input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn callback_impl(input: ItemImpl) -> syn::Result<proc_macro2::TokenStream> {
    let echo_agent = resolve_echo_crate_path(MacroCrate::Core)?;
    reject_generic_impl(&input, "#[callback]")?;
    let self_ty = &input.self_ty;
    let method_impls = impl_block_to_boxfuture_methods(&echo_agent, &input)?;

    Ok(quote! {
        impl #echo_agent::agent::AgentCallback for #self_ty {
            #(#method_impls)*
        }
    })
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// #[guard] — Generate Guard impl from async fn
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

struct NameAttr {
    name: String,
}

impl syn::parse::Parse for NameAttr {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut name: Option<String> = None;
        while !input.is_empty() {
            let ident: syn::Ident = input.parse()?;
            let _eq: syn::Token![=] = input.parse()?;
            let value: LitStr = input.parse()?;
            if ident == "name" {
                name = Some(value.value());
            } else {
                return Err(syn::Error::new_spanned(ident, "expected `name`"));
            }
            if !input.is_empty() {
                let _: syn::Token![,] = input.parse()?;
            }
        }
        let name =
            name.ok_or_else(|| syn::Error::new(Span::call_site(), "requires `name = \"...\"`"))?;
        Ok(NameAttr { name })
    }
}

/// Generate a `Guard` implementation from an async function.
///
/// # Example
///
/// ```rust,ignore
/// #[guard(name = "length-limit")]
/// async fn check_length(content: &str, direction: GuardDirection) -> Result<GuardResult> {
///     if content.len() > 10000 {
///         Ok(GuardResult::Block { reason: "Content too long".into() })
///     } else {
///         Ok(GuardResult::Pass)
///     }
/// }
/// // Generates: LengthLimitGuard + impl Guard
/// ```
#[proc_macro_attribute]
pub fn guard(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attrs = parse_macro_input!(attr as NameAttr);
    let input_fn = parse_macro_input!(item as ItemFn);
    match guard_impl(attrs, input_fn) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn guard_impl(attrs: NameAttr, func: ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    let echo_agent = resolve_echo_crate_path(MacroCrate::Core)?;
    let guard_name = &attrs.name;
    let struct_name = generated_ident(
        &format!("{}Guard", to_pascal_case(&guard_name.replace('-', "_"))),
        "guard name",
    )?;
    require_return_type(&func)?;
    let body = &func.block;

    Ok(quote! {
        pub struct #struct_name;

        impl #echo_agent::guard::Guard for #struct_name {
            fn name(&self) -> &str { #guard_name }

            fn check<'a>(
                &'a self,
                content: &'a str,
                direction: #echo_agent::guard::GuardDirection,
            ) -> #echo_agent::__macro_support::futures::future::BoxFuture<'a, #echo_agent::error::Result<#echo_agent::guard::GuardResult>> {
                Box::pin(async move #body)
            }
        }
    })
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// #[handler] — Generate HumanLoopHandler impl from impl block
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Generate a `HumanLoopHandler` implementation from an impl block.
///
/// # Overridable Methods
///
/// - `on_approval(&self, tool_name: &str, args: &Value, prompt: &str) -> ApprovalDecision`
/// - `on_input(&self, prompt: &str) -> String`
///
/// # Example
///
/// ```rust,ignore
/// struct AutoApproveHandler;
///
/// #[handler]
/// impl AutoApproveHandler {
///     async fn on_approval(&self, _tool: &str, _args: &Value, _prompt: &str) -> ApprovalDecision {
///         ApprovalDecision::Approved
///     }
///     async fn on_input(&self, prompt: &str) -> String {
///         println!("Agent asks: {prompt}");
///         "default response".to_string()
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn handler(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemImpl);
    match handler_impl(input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn handler_impl(input: ItemImpl) -> syn::Result<proc_macro2::TokenStream> {
    let orchestration = resolve_echo_crate_path(MacroCrate::Orchestration)?;
    let macro_support = resolve_echo_crate_path(MacroCrate::Core)?;
    reject_generic_impl(&input, "#[handler]")?;
    let self_ty = &input.self_ty;
    let method_impls = extract_boxfuture_methods_with_return(&macro_support, &input)?;

    Ok(quote! {
        impl #orchestration::human_loop::HumanLoopHandler for #self_ty {
            #(#method_impls)*
        }
    })
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// #[compressor] — Generate ContextCompressor impl from async fn
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Generate a `ContextCompressor` implementation from an async function.
///
/// # Example
///
/// ```rust,ignore
/// #[compressor]
/// async fn keep_recent(input: CompressionInput) -> Result<CompressionOutput> {
///     let keep = input.messages.len().min(20);
///     let evicted = input.messages[..input.messages.len() - keep].to_vec();
///     let messages = input.messages[input.messages.len() - keep..].to_vec();
///     Ok(CompressionOutput { messages, evicted })
/// }
/// // Generates: KeepRecentCompressor + impl ContextCompressor
/// ```
#[proc_macro_attribute]
pub fn compressor(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input_fn = parse_macro_input!(item as ItemFn);
    match compressor_impl(input_fn) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn compressor_impl(func: ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    let echo_agent = resolve_echo_crate_path(MacroCrate::Core)?;
    let fn_name = &func.sig.ident;
    let struct_name = format_ident!("{}Compressor", to_pascal_case(&fn_name.to_string()));
    require_return_type(&func)?;
    let body = &func.block;

    Ok(quote! {
        pub struct #struct_name;

        impl #echo_agent::compression::ContextCompressor for #struct_name {
            fn compress(
                &self,
                input: #echo_agent::compression::CompressionInput,
            ) -> #echo_agent::__macro_support::futures::future::BoxFuture<'_, #echo_agent::error::Result<#echo_agent::compression::CompressionOutput>> {
                Box::pin(async move #body)
            }
        }
    })
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// #[permission_policy] — Generate PermissionPolicy impl from async fn
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Generate a `PermissionPolicy` implementation from an async function.
///
/// # Example
///
/// ```rust,ignore
/// #[permission_policy]
/// async fn allow_all(tool_name: &str, permissions: &[ToolPermission]) -> PermissionDecision {
///     PermissionDecision::Allow
/// }
/// // Generates: AllowAllPolicy + impl PermissionPolicy
/// ```
#[proc_macro_attribute]
pub fn permission_policy(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input_fn = parse_macro_input!(item as ItemFn);
    match permission_policy_impl(input_fn) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn permission_policy_impl(func: ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    let echo_agent = resolve_echo_crate_path(MacroCrate::Core)?;
    let fn_name = &func.sig.ident;
    let struct_name = format_ident!("{}Policy", to_pascal_case(&fn_name.to_string()));
    require_return_type(&func)?;
    let body = &func.block;

    Ok(quote! {
        pub struct #struct_name;

        impl #echo_agent::tools::permission::PermissionPolicy for #struct_name {
            fn check<'a>(
                &'a self,
                tool_name: &'a str,
                permissions: &'a [#echo_agent::tools::permission::ToolPermission],
            ) -> #echo_agent::__macro_support::futures::future::BoxFuture<'a, #echo_agent::tools::permission::PermissionDecision> {
                Box::pin(async move #body)
            }
        }
    })
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// #[audit_logger] — Generate AuditLogger impl from impl block
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Generate an `AuditLogger` implementation from an impl block.
///
/// # Overridable Methods
///
/// - `log(&self, event: AuditEvent) -> Result<()>`
/// - `query(&self, filter: AuditFilter) -> Result<Vec<AuditEvent>>`
///
/// # Example
///
/// ```rust,ignore
/// struct PrintLogger;
///
/// #[audit_logger]
/// impl PrintLogger {
///     async fn log(&self, event: AuditEvent) -> Result<()> {
///         println!("[audit] {:?}", event.event_type);
///         Ok(())
///     }
///     async fn query(&self, _filter: AuditFilter) -> Result<Vec<AuditEvent>> {
///         Ok(vec![])
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn audit_logger(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemImpl);
    match audit_logger_impl(input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn audit_logger_impl(input: ItemImpl) -> syn::Result<proc_macro2::TokenStream> {
    let echo_agent = resolve_echo_crate_path(MacroCrate::Core)?;
    reject_generic_impl(&input, "#[audit_logger]")?;
    let self_ty = &input.self_ty;
    let method_impls = extract_boxfuture_methods_with_return(&echo_agent, &input)?;

    Ok(quote! {
        impl #echo_agent::audit::AuditLogger for #self_ty {
            #(#method_impls)*
        }
    })
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Shared helper functions
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

fn extract_fn_params(
    func: &ItemFn,
) -> syn::Result<(Vec<proc_macro2::TokenStream>, Vec<syn::Ident>)> {
    let mut param_fields = Vec::new();
    let mut param_names = Vec::new();

    for arg in func.sig.inputs.iter() {
        if let FnArg::Typed(pat_type) = arg {
            let pat = &pat_type.pat;
            let ty = &pat_type.ty;

            let field_name = if let Pat::Ident(pi) = pat.as_ref() {
                pi.ident.clone()
            } else {
                return Err(syn::Error::new_spanned(pat, "expected identifier pattern"));
            };

            let doc_str = extract_doc_comments(&pat_type.attrs);
            let schemars_attr = if let Some(doc) = &doc_str {
                quote! { #[schemars(description = #doc)] }
            } else {
                quote! {}
            };

            param_fields.push(quote! {
                #schemars_attr
                pub #field_name: #ty
            });
            param_names.push(field_name);
        }
    }

    Ok((param_fields, param_names))
}

/// For callback-style traits: returns `BoxFuture<'a, ()>`.
fn impl_block_to_boxfuture_methods(
    echo_agent: &syn::Path,
    input: &ItemImpl,
) -> syn::Result<Vec<proc_macro2::TokenStream>> {
    let mut methods = Vec::new();

    for item in &input.items {
        if let ImplItem::Fn(method) = item {
            let name_ident = &method.sig.ident;
            let body = &method.block;
            let lifetime_params = lifetimed_params(&method.sig.inputs);

            methods.push(quote! {
                fn #name_ident<'a>(#(#lifetime_params),*) -> #echo_agent::__macro_support::futures::future::BoxFuture<'a, ()> {
                    Box::pin(async move #body)
                }
            });
        }
    }

    Ok(methods)
}

/// For traits where user methods have explicit return types (HumanLoopHandler, AuditLogger).
fn extract_boxfuture_methods_with_return(
    echo_agent: &syn::Path,
    input: &ItemImpl,
) -> syn::Result<Vec<proc_macro2::TokenStream>> {
    let mut methods = Vec::new();

    for item in &input.items {
        if let ImplItem::Fn(method) = item {
            let name_ident = &method.sig.ident;
            let body = &method.block;
            let lifetime_params = lifetimed_params(&method.sig.inputs);

            let ret_ty = match &method.sig.output {
                ReturnType::Default => quote! { () },
                ReturnType::Type(_, ty) => quote! { #ty },
            };

            methods.push(quote! {
                fn #name_ident<'a>(#(#lifetime_params),*) -> #echo_agent::__macro_support::futures::future::BoxFuture<'a, #ret_ty> {
                    Box::pin(async move #body)
                }
            });
        }
    }

    Ok(methods)
}

/// Rewrites each `FnArg` so `&self` → `&'a self` and `&T` → `&'a T`.
fn lifetimed_params(
    inputs: &syn::punctuated::Punctuated<FnArg, syn::token::Comma>,
) -> Vec<proc_macro2::TokenStream> {
    inputs
        .iter()
        .map(|arg| match arg {
            FnArg::Receiver(_) => quote! { &'a self },
            FnArg::Typed(pat_type) => {
                let pat = &pat_type.pat;
                let ty = add_lifetime_a(&pat_type.ty);
                quote! { #pat: #ty }
            }
        })
        .collect()
}

/// Adds `'a` to top-level references that lack an explicit lifetime.
/// `&T` → `&'a T`, `&mut T` → `&'a mut T`.
/// References that already have a named lifetime (e.g., `&'b T`) are left unchanged.
/// Non-reference types pass through unchanged.
fn add_lifetime_a(ty: &syn::Type) -> proc_macro2::TokenStream {
    match ty {
        syn::Type::Reference(r) => {
            let elem = &r.elem;
            // Preserve existing explicit lifetimes; only add 'a if none is present
            let lifetime = r
                .lifetime
                .as_ref()
                .map(|lt| quote! { #lt })
                .unwrap_or(quote! { 'a });
            if r.mutability.is_some() {
                quote! { &#lifetime mut #elem }
            } else {
                quote! { &#lifetime #elem }
            }
        }
        other => quote! { #other },
    }
}

fn require_return_type(func: &ItemFn) -> syn::Result<()> {
    if let ReturnType::Default = &func.sig.output {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "function must have an explicit return type",
        ));
    }
    Ok(())
}

fn reject_generic_impl(input: &ItemImpl, macro_name: &str) -> syn::Result<()> {
    if input.generics.params.is_empty() && input.generics.where_clause.is_none() {
        Ok(())
    } else {
        Err(syn::Error::new_spanned(
            &input.generics,
            format!(
                "{macro_name} does not support generic impl blocks; use a concrete wrapper type"
            ),
        ))
    }
}

fn generated_ident(value: &str, source: &str) -> syn::Result<syn::Ident> {
    syn::parse_str::<syn::Ident>(value).map_err(|_| {
        syn::Error::new(
            Span::call_site(),
            format!("{source} does not produce a valid Rust identifier: `{value}`"),
        )
    })
}

pub(crate) fn extract_doc_comments(attrs: &[syn::Attribute]) -> Option<String> {
    let docs: Vec<String> = attrs
        .iter()
        .filter_map(|attr| {
            if !attr.path().is_ident("doc") {
                return None;
            }
            if let syn::Meta::NameValue(nv) = &attr.meta
                && let syn::Expr::Lit(expr_lit) = &nv.value
                && let syn::Lit::Str(s) = &expr_lit.lit
            {
                return Some(s.value().trim().to_string());
            }
            None
        })
        .collect();

    if docs.is_empty() {
        None
    } else {
        Some(docs.join(" "))
    }
}

fn to_pascal_case(s: &str) -> String {
    s.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => {
                    let upper: String = c.to_uppercase().collect();
                    upper + &chars.collect::<String>()
                }
            }
        })
        .collect()
}
