//! Signature checks and parameter classification shared by `#[tool]`,
//! `#[resource]` and `#[prompt]`.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Error, Expr, ExprArray, ExprLit, FnArg, GenericArgument, Ident, ItemFn, Lit, LitStr,
    Pat, PathArguments, ReturnType, Token, Type,
};

use crate::docs;

/// Root of every generated path, so user crates need no direct dependency
/// on serde, schemars or serde_json.
pub(crate) fn krate() -> TokenStream {
    quote!(::sdkmcprust::__private)
}

/// A parameter of a handler function.
pub(crate) enum Param {
    /// A client-provided value (JSON argument, prompt argument or URI
    /// variable).
    Arg(Arg),
    /// The request context.
    Ctx,
    /// Shared state of the inner type.
    State(Type),
}

pub(crate) struct Arg {
    pub(crate) ident: Ident,
    pub(crate) ty: Type,
    /// `#[serde]` / `#[schemars]` attributes (tools only).
    pub(crate) forwarded: Vec<Attribute>,
    /// Description from a doc comment on the parameter.
    pub(crate) doc: Option<String>,
    /// Completion provider (prompts and resource templates only).
    pub(crate) complete: Option<Complete>,
}

impl Arg {
    /// The argument name as seen by clients (raw identifiers unescaped).
    pub(crate) fn name(&self) -> String {
        self.ident.to_string().trim_start_matches("r#").to_owned()
    }
}

/// `#[complete(path)]` or `#[complete(values = ["a", "b"])]`.
pub(crate) enum Complete {
    Path(syn::Path),
    Values(Vec<LitStr>),
}

impl Parse for Complete {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        if input.peek(syn::Ident) && input.peek2(Token![=]) {
            let key: Ident = input.parse()?;
            if key != "values" {
                return Err(Error::new(
                    key.span(),
                    "expected `values = [..]` or a function path",
                ));
            }
            input.parse::<Token![=]>()?;
            let array: ExprArray = input.parse()?;
            let mut values = Vec::new();
            for elem in array.elems {
                match elem {
                    Expr::Lit(ExprLit {
                        lit: Lit::Str(lit), ..
                    }) => values.push(lit),
                    other => {
                        return Err(Error::new_spanned(other, "expected a string literal"));
                    }
                }
            }
            Ok(Self::Values(values))
        } else {
            Ok(Self::Path(input.parse()?))
        }
    }
}

/// What a macro accepts on parameters.
pub(crate) struct Options {
    pub(crate) macro_name: &'static str,
    /// `#[serde(..)]` / `#[schemars(..)]` are forwarded to a JSON schema.
    pub(crate) schema_attrs: bool,
    /// `#[complete(..)]` is accepted.
    pub(crate) complete: bool,
}

pub(crate) fn check_signature(func: &ItemFn, macro_name: &str) -> syn::Result<()> {
    let sig = &func.sig;
    if !sig.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &sig.generics,
            format!("#[{macro_name}] functions cannot be generic"),
        ));
    }
    if let Some(variadic) = &sig.variadic {
        return Err(Error::new_spanned(
            variadic,
            format!("#[{macro_name}] functions cannot be variadic"),
        ));
    }
    if let Some(abi) = &sig.abi {
        return Err(Error::new_spanned(
            abi,
            format!("#[{macro_name}] functions cannot be `extern`"),
        ));
    }
    if let syn::Safety::Unsafe(token) = &sig.safety {
        return Err(Error::new_spanned(
            token,
            format!("#[{macro_name}] functions cannot be `unsafe`"),
        ));
    }
    if let ReturnType::Type(_, ty) = &sig.output
        && let Type::ImplTrait(_) = &**ty
    {
        return Err(Error::new_spanned(
            ty,
            format!("#[{macro_name}] functions cannot return `impl Trait`; name the type"),
        ));
    }
    Ok(())
}

