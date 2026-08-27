//! Build-script entry point for config-driven generation (plan §8).
//!
//! Consumer setup:
//!
//! ```toml
//! [build-dependencies]
//! feignhttp-generator = "0.1"
//!
//! [package.metadata.feignhttp-generator]
//! spec = "openapi.json"    # local path or http(s):// URL
//! layout = "module"        # or "crate"
//! out = "feign_api.rs"     # module: file name in OUT_DIR; crate: output dir
//! generate = true          # false freezes the bindings
//! ```
//!
//! ```rust
//! // build.rs
//! fn main() { feignhttp_generator::build::run(); }
//! ```

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

/// Entry point intended to be called from a consumer's `build.rs`.
pub fn run() {
    if let Err(e) = try_run() {
        panic!("feignhttp-generator: {e:#}");
    }
}

fn try_run() -> Result<()> {
    if std::env::var_os("FEIGNHTTP_GENERATOR_SKIP").is_some() || std::env::var_os("FEIGNHTTP_OPENAPI_SKIP").is_some() {
        return Ok(());
    }

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").context("CARGO_MANIFEST_DIR not set")?);
    let manifest_path = manifest_dir.join("Cargo.toml");
    let raw =
        std::fs::read_to_string(&manifest_path).with_context(|| format!("cannot read {}", manifest_path.display()))?;
    let parsed: toml::Value =
        toml::from_str(&raw).with_context(|| format!("cannot parse {}", manifest_path.display()))?;

    let Some(cfg) = parsed.get("package").and_then(|p| p.get("metadata")).and_then(|m| m.get("feignhttp-generator"))
    else {
        // No generator configuration: nothing to do.
        return Ok(());
    };

    let generate = cfg.get("generate").and_then(toml::Value::as_bool).unwrap_or(true);
    if !generate {
        println!("cargo:warning=feignhttp-generator: generate = false, skipping");
        return Ok(());
    }

    let spec_cfg = cfg
        .get("spec")
        .and_then(toml::Value::as_str)
        .context("[package.metadata.feignhttp-generator] is missing `spec`")?
        .to_string();
    let is_remote = spec_cfg.starts_with("http://") || spec_cfg.starts_with("https://");
    // Resolve local paths against the consuming crate's directory; URLs pass through.
    let spec_src = if is_remote { spec_cfg.clone() } else { manifest_dir.join(&spec_cfg).display().to_string() };
    if !is_remote {
        println!("cargo:rerun-if-changed={}", spec_src);
    } else {
        // Cargo cannot watch URLs; the hash check below handles change detection.
        println!("cargo:rerun-if-changed={}", manifest_path.display());
    }
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    println!("cargo:rerun-if-env-changed=FEIGNHTTP_GENERATOR_SKIP");

    let layout = cfg.get("layout").and_then(toml::Value::as_str).unwrap_or("module");
    let out_cfg = cfg.get("out").and_then(toml::Value::as_str);
    let package_name = cfg.get("package_name").and_then(toml::Value::as_str).unwrap_or("generated-api").to_string();
    let feignhttp_path = cfg.get("feignhttp_path").and_then(toml::Value::as_str);

    let options = crate::Options {
        package_name,
        layout: match layout {
            "crate" => crate::Layout::Crate,
            _ => crate::Layout::Module,
        },
        feignhttp_dep: match feignhttp_path {
            Some(p) => crate::FeignDep::Path(p.to_string()),
            None => crate::FeignDep::default(),
        },
    };

    let mut hasher = DefaultHasher::new();
    let spec_bytes = crate::load_spec(&spec_src)?;
    spec_bytes.hash(&mut hasher);
    format!("{options:?}").hash(&mut hasher);
    let hash = format!("{:x}", hasher.finish());

    match options.layout {
        crate::Layout::Module => {
            let out_dir = PathBuf::from(std::env::var("OUT_DIR").context("OUT_DIR not set")?);
            let out_file = out_dir.join(out_cfg.unwrap_or("feign_api.rs"));
            let hash_file = out_dir.join(".feign_openapi.hash");
            if up_to_date(&hash_file, &hash) && out_file.exists() {
                return Ok(());
            }
            let files = crate::generate_from_reader(std::io::Cursor::new(spec_bytes), &options)?;
            for (_, content) in &files {
                std::fs::write(&out_file, content).with_context(|| format!("cannot write {}", out_file.display()))?;
            }
            std::fs::write(&hash_file, hash)?;
        }
        crate::Layout::Crate => {
            let out_root = manifest_dir.join(out_cfg.unwrap_or("generated"));
            let hash_file = out_root.join(".feign_openapi.hash");
            if up_to_date(&hash_file, &hash) {
                return Ok(());
            }
            crate::generate_from_source(&spec_src, &out_root, &options)?;
            std::fs::create_dir_all(&out_root)?;
            std::fs::write(&hash_file, hash)?;
        }
    }
    Ok(())
}

fn up_to_date(hash_file: &Path, current: &str) -> bool {
    std::fs::read_to_string(hash_file).map(|h| h == current).unwrap_or(false)
}
