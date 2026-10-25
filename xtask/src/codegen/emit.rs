//! Turns the [`Model`] into Rust tokens.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};

use super::ir::{
    Alias, Field, Item, Marker, Model, Scalar, ScalarUnion, StringEnum, Struct, Ty, Union,
    UnionKind,
};
use super::names;
use crate::revisions::Revision;

pub(crate) fn emit(model: &Model, revision: Revision) -> TokenStream {
    let module_doc = format!(
        " Protocol types for MCP specification revision `{}`.\n\n Generated from the official `schema.json` and `schema.ts` (commit `{}`).",
        revision.name, revision.commit
    );
    let module_doc = doc_lines(&module_doc);
    let version = revision.name;

    let error_codes = model.error_codes.iter().map(|(owner, code)| {
        let name = format_ident!("{}", names::shouty(owner));
        let doc = format!(" Error code of [`{owner}`](super::{owner}).");
        let doc = rust_doc_target(&doc, owner, model);
        quote! { #[doc = #doc] pub const #name: i64 = #code; }
    });
    let methods = model.methods.iter().map(|m| {
        let name = format_ident!("{}", names::shouty(m));
        let doc = format!(" The `{m}` method.");
        quote! { #[doc = #doc] pub const #name: &str = #m; }
    });

    let items = model.items.values().map(|item| match item {
        Item::Struct(s) => emit_struct(s, model),
        Item::Enum(e) => emit_enum(e),
        Item::Marker(m) => emit_marker(m),
        Item::Scalar(s) => emit_scalar(s),
        Item::Union(u) => emit_union(u),
        Item::Alias(a) => emit_alias(a),
    });

    quote! {
        #(#![doc = #module_doc])*

        use serde::{Deserialize, Serialize};

        /// The protocol revision these types describe.
        pub const PROTOCOL_VERSION: &str = #version;

        /// JSON-RPC error codes defined by the schema.
        pub mod error_codes {
            #(#error_codes)*
        }

        /// Method names defined by the schema.
        pub mod methods {
            #(#methods)*
        }

        #(#items)*
    }
}

/// Error-code docs link to the owning type only when it was generated.
fn rust_doc_target(doc: &str, owner: &str, model: &Model) -> String {
    if model.items.contains_key(owner) {
        doc.to_owned()
    } else {
        format!(" Error code of `{owner}`.")
    }
}

fn ident(name: &str) -> syn::Ident {
    if names::is_keyword(name) {
        syn::Ident::new_raw(name, Span::call_site())
    } else {
        syn::Ident::new(name, Span::call_site())
    }
}

fn doc_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| {
            if l.is_empty() || l.starts_with(' ') {
                l.to_owned()
            } else {
                format!(" {l}")
            }
        })
        .collect()
}

