//! Intermediate representation of the generated module, built from the JSON
//! Schema `$defs`.
//!
//! Mapping rules:
//! - object with `properties` → [`Struct`] (open, with an `extra` map, when the
//!   schema allows additional properties or the overrides say so);
//! - string `enum` → [`StringEnum`]; string `const` → zero-sized [`Marker`];
//! - `type: [..]` of scalars → [`ScalarUnion`];
//! - `anyOf` → [`Union`], tagged by a constant field shared by every variant
//!   when one exists, by `resultType` for `InputRequiredResult` unions, and
//!   untagged otherwise;
//! - `allOf` → a [`Struct`] merging every member's properties;
//! - everything else → [`Alias`].

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};

use super::deprecations::Deprecations;
use super::docs;
use super::names;
use super::overrides::Overrides;

/// A Rust type expression.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Ty {
    String,
    I64,
    U64,
    F64,
    Bool,
    /// `serde_json::Value`.
    Value,
    /// `serde_json::Map<String, serde_json::Value>`.
    Object,
    /// `BTreeMap<String, T>`.
    MapOf(Box<Ty>),
    Vec(Box<Ty>),
    Named(String),
    /// A Rust path from the overrides (already validated).
    Path(String),
    Boxed(Box<Ty>),
}

#[derive(Debug)]
pub(crate) struct Field {
    pub(crate) rust: String,
    pub(crate) json: String,
    pub(crate) ty: Ty,
    pub(crate) required: bool,
    pub(crate) doc: Option<String>,
    pub(crate) deprecated: Option<String>,
}

#[derive(Debug)]
pub(crate) struct Struct {
    pub(crate) name: String,
    pub(crate) doc: Option<String>,
    pub(crate) fields: Vec<Field>,
    pub(crate) open: bool,
    pub(crate) deprecated: Option<String>,
}

#[derive(Debug)]
pub(crate) struct StringEnum {
    pub(crate) name: String,
    pub(crate) doc: Option<String>,
    /// `(json value, Rust variant)`.
    pub(crate) variants: Vec<(String, String)>,
    pub(crate) deprecated: Option<String>,
}

#[derive(Debug)]
pub(crate) struct Marker {
    pub(crate) name: String,
    pub(crate) value: String,
    pub(crate) variant: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scalar {
    String,
    Integer,
    Boolean,
    Number,
}

#[derive(Debug)]
pub(crate) struct ScalarUnion {
    pub(crate) name: String,
    pub(crate) doc: Option<String>,
    pub(crate) kinds: Vec<Scalar>,
    pub(crate) deprecated: Option<String>,
}

#[derive(Debug)]
pub(crate) struct Variant {
    pub(crate) name: String,
    pub(crate) ty: Ty,
}

#[derive(Debug)]
pub(crate) enum UnionKind {
    Tagged {
        field: String,
        /// `(tag value, variant index)`.
        values: Vec<(String, usize)>,
        /// Variant used when the tag is absent.
        default: Option<usize>,
        /// Whether unknown tag values also select the default variant.
        default_on_unknown: bool,
    },
    Untagged,
}

#[derive(Debug)]
pub(crate) struct Union {
    pub(crate) name: String,
    pub(crate) doc: Option<String>,
    pub(crate) variants: Vec<Variant>,
    pub(crate) kind: UnionKind,
    pub(crate) deprecated: Option<String>,
}

#[derive(Debug)]
pub(crate) struct Alias {
    pub(crate) name: String,
    pub(crate) doc: Option<String>,
    pub(crate) ty: Ty,
    pub(crate) deprecated: Option<String>,
}

#[derive(Debug)]
pub(crate) enum Item {
    Struct(Struct),
    Enum(StringEnum),
    Marker(Marker),
    Scalar(ScalarUnion),
    Union(Union),
    Alias(Alias),
}

/// The whole generated module.
#[derive(Debug, Default)]
pub(crate) struct Model {
    pub(crate) items: BTreeMap<String, Item>,
    /// JSON-RPC error codes, by owning definition name.
    pub(crate) error_codes: BTreeMap<String, i64>,
    /// Every method name that appears as a constant.
    pub(crate) methods: BTreeSet<String>,
}

impl Model {
    pub(crate) fn is_marker(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Named(n) if matches!(self.items.get(n), Some(Item::Marker(_))))
    }
}

