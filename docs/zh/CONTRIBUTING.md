# 开发与贡献

[文档首页](README.md) · 中文 · [English](../en/CONTRIBUTING.md)

欢迎提交问题、文档修正和代码改进。较大的功能或架构调整，建议先开 Issue 讨论使用场景与范围。

## 本地运行

需要 macOS 13 或更高版本、Apple Silicon、Rust stable 和 Xcode Command Line Tools。首次构建需要下载依赖。

```bash
git clone https://github.com/KrabsWong/yes-sessions.git
cd yes-sessions
cargo run -p yes-sessions
```

## 项目结构

| 路径 | 职责 |
| --- | --- |
| `crates/yes-core/` | 会话模型、数据源解析、搜索、设置、终端和 Git 服务 |
| `crates/yes-app/` | GPUI 界面、会话阅读、文件预览和 Mermaid 宿主 |
| `packaging/` | macOS 应用元数据与 Homebrew 模板 |
| `scripts/` | 构建、打包与安装脚本 |
| `docs/`、`docs/en/` | 中文与英文文档 |

应用使用 Rust 与 GPUI Kit。仅 Mermaid 使用系统 WKWebView，不引入通用网页界面或独立浏览器运行时。数据源读取逻辑放在 `yes-core`；大型会话的列表读取保持轻量。UI 文案集中在 `crates/yes-app/src/i18n.rs`，新增文案需同时提供中英文。保持 macOS 13.0 兼容，新增依赖需评估许可证和二进制体积。

仓库内的 [开发约定](../../AGENTS.md) 与 [视觉规范](../../DESIGN.md) 提供更详细的项目约束。

## 验证

代码改动提交前运行：

```bash
cargo fmt --all --check
cargo check --workspace
cargo test --workspace
./scripts/package-macos.sh
codesign --verify --deep --strict "target/macos/Yes Sessions.app"
```

产物为 `target/macos/Yes Sessions.app` 和 `release/Yes-Sessions-<版本>-arm64.dmg`。无分发证书时可使用 ad-hoc 签名构建；这不等于 Apple 公证。只修改文档时，检查内容、相对链接、语言对应关系及命令是否与实现一致即可。

图标源文件为 `assets/logo.png`。仅重新生成图标时需要 ImageMagick，并运行 `./scripts/generate-icons.sh`；日常打包使用仓库中已生成的图标。

## 提交改动

1. 从最新 `main` 创建 `feature/` 分支；外部贡献者可以先 Fork 仓库。
2. 将改动限制在要解决的问题内，必要时补充测试，并同步中英文文档。
3. 检查 diff，避免提交会话历史、凭据、个人路径和构建产物。推送前扫描待上传的完整提交历史，而不仅是最终文件。
4. 提交信息包含简洁标题，以及说明改动原因与验证结果的正文。
5. 向 `main` 提交 PR，写清问题、行为变化和验证。由维护者确认版本标签与合并。

版本标签只能选择 `major`、`minor`、`patch`、`skip-release` 中的一个；纯文档通常使用 `skip-release`。完整规则见 [发布流程](RELEASE_PIPELINE.md)。

## 反馈问题

请提供应用版本、macOS 与来源工具版本、复现步骤及预期结果。使用脱敏样例复现解析问题，不要上传整个会话目录、访问令牌或私有源码。
