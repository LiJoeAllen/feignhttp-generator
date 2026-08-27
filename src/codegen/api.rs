use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::codegen::models::ModelRegistry;
use crate::codegen::Emission;
use crate::ir::{Location, RequestBody, Schema, SuccessResponse, TypeExpr};
use crate::mapper::MappedOperation;
use crate::naming::{sanitize_ident, sanitize_type_name};

/// A generated API module file.
pub struct ApiFile {
    /// Module path segments, e.g. ["a", "b"] for `src/a/b.rs`.
    pub path: Vec<String>,
    pub content: String,
}

pub struct CodegenOutput {
    pub models_src: String,
    pub api_files: Vec<ApiFile>,
    pub emission: Emission,
}

#[derive(PartialEq, Clone, Copy)]
enum PayloadKind {
    None,
    Json,
    Form,
    Multipart,
    Octets,
}

/// Generate models and per-file traits (layout-independent core).
pub fn generate(spec: &crate::ir::ApiSpec, mapped: &[MappedOperation]) -> CodegenOutput {
    let mut reg = ModelRegistry::new();

    for (raw, schema) in &spec.schemas {
        reg.seed_named(raw, schema);
    }

    // ---- Error model analysis (Q7) ----
    let mut fingerprints: BTreeMap<String, usize> = BTreeMap::new();
    for m in mapped {
        let mut seen_in_op: Vec<String> = Vec::new();
        for s in &m.op.error_schemas {
            let fp = fingerprint(s);
            if !seen_in_op.contains(&fp) {
                seen_in_op.push(fp);
            }
        }
        for fp in seen_in_op {
            *fingerprints.entry(fp).or_default() += 1;
        }
    }
    let majority = fingerprints
        .iter()
        .filter(|(_, c)| **c >= 2)
        .max_by(|a, b| a.1.cmp(b.1).then(a.0.cmp(b.0)))
        .map(|(fp, _)| fp.clone());

    let mut emitted_errors: BTreeMap<String, String> = BTreeMap::new();
    for m in mapped {
        for s in &m.op.error_schemas {
            let fp = fingerprint(s);
            if emitted_errors.contains_key(&fp) {
                continue;
            }
            let shared = majority.as_deref() == Some(fp.as_str());
            let hint = if shared {
                "ApiErrorPayload".to_string()
            } else {
                format!("{}Error", m.trait_name)
            };
            let ty = reg.rust_type(
                &TypeExpr {
                    schema: s.clone(),
                    nullable: false,
                },
                &hint,
            );
            emitted_errors.insert(fp, ty);
        }
    }

    let mut error_alias = String::new();
    if let Some(maj) = &majority {
        if let Some(ty) = emitted_errors.get(maj) {
            let _ = write!(
                error_alias,
                "/// Payload carried by the majority of error responses across operations.\n\
                 /// feignhttp surfaces 4xx/5xx as `ErrorKind::Status(code, body_text)`;\n\
                 /// parse this type from `body_text` when typed error handling is needed:\n\
                 /// `let e: feignhttp::Error = ...; if let ErrorKind::Status(_, body) = e.kind() {{\n\
                 ///     let payload: ApiError = serde_json::from_str(&body)?; }}`\n\
                 pub type ApiError = {ty};\n"
            );
        }
    }

    // ---- Traits grouped by file ----
    let mut groups: BTreeMap<Vec<String>, Vec<usize>> = BTreeMap::new();
    for (i, m) in mapped.iter().enumerate() {
        let mut p = m.dirs.clone();
        p.push(m.module.clone());
        groups.entry(p).or_default().push(i);
    }

    let mut emission = Emission::default();
    let mut api_files = Vec::new();

    for (path, indices) in &groups {
        let first = &mapped[indices[0]];
        let trait_name = first.trait_name.clone();

        // Emit methods first so the import list can reflect what is actually used.
        let mut methods = String::new();
        for &i in indices {
            emit_method(&mut reg, &mut methods, &mapped[i]);
        }
        let needs_multipart = methods.contains("#[part(") || methods.contains("#[file(");

        let mut content = String::new();

        // Inner attributes must precede everything else in the file/module.
        let _ = writeln!(content, "#![allow(clippy::too_many_arguments)]");
        let _ = writeln!(content, "// Path prefix: /{}", path.join("/"));

        // Method attribute macros are consumed by the `#[feign]` expansion;
        // only `feign`, the builder trait and (optionally) multipart are needed.
        if needs_multipart {
            let _ = writeln!(
                content,
                "use ::feignhttp::{{feign, multipart, FeignClientBuilder as _}};"
            );
        } else {
            let _ = writeln!(
                content,
                "use ::feignhttp::{{feign, FeignClientBuilder as _}};"
            );
        }

        let _ = writeln!(
            content,
            "/// Client for the `{}` path subtree.",
            path.last().map(String::as_str).unwrap_or("")
        );
        let _ = writeln!(content, "#[feign(url = \"{{base_url}}{{prefix}}\")]");
        let _ = writeln!(content, "pub trait {trait_name} {{");

        content.push_str(&methods);

        let _ = writeln!(content, "}}");
        api_files.push(ApiFile {
            path: path.clone(),
            content,
        });
    }

    // Feature requirements.
    emission.multipart = mapped
        .iter()
        .any(|m| payload_kind(m) == PayloadKind::Multipart);
    emission.json = mapped.iter().any(returns_json);
    emission.serde_json_value = reg.serde_json_value;

    // ---- Assemble model source ----
    let mut models_src = String::new();
    for src in reg.rendered() {
        models_src.push_str(src);
    }
    if !error_alias.is_empty() {
        models_src.push_str(&error_alias);
        models_src.push('\n');
    }

    CodegenOutput {
        models_src,
        api_files,
        emission,
    }
}