pub(crate) fn build(schema: &Value, ov: &Overrides, dep: &Deprecations) -> Result<Model> {
    let defs = schema
        .get("$defs")
        .and_then(Value::as_object)
        .context("schema has no `$defs`")?;

    for name in ov
        .skip
        .iter()
        .chain(ov.open.iter())
        .chain(ov.type_map.keys())
    {
        if !defs.contains_key(name) {
            bail!("override refers to unknown definition `{name}`");
        }
    }
    for (name, path) in &ov.type_map {
        syn::parse_str::<syn::Type>(path)
            .with_context(|| format!("type_map entry for `{name}` is not a Rust type: {path}"))?;
    }

    let mut b = Builder {
        defs,
        ov,
        dep,
        model: Model::default(),
        markers: BTreeMap::new(),
        enums: BTreeMap::new(),
        owner: String::new(),
    };

    for (name, def) in defs {
        if ov.skip.contains(name) || ov.type_map.contains_key(name) {
            continue;
        }
        b.owner.clone_from(name);
        let rust = b.def_name(name);
        let doc = description(def);
        let deprecated = dep.types.get(name).cloned();
        b.build_named(&rust, name, def, doc, deprecated)
            .with_context(|| format!("in definition `{name}`"))?;
    }

    b.check_refs()?;
    Ok(b.model)
}

struct Builder<'a> {
    defs: &'a Map<String, Value>,
    ov: &'a Overrides,
    dep: &'a Deprecations,
    model: Model,
    /// `(field, value)` → marker type name.
    markers: BTreeMap<(String, String), String>,
    /// `(field, values)` → enum type name.
    enums: BTreeMap<(String, Vec<String>), String>,
    /// JSON name of the definition being built.
    owner: String,
}

impl<'a> Builder<'a> {
    fn def_name(&self, json: &str) -> String {
        names::type_name(json, &self.ov.acronyms)
    }

    fn insert(&mut self, name: &str, item: Item) -> Result<()> {
        if self.model.items.contains_key(name) {
            bail!("two generated items are named `{name}`; add an override");
        }
        self.model.items.insert(name.to_owned(), item);
        Ok(())
    }

