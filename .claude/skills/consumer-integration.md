---
name: consumer-integration
description: Integrate feignhttp-generator as a build dependency in your Rust project
---

# Consumer Integration

## Quick Start

Add to your `Cargo.toml`:

```toml
[build-dependencies]
feignhttp-generator = "0.1"

[dependencies]
feignhttp = { git = "https://github.com/LiJoeAllen/feignhttp", branch = "dev", features = ["reqwest-client", "reqwest-json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"

[package.metadata.feignhttp-generator]
spec = "openapi.json"
layout = "module"
out = "feign_api.rs"
generate = true
```

Create `build.rs`:

```rust
fn main() {
    feignhttp_generator::build::run();
}
```

Include in your crate:

```rust
include!(concat!(env!("OUT_DIR"), "/feign_api.rs"));
```

## Configuration Reference

### Cargo.toml Metadata

The `[package.metadata.feignhttp-generator]` section supports:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `spec` | string | (required) | Path or `http(s)://` URL of the OpenAPI spec |
| `layout` | string | `"module"` | `"module"` or `"crate"` |
| `out` | string | `"feign_api.rs"` | Output filename (module) or directory (crate) |
| `package_name` | string | `"generated-api"` | Crate name for `--layout crate` |
| `feignhttp_path` | string | — | Path dependency for feignhttp (e.g. `"../feignhttp"`) |
| `generate` | bool | `true` | Set `false` to freeze bindings |

### Spec Source Resolution

- **Local paths** are resolved relative to the consuming crate's directory (`CARGO_MANIFEST_DIR`)
- **HTTP(S) URLs** are fetched on each build, with content-hash caching to skip unchanged specs
- Cargo's `rerun-if-changed` is set for local spec files; for remote specs, the manifest path is watched

## Layouts

### Module Layout (default)
Single generated file with nested `pub mod` blocks. Include via:
```rust
include!(concat!(env!("OUT_DIR"), "/feign_api.rs"));
```

### Crate Layout
Standalone Cargo project. Add as a path dependency:
```toml
[dependencies]
generated-api = { path = "path/to/generated-api" }
```

## Freezing Generated Code

Set `generate = false` in metadata to stop regeneration. The last generated files remain on disk and can be checked into version control:

```toml
[package.metadata.feignhttp-generator]
spec = "openapi.json"
out = "feign_api.rs"
generate = false
```

## Using the Generated Client

```rust
use my_api::{ApiContext, DeviceGroups, Index};
use feignhttp::FeignClientBuilder as _;

let ctx = ApiContext::new("https://api.example.com", "/v1");

// Create a client for a generated trait
let client = Index::builder().context(ctx.clone()).build()?;

// Call methods
let result = client.get_device_groups(1, 20).await?;

// Type-safe error handling
use my_api::ApiError;
if let Err(e) = result {
    if let Some(body) = e.body() {
        if let Ok(api_err) = serde_json::from_str::<ApiError>(&body) {
            eprintln!("API error {}: {}", api_err.code, api_err.message);
        }
    }
}
```

## Example: Petstore Consumer

See `consumer-test/` in this repo for a complete working example:

- `Cargo.toml` — build dependency + metadata configuration
- `build.rs` — calls `feignhttp_generator::build::run()`
- `src/lib.rs` — includes generated code via `include!`
- `tests/runtime.rs` — integration tests against the generated client

## Troubleshooting

### "OUT_DIR not set"
Ensure `build.rs` is present and correctly configured. The `OUT_DIR` env var is only available during Cargo builds.

### Spec parsing fails
- Verify the spec is valid JSON or YAML
- Check for `openapi: 3.x` or `swagger: 2.0` version field
- Use `--layout module` with `-o /dev/null` to test parsing without writing

### Generated code doesn't compile
- Ensure `feignhttp` dependency is correct with the right features
- For crate layout, verify the generated `Cargo.toml` has correct paths
- Run `cargo check` in the generated crate directory

### Build script skips generation
- Check `FEIGNHTTP_GENERATOR_SKIP` or `FEIGNHTTP_OPENAPI_SKIP` env vars
- Ensure `generate` is not set to `false` in metadata
- Check `.feign_openapi.hash` for content-hash caching