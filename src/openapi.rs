use std::collections::BTreeMap;

use anyhow::{anyhow, bail, Context as _, Result};
use serde_json::Value;

use crate::ir::*;

/// Parse a spec from any reader: JSON first, YAML fallback.
pub fn parse_reader(mut reader: impl std::io::Read) -> Result<Value> {
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf);
    match serde_json::from_str::<Value>(&text) {
        Ok(v) => Ok(v),
        Err(json_err) => serde_yaml::from_str::<Value>(&text)
            .map_err(|yaml_err| anyhow!("not valid JSON ({json_err}) nor YAML ({yaml_err})")),
    }
}

/// Normalize any supported OpenAPI dialect (2.0 / 3.0.x / 3.1.x) into the internal IR.
/// Returns the normalized spec plus non-fatal warnings.
pub fn normalize(root: &Value) -> Result<(ApiSpec, Vec<String>)> {
    if root.get("openapi").and_then(Value::as_str).is_some_and(|v| {
        v.split('.').next().is_some_and(|major| major == "3")
    }) {
        normalize_v3(root)
    } else if root
        .get("swagger")
        .and_then(Value::as_str)
        .is_some_and(|v| v.starts_with('2'))
    {
        normalize_v2(root)
    } else {
        bail!("unsupported spec: neither `openapi` (3.x) nor `swagger` (2.0) version field found")
    }
}

struct Ctx<'a> {
    root: &'a Value,
    schemas: BTreeMap<String, Schema>,
    warnings: Vec<String>,
}