    fn build_named(
        &mut self,
        rust: &str,
        path: &str,
        schema: &Value,
        doc: Option<String>,
        deprecated: Option<String>,
    ) -> Result<()> {
        if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
            let ty = self.resolve_ref(r)?;
            return self.insert(
                rust,
                Item::Alias(Alias {
                    name: rust.to_owned(),
                    doc,
                    ty,
                    deprecated,
                }),
            );
        }
        if let Some(list) = schema.get("anyOf").and_then(Value::as_array) {
            return self.build_union(rust, path, list, doc, deprecated);
        }
        if let Some(list) = schema.get("allOf").and_then(Value::as_array) {
            return self.build_all_of(rust, path, list, doc, deprecated);
        }
        if let Some(values) = schema.get("enum").and_then(Value::as_array) {
            let variants = string_values(values)?
                .into_iter()
                .map(|v| (v.clone(), names::pascal(&v)))
                .collect();
            return self.insert(
                rust,
                Item::Enum(StringEnum {
                    name: rust.to_owned(),
                    doc,
                    variants,
                    deprecated,
                }),
            );
        }
        match schema.get("type") {
            Some(Value::Array(types)) => {
                let kinds = scalar_kinds(types)?;
                self.insert(
                    rust,
                    Item::Scalar(ScalarUnion {
                        name: rust.to_owned(),
                        doc,
                        kinds,
                        deprecated,
                    }),
                )
            }
            Some(Value::String(t)) if t == "object" && schema.get("properties").is_some() => {
                let s = self.build_struct(rust, path, schema, doc, deprecated)?;
                self.insert(rust, Item::Struct(s))
            }
            _ => {
                let ty = self.ty_of(schema, path, rust, "")?;
                self.insert(
                    rust,
                    Item::Alias(Alias {
                        name: rust.to_owned(),
                        doc,
                        ty,
                        deprecated,
                    }),
                )
            }
        }
    }

    fn build_struct(
        &mut self,
        rust: &str,
        path: &str,
        schema: &Value,
        doc: Option<String>,
        deprecated: Option<String>,
    ) -> Result<Struct> {
        let empty = Map::new();
        let props = schema
            .get("properties")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let required: BTreeSet<&str> = schema
            .get("required")
            .and_then(Value::as_array)
            .map(|r| r.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let open =
            schema.get("additionalProperties").is_some() || self.ov.open.iter().any(|o| o == path);

        let mut seen = BTreeSet::new();
        if open {
            seen.insert("extra".to_owned());
        }
        let mut fields = Vec::with_capacity(props.len());
        for (json, prop) in props {
            let rust_field = names::field_name(json);
            if !seen.insert(rust_field.clone()) {
                bail!("field name `{rust_field}` is used twice in `{path}`");
            }
            let mut ty = self.ty_of(prop, &format!("{path}.{json}"), rust, json)?;
            if ty == Ty::Named(rust.to_owned()) {
                ty = Ty::Boxed(Box::new(ty));
            }
            fields.push(Field {
                rust: rust_field,
                json: json.clone(),
                ty,
                required: required.contains(json.as_str()),
                doc: description(prop),
                deprecated: self
                    .dep
                    .fields
                    .get(&(path.to_owned(), json.clone()))
                    .cloned(),
            });
        }
        Ok(Struct {
            name: rust.to_owned(),
            doc,
            fields,
            open,
            deprecated,
        })
    }

    fn build_all_of(
        &mut self,
        rust: &str,
        path: &str,
        list: &[Value],
        doc: Option<String>,
        deprecated: Option<String>,
    ) -> Result<()> {
        let mut props = Map::new();
        let mut required = BTreeSet::new();
        for member in list {
            let resolved = match member.get("$ref").and_then(Value::as_str) {
                Some(r) => self.def_json(r)?,
                None => member,
            };
            if let Some(p) = resolved.get("properties").and_then(Value::as_object) {
                for (k, v) in p {
                    props.insert(k.clone(), v.clone());
                }
            }
            if let Some(r) = resolved.get("required").and_then(Value::as_array) {
                required.extend(r.iter().filter_map(Value::as_str).map(str::to_owned));
            }
        }
        let synthetic = serde_json::json!({
            "type": "object",
            "properties": Value::Object(props),
            "required": required.into_iter().collect::<Vec<_>>(),
        });
        let s = self.build_struct(rust, path, &synthetic, doc, deprecated)?;
        self.insert(rust, Item::Struct(s))
    }

    fn build_union(
        &mut self,
        rust: &str,
        path: &str,
        list: &[Value],
        doc: Option<String>,
        deprecated: Option<String>,
    ) -> Result<()> {
        let mut variants = Vec::with_capacity(list.len());
        let mut ref_defs: Vec<Option<&Value>> = Vec::with_capacity(list.len());
        for (i, member) in list.iter().enumerate() {
            if let Some(r) = member.get("$ref").and_then(Value::as_str) {
                let ty = self.resolve_ref(r)?;
                let name = match &ty {
                    Ty::Named(n) => n.clone(),
                    _ => self.def_name(ref_name(r)?),
                };
                variants.push(Variant { name, ty });
                ref_defs.push(Some(self.def_json(r)?));
            } else {
                let vname = match member.get("type") {
                    Some(Value::String(t)) if t == "array" => "List".to_owned(),
                    Some(Value::Array(_)) => "Scalar".to_owned(),
                    Some(Value::String(t)) if t == "object" => "Object".to_owned(),
                    _ => format!("Variant{i}"),
                };
                let ty = self.ty_of(member, &format!("{path}|{vname}"), rust, &vname)?;
                variants.push(Variant { name: vname, ty });
                ref_defs.push(None);
            }
        }

        strip_common_suffix(&mut variants, &ref_defs);
        let mut seen = BTreeSet::new();
        for v in &variants {
            if !seen.insert(v.name.clone()) {
                bail!("variant name `{}` is used twice in union `{rust}`", v.name);
            }
        }

        let kind = detect_tag(&variants, &ref_defs);
        self.insert(
            rust,
            Item::Union(Union {
                name: rust.to_owned(),
                doc,
                variants,
                kind,
                deprecated,
            }),
        )
    }

    /// Maps a property schema to a type, registering inline types as needed.
    fn ty_of(&mut self, p: &Value, path: &str, parent: &str, field: &str) -> Result<Ty> {
        if is_empty_schema(p) {
            return Ok(Ty::Value);
        }
        if let Some(r) = p.get("$ref").and_then(Value::as_str) {
            return self.resolve_ref(r);
        }
        if let Some(c) = p.get("const") {
            return match c {
                Value::String(v) => Ok(Ty::Named(self.marker(field, v)?)),
                Value::Number(n) => {
                    let code = n.as_i64().context("non-integer numeric const")?;
                    if field == "code" {
                        self.model.error_codes.insert(self.owner.clone(), code);
                    }
                    Ok(Ty::I64)
                }
                _ => bail!("unsupported const at `{path}`"),
            };
        }
        let inline_name = self
            .ov
            .inline_names
            .get(path)
            .cloned()
            .unwrap_or_else(|| format!("{parent}{}", names::pascal(&names::field_name(field))));

        if let Some(list) = p.get("anyOf").and_then(Value::as_array) {
            self.build_union(&inline_name, path, list, description(p), None)?;
            return Ok(Ty::Named(inline_name));
        }
        if let Some(list) = p.get("allOf").and_then(Value::as_array) {
            self.build_all_of(&inline_name, path, list, description(p), None)?;
            return Ok(Ty::Named(inline_name));
        }
        if let Some(values) = p.get("enum").and_then(Value::as_array) {
            let values = string_values(values)?;
            let mut key_values = values.clone();
            key_values.sort();
            let key = (field.to_owned(), key_values);
            if let Some(existing) = self.enums.get(&key) {
                return Ok(Ty::Named(existing.clone()));
            }
            let variants = values
                .into_iter()
                .map(|v| (v.clone(), names::pascal(&v)))
                .collect();
            self.insert(
                &inline_name,
                Item::Enum(StringEnum {
                    name: inline_name.clone(),
                    doc: description(p),
                    variants,
                    deprecated: None,
                }),
            )?;
            self.enums.insert(key, inline_name.clone());
            return Ok(Ty::Named(inline_name));
        }

        match p.get("type") {
            Some(Value::Array(types)) => {
                let kinds = scalar_kinds(types)?;
                self.insert(
                    &inline_name,
                    Item::Scalar(ScalarUnion {
                        name: inline_name.clone(),
                        doc: description(p),
                        kinds,
                        deprecated: None,
                    }),
                )?;
                Ok(Ty::Named(inline_name))
            }
            Some(Value::String(t)) => match t.as_str() {
                "string" => Ok(Ty::String),
                "boolean" => Ok(Ty::Bool),
                "number" => Ok(Ty::F64),
                "integer" => {
                    let non_negative = p
                        .get("minimum")
                        .and_then(Value::as_f64)
                        .is_some_and(|m| m >= 0.0);
                    Ok(if non_negative { Ty::U64 } else { Ty::I64 })
                }
                "array" => {
                    let items = p.get("items").context("array without `items`")?;
                    let item_field = format!("{field}Item");
                    let inner = self.ty_of(items, &format!("{path}[]"), parent, &item_field)?;
                    Ok(Ty::Vec(Box::new(inner)))
                }
                "object" => {
                    if p.get("properties").is_some() {
                        let s = self.build_struct(&inline_name, path, p, description(p), None)?;
                        self.insert(&inline_name, Item::Struct(s))?;
                        return Ok(Ty::Named(inline_name));
                    }
                    match p.get("additionalProperties") {
                        None => Ok(Ty::Object),
                        Some(ap) if is_empty_schema(ap) => Ok(Ty::Object),
                        Some(ap) => {
                            let value_field = format!("{field}Value");
                            let inner =
                                self.ty_of(ap, &format!("{path}{{}}"), parent, &value_field)?;
                            Ok(Ty::MapOf(Box::new(inner)))
                        }
                    }
                }
                other => bail!("unsupported type `{other}` at `{path}`"),
            },
            _ => bail!("unsupported schema at `{path}`: {p}"),
        }
    }

    fn marker(&mut self, field: &str, value: &str) -> Result<String> {
        let key = (field.to_owned(), value.to_owned());
        if let Some(existing) = self.markers.get(&key) {
            return Ok(existing.clone());
        }
        let name = self
            .ov
            .marker_names
            .get(&format!("{field}={value}"))
            .cloned()
            .unwrap_or_else(|| {
                format!(
                    "{}{}",
                    names::pascal(value),
                    names::pascal(&names::field_name(field))
                )
            });
        if field == "method" {
            self.model.methods.insert(value.to_owned());
        }
        self.insert(
            &name,
            Item::Marker(Marker {
                name: name.clone(),
                value: value.to_owned(),
                variant: names::pascal(value),
            }),
        )?;
        self.markers.insert(key, name.clone());
        Ok(name)
    }

    fn resolve_ref(&self, r: &str) -> Result<Ty> {
        let name = ref_name(r)?;
        if let Some(path) = self.ov.type_map.get(name) {
            return Ok(Ty::Path(path.clone()));
        }
        if self.ov.skip.iter().any(|s| s == name) {
            bail!("reference to skipped definition `{name}`");
        }
        if !self.defs.contains_key(name) {
            bail!("reference to unknown definition `{name}`");
        }
        Ok(Ty::Named(self.def_name(name)))
    }

    fn def_json(&self, r: &str) -> Result<&'a Value> {
        let name = ref_name(r)?;
        self.defs
            .get(name)
            .with_context(|| format!("reference to unknown definition `{name}`"))
    }

    fn check_refs(&self) -> Result<()> {
        fn walk<'t>(ty: &'t Ty, out: &mut Vec<&'t str>) {
            match ty {
                Ty::Named(n) => out.push(n),
                Ty::MapOf(t) | Ty::Vec(t) | Ty::Boxed(t) => walk(t, out),
                _ => {}
            }
        }
        let mut refs = Vec::new();
        for item in self.model.items.values() {
            match item {
                Item::Struct(s) => s.fields.iter().for_each(|f| walk(&f.ty, &mut refs)),
                Item::Union(u) => u.variants.iter().for_each(|v| walk(&v.ty, &mut refs)),
                Item::Alias(a) => walk(&a.ty, &mut refs),
                Item::Enum(_) | Item::Marker(_) | Item::Scalar(_) => {}
            }
        }
        for r in refs {
            if !self.model.items.contains_key(r) {
                bail!("generated code refers to missing type `{r}`");
            }
        }
        Ok(())
    }
}