/// Classifies parameters and strips the attributes the compiler would reject.
pub(crate) fn collect_params(func: &mut ItemFn, opts: &Options) -> syn::Result<Vec<Param>> {
    let macro_name = opts.macro_name;
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
                    format!(
                        "#[{macro_name}] must be applied to a free function, not a method taking `self`"
                    ),
                ));
                continue;
            }
            FnArg::Typed(pat_type) => pat_type,
        };

        let mut forwarded = Vec::new();
        let mut doc_lines = Vec::new();
        let mut complete = None;
        for attr in std::mem::take(&mut pat_type.attrs) {
            if attr.path().is_ident("doc") {
                doc_lines.extend(docs::doc_lines(std::slice::from_ref(&attr)));
            } else if opts.schema_attrs
                && (attr.path().is_ident("serde") || attr.path().is_ident("schemars"))
            {
                forwarded.push(attr);
            } else if opts.complete && attr.path().is_ident("complete") {
                if complete.is_some() {
                    push_error(Error::new_spanned(&attr, "duplicate #[complete(..)]"));
                    continue;
                }
                match attr.parse_args::<Complete>() {
                    Ok(c) => complete = Some(c),
                    Err(e) => push_error(e),
                }
            } else {
                let allowed = if opts.schema_attrs {
                    "doc comments, #[serde(..)] and #[schemars(..)]"
                } else {
                    "doc comments and #[complete(..)]"
                };
                push_error(Error::new_spanned(
                    &attr,
                    format!("only {allowed} are allowed on #[{macro_name}] parameters"),
                ));
            }
        }

        let ident = match &*pat_type.pat {
            Pat::Ident(pat) if pat.by_ref.is_none() && pat.subpat.is_none() => pat.ident.clone(),
            other => {
                push_error(Error::new_spanned(
                    other,
                    format!("#[{macro_name}] parameters must be plain identifiers"),
                ));
                continue;
            }
        };

        match classify(&pat_type.ty, macro_name) {
            Ok(Some(param)) => {
                if !forwarded.is_empty() || !doc_lines.is_empty() || complete.is_some() {
                    push_error(Error::new_spanned(
                        &pat_type.ty,
                        "`Ctx` and `State<T>` parameters are not part of the schema and cannot carry attributes",
                    ));
                }
                params.push(param);
            }
            Ok(None) => {
                let doc = doc_lines.join(" ").trim().to_owned();
                params.push(Param::Arg(Arg {
                    ident,
                    ty: (*pat_type.ty).clone(),
                    forwarded,
                    doc: (!doc.is_empty()).then_some(doc),
                    complete,
                }));
            }
            Err(e) => push_error(e),
        }
    }

    match errors {
        Some(e) => Err(e),
        None => Ok(params),
    }
}