impl Ctx<'_> {
    fn warn(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }

    /// Convert a schema node; registers `$ref` targets under their last path token.
    /// Cyclic references terminate because a target already present (or being
    /// converted) short-circuits to `Schema::Ref`.
    fn convert(&mut self, v: &Value) -> Schema {
        if let Some(ptr) = v.get("$ref").and_then(Value::as_str) {
            let name = ptr.rsplit('/').next().unwrap_or("").to_string();
            if name.is_empty() {
                self.warn(format!("malformed `$ref`: {ptr}"));
                return Schema::Any;
            }
            if !self.schemas.contains_key(&name) {
                // Placeholder breaks cycles during nested conversion.
                self.schemas.insert(name.clone(), Schema::Any);
                // Clone the target so the immutable borrow of `root` ends
                // before the mutable borrow of `schemas` below.
                let target = match resolve_ref(self.root, ptr) {
                    Ok(t) => t.clone(),
                    Err(e) => {
                        self.warn(format!("{e}; falling back to opaque type"));
                        Value::Null
                    }
                };
                let converted = if target.is_null() {
                    Schema::Any
                } else {
                    self.convert_inner(&target)
                };
                self.schemas.insert(name.clone(), converted);
            }
            Schema::Ref(name)
        } else {
            self.convert_inner(v)
        }
    }

    fn convert_inner(&mut self, v: &Value) -> Schema {
        // OAS 3.1: `type` may be an array, e.g. ["string", "null"].
        if let Some(types) = v.get("type").and_then(Value::as_array) {
            let non_null: Vec<&str> = types
                .iter()
                .filter_map(Value::as_str)
                .filter(|t| *t != "null")
                .collect();
            if non_null.len() > 1 {
                self.warn(format!(
                    "multi-type schema {non_null:?} narrowed to `{}`",
                    non_null[0]
                ));
            }
            return self.convert_typed(non_null.first().copied(), v);
        }

        match v.get("type").and_then(Value::as_str) {
            Some(t) => self.convert_typed(Some(t), v),
            None => {
                if let Some(branches) = v.get("allOf").and_then(Value::as_array) {
                    return self.convert_all_of(branches, v);
                }
                for key in ["oneOf", "anyOf"] {
                    if let Some(branches) = v.get(key).and_then(Value::as_array) {
                        return self.convert_one_of(branches);
                    }
                }
                if v.get("properties").is_some() || v.get("additionalProperties").is_some() {
                    self.convert_object(v)
                } else if v.get("items").is_some() {
                    self.convert_array(v)
                } else if let Some(vals) = enum_strings(v) {
                    Schema::Str(StrSchema {
                        enum_values: Some(vals),
                    })
                } else {
                    Schema::Any
                }
            }
        }
    }

    /// Resolve a composition branch to its object shape, following `$ref`.
    fn branch_object(&mut self, branch: &Value) -> Option<ObjectSchema> {
        match self.convert(branch) {
            Schema::Object(o) => Some(o),
            Schema::Ref(name) => match self.schemas.get(&name) {
                Some(Schema::Object(o)) => Some(o.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    /// `allOf`: merge every object branch into one struct. Later branches
    /// win on field conflicts; sibling `properties`/`required` are honored.
    fn convert_all_of(&mut self, branches: &[Value], sibling: &Value) -> Schema {
        let mut merged: Vec<Field> = Vec::new();
        for b in branches {
            if let Some(o) = self.branch_object(b) {
                for f in o.fields {
                    if !merged.iter().any(|e| e.wire_name == f.wire_name) {
                        merged.push(f);
                    }
                }
            } else {
                let ptr = b.get("$ref").and_then(Value::as_str).unwrap_or("<inline>");
                self.warn(format!("allOf branch `{ptr}` is not an object; skipped"));
            }
        }
        // Sibling keywords alongside allOf (rare but legal).
        if sibling.get("properties").is_some() || sibling.get("required").is_some() {
            if let Schema::Object(o) = self.convert_object(sibling) {
                for f in o.fields {
                    if !merged.iter().any(|e| e.wire_name == f.wire_name) {
                        merged.push(f);
                    }
                }
            }
        }
        Schema::Object(ObjectSchema { fields: merged })
    }

    /// `oneOf` / `anyOf`: pick the first non-null branch (the common
    /// `[$ref, {"type":"null"}]` nullable-envelope idiom).
    fn convert_one_of(&mut self, branches: &[Value]) -> Schema {
        let non_null: Vec<&Value> = branches
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) != Some("null"))
            .collect();
        if non_null.is_empty() {
            return Schema::Any;
        }
        if branches.len() > 1 && non_null.len() > 1 {
            self.warn("composition with multiple non-null branches narrowed to the first");
        }
        self.convert(non_null[0])
    }

    fn convert_typed(&mut self, type_str: Option<&str>, v: &Value) -> Schema {
        match type_str {
            Some("object") => self.convert_object(v),
            Some("array") => self.convert_array(v),
            Some("string") => {
                if v.get("format").and_then(Value::as_str) == Some("binary") {
                    return Schema::Binary;
                }
                Schema::Str(StrSchema {
                    enum_values: enum_strings(v),
                })
            }
            Some("integer") => Schema::Integer(v.get("format").and_then(Value::as_str).map(String::from)),
            Some("number") => Schema::Number(v.get("format").and_then(Value::as_str).map(String::from)),
            Some("boolean") => Schema::Boolean,
            Some(other) => {
                self.warn(format!("unknown schema type `{other}` treated as opaque"));
                Schema::Any
            }
            None => Schema::Any,
        }
    }

    fn convert_object(&mut self, v: &Value) -> Schema {
        let mut fields = Vec::new();
        let required: Vec<&str> = v
            .get("required")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if let Some(props) = v.get("properties").and_then(Value::as_object) {
            for (wire, pv) in props {
                let required = required.contains(&wire.as_str());
                // OAS 3.0 nullability marker (3.1 uses type arrays instead).
                let nullable = pv.get("nullable").and_then(Value::as_bool).unwrap_or(false);
                fields.push(Field {
                    wire_name: wire.clone(),
                    type_: TypeExpr {
                        schema: self.convert(pv),
                        nullable,
                    },
                    required,
                    description: pv.get("description").and_then(Value::as_str).map(String::from),
                });
            }
        }
        Schema::Object(ObjectSchema { fields })
    }

    fn convert_array(&mut self, v: &Value) -> Schema {
        let items_val = v.get("items");
        let nullable = items_val
            .and_then(|i| i.get("nullable"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let items_owned = items_val.cloned().unwrap_or(Value::Object(Default::default()));
        let elem = self.convert(&items_owned);
        Schema::Array(Box::new(TypeExpr {
            schema: elem,
            nullable,
        }))
    }
}

fn enum_strings(v: &Value) -> Option<Vec<String>> {
    let vals: Vec<String> = v
        .get("enum")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|x| x.as_str().map(String::from))
        .collect();
    (!vals.is_empty()).then_some(vals)
}

/// Resolve a local `#/...` reference to its target value.
fn resolve_ref<'a>(root: &'a Value, reference: &str) -> Result<&'a Value> {
    let tokens = reference
        .strip_prefix("#/")
        .ok_or_else(|| anyhow!("external or malformed `$ref` not supported: {reference}"))?;
    let mut cur = root;
    for raw in tokens.split('/') {
        let token = raw.replace("~1", "/").replace("~0", "~");
        cur = cur
            .get(&token)
            .with_context(|| format!("unresolvable `$ref`: {reference}"))?;
    }
    Ok(cur)
}