fn returns_json(m: &MappedOperation) -> bool {
    match &m.op.success {
        Some(s) => is_json_media(&s.media_type),
        None => false,
    }
}

fn is_json_media(mt: &str) -> bool {
    mt == "application/json" || mt.ends_with("+json")
}

fn payload_kind(m: &MappedOperation) -> PayloadKind {
    let Some(rb) = &m.op.request_body else {
        return PayloadKind::None;
    };
    let keys: Vec<&String> = rb.content.iter().map(|(k, _)| k).collect();
    match pick_media(keys.into_iter()) {
        Some(mt) if is_json_media(mt) => PayloadKind::Json,
        Some(mt) if mt.starts_with("multipart/") => PayloadKind::Multipart,
        Some(mt) if mt == "application/x-www-form-urlencoded" => PayloadKind::Form,
        Some(mt) if mt == "application/octet-stream" => PayloadKind::Octets,
        Some(_) => PayloadKind::Json,
        None => PayloadKind::None,
    }
}

fn pick_media<'x>(keys: impl Iterator<Item = &'x String>) -> Option<&'x String> {
    let keys: Vec<&String> = keys.collect();
    for want in [
        "application/json",
        "text/plain",
        "application/x-www-form-urlencoded",
        "multipart/form-data",
        "application/octet-stream",
    ] {
        if let Some(k) = keys.iter().find(|k| k.as_str() == want) {
            return Some(k);
        }
    }
    if let Some(k) = keys.iter().find(|k| k.ends_with("+json")) {
        return Some(k);
    }
    if let Some(k) = keys.iter().find(|k| k.starts_with("text/")) {
        return Some(k);
    }
    keys.first().copied()
}

