# Yes Sessions

Yes Sessions 是一款只面向 macOS 的 AI CLI 会话浏览与恢复工具。当前主线实现为纯 Rust 桌面应用：界面使用 GPUI Kit，只有 Mermaid 图表通过 `gpui-wry` 嵌入系统 WKWebView；应用不再携带 Electron、Chromium 或 Node.js 运行时。

![Logo](./build/icons/256x256.png)

## 功能

- 浏览 CodeBuddy、Claude Code、OpenCode 与 Codex CLI 的本地会话
- 按日期和项目目录分组、折叠与快速定位用户消息
- 展示 Markdown、代码、推理过程、工具调用和子 Agent 会话
- 通过系统 WKWebView 渲染 Mermaid，并支持缩放、平移和重置
- 在 Ghostty、Kitty 或 Terminal.app 中恢复会话
- 中文/英文、浅色/深色/跟随系统、强调色和阅读布局设置
- 原生 macOS 窗口，无启动页，无前后端 IPC

文件预览与 Git Diff 视图暂不包含在本次原生迁移中；会话中的普通文本、代码和图片引用仍可正常展示。

## 系统要求

- macOS 13.0 或更高版本
- 当前发布目标：Apple Silicon
- 开发环境：Rust stable、Xcode Command Line Tools

## 开发

```bash
# 运行
cargo run -p yes-sessions

# 静态检查与测试
cargo check --workspace
cargo test --workspace

# 生成 .app 与 DMG
./scripts/package-macos.sh
```

产物位于：

- `target/macos/Yes Sessions.app`
- `release/Yes-Sessions-<version>-arm64.dmg`

默认使用 ad-hoc 签名，适合本地验证。正式分发使用 GitHub Release 工作流；它会进行 Developer ID 签名、Apple 公证和 stapling。仓库需要配置以下 Actions Secrets：

- `MACOS_CERTIFICATE`：Developer ID Application 证书的 Base64 编码 `.p12`
- `MACOS_CERTIFICATE_PASSWORD`：导出 `.p12` 时使用的密码
- `CODESIGN_IDENTITY`：证书身份，例如 `Developer ID Application: ...`
- `APPLE_ID`、`APPLE_TEAM_ID`、`APPLE_APP_PASSWORD`：Apple 公证账号、团队 ID 与 app-specific password
- `HOMEBREW_TAP_TOKEN`：更新 Homebrew tap 与发布附件
- `WORKFLOW_PAT`：自动版本工作流推送 release tag，并触发后续 Release 工作流

缺少任意分发凭据时，Release 工作流会立即失败，不会发布未经公证的安装包。本地也可同时设置 `CODESIGN_IDENTITY` 和三项 Apple 公证凭据，让 `./scripts/package-macos.sh` 生成已签名、公证并 stapled 的 `.app` 与 DMG。

## 架构

```text
crates/
  yes-core/             会话模型、四类数据源解析、设置、终端与 Git 服务
  yes-app/              GPUI Kit 界面、会话交互、Markdown 与 Mermaid 宿主
    assets/             本地 Mermaid 运行资源
packaging/              macOS Info.plist
scripts/package-macos.sh
```

应用是单进程 Rust 架构。耗时的会话读取通过 GPUI 后台任务执行并回到 UI 更新状态，不再经过 Electron IPC。Mermaid 是唯一的 WebView 使用场景，且使用 macOS 自带的 WebKit，不内置浏览器内核。

## 数据路径

| 工具 | 路径 |
| --- | --- |
| CodeBuddy | `~/.codebuddy/projects/<project>/*.jsonl` |
| Claude Code | `~/.claude/projects/<project>/*.jsonl` |
| OpenCode | `~/.local/share/opencode/opencode.db`（兼容文件存储） |
| Codex CLI | `~/.codex/sessions/**/*.jsonl` 与 `~/.codex/session_index.jsonl` |

大型 JSONL 会话采用轻量摘要扫描；只有打开具体会话时才解析完整内容，避免启动时读取数 GB 历史记录。

## 安装

发布后可通过 Homebrew 安装：

```bash
brew tap krabswong/yes-sessions
brew install --cask yes-sessions
```

也可从 GitHub Releases 下载已经签名、公证并 stapled 的 DMG，无需绕过 Gatekeeper。本地生成的 ad-hoc 调试包如果被 macOS 标记为 quarantine，可在首次打开前手工执行：

```bash
xattr -c ~/Downloads/Yes-Sessions-*.dmg
```

## 许可证

MIT。Mermaid 的第三方许可证随应用资源一起分发。
