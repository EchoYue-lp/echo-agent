use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::{FnArg, ImplItem, ItemFn, ItemImpl, LitStr, Pat, ReturnType, parse_macro_input};

fn echo_agent_crate_path() -> syn::Result<syn::Path> {
    match crate_name("echo_agent").map_err(|e| syn::Error::new(Span::call_site(), e.to_string()))? {
        FoundCrate::Itself => Ok(syn::parse_quote!(::echo_agent)),
        FoundCrate::Name(name) => {
            let ident = syn::Ident::new(&name, Span::call_site());
            Ok(syn::parse_quote!(::#ident))
        }
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// #[tool] — 从 async fn 生成 TypedTool 实现
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

/// 从 async fn 生成 `TypedTool` 实现，自动生成参数结构体和 JSON Schema。
///
/// # 属性
///
/// - `name` (必填): 工具名称
/// - `description` (必填): 工具描述
/// - `permissions` (可选): 权限列表，如 `[Execute, Network]`
///
/// # 示例
///
/// ```rust,ignore
/// #[tool(name = "add", description = "两数相加")]
/// async fn add(
///     /// 第一个数
///     a: f64,
///     /// 第二个数
///     b: f64,
/// ) -> Result<ToolResult> {
///     Ok(ToolResult::success(format!("{}", a + b)))
/// }
/// // 生成: AddParams 结构体 + AddTool 单元结构体 + impl TypedTool
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
    let echo_agent = echo_agent_crate_path()?;
    let tool_name = &attrs.name;
    let tool_desc = &attrs.description;
    let fn_name = &func.sig.ident;
    let fn_name_str = fn_name.to_string();
    let struct_name = format_ident!("{}Tool", to_pascal_case(&fn_name_str));
    let params_name = format_ident!("{}Params", to_pascal_case(&fn_name_str));

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
        #[derive(::serde::Deserialize, ::schemars::JsonSchema)]
        pub struct #params_name {
            #(#param_fields),*
        }

        pub struct #struct_name;

        impl #echo_agent::tools::TypedTool for #struct_name {
            type Params = #params_name;

            fn name(&self) -> &str { #tool_name }
            fn description(&self) -> &str { #tool_desc }

            fn execute_typed(&self, params: #params_name) -> ::futures::future::BoxFuture<'_, #echo_agent::error::Result<#echo_agent::tools::ToolResult>> {
                Box::pin(async move {
                    let #params_name { #(#param_names),* } = params;
                    #body
                })
            }
        }

        impl #struct_name {
            #permissions_override
        }
    };

    Ok(expanded)
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// #[callback] — 从 impl 块生成 AgentCallback 实现
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// 从 impl 块生成 `AgentCallback` 实现，只覆写你定义的方法。
///
/// # 示例
///
/// ```rust,ignore
/// struct LogCallback;
///
/// #[callback]
/// impl LogCallback {
///     async fn on_tool_start(&self, _agent: &str, tool: &str, _args: &Value) {
///         println!("工具开始: {tool}");
///     }
///     async fn on_final_answer(&self, _agent: &str, answer: &str) {
///         println!("最终答案: {answer}");
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
    let echo_agent = echo_agent_crate_path()?;
    let self_ty = &input.self_ty;
    let method_impls = impl_block_to_boxfuture_methods(&input, "()")?;

    Ok(quote! {
        impl #echo_agent::agent::AgentCallback for #self_ty {
            #(#method_impls)*
        }
    })
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// #[guard] — 从 async fn 生成 Guard 实现
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

/// 从 async fn 生成 `Guard` 实现。
///
/// # 示例
///
/// ```rust,ignore
/// #[guard(name = "length-limit")]
/// async fn check_length(content: &str, direction: GuardDirection) -> Result<GuardResult> {
///     if content.len() > 10000 {
///         Ok(GuardResult::Block { reason: "内容过长".into() })
///     } else {
///         Ok(GuardResult::Pass)
///     }
/// }
/// // 生成: LengthLimitGuard + impl Guard
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
    let echo_agent = echo_agent_crate_path()?;
    let guard_name = &attrs.name;
    let struct_name = format_ident!("{}Guard", to_pascal_case(&guard_name.replace('-', "_")));
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
            ) -> ::futures::future::BoxFuture<'a, #echo_agent::error::Result<#echo_agent::guard::GuardResult>> {
                Box::pin(async move #body)
            }
        }
    })
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// #[handler] — 从 impl 块生成 HumanLoopHandler 实现
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// 从 impl 块生成 `HumanLoopHandler` 实现。
///
/// # 可覆写方法
///
/// - `on_approval(&self, tool_name: &str, args: &Value, prompt: &str) -> ApprovalDecision`
/// - `on_input(&self, prompt: &str) -> String`
///
/// # 示例
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
///         println!("Agent 问: {prompt}");
///         "默认回答".to_string()
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
    let echo_agent = echo_agent_crate_path()?;
    let self_ty = &input.self_ty;
    let method_impls = extract_boxfuture_methods_with_return(&input)?;

    Ok(quote! {
        impl #echo_agent::human_loop::HumanLoopHandler for #self_ty {
            #(#method_impls)*
        }
    })
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// #[compressor] — 从 async fn 生成 ContextCompressor 实现
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// 从 async fn 生成 `ContextCompressor` 实现。
///
/// # 示例
///
/// ```rust,ignore
/// #[compressor]
/// async fn keep_recent(input: CompressionInput) -> Result<CompressionOutput> {
///     let keep = input.messages.len().min(20);
///     let evicted = input.messages[..input.messages.len() - keep].to_vec();
///     let messages = input.messages[input.messages.len() - keep..].to_vec();
///     Ok(CompressionOutput { messages, evicted })
/// }
/// // 生成: KeepRecentCompressor + impl ContextCompressor
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
    let echo_agent = echo_agent_crate_path()?;
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
            ) -> ::futures::future::BoxFuture<'_, #echo_agent::error::Result<#echo_agent::compression::CompressionOutput>> {
                Box::pin(async move #body)
            }
        }
    })
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// #[permission_policy] — 从 async fn 生成 PermissionPolicy 实现
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// 从 async fn 生成 `PermissionPolicy` 实现。
///
/// # 示例
///
/// ```rust,ignore
/// #[permission_policy]
/// async fn allow_all(tool_name: &str, permissions: &[ToolPermission]) -> PermissionDecision {
///     PermissionDecision::Allow
/// }
/// // 生成: AllowAllPolicy + impl PermissionPolicy
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
    let echo_agent = echo_agent_crate_path()?;
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
            ) -> ::futures::future::BoxFuture<'a, #echo_agent::tools::permission::PermissionDecision> {
                Box::pin(async move #body)
            }
        }
    })
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// #[audit_logger] — 从 impl 块生成 AuditLogger 实现
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// 从 impl 块生成 `AuditLogger` 实现。
///
/// # 可覆写方法
///
/// - `log(&self, event: AuditEvent) -> Result<()>`
/// - `query(&self, filter: AuditFilter) -> Result<Vec<AuditEvent>>`
///
/// # 示例
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
    let echo_agent = echo_agent_crate_path()?;
    let self_ty = &input.self_ty;
    let method_impls = extract_boxfuture_methods_with_return(&input)?;

    Ok(quote! {
        impl #echo_agent::audit::AuditLogger for #self_ty {
            #(#method_impls)*
        }
    })
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// 共用辅助函数
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
    input: &ItemImpl,
    _default_return: &str,
) -> syn::Result<Vec<proc_macro2::TokenStream>> {
    let mut methods = Vec::new();

    for item in &input.items {
        if let ImplItem::Fn(method) = item {
            let name_ident = &method.sig.ident;
            let body = &method.block;
            let lifetime_params = lifetimed_params(&method.sig.inputs);

            methods.push(quote! {
                fn #name_ident<'a>(#(#lifetime_params),*) -> ::futures::future::BoxFuture<'a, ()> {
                    Box::pin(async move #body)
                }
            });
        }
    }

    Ok(methods)
}

/// For traits where user methods have explicit return types (HumanLoopHandler, AuditLogger).
fn extract_boxfuture_methods_with_return(
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
                fn #name_ident<'a>(#(#lifetime_params),*) -> ::futures::future::BoxFuture<'a, #ret_ty> {
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

/// Adds `'a` to top-level references: `&T` → `&'a T`, `&mut T` → `&'a mut T`.
/// Non-reference types pass through unchanged.
fn add_lifetime_a(ty: &syn::Type) -> proc_macro2::TokenStream {
    match ty {
        syn::Type::Reference(r) => {
            let elem = &r.elem;
            if r.mutability.is_some() {
                quote! { &'a mut #elem }
            } else {
                quote! { &'a #elem }
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

fn extract_doc_comments(attrs: &[syn::Attribute]) -> Option<String> {
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