/// HTTP method keys present on a path item.
fn keys_of(item: &Value) -> Vec<String> {
    item.as_object()
        .map(|o| {
            o.keys()
                .filter(|k| HttpMethod::from_str(k).is_some())
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Convert an OAS3 `content` map into ordered (media type, schema) entries.
fn content_entries(
    content: &serde_json::Map<String, Value>,
    ctx: &mut Ctx,
) -> Vec<(String, Schema)> {
    content
        .iter()
        .map(|(mt, mv)| {
            let schema = mv
                .get("schema")
                .map(|s| ctx.convert(s))
                .unwrap_or(Schema::Any);
            (mt.clone(), schema)
        })
        .collect()
}

fn normalize_v3(root: &Value) -> Result<(ApiSpec, Vec<String>)> {
    let info = root.get("info");
    let title = info
        .and_then(|i| i.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("API")
        .to_string();
    let version = info
        .and_then(|i| i.get("version"))
        .and_then(Value::as_str)
        .unwrap_or("0.0.0")
        .to_string();

    let server_url = root
        .get("servers")
        .and_then(Value::as_array)
        .and_then(|s| s.first())
        .and_then(|s| s.get("url"))
        .and_then(Value::as_str)
        .unwrap_or("/");
    let (base_url, prefix) = split_server_url(server_url);

    let mut ctx = Ctx {
        root,
        schemas: BTreeMap::new(),
        warnings: Vec::new(),
    };

    // Seed all named component schemas first so refs always resolve.
    if let Some(schemas) = root
        .get("components")
        .and_then(|c| c.get("schemas"))
        .and_then(Value::as_object)
    {
        for (name, v) in schemas {
            let converted = ctx.convert(v);
            match (&converted, ctx.schemas.contains_key(name)) {
                // The value was itself a `$ref` to another component: alias it.
                (Schema::Ref(other), false) if other != name => {
                    ctx.schemas.insert(name.clone(), converted);
                }
                (_, false) => {
                    ctx.schemas.insert(name.clone(), converted);
                }
                _ => {}
            }
        }
    }

    let paths = root
        .get("paths")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("spec has no `paths` object"))?;

    let mut operations = Vec::new();
    for (path, item) in paths {
        let shared_params = item.get("parameters").cloned();
        for key in keys_of(item) {
            let Some(method) = HttpMethod::from_str(&key) else {
                continue;
            };
            let op = item.get(&key).expect("key came from object");

            let mut parameters = params_from_value(shared_params.as_ref(), &mut ctx)?;
            parameters.extend(params_from_value(op.get("parameters"), &mut ctx)?);

            let request_body = op
                .get("requestBody")
                .and_then(|rb| rb.get("content"))
                .and_then(Value::as_object)
                .map(|content| RequestBody {
                    content: content_entries(content, &mut ctx),
                });

            operations.push(Operation {
                method,
                path: path.clone(),
                operation_id: op.get("operationId").and_then(Value::as_str).map(String::from),
                summary: op.get("summary").and_then(Value::as_str).map(String::from),
                parameters,
                request_body,
                success: pick_success(op.get("responses"), &mut ctx),
                error_schemas: collect_errors(op.get("responses"), &mut ctx),
            });
        }
    }

    Ok((
        ApiSpec {
            title,
            version,
            base_url,
            prefix,
            operations,
            schemas: ctx.schemas,
        },
        ctx.warnings,
    ))
}

fn normalize_v2(root: &Value) -> Result<(ApiSpec, Vec<String>)> {
    let info = root.get("info");
    let title = info
        .and_then(|i| i.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("API")
        .to_string();
    let version = info
        .and_then(|i| i.get("version"))
        .and_then(Value::as_str)
        .unwrap_or("0.0.0")
        .to_string();

    let host = root.get("host").and_then(Value::as_str).unwrap_or("localhost");
    let scheme = root
        .get("schemes")
        .and_then(Value::as_array)
        .and_then(|s| s.first())
        .and_then(Value::as_str)
        .unwrap_or("https");
    let base_path = root.get("basePath").and_then(Value::as_str).unwrap_or("");
    let base_url = Some(format!("{scheme}://{host}"));
    let trimmed = base_path.trim_end_matches('/');
    let prefix = if trimmed.is_empty() {
        String::new()
    } else {
        format!("/{trimmed}")
    };

    let global_produces = produces_of(root);

    let mut ctx = Ctx {
        root,
        schemas: BTreeMap::new(),
        warnings: Vec::new(),
    };

    if let Some(defs) = root.get("definitions").and_then(Value::as_object) {
        for (name, v) in defs {
            let converted = ctx.convert(v);
            if !ctx.schemas.contains_key(name) {
                ctx.schemas.insert(name.clone(), converted);
            }
        }
    }

    let paths = root
        .get("paths")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("spec has no `paths` object"))?;

    let mut operations = Vec::new();
    for (path, item) in paths {
        let shared_params = item.get("parameters").cloned();
        for key in keys_of(item) {
            let Some(method) = HttpMethod::from_str(&key) else {
                continue;
            };
            let op = item.get(&key).expect("key came from object");

            // v2 classifies body/formData through parameter `in`, handled separately.
            let mut parameters = Vec::new();
            let mut body_schema = None;
            let mut form_fields: Vec<Field> = Vec::new();
            let mut has_file = false;
            for src in [shared_params.as_ref(), op.get("parameters")] {
                let arr = src.and_then(Value::as_array);
                let Some(arr) = arr else { continue };
                for p in arr {
                    let resolved: Value;
                    let p = match p.get("$ref").and_then(Value::as_str) {
                        Some(ptr) => {
                            resolved = resolve_ref(ctx.root, ptr)?.clone();
                            &resolved
                        }
                        None => p,
                    };
                    match p.get("in").and_then(Value::as_str) {
                        Some("body") => {
                            if body_schema.is_none() {
                                body_schema = Some(ctx.convert(&param_schema_view(p)));
                            }
                        }
                        Some("formData") => {
                            let wire = p
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("field")
                                .to_string();
                            let is_file =
                                p.get("type").and_then(Value::as_str) == Some("file");
                            let schema = if is_file {
                                has_file = true;
                                Schema::Binary
                            } else {
                                ctx.convert(&param_schema_view(p))
                            };
                            form_fields.push(Field {
                                wire_name: wire,
                                type_: TypeExpr {
                                    schema,
                                    nullable: false,
                                },
                                required: p
                                    .get("required")
                                    .and_then(Value::as_bool)
                                    .unwrap_or(false),
                                description: None,
                            });
                        }
                        _ => parameters.push(param_from_obj(p, &mut ctx)?),
                    }
                }
            }

            let request_body = if let Some(s) = body_schema {
                Some(RequestBody {
                    content: vec![(
                        op_consumes(op).unwrap_or_else(|| "application/json".into()),
                        s,
                    )],
                })
            } else if !form_fields.is_empty() {
                let media = if has_file {
                    "multipart/form-data"
                } else {
                    "application/x-www-form-urlencoded"
                };
                Some(RequestBody {
                    content: vec![(media.into(), Schema::Object(ObjectSchema { fields: form_fields }))],
                })
            } else {
                None
            };

            operations.push(Operation {
                method,
                path: path.clone(),
                operation_id: op.get("operationId").and_then(Value::as_str).map(String::from),
                summary: op.get("summary").and_then(Value::as_str).map(String::from),
                parameters,
                request_body,
                success: pick_success_v2(op.get("responses"), &global_produces, &mut ctx),
                error_schemas: collect_errors_v2(op.get("responses"), &mut ctx),
            });
        }
    }

    Ok((
        ApiSpec {
            title,
            version,
            base_url,
            prefix,
            operations,
            schemas: ctx.schemas,
        },
        ctx.warnings,
    ))
}

fn op_consumes(op: &Value) -> Option<String> {
    op.get("consumes")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .map(String::from)
}

fn produces_of(v: &Value) -> Option<String> {
    v.get("produces")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .map(String::from)
}

/// View a v2 parameter as a bare schema (drop parameter metadata).
fn param_schema_view(p: &Value) -> Value {
    let obj = match p.as_object() {
        Some(o) => o.clone(),
        None => return Value::Null,
    };
    let filtered: serde_json::Map<String, Value> = obj
        .into_iter()
        .filter(|(k, _)| {
            !matches!(
                k.as_str(),
                "name"
                    | "in"
                    | "description"
                    | "required"
                    | "collectionFormat"
                    | "allowEmptyValue"
                    | "uniqueItems"
            )
        })
        .collect();
    Value::Object(filtered)
}

fn params_from_value(v: Option<&Value>, ctx: &mut Ctx) -> Result<Vec<Parameter>> {
    let mut out = Vec::new();
    let Some(arr) = v.and_then(Value::as_array) else {
        return Ok(out);
    };
    for p in arr {
        let resolved: Value;
        let p = match p.get("$ref").and_then(Value::as_str) {
            Some(ptr) => {
                resolved = resolve_ref(ctx.root, ptr)?.clone();
                &resolved
            }
            None => p,
        };
        out.push(param_from_obj(p, ctx)?);
    }
    Ok(out)
}

fn param_from_obj(p: &Value, ctx: &mut Ctx) -> Result<Parameter> {
    let location = match p.get("in").and_then(Value::as_str) {
        Some("path") => Location::Path,
        Some("query") => Location::Query,
        Some("header") => Location::Header,
        Some("cookie") => Location::Cookie,
        other => bail!(
            "parameter `{}` has unsupported location {:?}",
            p.get("name").and_then(Value::as_str).unwrap_or("?"),
            other.unwrap_or("?")
        ),
    };
    // OAS3 keeps the schema under `schema`; v2 scalar parameters inline it.
    let schema = match p.get("schema") {
        Some(s) => ctx.convert(s),
        None => ctx.convert(&param_schema_view(p)),
    };
    Ok(Parameter {
        location,
        wire_name: p
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("param")
            .to_string(),
        required: p
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(matches!(location, Location::Path)),
        schema,
        description: p.get("description").and_then(Value::as_str).map(String::from),
    })
}

fn pick_success(responses: Option<&Value>, ctx: &mut Ctx) -> Option<SuccessResponse> {
    let map = responses?.as_object()?;
    for status in ["200", "201", "202", "203", "204", "206"] {
        if let Some(resp) = map.get(status) {
            if let Some(mt) = pick_media_v3(resp, ctx) {
                return Some(mt);
            }
        }
    }
    for (status, resp) in map {
        if status.starts_with('2') {
            if let Some(mt) = pick_media_v3(resp, ctx) {
                return Some(mt);
            }
        }
    }
    None
}

fn pick_media_v3(resp: &Value, ctx: &mut Ctx) -> Option<SuccessResponse> {
    let content = resp.get("content").and_then(Value::as_object)?;
    let keys: Vec<&String> = content.keys().collect();
    let entry = prefer_media(keys.into_iter())?;
    let mv = content.get(entry)?;
    let schema = mv.get("schema").map(|s| ctx.convert(s)).unwrap_or(Schema::Any);
    Some(SuccessResponse {
        media_type: entry.clone(),
        schema,
    })
}

fn collect_errors(responses: Option<&Value>, ctx: &mut Ctx) -> Vec<Schema> {
    let Some(map) = responses.and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (status, resp) in map {
        if !(status.starts_with('4') || status.starts_with('5') || status == "default") {
            continue;
        }
        if let Some(content) = resp.get("content").and_then(Value::as_object) {
            let keys: Vec<&String> = content.keys().collect();
            if let Some(entry) = prefer_media(keys.into_iter()) {
                if let Some(mv) = content.get(entry) {
                    if let Some(s) = mv.get("schema") {
                        out.push(ctx.convert(s));
                    }
                }
            }
        }
    }
    out
}

fn pick_success_v2(
    responses: Option<&Value>,
    global_produces: &Option<String>,
    ctx: &mut Ctx,
) -> Option<SuccessResponse> {
    let map = responses?.as_object()?;
    for status in ["200", "201", "202", "204"] {
        let Some(resp) = map.get(status) else {
            continue;
        };
        if let Some(schema_val) = resp.get("schema") {
            let mt = resp
                .get("produces")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(Value::as_str)
                .map(String::from)
                .or_else(|| global_produces.clone())
                .unwrap_or_else(|| "application/json".into());
            return Some(SuccessResponse {
                media_type: mt,
                schema: ctx.convert(schema_val),
            });
        }
    }
    None
}

fn collect_errors_v2(responses: Option<&Value>, ctx: &mut Ctx) -> Vec<Schema> {
    let Some(map) = responses.and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (status, resp) in map {
        if !(status.starts_with('4') || status.starts_with('5') || status == "default") {
            continue;
        }
        if let Some(s) = resp.get("schema") {
            out.push(ctx.convert(s));
        }
    }
    out
}

/// Media-type preference order used across versions.
fn prefer_media<'x>(keys: impl Iterator<Item = &'x String>) -> Option<&'x String> {
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

fn split_server_url(url: &str) -> (Option<String>, String) {
    match url.split_once("://") {
        Some((_, rest)) => match rest.find('/') {
            Some(idx) => {
                let path = &rest[idx..];
                (
                    Some(url[..url.len() - path.len()].to_string()),
                    path.trim_end_matches('/').to_string(),
                )
            }
            None => (Some(url.trim_end_matches('/').to_string()), String::new()),
        },
        None => {
            // Relative server URL: only a path.
            (None, url.trim_end_matches('/').to_string())
        }
    }
}