fn description(v: &Value) -> Option<String> {
    v.get("description")
        .and_then(Value::as_str)
        .map(docs::sanitize)
        .filter(|d| !d.is_empty())
}

fn is_empty_schema(v: &Value) -> bool {
    v.as_object()
        .is_some_and(|o| o.keys().all(|k| k == "description"))
}

fn ref_name(r: &str) -> Result<&str> {
    r.strip_prefix("#/$defs/")
        .with_context(|| format!("unsupported $ref `{r}`"))
}

fn string_values(values: &[Value]) -> Result<Vec<String>> {
    values
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .context("non-string enum value")
        })
        .collect()
}

fn scalar_kinds(types: &[Value]) -> Result<Vec<Scalar>> {
    types
        .iter()
        .map(|t| match t.as_str() {
            Some("string") => Ok(Scalar::String),
            Some("integer") => Ok(Scalar::Integer),
            Some("boolean") => Ok(Scalar::Boolean),
            Some("number") => Ok(Scalar::Number),
            other => bail!("unsupported scalar type {other:?}"),
        })
        .collect()
}

/// Removes the longest common UpperCamelCase-word suffix shared by every
/// `$ref` variant (`CallToolRequest`, `ListToolsRequest` → `CallTool`,
/// `ListTools`).
fn strip_common_suffix(variants: &mut [Variant], ref_defs: &[Option<&Value>]) {
    let names: Vec<String> = variants
        .iter()
        .zip(ref_defs)
        .filter(|(_, d)| d.is_some())
        .map(|(v, _)| v.name.clone())
        .collect();
    if names.len() < 2 || names.len() != variants.len() {
        return;
    }
    let Some(first) = names.first() else { return };
    let mut len = 0;
    while len < first.len() {
        let candidate = first.get(first.len() - len - 1..).unwrap_or_default();
        if names
            .iter()
            .all(|n| n.len() > candidate.len() && n.ends_with(candidate))
        {
            len += 1;
        } else {
            break;
        }
    }
    // Shrink to a word boundary: the suffix must start with an uppercase letter.
    while len > 0 {
        let suffix = first.get(first.len() - len..).unwrap_or_default();
        if suffix.starts_with(|c: char| c.is_ascii_uppercase()) {
            break;
        }
        len -= 1;
    }
    if len == 0 {
        return;
    }
    for v in variants.iter_mut() {
        let keep = v.name.len() - len;
        if let Some(stem) = v.name.get(..keep) {
            v.name = stem.to_owned();
        }
    }
}

