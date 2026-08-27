# Changelog

## 0.1.0+1 (2026-08-27)

- 修复：将 `escape()` 函数提取到 `naming` 模块，消除三处重复代码
- 新增：`#[non_exhaustive]` 属性添加到生成的枚举类型
- 新增：`deprecated` 标记从 OpenAPI 规范传递到生成的 trait 方法
- 新增：`mapper.rs`、`openapi.rs`、`codegen/api.rs` 单元测试
- 新增：`rustfmt.toml`、`clippy.toml` 配置文件
- 新增：GitHub Actions CI 工作流
- 新增：`CHANGELOG.md`
- 变更：`Cargo.lock` 不再跟踪（库 crate）
- 文档：同步英文 README 配置表

## 0.1.0-26.8.26 (2026-08-26)

- 初始发布版本
- 支持 OpenAPI 2.0 / 3.0 / 3.1
- 支持 Module 和 Crate 两种输出布局
- 支持 CLI、build.rs、编程 API 三种使用方式