/// `Ok(Some(..))` for injected parameters, `Ok(None)` for client values.
fn classify(ty: &Type, macro_name: &str) -> syn::Result<Option<Param>> {
    match ty {
        Type::Reference(r) => Err(Error::new_spanned(
            r,
            format!("#[{macro_name}] parameters must be owned (e.g. `String` instead of `&str`)"),
        )),
        Type::ImplTrait(t) => Err(Error::new_spanned(
            t,
            format!("#[{macro_name}] parameters cannot use `impl Trait`"),
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

/// The `T` of an `Option<T>` type, recognized syntactically.
pub(crate) fn option_inner(ty: &Type) -> Option<&Type> {
    let Type::Path(path) = ty else { return None };
    if path.qself.is_some() {
        return None;
    }
    let last = path.path.segments.last()?;
    if last.ident != "Option" {
        return None;
    }
    match &last.arguments {
        PathArguments::AngleBracketed(args) if args.args.len() == 1 => match args.args.first() {
            Some(GenericArgument::Type(inner)) => Some(inner),
            _ => None,
        },
        _ => None,
    }
}

/// Description of `arg`: its own doc comment, else the `# Arguments` entry.
pub(crate) fn arg_doc(
    arg: &Arg,
    arg_docs: &std::collections::BTreeMap<String, String>,
) -> Option<String> {
    arg.doc
        .clone()
        .or_else(|| arg_docs.get(&arg.ident.to_string()).cloned())
}

/// Rejects `# Arguments` entries that do not name a client value.
pub(crate) fn check_documented_args(
    params: &[Param],
    arg_docs: &std::collections::BTreeMap<String, String>,
    fn_ident: &Ident,
    what: &str,
) -> syn::Result<()> {
    for documented in arg_docs.keys() {
        let known = params
            .iter()
            .any(|p| matches!(p, Param::Arg(a) if a.ident == documented.as_str()));
        if !known {
            return Err(Error::new(
                fn_ident.span(),
                format!("`# Arguments` documents `{documented}`, which is not {what}"),
            ));
        }
    }
    Ok(())
}

/// Generated code that extracts `State<T>` parameters from `__ctx`, plus the
/// state types to declare.
pub(crate) struct Injected {
    pub(crate) extract: Vec<TokenStream>,
    pub(crate) states: Vec<Type>,
}

/// Builds the call arguments: client values by identifier, `Ctx` cloned from
/// `__ctx`, `State<T>` extracted from it.
pub(crate) fn call_args(params: &[Param]) -> (Vec<TokenStream>, Injected) {
    let krate = krate();
    let mut args = Vec::new();
    let mut injected = Injected {
        extract: Vec::new(),
        states: Vec::new(),
    };
    for (i, param) in params.iter().enumerate() {
        match param {
            Param::Arg(arg) => {
                let ident = &arg.ident;
                args.push(quote!(#ident));
            }
            Param::Ctx => args.push(quote!(::core::clone::Clone::clone(&__ctx))),
            Param::State(inner) => {
                let var = format_ident!("__state{}", i);
                injected.extract.push(quote! {
                    let #var = __ctx
                        .state::<#inner>()
                        .ok_or_else(#krate::missing_state)?;
                });
                args.push(quote!(#var));
                injected.states.push(inner.clone());
            }
        }
    }
    (args, injected)
}

/// Completion functions for arguments with `#[complete]`, and the
/// `.complete(..)` calls that register them.
pub(crate) fn completers(params: &[Param]) -> (Vec<TokenStream>, Vec<TokenStream>) {
    let krate = krate();
    let mut fns = Vec::new();
    let mut registrations = Vec::new();
    for param in params {
        let Param::Arg(arg) = param else { continue };
        let Some(complete) = &arg.complete else {
            continue;
        };
        let fn_ident = format_ident!("__complete_{}", arg.name());
        let body = match complete {
            Complete::Path(path) => quote! {
                #krate::Completion::new(#path(request.value()))
            },
            Complete::Values(values) => quote! {
                #krate::Completion::filter_prefix(&[#(#values),*], request.value())
            },
        };
        fns.push(quote! {
            #[allow(non_snake_case)]
            fn #fn_ident(request: &#krate::CompletionRequest<'_>) -> #krate::Completion {
                #body
            }
        });
        let name = arg.name();
        registrations.push(quote!(.complete(#name, #fn_ident)));
    }
    (fns, registrations)
}

/// Parses `key = value` arguments after an optional leading string literal.
pub(crate) struct KeyValues {
    pub(crate) positional: Option<LitStr>,
    pub(crate) pairs: Vec<(Ident, Expr)>,
}

impl Parse for KeyValues {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let positional = if input.peek(LitStr) {
            let lit = input.parse()?;
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
            Some(lit)
        } else {
            None
        };
        let pairs = Punctuated::<KeyValue, Token![,]>::parse_terminated(input)?
            .into_iter()
            .map(|kv| (kv.key, kv.value))
            .collect();
        Ok(Self { positional, pairs })
    }
}

struct KeyValue {
    key: Ident,
    value: Expr,
}

impl Parse for KeyValue {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let key: Ident = input.parse()?;
        if !input.peek(Token![=]) {
            // A bare flag, such as `app`, means `app = true`.
            let value = Expr::Lit(ExprLit {
                attrs: Vec::new(),
                lit: Lit::Bool(syn::LitBool::new(true, key.span())),
            });
            return Ok(Self { key, value });
        }
        input.parse::<Token![=]>()?;
        Ok(Self {
            key,
            value: input.parse()?,
        })
    }
}

/// A string literal argument value.
pub(crate) fn lit_str(key: &Ident, value: &Expr) -> syn::Result<LitStr> {
    match value {
        Expr::Lit(ExprLit {
            lit: Lit::Str(lit), ..
        }) => Ok(lit.clone()),
        other => Err(Error::new_spanned(
            other,
            format!("`{key}` expects a string literal"),
        )),
    }
}

/// `scopes = ["a", "b"]`: OAuth scopes a handler needs, checked here so a
/// malformed scope is a compile error.
pub(crate) fn scopes(key: &Ident, value: &Expr) -> syn::Result<Vec<LitStr>> {
    let list = string_list(key, value, "files:read")?;
    let mut scopes = Vec::with_capacity(list.len());
    for lit in list {
        let scope = lit.value();
        let valid = !scope.is_empty()
            && scope
                .bytes()
                .all(|b| matches!(b, 0x21 | 0x23..=0x5B | 0x5D..=0x7E));
        if !valid {
            return Err(Error::new(
                lit.span(),
                "invalid OAuth scope: use visible ASCII except `\"` and `\\`, without spaces",
            ));
        }
        scopes.push(lit);
    }
    Ok(scopes)
}

/// `key = ["a", "b"]`: an array of string literals.
pub(crate) fn string_list(key: &Ident, value: &Expr, example: &str) -> syn::Result<Vec<LitStr>> {
    let Expr::Array(ExprArray { elems, .. }) = value else {
        return Err(Error::new_spanned(
            value,
            format!("`{key}` expects an array of strings, e.g. `{key} = [\"{example}\"]`"),
        ));
    };
    elems.iter().map(|elem| lit_str(key, elem)).collect()
}

/// A boolean literal argument value.
pub(crate) fn lit_bool(key: &Ident, value: &Expr) -> syn::Result<syn::LitBool> {
    match value {
        Expr::Lit(ExprLit {
            lit: Lit::Bool(lit),
            ..
        }) => Ok(lit.clone()),
        other => Err(Error::new_spanned(
            other,
            format!("`{key}` expects `true` or `false`"),
        )),
    }
}

/// The `.scopes([..])` call for declared scopes (nothing when none).
pub(crate) fn scopes_call(scopes: Option<&Vec<LitStr>>) -> TokenStream {
    match scopes {
        Some(scopes) if !scopes.is_empty() => quote! { .scopes([#(#scopes),*]) },
        _ => TokenStream::new(),
    }
}

/// Stores `value` in `slot`, rejecting duplicates.
pub(crate) fn set_once<T>(slot: &mut Option<T>, key: &Ident, value: T) -> syn::Result<()> {
    if slot.is_some() {
        return Err(Error::new(key.span(), "duplicate argument"));
    }
    *slot = Some(value);
    Ok(())
}
