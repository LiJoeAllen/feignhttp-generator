//! `feignhttp-generator` turns any OpenAPI specification (JSON / YAML,
//! Swagger 2.0 / OpenAPI 3.0 / 3.1) into Rust client code built on
//! [feignhttp](https://docs.rs/feignhttp).
//!
//! Two output layouts:
//! - [`Layout::Module`] – one self-contained file (nested `pub mod` tree),
//!   intended for `include!(concat!(env!("OUT_DIR"), "/feign_api.rs"))`.
//! - [`Layout::Crate`] – a full standalone crate tree (`Cargo.toml`, `src/**`),
//!   consumed as an ordinary path dependency.
//!
//! Runtime base URL switching works through the generated `ApiContext`
//! (`#[url_path]` fields substituted into the `{base_url}{prefix}` template).

pub mod build;
mod codegen;
mod ir;
mod mapper;
mod naming;
mod openapi;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

pub use codegen::api::CodegenOutput;

/// Output shape of the generated bindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Single-file nested-module bindings for `include!`.
    Module,
    /// Standalone multi-file crate.
    Crate,
}

/// How the generated crate depends on feignhttp.
#[derive(Debug, Clone)]
pub enum FeignDep {
    Version(String),
    Path(String),
}

impl Default for FeignDep {
    fn default() -> Self {
        FeignDep::Version("0.6".to_string())
    }
}

/// Generation options.
#[derive(Debug, Clone)]
pub struct Options {
    /// Package name used by `Layout::Crate`.
    pub package_name: String,
    pub layout: Layout,
    pub feignhttp_dep: FeignDep,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            package_name: "generated-api".to_string(),
            layout: Layout::Module,
            feignhttp_dep: FeignDep::default(),
        }
    }
}

/// Parse and normalize a spec, then render bindings. Returns
/// `(relative paths, contents)` without touching the filesystem.
pub fn generate_from_reader(
    reader: impl std::io::Read,
    options: &Options,
) -> Result<Vec<(PathBuf, String)>> {
    let root = openapi::parse_reader(reader)?;
    let (spec, warnings) = openapi::normalize(&root)?;
    for w in &warnings {
        eprintln!("warning: {w}");
    }
    let mapped =
        mapper::map_operations(spec.operations.clone()).map_err(anyhow::Error::msg)?;
    let out = codegen::api::generate(&spec, &mapped);

    match options.layout {
        Layout::Module => Ok(vec![(
            PathBuf::from("feign_api.rs"),
            render_module_file(&spec, &out),
        )]),
        Layout::Crate => Ok(render_crate_tree(options, &spec, &out)),
    }
}

/// Convenience wrapper over [`generate_from_reader`] for in-memory specs.
pub fn generate_from_str(spec: &str, options: &Options) -> Result<Vec<(PathBuf, String)>> {
    generate_from_reader(std::io::Cursor::new(spec.to_owned()), options)
}

/// Load spec bytes from a local path or an `http(s)://` URL.
pub fn load_spec(source: &str) -> Result<Vec<u8>> {
    if source.starts_with("http://") || source.starts_with("https://") {
        let body = ureq::get(source)
            .call()
            .with_context(|| format!("cannot fetch spec {source}"))?
            .into_string()
            .with_context(|| format!("cannot read response body of {source}"))?;
        Ok(body.into_bytes())
    } else {
        std::fs::read(source).with_context(|| format!("cannot open spec {source}"))
    }
}

/// Like [`generate`], but `spec_source` may be a local path or an
/// `http(s)://` URL.
pub fn generate_from_source(
    spec_source: &str,
    out_root: impl AsRef<Path>,
    options: &Options,
) -> Result<()> {
    let bytes = load_spec(spec_source)?;
    let files = generate_from_reader(std::io::Cursor::new(bytes), options)?;

    let out_root = out_root.as_ref();
    match options.layout {
        Layout::Module => {
            // `out_root` IS the target file for module bindings.
            if let Some(parent) = out_root.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let (_, content) = files
                .first()
                .context("module layout produced no output file")?;
            std::fs::write(out_root, content)
                .with_context(|| format!("cannot write {}", out_root.display()))?;
        }
        Layout::Crate => {
            std::fs::create_dir_all(out_root)?;
            for (rel, content) in &files {
                let target = out_root.join(rel);
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&target, content)
                    .with_context(|| format!("cannot write {}", target.display()))?;
            }
        }
    }
    Ok(())
}

