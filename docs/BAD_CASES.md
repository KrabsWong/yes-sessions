# 开发 Bad Case 记录（归档）

本文原记录早期 CC Switch/Electron 重写过程中的问题，许多示例已经不再符合当前 Yes Sessions 的代码结构。

仍然有效的项目约束已经迁移或保留在：

- `AGENTS.md`
- `README.md`
- `docs/README.md`
- `DESIGN.md`

当前特别需要继续遵守的经验：

- UI 文案统一维护在 `crates/yes-app/src/i18n.rs`，同时提供中文和英文。
- provider 解析与路径边界留在 `yes-core`，UI 不直接读取外部会话文件。
- 历史计划文档不能作为当前功能事实来源。
