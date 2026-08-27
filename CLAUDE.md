# feignhttp-generator

OpenAPI → feignhttp Rust 客户端代码生成器。

## Skills

本项目已配置以下 Claude Code 技能：

| 技能 | 用途 |
|------|------|
| `build-test` | 构建、测试和运行项目 |
| `codegen-workflow` | 使用 CLI / build.rs / 编程 API 生成代码 |
| `architecture` | 代码架构概览与开发指南 |
| `consumer-integration` | 作为构建依赖集成到下游项目 |

## 快速参考

```bash
# 构建
cargo build

# 测试
cargo test

# 生成客户端代码
cargo run -- generate -s openapi.json -o api.rs --layout module

# 运行消费者集成测试
cargo test -p consumer-test
```

调用技能：在对话中输入 `/skill-name`，例如 `/build-test`。