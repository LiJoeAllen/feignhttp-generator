use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::ir::{ObjectSchema, Schema, StrSchema, TypeExpr};
use crate::naming::{sanitize_ident, sanitize_type_name};

/// Rendered model items plus bookkeeping for synthesized names.
pub struct ModelRegistry {
    /// Raw schema name -> Rust identifier.
    idents: BTreeMap<String, String>,
    /// Rust identifier -> rendered item source.
    items: BTreeMap<String, String>,
    /// Name counters for synthesized identifiers.
    counters: BTreeMap<String, usize>,
    /// Set when `serde_json::Value` was emitted somewhere.
    pub serde_json_value: bool,
}

impl Default for ModelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelRegistry {
    pub fn new() -> Self {
        Self {
            idents: BTreeMap::new(),
            items: BTreeMap::new(),
            counters: BTreeMap::new(),
            serde_json_value: false,
        }
    }

    pub fn rendered(&self) -> impl Iterator<Item = &String> {
        self.items.values()
    }

    fn ident_of(&mut self, raw: &str) -> String {
        if let Some(id) = self.idents.get(raw) {
            return id.clone();
        }
        let base = sanitize_type_name(raw);
        let n = self.counters.entry(base.clone()).or_insert(0);
        *n += 1;
        let id = if *n == 1 {
            base
        } else {
            format!("{base}_{}", n)
        };
        self.idents.insert(raw.to_string(), id.clone());
        id
    }

    fn synth_name(&mut self, hint: &str) -> String {
        let base = sanitize_type_name(hint);
        let n = self.counters.entry(base.clone()).or_insert(0);
        *n += 1;
        if *n == 1 {
            base
        } else {
            format!("{base}{}", n)
        }
    }

    /// Compute the Rust type expression for a type expression,
    /// registering any inline models it needs.
    pub fn rust_type(&mut self, te: &TypeExpr, hint: &str) -> String {
        let inner = match &te.schema {
            Schema::Ref(raw) => {
                let id = self.ident_of(raw);
                if !self.items.contains_key(&id) {
                    // Referenced before seeded: emit an opaque placeholder.
                    self.put(
                        &id,
                        format!(
                            "#[derive(Clone, Debug, ::serde::Deserialize, ::serde::Serialize)]\n\
                             #[serde(transparent)]\npub struct {id}(pub serde_json::Value);"
                        ),
                    );
                    self.serde_json_value = true;
                }
                format!("crate::models::{id}")
            }
            Schema::Object(obj) => {
                let id = self.synth_name(hint);
                let src = self.render_struct(&id, obj);
                self.put(&id, src);
                format!("crate::models::{id}")
            }
            Schema::Array(el) => {
                let inner = self.rust_type(el, &format!("{hint}Item"));
                format!("Vec<{inner}>")
            }
            Schema::Str(s) => match &s.enum_values {
                Some(vals) => {
                    let id = self.synth_name(hint);
                    let src = render_enum(&id, vals);
                    self.put(&id, src);
                    format!("crate::models::{id}")
                }
                None => "String".to_string(),
            },
            Schema::Integer(format) => match format.as_deref() {
                Some("int32") => "i32".to_string(),
                _ => "i64".to_string(),
            },
            Schema::Number(format) => match format.as_deref() {
                Some("float") => "f32".to_string(),
                _ => "f64".to_string(),
            },
            Schema::Boolean => "bool".to_string(),
            Schema::Binary => "Vec<u8>".to_string(),
            Schema::Any => {
                self.serde_json_value = true;
                "serde_json::Value".to_string()
            }
        };
        if te.nullable && !inner.starts_with("Option<") {
            format!("Option<{inner}>")
        } else {
            inner
        }
    }

    /// Register a named schema from components/definitions.
    pub fn seed_named(&mut self, raw: &str, schema: &Schema) {
        let id = self.ident_of(raw);
        if self.items.contains_key(&id) {
            return;
        }
        match schema {
            Schema::Object(obj) => {
                let src = self.render_struct(&id, obj);
                self.put(&id, src);
            }
            Schema::Str(StrSchema {
                enum_values: Some(vals),
                ..
            }) => {
                let src = render_enum(&id, vals);
                self.put(&id, src);
            }
            other => {
                let ty = self.rust_type(
                    &TypeExpr {
                        schema: clone_schema(other),
                        nullable: false,
                    },
                    raw,
                );
                self.put(&id, format!("pub type {id} = {ty};"));
            }
        }
    }

    /// Register an object under a fixed name (used for form payloads).
    pub fn register_object(&mut self, hint: &str, obj: &ObjectSchema) -> String {
        let id = self.synth_name(hint);
        let src = self.render_struct(&id, obj);
        self.put(&id, src);
        format!("crate::models::{id}")
    }

    fn put(&mut self, ident: &str, src: String) {
        self.items.insert(ident.to_string(), src);
    }

    fn render_struct(&mut self, id: &str, obj: &ObjectSchema) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "#[derive(Clone, Debug, ::serde::Deserialize, ::serde::Serialize)]"
        );
        let _ = writeln!(out, "pub struct {id} {{");
        if obj.fields.is_empty() {
            let _ = writeln!(out, "}}");
            return out;
        }
        for f in &obj.fields {
            if let Some(doc) = &f.description {
                for line in doc.lines() {
                    let _ = writeln!(out, "/// {line}");
                }
            }
            let field_ident = sanitize_ident(&f.wire_name);
            let clean = field_ident.trim_start_matches("r#").to_string();
            let ty = self.rust_type(&f.type_, &format!("{id}{}", snake_to_camel(&clean)));
            // serde treats missing `Option<T>` fields as `None` natively, so no
            // `#[serde(default)]` is emitted; wrap only when not already optional.
            let optional = !f.required || f.type_.nullable;
            let ty = if optional && !ty.starts_with("Option<") {
                format!("Option<{ty}>")
            } else {
                ty
            };
            if clean != f.wire_name {
                let _ = writeln!(
                    out,
                    "    #[serde(rename = \"{}\")]",
                    escape(&f.wire_name)
                );
            }
            let _ = writeln!(out, "    pub {field_ident}: {ty},");
        }
        let _ = writeln!(out, "}}");
        out
    }
}

fn render_enum(id: &str, values: &[String]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "#[derive(Clone, Debug, ::serde::Deserialize, ::serde::Serialize, PartialEq, Eq)]"
    );
    let _ = writeln!(out, "pub enum {id} {{");
    let mut display_arms = Vec::new();
    for v in values {
        let variant = sanitize_type_name(v);
        display_arms.push((variant.clone(), v.clone()));
        let _ = writeln!(
            out,
            "    #[serde(rename = \"{}\")]\n    {variant},",
            escape(v)
        );
    }
    let _ = writeln!(out, "}}");
    // feignhttp serializes scalar params with `.to_string()`; give variants
    // their wire representation.
    let _ = writeln!(
        out,
        "impl ::std::fmt::Display for {id} {{\n    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {{\n        match self {{"
    );
    for (variant, wire) in &display_arms {
        let _ = writeln!(
            out,
            "            {id}::{variant} => f.write_str(\"{}\"),",
            escape(wire)
        );
    }
    let _ = writeln!(out, "        }}\n    }}\n}}");
    out
}

fn clone_schema(s: &Schema) -> Schema {
    s.clone()
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn snake_to_camel(s: &str) -> String {
    let mut out = String::new();
    let mut upper = true;
    for c in s.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}
