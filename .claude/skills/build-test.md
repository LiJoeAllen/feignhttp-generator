---
name: build-test
description: Build, test, and run the feignhttp-generator project
---

# Build & Test

## Prerequisites

- Rust toolchain (edition 2021)
- The project is a Cargo workspace with two members: `feignhttp-generator` (root) and `consumer-test`

## Build

```bash
# Build the generator binary
cargo build

# Build in release mode
cargo build --release

# Build all workspace members
cargo build --workspace
```

## Run Tests

```bash
# Run all unit tests
cargo test

# Run tests with output
cargo test -- --nocapture

# Run a specific test
cargo test test_name

# Run tests for a specific package
cargo test -p feignhttp-generator

# Run naming tests
cargo test -p feignhttp-generator -- naming
```

## Run the CLI

```bash
# Generate from a local OpenAPI file (module layout)
cargo run -- generate -s openapi.json -o src/api.rs --layout module

# Generate from a remote spec (crate layout)
cargo run -- generate \
  -s https://petstore3.swagger.io/api/v3/openapi.json \
  -o ./petstore-api \
  --layout crate \
  --package-name petstore-api

# Generate with custom feignhttp path dependency
cargo run -- generate \
  -s openapi.json \
  -o src/api.rs \
  --layout module \
  --feignhttp-path ../feignhttp

# See all CLI options
cargo run -- generate --help
```

## Consumer Test

The `consumer-test/` workspace member is an integration test that:
1. Uses the generator as a build dependency (`build.rs`)
2. Downloads the Petstore 3.0 spec
3. Generates a module-layout client
4. Runs runtime tests against the generated code

```bash
# Run the consumer test (build + test)
cargo test -p consumer-test

# Run only the runtime integration tests
cargo test -p consumer-test --test runtime
```

## Environment Variables

| Variable | Effect |
|----------|--------|
| `FEIGNHTTP_GENERATOR_SKIP=1` | Skip generation in build.rs |
| `FEIGNHTTP_OPENAPI_SKIP=1` | Same as above (alias) |

## Skip Checks

To skip tests and just verify compilation:
```bash
cargo build --workspace
```

## Watch Mode

```bash
cargo watch -x 'test -p feignhttp-generator'
```