fn detect_tag(variants: &[Variant], ref_defs: &[Option<&Value>]) -> UnionKind {
    if ref_defs.iter().any(Option::is_none) {
        return UnionKind::Untagged;
    }
    let defs: Vec<&Value> = ref_defs.iter().flatten().copied().collect();

    // MRTR: `InputRequiredResult` is identified by `resultType`.
    if variants.len() == 2
        && let Some(ir) = variants
            .iter()
            .position(|v| v.ty == Ty::Named("InputRequiredResult".to_owned()))
    {
        return UnionKind::Tagged {
            field: "resultType".to_owned(),
            values: vec![("input_required".to_owned(), ir)],
            default: Some(1 - ir),
            default_on_unknown: true,
        };
    }

    let Some(first) = defs.first() else {
        return UnionKind::Untagged;
    };
    let candidates: Vec<String> = first
        .get("properties")
        .and_then(Value::as_object)
        .map(|p| {
            p.iter()
                .filter(|(_, v)| v.get("const").is_some_and(Value::is_string))
                .map(|(k, _)| k.clone())
                .collect()
        })
        .unwrap_or_default();

    'fields: for field in candidates {
        let mut values: Vec<(String, usize)> = Vec::new();
        let mut default = None;
        for (i, def) in defs.iter().enumerate() {
            let prop = def.get("properties").and_then(|p| p.get(&field));
            let constant = prop.and_then(|p| p.get("const")).and_then(Value::as_str);
            let required = def
                .get("required")
                .and_then(Value::as_array)
                .is_some_and(|r| r.iter().any(|x| x.as_str() == Some(field.as_str())));
            if let Some(c) = constant {
                if values.iter().any(|(v, _)| v == c) {
                    continue 'fields;
                }
                values.push((c.to_owned(), i));
            }
            if constant.is_none() || !required {
                if default.is_some() {
                    continue 'fields;
                }
                default = Some(i);
            }
        }
        if !values.is_empty() {
            return UnionKind::Tagged {
                field,
                values,
                default,
                default_on_unknown: false,
            };
        }
    }
    UnionKind::Untagged
}
