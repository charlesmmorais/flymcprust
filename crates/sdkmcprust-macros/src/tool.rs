//! Expansion of `#[tool]`.

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::Parser as _;
use syn::spanned::Spanned as _;
use syn::{Error, Ident, ItemFn, LitBool, LitStr, ReturnType};

use crate::docs;
use crate::params::{self, Options, Param};

const OPTIONS: Options = Options {
    macro_name: "tool",
    schema_attrs: true,
    complete: false,
};

#[derive(Default)]
struct ToolArgs {
    name: Option<LitStr>,
    title: Option<LitStr>,
    description: Option<LitStr>,
    read_only: Option<LitBool>,
    destructive: Option<LitBool>,
    idempotent: Option<LitBool>,
    open_world: Option<LitBool>,
    scopes: Option<Vec<LitStr>>,
}

fn parse_args(attr: TokenStream) -> syn::Result<ToolArgs> {
    let mut args = ToolArgs::default();
    let parser = syn::meta::parser(|meta| {
        let set_str = |slot: &mut Option<LitStr>, meta: &syn::meta::ParseNestedMeta<'_>| {
            if slot.is_some() {
                return Err(meta.error("duplicate argument"));
            }
            *slot = Some(meta.value()?.parse()?);
            Ok(())
        };
        let set_flag = |slot: &mut Option<LitBool>, meta: &syn::meta::ParseNestedMeta<'_>| {
            if slot.is_some() {
                return Err(meta.error("duplicate argument"));
            }
            *slot = Some(if meta.input.peek(syn::Token![=]) {
                meta.value()?.parse()?
            } else {
                LitBool::new(true, meta.path.span())
            });
            Ok(())
        };
        if meta.path.is_ident("name") {
            set_str(&mut args.name, &meta)
        } else if meta.path.is_ident("title") {
            set_str(&mut args.title, &meta)
        } else if meta.path.is_ident("description") {
            set_str(&mut args.description, &meta)
        } else if meta.path.is_ident("read_only") {
            set_flag(&mut args.read_only, &meta)
        } else if meta.path.is_ident("destructive") {
            set_flag(&mut args.destructive, &meta)
        } else if meta.path.is_ident("idempotent") {
            set_flag(&mut args.idempotent, &meta)
        } else if meta.path.is_ident("open_world") {
            set_flag(&mut args.open_world, &meta)
        } else if let Some(key) = meta.path.get_ident().filter(|i| *i == "scopes") {
            let value: syn::Expr = meta.value()?.parse()?;
            params::set_once(&mut args.scopes, key, params::scopes(key, &value)?)
        } else {
            Err(meta.error(
                "unknown `tool` argument; expected `name`, `title`, `description`, \
                 `read_only`, `destructive`, `idempotent`, `open_world` or `scopes`",
            ))
        }
    });
    parser.parse2(attr)?;

    if let (Some(read_only), Some(destructive)) = (&args.read_only, &args.destructive)
        && read_only.value
        && destructive.value
    {
        return Err(Error::new(
            destructive.span(),
            "a `read_only` tool cannot be `destructive`",
        ));
    }
    Ok(args)
}

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let args = parse_args(attr)?;
    let mut func: ItemFn = syn::parse2(item)?;
    params::check_signature(&func, OPTIONS.macro_name)?;

    let params = params::collect_params(&mut func, &OPTIONS)?;
    let krate = params::krate();
    let scopes = params::scopes_call(args.scopes.as_ref());
    let fn_ident = &func.sig.ident;
    let vis = &func.vis;

    let name = match &args.name {
        Some(lit) => lit.clone(),
        None => LitStr::new(
            fn_ident.to_string().trim_start_matches("r#"),
            fn_ident.span(),
        ),
    };
    if !is_valid_tool_name(&name.value()) {
        return Err(Error::new(
            name.span(),
            "invalid tool name: use 1-128 characters from A-Z, a-z, 0-9, `_`, `-` and `.`",
        ));
    }

    let (doc_description, arg_docs) = docs::split(&docs::doc_lines(&func.attrs));
    params::check_documented_args(
        &params,
        &arg_docs,
        fn_ident,
        "a JSON parameter of this tool",
    )?;
    let description = args
        .description
        .as_ref()
        .map(LitStr::value)
        .or(doc_description);

    // The argument struct and the call.
    let mut fields = Vec::new();
    let mut destructure = Vec::new();
    for param in &params {
        if let Param::Arg(arg) = param {
            let ident = &arg.ident;
            let ty = &arg.ty;
            let forwarded = &arg.forwarded;
            let doc_attr = params::arg_doc(arg, &arg_docs).map(|d| {
                let d = format!(" {d}");
                quote!(#[doc = #d])
            });
            fields.push(quote! { #doc_attr #(#forwarded)* #ident: #ty });
            destructure.push(quote!(#ident));
        }
    }
    let (call_args, injected) = params::call_args(&params);
    let extract = &injected.extract;
    let state_types = &injected.states;

    let call = if func.sig.asyncness.is_some() {
        quote!(#fn_ident(#(#call_args),*).await)
    } else {
        quote!(#fn_ident(#(#call_args),*))
    };
    let ret_ty = match &func.sig.output {
        ReturnType::Default => quote!(()),
        ReturnType::Type(_, ty) => quote!(#ty),
    };

    let set_description = description.map(|d| {
        quote! { tool.description = ::core::option::Option::Some(::std::string::String::from(#d)); }
    });
    let set_title = args.title.as_ref().map(|t| {
        quote! { tool.title = ::core::option::Option::Some(::std::string::String::from(#t)); }
    });
    let hint = |flag: &Option<LitBool>| match flag {
        Some(b) => quote!(::core::option::Option::Some(#b)),
        None => quote!(::core::option::Option::None),
    };
    let annotations = if args.read_only.is_some()
        || args.destructive.is_some()
        || args.idempotent.is_some()
        || args.open_world.is_some()
    {
        let read_only = hint(&args.read_only);
        let destructive = hint(&args.destructive);
        let idempotent = hint(&args.idempotent);
        let open_world = hint(&args.open_world);
        Some(quote! {
            tool.annotations = ::core::option::Option::Some(#krate::spec::ToolAnnotations {
                destructive_hint: #destructive,
                idempotent_hint: #idempotent,
                open_world_hint: #open_world,
                read_only_hint: #read_only,
                title: ::core::option::Option::None,
            });
        })
    } else {
        None
    };

    let auto_register = auto_register(fn_ident);
    let marker_doc = format!(" MCP tool marker for [`{fn_ident}()`], generated by `#[tool]`.");

    Ok(quote! {
        #func

        #[doc = #marker_doc]
        #[allow(non_camel_case_types)]
        #vis struct #fn_ident {}

        impl #krate::Tool for #fn_ident {
            fn definition() -> #krate::ToolDef {
                #[derive(#krate::serde::Deserialize, #krate::schemars::JsonSchema)]
                #[serde(crate = "::sdkmcprust::__private::serde", deny_unknown_fields)]
                #[schemars(crate = "::sdkmcprust::__private::schemars")]
                struct __ToolArgs {
                    #(#fields,)*
                }

                #[allow(unused_variables, non_snake_case, clippy::needless_pass_by_value)]
                fn __call(
                    arguments: ::core::option::Option<&#krate::RawValue>,
                    __ctx: #krate::Ctx,
                ) -> ::core::result::Result<#krate::ToolFuture, #krate::ToolError> {
                    let __ToolArgs { #(#destructure),* } =
                        #krate::parse_arguments::<__ToolArgs>(arguments)?;
                    #(#extract)*
                    ::core::result::Result::Ok(::std::boxed::Box::pin(async move {
                        #krate::IntoToolResult::into_tool_result(#call)
                    }))
                }

                let mut tool = #krate::spec::Tool::new(
                    #krate::input_schema_for::<__ToolArgs>(),
                    ::std::string::String::from(#name),
                );
                #set_description
                #set_title
                #annotations
                tool.output_schema = <#ret_ty as #krate::IntoToolResult>::output_schema()
                    .map(#krate::output_schema_from);
                #krate::ToolDef::new(tool, __call)
                    #(.require_state::<#state_types>())*
                    #scopes
            }
        }

        #auto_register
    })
}

#[cfg(feature = "auto-register")]
fn auto_register(fn_ident: &Ident) -> TokenStream {
    let krate = params::krate();
    quote! {
        #krate::inventory::submit! {
            #krate::AutoTool::new(<#fn_ident as #krate::Tool>::definition)
        }
    }
}

#[cfg(not(feature = "auto-register"))]
fn auto_register(_: &Ident) -> TokenStream {
    TokenStream::new()
}

fn is_valid_tool_name(name: &str) -> bool {
    (1..=128).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}