const CONTEXT_SRC: &str = r#"/// Shared runtime configuration injected into every generated client.
///
/// * `base_url` - scheme and host, e.g. `https://api.example.com`
/// * `prefix` - path portion declared by the spec's server entry,
///   e.g. `/dmgt-api/v1`; empty when the spec has none.
#[derive(Clone, ::feignhttp::Context)]
pub struct ApiContext {
    #[url_path]
    pub base_url: String,
    #[url_path]
    pub prefix: String,
}

impl ApiContext {
    pub fn new(base_url: impl Into<String>, prefix: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            prefix: prefix.into(),
        }
    }
}"#;

/// Context source plus a spec-derived `Default` when the spec names a server.
fn context_src(spec: &crate::ir::ApiSpec) -> String {
    let mut s = String::from(CONTEXT_SRC);
    if let Some(base) = &spec.base_url {
        s.push_str(&format!(
            "\n\nimpl Default for ApiContext {{\n    fn default() -> Self {{\n        Self::new(\"{base}\", \"{}\")\n    }}\n}}",
            escape_str(&spec.prefix)
        ));
    }
    s
}

fn escape_str(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn header_docs(spec_title: &str, spec_version: &str) -> String {
    format!(
        "// @generated by feignhttp-generator v{}. DO NOT EDIT.\n\
         // Source spec: `{title}` v{ver}\n\
         //\n\
         // Callers need `use feignhttp::FeignClientBuilder as _;` in scope for\n\
         // `<Trait>::builder().context(ApiContext::new(base, prefix)).build()`.\n",
        env!("CARGO_PKG_VERSION"),
        title = spec_title,
        ver = spec_version,
    )
}

fn indent_block(src: &str, level: usize) -> String {
    let pad = "    ".repeat(level);
    src.lines()
        .map(|l| {
            if l.trim().is_empty() {
                String::from("\n")
            } else {
                format!("{pad}{l}\n")
            }
        })
        .collect()
}

/// Render the whole module-layout binding file (nested mods, single include).
fn render_module_file(spec: &crate::ir::ApiSpec, out: &CodegenOutput) -> String {
    use std::fmt::Write as _;

    let mut s = String::new();
    s.push_str(&header_docs(&spec.title, &spec.version));

    let _ = writeln!(s, "pub mod models {{");
    s.push_str(&indent_block(&out.models_src, 1));
    let _ = writeln!(s, "}}");

    s.push_str(&context_src(spec));
    s.push('\n');

    // Nest api files into their directory chain.
    let mut root: Node = Node::Dir(BTreeMap::new());
    for f in &out.api_files {
        insert_file(&mut root, &f.path, &f.content);
    }
    s.push_str(&render_nodes(root, 0));
    s
}

enum Node {
    Dir(BTreeMap<String, Node>),
    File(String),
}

fn insert_file(root: &mut Node, path: &[String], content: &str) {
    let mut cur = root;
    for seg in path[..path.len() - 1].iter() {
        let next = match cur {
            Node::Dir(map) => map.entry(seg.clone()).or_insert_with(|| Node::Dir(BTreeMap::new())),
            Node::File(_) => unreachable!("file cannot contain children"),
        };
        cur = next;
    }
    if let Node::Dir(map) = cur {
        map.insert(path[path.len() - 1].clone(), Node::File(content.to_string()));
    }
}

fn render_nodes(node: Node, level: usize) -> String {
    let mut s = String::new();
    let pad = "    ".repeat(level);
    if let Node::Dir(map) = node {
        for (name, child) in map {
            match child {
                Node::Dir(_) => {
                    let _ = writeln!(s, "{pad}pub mod {name} {{");
                    s.push_str(&render_nodes(child, level + 1));
                    let _ = writeln!(s, "{pad}}}");
                }
                Node::File(content) => {
                    let _ = writeln!(s, "{pad}pub mod {name} {{");
                    s.push_str(&indent_block(&content, level + 1));
                    let _ = writeln!(s, "{pad}}}");
                }
            }
        }
    }
    s
}

/// Render the standalone crate tree for `Layout::Crate`.
fn render_crate_tree(options: &Options, spec: &crate::ir::ApiSpec, out: &CodegenOutput) -> Vec<(PathBuf, String)> {
    let features = out.emission.cargo_features().join(", ");
    let feign_dep = match &options.feignhttp_dep {
        FeignDep::Version(v) => format!("version = \"{v}\""),
        FeignDep::Path(p) => format!("path = \"{}\"", p.replace('\\', "\\\\")),
    };
    let _ = &features;

    let cargo = format!(
        "# Generated by feignhttp-generator v{}. DO NOT EDIT.\n\
         [package]\n\
         name = \"{}\"\n\
         version = \"0.1.0\"\n\
         edition = \"2021\"\n\
         \n\
         [dependencies]\n\
         feignhttp = {{ {feign_dep}, features = [{features}] }}\n\
         serde = {{ version = \"1\", features = [\"derive\"] }}\n\
         serde_json = \"1\"\n",
        env!("CARGO_PKG_VERSION"),
        options.package_name,
    );

    let mut lib = String::new();
    lib.push_str(&header_docs(&spec.title, &spec.version));
    lib.push_str("#![allow(clippy::too_many_arguments)]\n\n");
    lib.push_str("pub mod models;\n\n");

    // Declare the module tree.
    let mut dirs: BTreeMap<Vec<String>, ()> = BTreeMap::new();
    for f in &out.api_files {
        dirs.insert(f.path.clone(), ());
    }
    let mut tree: BTreeMap<String, Node2> = BTreeMap::new();
    for path in dirs.keys() {
        insert_node(&mut tree, path);
    }
    lib.push_str(&render_decls(&tree, 0));
    lib.push('\n');
    lib.push_str(&context_src(spec));
    lib.push('\n');

    let mut files: Vec<(PathBuf, String)> = vec![
        (PathBuf::from("Cargo.toml"), cargo),
        (PathBuf::from("src/lib.rs"), lib),
        (PathBuf::from("src/models.rs"), out.models_src.clone()),
    ];
    for f in &out.api_files {
        let mut p = PathBuf::from("src");
        for seg in &f.path {
            p.push(seg);
        }
        p.set_extension("rs");
        files.push((p, f.content.clone()));
    }
    files
}

enum Node2 {
    Dir(BTreeMap<String, Node2>),
    Leaf,
}

fn insert_node(tree: &mut BTreeMap<String, Node2>, path: &[String]) {
    let mut cur = tree;
    for (i, seg) in path.iter().enumerate() {
        let last = i + 1 == path.len();
        let entry = cur.entry(seg.clone()).or_insert_with(|| {
            if last {
                Node2::Leaf
            } else {
                Node2::Dir(BTreeMap::new())
            }
        });
        match entry {
            Node2::Dir(map) => {
                cur = map;
            }
            Node2::Leaf => break,
        }
    }
}

fn render_decls(tree: &BTreeMap<String, Node2>, level: usize) -> String {
    let mut s = String::new();
    let pad = "    ".repeat(level);
    for (name, node) in tree {
        match node {
            Node2::Leaf => {
                s.push_str(&format!("{pad}pub mod {name};\n"));
            }
            Node2::Dir(children) => {
                s.push_str(&format!("{pad}pub mod {name} {{\n"));
                s.push_str(&render_decls(children, level + 1));
                s.push_str(&format!("{pad}}}\n"));
            }
        }
    }
    s
}
