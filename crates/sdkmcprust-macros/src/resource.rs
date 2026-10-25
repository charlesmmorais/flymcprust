//! Expansion of `#[resource]`.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Error, Expr, ExprLit, Ident, ItemFn, Lit, LitInt, LitStr};

use crate::docs;
use crate::params::{self, KeyValues, Options, Param};

const OPTIONS: Options = Options {
    macro_name: "resource",
    schema_attrs: false,
    complete: true,
};

#[derive(Default)]
struct ResourceArgs {
    uri: Option<LitStr>,
    template: Option<LitStr>,
    name: Option<LitStr>,
    title: Option<LitStr>,
    description: Option<LitStr>,
    mime_type: Option<LitStr>,
    ttl_ms: Option<LitInt>,
    cache: Option<LitStr>,
}

fn parse_args(attr: TokenStream) -> syn::Result<ResourceArgs> {
    let kv: KeyValues = syn::parse2(attr)?;
    let mut args = ResourceArgs {
        uri: kv.positional,
        ..ResourceArgs::default()
    };
    for (key, value) in &kv.pairs {
        let k = key.to_string();
        match k.as_str() {
            "template" => params::set_once(&mut args.template, key, params::lit_str(key, value)?)?,
            "name" => params::set_once(&mut args.name, key, params::lit_str(key, value)?)?,
            "title" => params::set_once(&mut args.title, key, params::lit_str(key, value)?)?,
            "description" => {
                params::set_once(&mut args.description, key, params::lit_str(key, value)?)?;
            }
            "mime_type" => {
                params::set_once(&mut args.mime_type, key, params::lit_str(key, value)?)?;
            }
            "ttl_ms" => match value {
                Expr::Lit(ExprLit {
                    lit: Lit::Int(lit), ..
                }) => {
                    lit.base10_parse::<u64>()?;
                    params::set_once(&mut args.ttl_ms, key, lit.clone())?;
                }
                other => {
                    return Err(Error::new_spanned(
                        other,
                        "`ttl_ms` expects an integer literal",
                    ));
                }
            },
            "cache" => {
                let lit = params::lit_str(key, value)?;
                if !matches!(lit.value().as_str(), "public" | "private") {
                    return Err(Error::new(
                        lit.span(),
                        "`cache` must be \"public\" or \"private\"",
                    ));
                }
                params::set_once(&mut args.cache, key, lit)?;
            }
            _ => {
                return Err(Error::new(
                    key.span(),
                    "unknown `resource` argument; expected a URI string, `template`, `name`, \
                     `title`, `description`, `mime_type`, `ttl_ms` or `cache`",
                ));
            }
        }
    }
    match (&args.uri, &args.template) {
        (Some(_), Some(t)) => Err(Error::new(
            t.span(),
            "give either a URI or `template = \"..\"`, not both",
        )),
        (None, None) => Err(Error::new(
            proc_macro2::Span::call_site(),
            "expected a URI, e.g. #[resource(\"config://app\")], or a template, e.g. \
             #[resource(template = \"file:///{path}\")]",
        )),
        _ => Ok(args),
    }
}

/// A variable of a URI template, as seen by the macro.
struct TemplateVar {
    name: String,
    query: bool,
}