fn doc_attrs(doc: Option<&str>) -> TokenStream {
    match doc {
        Some(text) => {
            let lines = doc_lines(text);
            quote! { #(#[doc = #lines])* }
        }
        None => TokenStream::new(),
    }
}

fn deprecated_attr(note: Option<&str>) -> TokenStream {
    match note {
        Some(note) => quote! { #[deprecated(note = #note)] },
        None => TokenStream::new(),
    }
}

fn ty_tokens(ty: &Ty) -> TokenStream {
    match ty {
        Ty::String => quote!(String),
        Ty::I64 => quote!(i64),
        Ty::U64 => quote!(u64),
        Ty::F64 => quote!(f64),
        Ty::Bool => quote!(bool),
        Ty::Value => quote!(serde_json::Value),
        Ty::Object => quote!(serde_json::Map<String, serde_json::Value>),
        Ty::MapOf(t) => {
            let t = ty_tokens(t);
            quote!(std::collections::BTreeMap<String, #t>)
        }
        Ty::Vec(t) => {
            let t = ty_tokens(t);
            quote!(Vec<#t>)
        }
        Ty::Named(n) => {
            let n = ident(n);
            quote!(#n)
        }
        // Validated as a Rust type when the model was built.
        Ty::Path(p) => p
            .parse::<TokenStream>()
            .unwrap_or_else(|_| quote!(compile_error!("invalid type_map path"))),
        Ty::Boxed(t) => {
            let t = ty_tokens(t);
            quote!(Box<#t>)
        }
    }
}

fn emit_struct(s: &Struct, model: &Model) -> TokenStream {
    let name = ident(&s.name);
    let doc = doc_attrs(s.doc.as_deref());
    let deprecated = deprecated_attr(s.deprecated.as_deref());
    let all_defaultable = s
        .fields
        .iter()
        .all(|f| !f.required || model.is_marker(&f.ty));
    let default_derive = if all_defaultable {
        quote!(Default,)
    } else {
        TokenStream::new()
    };

    let fields = s.fields.iter().map(emit_field);
    let extra = if s.open {
        quote! {
            /// Additional properties not described by the schema.
            #[serde(flatten)]
            pub extra: serde_json::Map<String, serde_json::Value>,
        }
    } else {
        TokenStream::new()
    };

    let constructor = emit_constructor(s, model);

    quote! {
        #doc
        #deprecated
        #[derive(Debug, Clone, PartialEq, #default_derive Serialize, Deserialize)]
        pub struct #name {
            #(#fields)*
            #extra
        }
        #constructor
    }
}

fn emit_field(f: &Field) -> TokenStream {
    let name = ident(&f.rust);
    let json = &f.json;
    let doc = doc_attrs(f.doc.as_deref());
    let deprecated = deprecated_attr(f.deprecated.as_deref());
    let ty = ty_tokens(&f.ty);
    if f.required {
        quote! {
            #doc
            #deprecated
            #[serde(rename = #json)]
            pub #name: #ty,
        }
    } else {
        quote! {
            #doc
            #deprecated
            #[serde(rename = #json, default, skip_serializing_if = "Option::is_none")]
            pub #name: Option<#ty>,
        }
    }
}

/// `new` takes every required, non-constant field; the rest are defaulted.
fn emit_constructor(s: &Struct, model: &Model) -> TokenStream {
    let params: Vec<&Field> = s
        .fields
        .iter()
        .filter(|f| f.required && !model.is_marker(&f.ty))
        .collect();
    if params.is_empty() {
        return TokenStream::new();
    }
    let name = ident(&s.name);
    let args = params.iter().map(|f| {
        let n = ident(&f.rust);
        let t = ty_tokens(&f.ty);
        quote!(#n: #t)
    });
    let inits = s.fields.iter().map(|f| {
        let n = ident(&f.rust);
        if !f.required {
            quote!(#n: None)
        } else if model.is_marker(&f.ty) {
            quote!(#n: Default::default())
        } else {
            quote!(#n)
        }
    });
    let extra = if s.open {
        quote!(extra: serde_json::Map::new(),)
    } else {
        TokenStream::new()
    };
    quote! {
        impl #name {
            /// Creates a value from its required fields; optional fields are `None`.
            #[must_use]
            pub fn new(#(#args),*) -> Self {
                Self { #(#inits,)* #extra }
            }
        }
    }
}

fn emit_enum(e: &StringEnum) -> TokenStream {
    let name = ident(&e.name);
    let doc = doc_attrs(e.doc.as_deref());
    let deprecated = deprecated_attr(e.deprecated.as_deref());
    let variants = e.variants.iter().map(|(json, rust)| {
        let v = ident(rust);
        let vdoc = format!(" `\"{json}\"`");
        quote! { #[doc = #vdoc] #[serde(rename = #json)] #v, }
    });
    let as_str = e.variants.iter().map(|(json, rust)| {
        let v = ident(rust);
        quote!(Self::#v => #json,)
    });
    quote! {
        #doc
        #deprecated
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum #name {
            #(#variants)*
        }
        #deprecated
        impl #name {
            /// The wire representation of this value.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self { #(#as_str)* }
            }
        }
    }
}

fn emit_marker(m: &Marker) -> TokenStream {
    let name = ident(&m.name);
    let variant = ident(&m.variant);
    let value = &m.value;
    let doc = format!(" The constant `\"{value}\"`.");
    quote! {
        #[doc = #doc]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
        pub enum #name {
            #[doc = #doc]
            #[default]
            #[serde(rename = #value)]
            #variant,
        }
        impl #name {
            /// The wire representation of this constant.
            pub const VALUE: &'static str = #value;
        }
    }
}

fn emit_scalar(s: &ScalarUnion) -> TokenStream {
    let name = ident(&s.name);
    let doc = doc_attrs(s.doc.as_deref());
    let deprecated = deprecated_attr(s.deprecated.as_deref());
    let has = |k: Scalar| s.kinds.contains(&k);

    let mut variants = Vec::new();
    let mut visits = Vec::new();
    let mut froms = Vec::new();
    let mut expecting = Vec::new();

    if has(Scalar::String) {
        variants.push(quote! { /// A string value.
        String(String), });
        visits.push(quote! {
            fn visit_str<E: serde::de::Error>(self, v: &str) -> ::core::result::Result<Self::Value, E> {
                Ok(#name::String(v.to_owned()))
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> ::core::result::Result<Self::Value, E> {
                Ok(#name::String(v))
            }
        });
        froms.push(quote! {
            impl From<String> for #name { fn from(v: String) -> Self { Self::String(v) } }
            impl From<&str> for #name { fn from(v: &str) -> Self { Self::String(v.to_owned()) } }
        });
        expecting.push("a string");
    }
    if has(Scalar::Integer) {
        variants.push(quote! { /// An integer value.
        Integer(i64), });
        visits.push(quote! {
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> ::core::result::Result<Self::Value, E> {
                Ok(#name::Integer(v))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> ::core::result::Result<Self::Value, E> {
                i64::try_from(v)
                    .map(#name::Integer)
                    .map_err(|_| E::invalid_value(serde::de::Unexpected::Unsigned(v), &self))
            }
        });
        if !has(Scalar::Number) {
            // Integral floats (`1.0`) are accepted as integers within the
            // range exactly representable by an IEEE-754 double.
            visits.push(quote! {
                #[allow(clippy::cast_possible_truncation, clippy::float_cmp)]
                fn visit_f64<E: serde::de::Error>(self, v: f64) -> ::core::result::Result<Self::Value, E> {
                    const MAX_SAFE: f64 = 9_007_199_254_740_991.0;
                    if v.fract() == 0.0 && (-MAX_SAFE..=MAX_SAFE).contains(&v) {
                        Ok(#name::Integer(v as i64))
                    } else {
                        Err(E::invalid_type(serde::de::Unexpected::Float(v), &self))
                    }
                }
            });
        }
        froms.push(quote! {
            impl From<i64> for #name { fn from(v: i64) -> Self { Self::Integer(v) } }
        });
        expecting.push("an integer");
    }
    if has(Scalar::Boolean) {
        variants.push(quote! { /// A boolean value.
        Boolean(bool), });
        visits.push(quote! {
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> ::core::result::Result<Self::Value, E> {
                Ok(#name::Boolean(v))
            }
        });
        froms.push(quote! {
            impl From<bool> for #name { fn from(v: bool) -> Self { Self::Boolean(v) } }
        });
        expecting.push("a boolean");
    }
    if has(Scalar::Number) {
        variants.push(quote! { /// A number value.
        Number(f64), });
        visits.push(quote! {
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> ::core::result::Result<Self::Value, E> {
                Ok(#name::Number(v))
            }
        });
        if !has(Scalar::Integer) {
            visits.push(quote! {
                #[allow(clippy::cast_precision_loss)]
                fn visit_i64<E: serde::de::Error>(self, v: i64) -> ::core::result::Result<Self::Value, E> {
                    Ok(#name::Number(v as f64))
                }
                #[allow(clippy::cast_precision_loss)]
                fn visit_u64<E: serde::de::Error>(self, v: u64) -> ::core::result::Result<Self::Value, E> {
                    Ok(#name::Number(v as f64))
                }
            });
        }
        expecting.push("a number");
    }
    let expecting = expecting.join(" or ");
    // Floats rule out `Eq`/`Hash`; ids and tokens need them as map keys.
    let eq_hash = if has(Scalar::Number) {
        TokenStream::new()
    } else {
        quote!(Eq, Hash,)
    };

    quote! {
        #doc
        #deprecated
        #[derive(Debug, Clone, PartialEq, #eq_hash Serialize)]
        #[serde(untagged)]
        pub enum #name {
            #(#variants)*
        }
        #(#froms)*
        impl<'de> Deserialize<'de> for #name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> ::core::result::Result<Self, D::Error> {
                struct Visitor;
                impl serde::de::Visitor<'_> for Visitor {
                    type Value = #name;
                    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        f.write_str(#expecting)
                    }
                    #(#visits)*
                }
                deserializer.deserialize_any(Visitor)
            }
        }
    }
}

fn emit_union(u: &Union) -> TokenStream {
    let name = ident(&u.name);
    let doc = doc_attrs(u.doc.as_deref());
    let deprecated = deprecated_attr(u.deprecated.as_deref());
    let variants = u.variants.iter().map(|v| {
        let vn = ident(&v.name);
        let ty = ty_tokens(&v.ty);
        let vdoc = match &v.ty {
            Ty::Named(n) => format!(" The [`{n}`] variant."),
            _ => " An inline variant.".to_owned(),
        };
        quote! { #[doc = #vdoc] #vn(#ty), }
    });

    // `From` for every variant whose type is unique within the union.
    let froms = u.variants.iter().filter_map(|v| {
        let unique = u.variants.iter().filter(|o| o.ty == v.ty).count() == 1;
        if !unique {
            return None;
        }
        let vn = ident(&v.name);
        let ty = ty_tokens(&v.ty);
        Some(quote! {
            impl From<#ty> for #name { fn from(v: #ty) -> Self { Self::#vn(v) } }
        })
    });

    let (derive_de, de_impl) = match &u.kind {
        UnionKind::Untagged => (quote!(Deserialize,), TokenStream::new()),
        UnionKind::Tagged {
            field,
            values,
            default,
            default_on_unknown,
        } => {
            let arms = values.iter().filter_map(|(value, idx)| {
                let v = u.variants.get(*idx)?;
                let vn = ident(&v.name);
                Some(quote! {
                    Some(#value) => serde_json::from_value(value).map(Self::#vn).map_err(D::Error::custom),
                })
            });
            let default_variant = default
                .and_then(|i| u.variants.get(i))
                .map(|v| ident(&v.name));
            let tags: Vec<&String> = values.iter().map(|(v, _)| v).collect();
            let fallback = match (default_variant, default_on_unknown) {
                (Some(dv), true) => quote! {
                    _ => serde_json::from_value(value).map(Self::#dv).map_err(D::Error::custom),
                },
                (Some(dv), false) => quote! {
                    None => serde_json::from_value(value).map(Self::#dv).map_err(D::Error::custom),
                    Some(other) => Err(D::Error::unknown_variant(other, &[#(#tags),*])),
                },
                (None, _) => quote! {
                    None => Err(D::Error::missing_field(#field)),
                    Some(other) => Err(D::Error::unknown_variant(other, &[#(#tags),*])),
                },
            };
            let de_impl = quote! {
                impl<'de> Deserialize<'de> for #name {
                    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> ::core::result::Result<Self, D::Error> {
                        use serde::de::Error as _;
                        let value = serde_json::Value::deserialize(deserializer)?;
                        let tag = value.get(#field).and_then(serde_json::Value::as_str).map(str::to_owned);
                        match tag.as_deref() {
                            #(#arms)*
                            #fallback
                        }
                    }
                }
            };
            (TokenStream::new(), de_impl)
        }
    };

    quote! {
        #doc
        #deprecated
        #[derive(Debug, Clone, PartialEq, Serialize, #derive_de)]
        #[serde(untagged)]
        pub enum #name {
            #(#variants)*
        }
        #(#froms)*
        #de_impl
    }
}

fn emit_alias(a: &Alias) -> TokenStream {
    let name = ident(&a.name);
    let doc = doc_attrs(a.doc.as_deref());
    let deprecated = deprecated_attr(a.deprecated.as_deref());
    let ty = ty_tokens(&a.ty);
    quote! {
        #doc
        #deprecated
        pub type #name = #ty;
    }
}
