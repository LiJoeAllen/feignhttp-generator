---
name: codegen-workflow
description: Generate feignhttp Rust client code from OpenAPI specs using CLI, build.rs, or programmatic API
---

# Code Generation Workflow

## Overview

`feignhttp-generator` reads OpenAPI specs (2.0 / 3.0 / 3.1, JSON or YAML) and generates Rust client code for the [feignhttp](https://github.com/user/feignhttp) framework.

## CLI Usage

### Module Layout

Single `.rs` file with nested `pub mod` tree — ideal for `include!`:

```bash
cargo run -- generate \
  -s openapi.json \
  -o src/api.rs \
  --layout module
```

### Crate Layout

Standalone Cargo project — ideal for path dependency:

```bash
cargo run -- generate \
  -s https://petstore3.swagger.io/api/v3/openapi.json \
  -o ./vending-api \
  --layout crate \
  --package-name vending-api \
  --feignhttp-path ../feignhttp
```

### CLI Options

| Flag | Description |
|------|-------------|
| `-s, --spec` | Path or http(s) URL of the OpenAPI spec (required) |
| `-o, --out` | Output target: file (module) or directory (crate) (required) |
| `--layout` | `module` (default) or `crate` |
| `--package-name` | Crate name for crate layout (default: `generated-api`) |
| `--feignhttp-path` | Local path dependency for feignhttp |

## Build Script (build.rs)

### Setup

Add to `Cargo.toml`:
```toml
[build-dependencies]
feignhttp-generator = "0.1"

[package.metadata.feignhttp-generator]
spec = "openapi.json"
layout = "module"
out = "feign_api.rs"
generate = true
```

Add `build.rs`:
```rust
fn main() {
    feignhttp_generator::build::run();
}
```

Include in your crate:
```rust
include!(concat!(env!("OUT_DIR"), "/feign_api.rs"));
```

### Metadata Options

| Field | Default | Description |
|-------|---------|-------------|
| `spec` | (required) | OpenAPI spec path or URL |
| `layout` | `module` | `module` or `crate` |
| `out` | `feign_api.rs` | Output filename (module) or directory (crate) |
| `package_name` | `generated-api` | For crate layout |
| `feignhttp_path` | `feignhttp = "0.6"` | Local path or version |
| `generate` | `true` | Set to `false` to freeze bindings |

## Programmatic API

```rust
use feignhttp_generator::{generate_from_reader, Options, Layout, FeignDep};

let options = Options {
    package_name: "my-api".into(),
    layout: Layout::Module,
    feignhttp_dep: FeignDep::default(),
};

let bytes = std::fs::read("openapi.json")?;
let files = generate_from_reader(std::io::Cursor::new(bytes), &options)?;
for (path, content) in &files {
    std::fs::write(path, content)?;
}
```

## Using Generated Code

```rust
use my_api::{ApiContext, Device, Index};
use feignhttp::FeignClientBuilder;

let ctx = ApiContext::new("https://api.example.com", "/v1");
let client = Index::builder().context(ctx).build()?;
let groups = client.get_device_groups(1, 20).await?;
```

### Error Handling

```rust
use my_api::ApiError;
if let Err(e) = result {
    let api_err: ApiError = serde_json::from_str(&e.body())?;
    eprintln!("error {}: {}", api_err.code, api_err.message);
}
```

## Supported Features

- **HTTP methods**: GET, POST, PUT, PATCH, HEAD, OPTIONS
- **Parameters**: path, query, header, cookie
- **Request bodies**: JSON (`#[body]`), form (`#[form]`), multipart (`#[file]`/`#[part]`), octet-stream
- **Response types**: JSON structs/enums, `Vec<u8>`, `String`, `()`
- **Schema types**: string (incl. enum), integer (int32/int64), number (float/double), boolean, array, object, `$ref`
- **AllOf composition**: merged into single structs
- **OneOf/AnyOf**: narrowed to first non-null branch
- **Swagger 2.0**: full support (body/formData parameters, `definitions`)

## Output Structure

### Module Layout
```
feign_api.rs
├── pub mod models { ... }
├── pub struct ApiContext { ... }
├── pub mod device_groups { trait DeviceGroups { ... } }
└── pub mod index { trait Index { ... } }
```

### Crate Layout
```
vending-api/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── models.rs
    ├── device_groups.rs
    └── index.rs
```

## Path Mapping Rules

- Paths truncated at first `{placeholder}` segment
- `>=2` segments before truncation: last = method name, second-to-last = file/trait, rest = directories
- `1` segment: file/trait = `index`, segment = method name
- `0` segments: file/trait = `index`, name derived from `operationId` or HTTP verb
- Same-name methods on same path with different verbs → `get_foo` / `post_foo` disambiguation