fn emit_method(reg: &mut ModelRegistry, out: &mut String, m: &MappedOperation) {
    let op = &m.op;

    let fn_camel = sanitize_type_name(m.fn_name.trim_start_matches("r#"));
    let hint_base = format!("{trait}{fn_camel}", trait = m.trait_name);

    if let Some(s) = &op.summary {
        for line in s.lines() {
            let _ = writeln!(out, "    /// {line}");
        }
    }
    let _ = writeln!(out, "    /// `{}`", m.origin());
    for p in &op.parameters {
        if let Some(d) = &p.description {
            let optional = if p.required { "" } else { " (optional)" };
            let _ = writeln!(out, "    /// - `{}`{}: {d}", p.wire_name, optional);
        } else if !p.required {
            let _ = writeln!(out, "    /// - `{}` (optional)", p.wire_name);
        }
    }

    let macro_name = op.method.as_str();
    let _ = writeln!(out, "    #[{macro_name}(\"{}\")]", escape(&op.path));
    let _ = writeln!(out, "    async fn {}(", m.fn_name);

    let mut arg_lines: Vec<String> = vec!["        &self".to_string()];

    // Path parameters (feignhttp restricts these to scalars; enums degrade to String).
    for p in op
        .parameters
        .iter()
        .filter(|p| matches!(p.location, Location::Path))
    {
        let ident = sanitize_ident(&p.wire_name);
        let ty = match &p.schema {
            Schema::Str(_) => "String".to_string(),
            Schema::Integer(format) => match format.as_deref() {
                Some("int32") => "i32".to_string(),
                _ => "i64".to_string(),
            },
            Schema::Number(format) => match format.as_deref() {
                Some("float") => "f32".to_string(),
                _ => "f64".to_string(),
            },
            Schema::Boolean => "bool".to_string(),
            _ => "String".to_string(),
        };
        arg_lines.push(arg_line(&p.wire_name, "path", &ident, &ty));
    }
    // Query parameters.
    for p in op
        .parameters
        .iter()
        .filter(|p| matches!(p.location, Location::Query))
    {
        let ident = sanitize_ident(&p.wire_name);
        let ty = param_type(reg, &p.schema, &format!("{hint_base}Query"));
        arg_lines.push(arg_line(&p.wire_name, "query", &ident, &ty));
    }
    // Header parameters.
    for p in op
        .parameters
        .iter()
        .filter(|p| matches!(p.location, Location::Header))
    {
        let ident = sanitize_ident(&p.wire_name);
        let ty = scalar_type(reg, &p.schema, &format!("{hint_base}Header"));
        arg_lines.push(arg_line(&p.wire_name, "header", &ident, &ty));
    }
    // Cookie parameters are not supported by feignhttp; skipped with a note.
    let cookies = op
        .parameters
        .iter()
        .filter(|p| matches!(p.location, Location::Cookie))
        .count();
    if cookies > 0 {
        let _ = writeln!(
            out,
            "    // NOTE: {} cookie parameter(s) skipped: feignhttp does not support cookies.",
            cookies
        );
    }

    // Request payload.
    match payload_kind(m) {
        PayloadKind::Json => {
            if let Some(rb) = &op.request_body {
                if let Some((_, schema)) = pick_content(rb) {
                    let ty = reg.rust_type(
                        &TypeExpr {
                            schema: schema.clone(),
                            nullable: false,
                        },
                        &format!("{hint_base}Body"),
                    );
                    arg_lines.push(format!("        #[body] body: {ty},"));
                }
            }
        }
        PayloadKind::Octets => {
            arg_lines.push("        #[body] body: Vec<u8>,".to_string());
        }
        PayloadKind::Form => {
            if let Some(rb) = &op.request_body {
                if let Some((_, Schema::Object(obj))) = pick_content(rb) {
                    let ty = reg.register_object(&format!("{hint_base}Form"), obj);
                    arg_lines.push(format!("        #[form] form: {ty},"));
                } else if let Some((_, schema)) = pick_content(rb) {
                    let ty = reg.rust_type(
                        &TypeExpr {
                            schema: schema.clone(),
                            nullable: false,
                        },
                        &format!("{hint_base}Form"),
                    );
                    arg_lines.push(format!("        #[form] form: {ty},"));
                }
            }
        }
        PayloadKind::Multipart => {
            if let Some(rb) = &op.request_body {
                if let Some((_, Schema::Object(obj))) = pick_content(rb) {
                    for f in &obj.fields {
                        let ident = sanitize_ident(&f.wire_name);
                        match f.type_.schema {
                            Schema::Binary => {
                                arg_lines.push(format!(
                                    "        #[file(\"{}\")] {ident}: Vec<u8>",
                                    escape(&f.wire_name)
                                ));
                            }
                            _ => {
                                arg_lines.push(format!(
                                    "        #[part(\"{}\")] {ident}: String",
                                    escape(&f.wire_name)
                                ));
                            }
                        }
                    }
                }
            }
        }
        PayloadKind::None => {}
    }

    for (i, line) in arg_lines.iter().enumerate() {
        let comma = if i + 1 == arg_lines.len() { "" } else { "," };
        let _ = writeln!(out, "{line}{comma}");
    }

    // Return type.
    let ret = return_type(reg, m);
    let _ = writeln!(out, "    ) -> feignhttp::Result<{ret}>;");
}