/// Extracts the variables of a URI template, rejecting syntax the runtime
/// does not support. The runtime re-validates the full template when the
/// server is built.
fn template_vars(template: &LitStr) -> syn::Result<Vec<TemplateVar>> {
    let text = template.value();
    let fail = |msg: &str| Error::new(template.span(), format!("invalid URI template: {msg}"));
    let mut vars = Vec::new();
    let mut rest = text.as_str();
    while let Some(open) = rest.find('{') {
        let after = rest.get(open + 1..).unwrap_or_default();
        let close = after.find('}').ok_or_else(|| fail("unclosed `{`"))?;
        let expr = after.get(..close).unwrap_or_default();
        rest = after.get(close + 1..).unwrap_or_default();
        let (query, list) = match expr.chars().next() {
            Some('?' | '&') => (true, expr.get(1..).unwrap_or_default()),
            Some('+' | '/') => (false, expr.get(1..).unwrap_or_default()),
            Some(c) if c.is_ascii_alphanumeric() || c == '_' => (false, expr),
            _ => {
                return Err(fail(
                    "only `{var}`, `{+var}`, `{/var}` and `{?var}` expressions are supported",
                ));
            }
        };
        for name in list.split(',') {
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                return Err(fail(
                    "variable names use only A-Z, a-z, 0-9 and `_` (no modifiers)",
                ));
            }
            vars.push(TemplateVar {
                name: name.to_owned(),
                query,
            });
        }
    }
    if text.starts_with('{') {
        return Err(fail("the template must start with a literal scheme"));
    }
    Ok(vars)
}

