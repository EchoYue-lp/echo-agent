use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::{FnArg, ItemFn, LitStr, Pat, ReturnType, parse_macro_input};

struct ToolAttrs {
    name: String,
    description: String,
}

impl syn::parse::Parse for ToolAttrs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut name: Option<String> = None;
        let mut description: Option<String> = None;

        while !input.is_empty() {
            let ident: syn::Ident = input.parse()?;
            let _eq: syn::Token![=] = input.parse()?;
            let value: LitStr = input.parse()?;

            if ident == "name" {
                name = Some(value.value());
            } else if ident == "description" {
                description = Some(value.value());
            } else {
                return Err(syn::Error::new_spanned(
                    ident,
                    "unknown attribute, expected `name` or `description`",
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

        Ok(ToolAttrs { name, description })
    }
}

/// Attribute macro that generates a `TypedTool` implementation from an async function.
///
/// # Attributes
///
/// - `name` (required): Tool name exposed to the LLM
/// - `description` (required): Tool description for the LLM
///
/// # Generated code
///
/// Given a function `add`, the macro generates:
/// - `AddParams` struct with `#[derive(Deserialize, JsonSchema)]`
/// - `AddTool` struct (unit struct)
/// - `impl TypedTool for AddTool` with automatic schema generation
///
/// # Parameter documentation
///
/// Doc comments (`///`) on function parameters become `#[schemars(description = "...")]`
/// annotations on the generated params struct fields.
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
/// // Generates: AddParams, AddTool, impl TypedTool for AddTool
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

    let body = &func.block;

    let expanded = quote! {
        #[derive(::serde::Deserialize, ::schemars::JsonSchema)]
        pub struct #params_name {
            #(#param_fields),*
        }

        pub struct #struct_name;

        impl ::echo_agent::tools::TypedTool for #struct_name {
            type Params = #params_name;

            fn name(&self) -> &str {
                #tool_name
            }

            fn description(&self) -> &str {
                #tool_desc
            }

            fn execute_typed(&self, params: #params_name) -> ::futures::future::BoxFuture<'_, ::echo_agent::error::Result<::echo_agent::tools::ToolResult>> {
                Box::pin(async move {
                    let #params_name { #(#param_names),* } = params;
                    #body
                })
            }
        }
    };

    Ok(expanded)
}

fn extract_doc_comments(attrs: &[syn::Attribute]) -> Option<String> {
    let docs: Vec<String> = attrs
        .iter()
        .filter_map(|attr| {
            if !attr.path().is_ident("doc") {
                return None;
            }
            if let syn::Meta::NameValue(nv) = &attr.meta {
                if let syn::Expr::Lit(expr_lit) = &nv.value {
                    if let syn::Lit::Str(s) = &expr_lit.lit {
                        return Some(s.value().trim().to_string());
                    }
                }
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
