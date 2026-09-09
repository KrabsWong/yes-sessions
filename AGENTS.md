# AGENTS.md

## 项目概述

Yes Sessions 是一款仅面向 macOS 的 AI CLI 会话管理器，使用 Rust、GPUI Kit 和系统 WebKit 实现。

## 技术栈

- Rust 2024 edition
- GPUI Kit / GPUI Component
- `gpui-wry` + WKWebView（仅用于 Mermaid）
- rusqlite（OpenCode 数据）
- serde / serde_json（JSON 与 JSONL 数据）

项目保持纯 Rust 原生架构，不得引入浏览器或 JavaScript 运行时，也不得使用通用 WebView 页面。原生 UI 优先使用 GPUI Kit 组件；只有 Mermaid 内容可以进入系统 WKWebView。

## 目录结构

```text
crates/yes-core/    无 UI 的领域模型、provider、设置与系统服务
crates/yes-app/     GPUI 应用和视图
packaging/          macOS bundle 元数据
scripts/            构建与打包脚本
```

## 约定

- UI 文案集中在 `crates/yes-app/src/i18n.rs`，新增文案必须同时提供中文和英文。
- provider 解析逻辑放在 `yes-core`，不要让 UI 直接读取外部会话文件。
- 列表读取必须保持轻量；大型 JSONL 文件只在打开详情时完整解析。
- 文件路径必须进行边界检查，外部命令参数必须安全转义。
- 保持 macOS 13.0 为最低系统版本。
- 新依赖需评估 release 二进制体积和许可证。

## 验证

```bash
cargo fmt --all --check
cargo check --workspace
cargo test --workspace
./scripts/package-macos.sh
```

打包后还需执行 `codesign --verify --deep --strict`，无证书时允许 ad-hoc 签名分发，并说明首次启动的手动放行步骤；配置完整分发凭据时完成 Developer ID 签名、notarization 和 stapling。

## 提交与发布流程

- 常规迭代必须通过功能分支和 PR 进入 `main`，不得直接推送 `main`。用户要求“提交并推送”时，也应按此流程执行。
- 从最新 `main` 创建 `feature/` 分支，完成验证和提交前审查后提交；每个非空提交都必须有说明具体改动和验证结果的正文。
- 每次推送前扫描将上传的完整 Git 历史中的密钥和凭据，不能只检查工作区或最终 diff；发现疑似密钥时停止推送并请求确认，不得输出完整密钥。
- 推送功能分支后，`.github/workflows/auto-create-pr.yml` 会尝试自动创建指向 `main` 的 PR。确认 PR 已创建，必要时手动补建，避免重复创建。
- 合并前为 PR 添加且仅添加一个版本标签：`major`（重大版本升级）、`minor`（新功能）、`patch`（修复或小版本发布）、`skip-release`（无需发版的文档或配置改动）。不要依赖缺少标签时的默认行为。
- 提交 PR 后返回 PR 链接及版本标签；没有用户的合并授权时，不要自行合并。
- PR 合并后，`.github/workflows/auto-version-bump.yml` 按标签更新 Cargo 版本并生成、推送 `vMAJOR.MINOR.PATCH` Git tag；`skip-release` 会跳过发版。若 PR 已准备一个尚未打 tag 的版本，工作流会直接发布该版本。
- 版本 tag 的推送触发 `.github/workflows/release.yml`，构建 macOS DMG、创建 GitHub Release 并更新 Homebrew 分发。PR 上的版本标签和 Git 版本 tag 是不同的对象：前者控制升版，后者触发发布。
- 不要用直接推送 `main` 代替 PR，不要擅自创建、移动或覆盖版本 tag。工作流需使用已配置的 `WORKFLOW_PAT` 推送 tag，以触发后续 Release 工作流。
- 获准合并或发布后，检查版本工作流和 Release 工作流的运行结果；只有发布成功才能报告部署完成。若误将改动直接推送到 `main`，不要重写远程历史，应补建带适当版本标签的 PR，合并后发布现有改动。