fn is_absolute_uri(uri: &str) -> bool {
    let Some((scheme, rest)) = uri.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && !rest.is_empty()
        && !uri.chars().any(|c| c.is_control() || c.is_whitespace())
}

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let args = parse_args(attr)?;
    let mut func: ItemFn = syn::parse2(item)?;
    params::check_signature(&func, OPTIONS.macro_name)?;
    let params = params::collect_params(&mut func, &OPTIONS)?;
    let krate = params::krate();
    let fn_ident = &func.sig.ident;
    let vis = &func.vis;

    let template_vars = match &args.template {
        Some(t) => template_vars(t)?,
        None => Vec::new(),
    };
    if let Some(uri) = &args.uri
        && !is_absolute_uri(&uri.value())
    {
        return Err(Error::new(
            uri.span(),
            "expected an absolute URI such as `config://app` (use `template = \"..\"` for URI templates)",
        ));
    }

    // Every client value must be a template variable, and vice versa.
    let mut extract_vars = Vec::new();
    for param in &params {
        let Param::Arg(arg) = param else { continue };
        let name = arg.name();
        let Some(var) = template_vars.iter().find(|v| v.name == name) else {
            let msg = if args.template.is_some() {
                format!("parameter `{name}` is not a variable of the URI template")
            } else {
                "a static #[resource] takes no parameters besides `Ctx` and `State<T>`; \
                 use `template = \"..\"` for parameterized URIs"
                    .to_owned()
            };
            return Err(Error::new_spanned(&arg.ident, msg));
        };
        if arg.complete.is_some() && args.template.is_none() {
            return Err(Error::new_spanned(
                &arg.ident,
                "#[complete] is only meaningful for template variables",
            ));
        }
        let ident = &arg.ident;
        let ty = &arg.ty;
        let parse = match (params::option_inner(ty), var.query) {
            (Some(inner), true) => quote! {
                let #ident: #ty = #krate::parse_optional_uri_var::<#inner>(__vars, #name)?;
            },
            (None, false) => quote! {
                let #ident: #ty = #krate::parse_uri_var::<#ty>(__vars, #name)?;
            },
            (Some(_), false) => {
                return Err(Error::new_spanned(
                    ty,
                    "path variables are always present; use a non-`Option` type",
                ));
            }
            (None, true) => {
                return Err(Error::new_spanned(
                    ty,
                    "query variables are optional; use `Option<T>`",
                ));
            }
        };
        extract_vars.push(parse);
    }
    for var in &template_vars {
        let has_param = params
            .iter()
            .any(|p| matches!(p, Param::Arg(a) if a.name() == var.name));
        if !has_param {
            return Err(Error::new(
                fn_ident.span(),
                format!(
                    "URI template variable `{}` has no matching parameter",
                    var.name
                ),
            ));
        }
    }

    let (doc_description, arg_docs) = docs::split(&docs::doc_lines(&func.attrs));
    if let Some(documented) = arg_docs.keys().next() {
        return Err(Error::new(
            fn_ident.span(),
            format!(
                "`# Arguments` documents `{documented}`, but resources have no arguments; describe template variables in the main text"
            ),
        ));
    }
    let description = args
        .description
        .as_ref()
        .map(LitStr::value)
        .or(doc_description);

    let (call_args, injected) = params::call_args(&params);
    let extract = &injected.extract;
    let state_types = &injected.states;
    let (complete_fns, complete_regs) = params::completers(&params);

    let call = if func.sig.asyncness.is_some() {
        quote!(#fn_ident(#(#call_args),*).await)
    } else {
        quote!(#fn_ident(#(#call_args),*))
    };

    let name = match &args.name {
        Some(lit) => lit.value(),
        None => fn_ident.to_string().trim_start_matches("r#").to_owned(),
    };
    let opt = |lit: &Option<LitStr>| match lit {
        Some(l) => quote!(::core::option::Option::Some(::std::string::String::from(#l))),
        None => quote!(::core::option::Option::None),
    };
    let description = match description {
        Some(d) => quote!(::core::option::Option::Some(::std::string::String::from(#d))),
        None => quote!(::core::option::Option::None),
    };
    let title = opt(&args.title);
    let mime_type = opt(&args.mime_type);
    let mime_ref = match &args.mime_type {
        Some(l) => quote!(::core::option::Option::Some(#l)),
        None => quote!(::core::option::Option::None),
    };

    let definition = if let Some(uri) = &args.uri {
        quote! {
            let mut resource = #krate::spec::Resource::new(
                ::std::string::String::from(#name),
                ::std::string::String::from(#uri),
            );
            resource.description = #description;
            resource.title = #title;
            resource.mime_type = #mime_type;
            #krate::ResourceDef::new(resource, __read)
        }
    } else {
        let template = args.template.as_ref();
        quote! {
            let mut template = #krate::spec::ResourceTemplate::new(
                ::std::string::String::from(#name),
                ::std::string::String::from(#template),
            );
            template.description = #description;
            template.title = #title;
            template.mime_type = #mime_type;
            #krate::ResourceDef::template(template, __read)
        }
    };
    let ttl = args
        .ttl_ms
        .as_ref()
        .map(|ms| quote!(.ttl(::core::time::Duration::from_millis(#ms))));
    let cache = args.cache.as_ref().map(|c| {
        let variant = if c.value() == "public" {
            quote!(Public)
        } else {
            quote!(Private)
        };
        quote!(.cache_scope(#krate::spec::CacheScope::#variant))
    });

    let auto_register = auto_register(fn_ident);
    let marker_doc =
        format!(" MCP resource marker for [`{fn_ident}()`], generated by `#[resource]`.");

    Ok(quote! {
        #func

        #[doc = #marker_doc]
        #[allow(non_camel_case_types)]
        #vis struct #fn_ident {}

        impl #krate::Resource for #fn_ident {
            fn definition() -> #krate::ResourceDef {
                #[allow(unused_variables, clippy::needless_pass_by_value)]
                fn __read(
                    __uri: &str,
                    __vars: &#krate::UriVars,
                    __ctx: #krate::Ctx,
                ) -> ::core::result::Result<#krate::ResourceFuture, #krate::ToolError> {
                    #(#extract_vars)*
                    #(#extract)*
                    let __uri = ::std::borrow::ToOwned::to_owned(__uri);
                    ::core::result::Result::Ok(::std::boxed::Box::pin(async move {
                        #krate::IntoResourceContents::into_contents(#call, &__uri, #mime_ref)
                    }))
                }
                #(#complete_fns)*

                { #definition }
                    #ttl
                    #cache
                    #(.require_state::<#state_types>())*
                    #(#complete_regs)*
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
            #krate::AutoResource::new(<#fn_ident as #krate::Resource>::definition)
        }
    }
}

#[cfg(not(feature = "auto-register"))]
fn auto_register(_: &Ident) -> TokenStream {
    TokenStream::new()
}
