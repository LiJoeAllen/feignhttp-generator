# feignhttp-generator

OpenAPI (2.0 / 3.0 / 3.1, JSON or YAML) to [feignhttp](https://github.com/user/feignhttp) Rust client code generator.

Reads an OpenAPI specification and emits a ready-to-compile Rust module or crate containing:
- Typed request/response models (structs, enums)
- Per-path-subtree trait definitions decorated with `#[feign]`
- Shared `ApiContext` for base URL + prefix injection

## Installation

```bash
cargo install feignhttp-generator
```

Or use as a build dependency:

```toml
[build-dependencies]
feignhttp-generator = "0.1"
```

## Usage

### CLI

```bash
# Module layout (single .rs file, include! from your crate)
feignhttp-generator generate -s openapi.json -o src/api.rs --layout module

# Crate layout (standalone Cargo project)
feignhttp-generator generate -s openapi.json -o ./vending-api \
  --layout crate --package-name vending-api \
  --feignhttp-path ../feignhttp

# Remote spec via URL
feignhttp-generator generate -s https://petstore3.swagger.io/api/v3/openapi.json -o petstore.rs
```

The `spec` field accepts a local path or an `http(s)://` URL in both the CLI
and `[package.metadata.feignhttp-generator]`. For remote specs the build script
re-fetches on every rebuild and uses the content hash to skip unchanged output.

### build.rs (config-driven)

```toml
# Cargo.toml
[build-dependencies]
feignhttp-generator = "0.1"

[package.metadata.feignhttp-generator]
spec = "openapi.json"
layout = "module"          # or "crate"
out = "feign_api.rs"       # module: file in OUT_DIR; crate: output dir
generate = true            # false = freeze bindings, skip generation
```

```rust
// build.rs
fn main() {
    feignhttp_generator::build::run();
}
```

```rust
// src/lib.rs
include!(concat!(env!("OUT_DIR"), "/feign_api.rs"));
```

### Programmatic

```rust
use feignhttp_generator::{generate_from_reader, Options, Layout};

let options = Options {
    package_name: "my-api".into(),
    layout: Layout::Module,
    feignhttp_dep: Default::default(),
};
let files = generate_from_reader(std::io::Cursor::new(json_bytes), &options)?;
```

## Output

### Module layout

A single `.rs` file containing:

```rust
pub mod models { /* ... */ }
pub struct ApiContext { /* ... */ }
pub mod device { /* trait Device { ... } */ }
pub mod index { /* trait Index { ... } */ }
```

### Crate layout

A full Cargo project:

```
vending-api/
  Cargo.toml          # deps: feignhttp, serde, serde_json
  src/
    lib.rs            # re-exports + include! of sub-modules
    models.rs         # all structs/enums
    device.rs         # trait Device
    index.rs          # trait Index
```

## Options

| Option | CLI flag | Default | Description |
|--------|----------|---------|-------------|
| `spec` | `-s, --spec` | (required) | OpenAPI JSON/YAML path or `http(s)://` URL |
| `out` | `-o, --out` | (required) | Output path (file for module, dir for crate) |
| `layout` | `--layout` | `module` | `module` or `crate` |
| `package_name` | `--package-name` | `generated-api` | Crate name (crate layout only) |
| `feignhttp_path` | `--feignhttp-path` | `feignhttp = "0.6"` | Local path dep for feignhttp |
| `generate` | — | `true` | Set `false` in metadata to freeze bindings |

## Environment

| Variable | Effect |
|----------|--------|
| `FEIGNHTTP_GENERATOR_SKIP=1` | Skip generation in build.rs |
| `FEIGNHTTP_OPENAPI_SKIP=1` | Same (alias) |

## Generated usage

```rust
use my_api::{ApiContext, Device, Index};
use feignhttp::FeignClientBuilder;

let ctx = ApiContext::new("https://api.example.com", "/v1");
let client = Index::builder().context(ctx).build()?;

let groups = client.get_device_groups(1, 20).await?;
```

When the server returns an error response, the `ApiError` type alias provides the shared error payload schema:

```rust
use my_api::ApiError;
if let Err(e) = result {
    // e.body() contains the raw error JSON
    let api_err: ApiError = serde_json::from_str(&e.body())?;
    eprintln!("error {}: {}", api_err.code, api_err.message);
}
```

## Supported features

- **Methods**: GET, POST, PUT, PATCH, HEAD, OPTIONS
- **Parameters**: path, query, header, cookie (via `#[path]`, `#[query]`, `#[header]`)
- **Request bodies**: JSON (`#[body]`), form (`#[form]`), multipart (`#[file]`/`#[part]`)
- **Response types**: JSON (struct/enum), binary (`Vec<u8>`), unit `()`
- **Schema types**: string (with enum), integer (int32/int64), number (float/double), boolean, array, object, `$ref`
- **Path truncation**: `/device-groups/{groupId}/devices/{deviceId}/status` → module `device_groups`
- **Verb disambiguation**: GET + POST on same path → `get_device_groups` / `post_device_groups`
- **Shared error type**: majority-vote schema across error responses → `pub type ApiError = crate::models::...`

## License

MIT
