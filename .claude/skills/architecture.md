---
name: architecture
description: Codebase architecture, module layout, and development guide for feignhttp-generator
---

# Architecture & Development

## Module Layout

```
src/
├── main.rs          # CLI entry point (clap-based)
├── lib.rs           # Public API: generate_from_reader, generate_from_source, load_spec
├── build.rs         # build.rs integration entry point
├── openapi.rs       # OpenAPI parser: parse_reader, normalize (v2 / v3)
├── ir.rs            # Internal IR types: ApiSpec, Operation, Schema, Parameter, etc.
├── mapper.rs        # Path-to-module mapping: split_path, disambiguate
├── naming.rs        # Identifier sanitization: snake_case, UpperCamel, keyword handling
└── codegen/
    ├── mod.rs       # Emission struct (feignhttp feature requirements)
    ├── api.rs       # Trait/method generation: generate, emit_method, return_type
    └── models.rs    # Model generation: ModelRegistry, struct/enum rendering
```

## Data Flow

```
OpenAPI Spec (JSON/YAML)
    │
    ▼
openapi.rs::parse_reader()    ────  serde_json::Value or serde_yaml::Value
    │
    ▼
openapi.rs::normalize()       ────  ir::ApiSpec (normalized IR)
    │
    ▼
mapper.rs::map_operations()   ────  Vec<MappedOperation> (dirs, module, trait, fn)
    │
    ▼
codegen::api::generate()     ────  CodegenOutput (models_src + api_files)
    │
    ▼
lib.rs::render_*()            ────  Module (single file) or Crate (directory tree)
```

## Key Design Decisions

### Internal Representation (`ir.rs`)
- A clean IR decoupled from OpenAPI JSON structure
- `Schema` enum: Ref, Object, Array, Str, Integer, Number, Boolean, Binary, Any
- `TypeExpr` wraps Schema + nullable flag for composition patterns
- `ObjectSchema` holds a flat `Vec<Field>` — no nesting

### OpenAPI Parsing (`openapi.rs`)
- Two normalization paths: `normalize_v3` (OAS 3.0/3.1) and `normalize_v2` (Swagger 2.0)
- Auto-detection via `openapi` vs `swagger` version field
- v2 body/formData parameters are converted to v3-style request body
- `allOf` branches merged into a single struct; later branches win on field conflicts
- `oneOf`/`anyOf` narrowed to first non-null branch (handles nullable-envelope idiom)
- Cyclic `$ref` references handled via placeholder-then-replace pattern

### Path Mapping (`mapper.rs`)
- Truncates at first `{placeholder}` segment — removes variable path parts
- `>=2` prefix segments: `dir/dir/file_trait/method_name`
- Single segment: `index/method_name`
- Empty prefix: `index/operationId_or_verb_root`
- Disambiguation: multi-verb methods on same path yield `{verb}_{base}`; duplicates yield `_2`, `_3`

### Code Generation (`codegen/`)
- `ModelRegistry` caches rendered types and synthesizes names for inline schemas
- `Emission` tracks required feignhttp features (json, multipart, serde_json_value)
- `fingerprint()` generates structural hashes for error schema deduplication
- Majority-vote error type yields `pub type ApiError = ...` alias

## Extending the Generator

### Adding a New Schema Type
1. Add variant to `Schema` enum in `ir.rs`
2. Handle parsing in `openapi.rs` (`convert_typed` or `convert_inner`)
3. Handle type expression in `codegen/models.rs` (`rust_type` method)
4. Handle parameter emission in `codegen/api.rs` (`emit_method`)

### Adding a New Output Layout
1. Add variant to `Layout` enum in `lib.rs`
2. Add rendering function in `lib.rs` (like `render_module_file` / `render_crate_tree`)
3. Wire up in `generate_from_source` and `build.rs`

### Adding a New HTTP Method
1. Add variant to `HttpMethod` enum in `ir.rs`
2. Update `from_str` / `as_str`
3. No codegen changes needed — methods are emitted dynamically

## Testing Strategy

- **Unit tests**: `naming.rs` has tests for identifier sanitization
- **Integration tests**: `consumer-test/` runs a full build.rs pipeline against Petstore spec
- **Runtime tests**: `consumer-test/tests/runtime.rs` exercises the generated code
- **Manual testing**: Use `cargo run -- generate` with various specs

## Common Development Tasks

### Add a new feature flag
1. Add field to `Emission` in `codegen/mod.rs`
2. Update `cargo_features()` and `summary()`
3. Set the flag in `codegen/api.rs::generate()`

### Rebuild on spec changes
The build script uses content hashing (`.feign_openapi.hash`) to skip regeneration when spec hasn't changed.