fn arg_line(wire: &str, attr: &str, ident: &str, ty: &str) -> String {
    format!("        #[{attr}(\"{}\")] {ident}: {ty}", escape(wire))
}

fn pick_content(rb: &RequestBody) -> Option<&(String, Schema)> {
    rb.content
        .iter()
        .find(|(mt, _)| is_json_media(mt) || mt.starts_with("text/"))
        .or_else(|| rb.content.first())
}

fn param_type(reg: &mut ModelRegistry, schema: &Schema, hint: &str) -> String {
    match schema {
        Schema::Array(el) => {
            // Query arrays of scalars are supported natively.
            let inner = scalar_type(reg, &el.schema, &format!("{hint}Item"));
            format!("Vec<{inner}>")
        }
        other => scalar_type(reg, other, hint),
    }
}

/// Types acceptable to feignhttp scalar params; complex schemas become structs.
fn scalar_type(reg: &mut ModelRegistry, schema: &Schema, hint: &str) -> String {
    reg.rust_type(
        &TypeExpr {
            schema: schema.clone(),
            nullable: false,
        },
        hint,
    )
}

fn return_type(reg: &mut ModelRegistry, m: &MappedOperation) -> String {
    if matches!(m.op.method, crate::ir::HttpMethod::Head) {
        return "()".to_string();
    }
    match &m.op.success {
        None => "()".to_string(),
        Some(SuccessResponse { media_type, schema }) => {
            if media_type.starts_with("text/") {
                return "String".to_string();
            }
            if media_type == "application/octet-stream" {
                return "Vec<u8>".to_string();
            }
            let fn_camel = sanitize_type_name(m.fn_name.trim_start_matches("r#"));
            reg.rust_type(
                &TypeExpr {
                    schema: schema.clone(),
                    nullable: false,
                },
                &format!("{}{}Resp", m.trait_name, fn_camel),
            )
        }
    }
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Structural fingerprint used to deduplicate error schemas.
pub fn fingerprint(s: &Schema) -> String {
    match s {
        Schema::Ref(n) => format!("ref:{n}"),
        Schema::Object(o) => {
            let fields: Vec<String> = o
                .fields
                .iter()
                .map(|f| {
                    format!(
                        "{}:{}:{}:{}",
                        f.wire_name,
                        fingerprint(&f.type_.schema),
                        f.required,
                        f.type_.nullable
                    )
                })
                .collect();
            format!("obj{{{}}}", fields.join(","))
        }
        Schema::Array(el) => format!("[{}]", fingerprint(&el.schema)),
        Schema::Str(s) => format!("str:{:?}", s.enum_values),
        Schema::Integer(f) => format!("int:{f:?}"),
        Schema::Number(f) => format!("num:{f:?}"),
        Schema::Boolean => "bool".to_string(),
        Schema::Binary => "bin".to_string(),
        Schema::Any => "any".to_string(),
    }
}
