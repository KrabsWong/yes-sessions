# AGENTS.md

## 项目概述

Yes Sessions 是一款仅面向 macOS 的 AI CLI 会话管理器，使用 Rust、GPUI Kit 和系统 WebKit 实现。

## 技术栈

- Rust 2024 edition
- GPUI Kit / GPUI Component
- `gpui-wry` + WKWebView（仅用于 Mermaid）
- rusqlite（OpenCode 数据）
- serde / serde_json（JSON 与 JSONL 数据）

项目不得重新引入 Electron、Chromium、Node.js 或通用 WebView 页面。需要原生 UI 时优先使用 GPUI Kit 组件；只有 Mermaid 内容可以进入 WKWebView。

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

打包后还需执行 `codesign --verify --deep --strict`，正式分发版本需完成 Developer ID 签名、notarization 和 stapling。
