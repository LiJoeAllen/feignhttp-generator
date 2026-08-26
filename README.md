# feignhttp-generator

> 其他语言版本：[English](./doc/readme/README_en.md)

将 OpenAPI（2.0 / 3.0 / 3.1，JSON 或 YAML）转换为 [feignhttp](https://github.com/user/feignhttp) Rust 客户端代码的生成器。

读取 OpenAPI 规范并生成可直接编译的 Rust 模块或 crate，包含：
- 类型化的请求/响应模型（结构体、枚举）
- 按路径子树划分、以 `#[feign]` 装饰的 trait 定义
- 用于注入基础 URL 和前缀的共享 `ApiContext`

## 安装

```bash
cargo install feignhttp-generator
```

或作为构建依赖使用：

```toml
[build-dependencies]
feignhttp-generator = "0.1"
```

## 用法

### CLI

```bash
# module 布局（单个 .rs 文件，在 crate 中 include!）
feignhttp-generator generate -s openapi.json -o src/api.rs --layout module

# crate 布局（独立的 Cargo 项目）
feignhttp-generator generate -s openapi.json -o ./vending-api \
  --layout crate --package-name vending-api \
  --feignhttp-path ../feignhttp

# 通过 URL 读取远程规范
feignhttp-generator generate -s https://petstore3.swagger.io/api/v3/openapi.json -o petstore.rs
```

`spec` 字段在 CLI 和 `[package.metadata.feignhttp-generator]` 中均接受本地路径或 `http(s)://` URL。
对于远程规范，构建脚本会在每次重新构建时重新获取，并通过内容哈希跳过未变更的输出。

### build.rs（配置驱动）

```toml
# Cargo.toml
[build-dependencies]
feignhttp-generator = "0.1"

[package.metadata.feignhttp-generator]
spec = "openapi.json"
layout = "module"          # 或 "crate"
out = "feign_api.rs"       # module：OUT_DIR 下的文件；crate：输出目录
generate = true            # false = 冻结绑定，跳过生成
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

### 编程方式调用

```rust
use feignhttp_generator::{generate_from_reader, Options, Layout};

let options = Options {
    package_name: "my-api".into(),
    layout: Layout::Module,
    feignhttp_dep: Default::default(),
};
let files = generate_from_reader(std::io::Cursor::new(json_bytes), &options)?;
```

## 输出

### module 布局

单个 `.rs` 文件，包含：

```rust
pub mod models { /* ... */ }
pub struct ApiContext { /* ... */ }
pub mod device { /* trait Device { ... } */ }
pub mod index { /* trait Index { ... } */ }
```

### crate 布局

一个完整的 Cargo 项目：

```
vending-api/
  Cargo.toml          # 依赖：feignhttp, serde, serde_json
  src/
    lib.rs            # 重导出 + 各子模块的 include!
    models.rs         # 所有结构体/枚举
    device.rs         # trait Device
    index.rs          # trait Index
```

## 配置项

| 配置项 | CLI 参数 | 默认值 | 说明 |
|--------|----------|--------|------|
| `spec` | `-s, --spec` | （必填） | OpenAPI JSON/YAML 路径或 `http(s)://` URL |
| `out` | `-o, --out` | （必填） | 输出路径（module 为文件，crate 为目录） |
| `layout` | `--layout` | `module` | `module` 或 `crate` |
| `package_name` | `--package-name` | `generated-api` | crate 名称（仅 crate 布局） |
| `feignhttp_path` | `--feignhttp-path` | `feignhttp = "0.6"` | feignhttp 的本地路径依赖 |
| `generate` | — | `true` | 在 metadata 中设为 `false` 可冻结绑定 |

## 环境变量

| 变量 | 作用 |
|------|------|
| `FEIGNHTTP_GENERATOR_SKIP=1` | 在 build.rs 中跳过生成 |
| `FEIGNHTTP_OPENAPI_SKIP=1` | 同上（别名） |

## 生成代码的使用

```rust
use my_api::{ApiContext, Device, Index};
use feignhttp::FeignClientBuilder;

let ctx = ApiContext::new("https://api.example.com", "/v1");
let client = Index::builder().context(ctx).build()?;

let groups = client.get_device_groups(1, 20).await?;
```

当服务端返回错误响应时，`ApiError` 类型别名提供了共享的错误负载结构：

```rust
use my_api::ApiError;
if let Err(e) = result {
    // e.body() 包含原始错误 JSON
    let api_err: ApiError = serde_json::from_str(&e.body())?;
    eprintln!("error {}: {}", api_err.code, api_err.message);
}
```

## 支持的特性

- **HTTP 方法**：GET、POST、PUT、PATCH、HEAD、OPTIONS
- **参数**：path、query、header、cookie（通过 `#[path]`、`#[query]`、`#[header]`）
- **请求体**：JSON（`#[body]`）、表单（`#[form]`）、multipart（`#[file]`/`#[part]`）
- **响应类型**：JSON（结构体/枚举）、二进制（`Vec<u8>`）、单元类型 `()`
- **Schema 类型**：string（含枚举）、integer（int32/int64）、number（float/double）、boolean、array、object、`$ref`
- **路径截断**：`/device-groups/{groupId}/devices/{deviceId}/status` → 模块 `device_groups`
- **动词消歧**：同一路径上的 GET + POST → `get_device_groups` / `post_device_groups`
- **共享错误类型**：对错误响应 schema 进行多数投票 → `pub type ApiError = crate::models::...`

## 许可证

MIT
