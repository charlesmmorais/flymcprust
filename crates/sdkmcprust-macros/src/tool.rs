//! Expansion of `#[tool]`.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::parse::Parser as _;
use syn::spanned::Spanned as _;
use syn::{
    Attribute, Error, FnArg, GenericArgument, Ident, ItemFn, LitBool, LitStr, Pat, PathArguments,
    ReturnType, Type,
};

use crate::docs;

/// Root of every generated path, so user crates need no direct dependency
/// on serde, schemars or serde_json.
fn krate() -> TokenStream {
    quote!(::sdkmcprust::__private)
}

#[derive(Default)]
struct ToolArgs {
    name: Option<LitStr>,
    title: Option<LitStr>,
    description: Option<LitStr>,
    read_only: Option<LitBool>,
    destructive: Option<LitBool>,
    idempotent: Option<LitBool>,
    open_world: Option<LitBool>,
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
        } else {
            Err(meta.error(
                "unknown `tool` argument; expected `name`, `title`, `description`, \
                 `read_only`, `destructive`, `idempotent` or `open_world`",
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

enum Param {
    /// A JSON argument, part of the input schema.
    Arg {
        ident: Ident,
        ty: Type,
        /// `#[serde]` / `#[schemars]` attributes forwarded to the schema field.
        forwarded: Vec<Attribute>,
        /// Description from a doc comment on the parameter.
        doc: Option<String>,
    },
    /// The request context.
    Ctx,
    /// Shared state of the inner type.
    State(Type),
}

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let args = parse_args(attr)?;
    let mut func: ItemFn = syn::parse2(item)?;
    check_signature(&func)?;

    let params = collect_params(&mut func)?;
    let krate = krate();
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
    for documented in arg_docs.keys() {
        let known = params
            .iter()
            .any(|p| matches!(p, Param::Arg { ident, .. } if ident == documented));
        if !known {
            return Err(Error::new(
                fn_ident.span(),
                format!(
                    "`# Arguments` documents `{documented}`, which is not a JSON parameter of this tool"
                ),
            ));
        }
    }
    let description = args
        .description
        .as_ref()
        .map(LitStr::value)
        .or(doc_description);

    // The argument struct and the call.
    let mut fields = Vec::new();
    let mut destructure = Vec::new();
    let mut extract = Vec::new();
    let mut call_args = Vec::new();
    let mut state_types = Vec::new();
    for (i, param) in params.iter().enumerate() {
        match param {
            Param::Arg {
                ident,
                ty,
                forwarded,
                doc,
            } => {
                let doc = doc
                    .clone()
                    .or_else(|| arg_docs.get(&ident.to_string()).cloned());
                let doc_attr = doc.map(|d| {
                    let d = format!(" {d}");
                    quote!(#[doc = #d])
                });
                fields.push(quote! { #doc_attr #(#forwarded)* #ident: #ty });
                destructure.push(quote!(#ident));
                call_args.push(quote!(#ident));
            }
            Param::Ctx => call_args.push(quote!(::core::clone::Clone::clone(&__ctx))),
            Param::State(inner) => {
                let var = format_ident!("__state{}", i);
                extract.push(quote! {
                    let #var = __ctx
                        .state::<#inner>()
                        .ok_or_else(#krate::missing_state)?;
                });
                call_args.push(quote!(#var));
                state_types.push(inner.clone());
            }
        }
    }

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

                #[allow(unused_variables, clippy::needless_pass_by_value)]
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
            }
        }

        #auto_register
    })
}

#[cfg(feature = "auto-register")]
fn auto_register(fn_ident: &Ident) -> TokenStream {
    let krate = krate();
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

fn check_signature(func: &ItemFn) -> syn::Result<()> {
    let sig = &func.sig;
    if !sig.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &sig.generics,
            "#[tool] functions cannot be generic",
        ));
    }
    if let Some(variadic) = &sig.variadic {
        return Err(Error::new_spanned(
            variadic,
            "#[tool] functions cannot be variadic",
        ));
    }
    if let Some(abi) = &sig.abi {
        return Err(Error::new_spanned(
            abi,
            "#[tool] functions cannot be `extern`",
        ));
    }
    if let syn::Safety::Unsafe(token) = &sig.safety {
        return Err(Error::new_spanned(
            token,
            "#[tool] functions cannot be `unsafe`",
        ));
    }
    if let ReturnType::Type(_, ty) = &sig.output
        && let Type::ImplTrait(_) = &**ty
    {
        return Err(Error::new_spanned(
            ty,
            "#[tool] functions cannot return `impl Trait`; name the type",
        ));
    }
    Ok(())
}

/// Classifies parameters and strips the attributes the compiler would reject.
fn collect_params(func: &mut ItemFn) -> syn::Result<Vec<Param>> {
    let mut params = Vec::new();
    let mut errors: Option<Error> = None;
    let mut push_error = |e: Error| match &mut errors {
        Some(existing) => existing.combine(e),
        None => errors = Some(e),
    };

    for input in &mut func.sig.inputs {
        let pat_type = match input {
            FnArg::Receiver(receiver) => {
                push_error(Error::new_spanned(
                    receiver,
                    "#[tool] must be applied to a free function, not a method taking `self`",
                ));
                continue;
            }
            FnArg::Typed(pat_type) => pat_type,
        };

        let mut forwarded = Vec::new();
        let mut doc_lines = Vec::new();
        for attr in std::mem::take(&mut pat_type.attrs) {
            if attr.path().is_ident("doc") {
                doc_lines.extend(docs::doc_lines(std::slice::from_ref(&attr)));
            } else if attr.path().is_ident("serde") || attr.path().is_ident("schemars") {
                forwarded.push(attr);
            } else {
                push_error(Error::new_spanned(
                    &attr,
                    "only doc comments, #[serde(..)] and #[schemars(..)] are allowed on #[tool] parameters",
                ));
            }
        }

        let ident = match &*pat_type.pat {
            Pat::Ident(pat) if pat.by_ref.is_none() && pat.subpat.is_none() => pat.ident.clone(),
            other => {
                push_error(Error::new_spanned(
                    other,
                    "#[tool] parameters must be plain identifiers",
                ));
                continue;
            }
        };

        match classify(&pat_type.ty) {
            Ok(Some(param)) => {
                if !forwarded.is_empty() || !doc_lines.is_empty() {
                    push_error(Error::new_spanned(
                        &pat_type.ty,
                        "`Ctx` and `State<T>` parameters are not part of the schema and cannot carry attributes",
                    ));
                }
                params.push(param);
            }
            Ok(None) => {
                let doc = doc_lines.join(" ").trim().to_owned();
                params.push(Param::Arg {
                    ident,
                    ty: (*pat_type.ty).clone(),
                    forwarded,
                    doc: (!doc.is_empty()).then_some(doc),
                });
            }
            Err(e) => push_error(e),
        }
    }

    match errors {
        Some(e) => Err(e),
        None => Ok(params),
    }
}

/// `Ok(Some(..))` for injected parameters, `Ok(None)` for JSON arguments.
fn classify(ty: &Type) -> syn::Result<Option<Param>> {
    match ty {
        Type::Reference(r) => Err(Error::new_spanned(
            r,
            "#[tool] parameters must be owned (e.g. `String` instead of `&str`)",
        )),
        Type::ImplTrait(t) => Err(Error::new_spanned(
            t,
            "#[tool] parameters cannot use `impl Trait`",
        )),
        Type::Path(path) if path.qself.is_none() => {
            let Some(last) = path.path.segments.last() else {
                return Ok(None);
            };
            if last.ident == "Ctx" && last.arguments.is_empty() {
                return Ok(Some(Param::Ctx));
            }
            if last.ident == "State" {
                if let PathArguments::AngleBracketed(args) = &last.arguments
                    && let (1, Some(GenericArgument::Type(inner))) =
                        (args.args.len(), args.args.first())
                {
                    return Ok(Some(Param::State(inner.clone())));
                }
                return Err(Error::new_spanned(
                    last,
                    "expected `State<T>` with exactly one type argument",
                ));
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn is_valid_tool_name(name: &str) -> bool {
    (1..=128).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}
