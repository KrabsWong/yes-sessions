# Yes Sessions

Yes Sessions 是一款只面向 macOS 的 AI CLI 会话浏览与恢复工具。当前主线实现为纯 Rust 桌面应用：界面使用 GPUI Kit，只有 Mermaid 图表通过 `gpui-wry` 嵌入系统 WKWebView；应用不再携带 Electron、Chromium 或 Node.js 运行时。

![Logo](./build/icons/256x256.png)

## 功能

- 浏览 CodeBuddy、Claude Code、OpenCode 与 Codex CLI 的本地会话
- 按日期和项目目录分组、折叠与快速定位用户消息；右侧刻度导航悬停展示提问和回复摘要，点击跳转，长会话可在刻度上滚动浏览
- 展示 Markdown、代码、推理过程、工具调用和子 Agent 会话；Edit 工具可直接对比历史替换片段
- 通过系统 WKWebView 渲染 Mermaid，并支持缩放、平移和重置；渲染失败可查看原文并重试
- 阅读历史消息时保持当前位置，新消息以条数提示，点击跳转到底部
- 在 Ghostty、Kitty 或 Terminal.app 中恢复会话
- 中文/英文、浅色/深色/跟随系统、强调色和阅读布局设置
- 原生 macOS 窗口，无启动页，无前后端 IPC

标题栏右侧的收起／展开图标可打开文件与变更面板，与左侧会话区域等高并可拖动宽度。面板内通过 Files／Changes 切换文件树和工作区变更。文件树只在点击展开文件夹时读取一级子项，收起后保留缓存；支持文件名搜索、文本行号与 PNG/JPEG/GIF/WebP/BMP/TIFF/ICO/SVG 图片预览（等比缩放）；不支持的格式及无法解码的图片会显示提示。文件与 diff 支持语法高亮（包括 Rust、TypeScript/TSX、Kotlin、Swift、Python、C/C++/Objective-C、JSON、TOML，以及 Makefile/Dockerfile 和 shebang 脚本），颜色跟随浅色／深色主题；未知语言及超过高亮预算的内容以纯文本展示。Git diff 区分已暂存／未暂存／未跟踪文件，文本支持切换合并与左右对比并强调行内修改；未修改的上下文可点击展开，再通过「收起上下文」恢复紧凑展示。上下文来自本次加载的缓存，超出读取上限则退回常规 diff。图片支持修改前后并排预览，Changes 列表显示当前分支（游离 HEAD 时显示提交短哈希）。后台检测到 Git 变化时仅提示新内容，点击刷新才更新预览。Changes 列表、计数和搜索复用同一份变更结果；diff 先展示内容，再在后台为展开的内容补充高亮。预览顶部的返回按钮或 Escape 可返回列表并保留目录状态；再次点击右侧图标或在列表中按 Escape 收起面板。触控板滚动锁定手势主方向，避免横向和纵向串动；Read/Write/Edit 等工具调用中的明确文件路径可直接打开预览。

`⌘W` 隐藏应用并保留当前会话、滚动位置和面板状态，从 Dock 打开后继续阅读；`⌘M` 最小化窗口，`⌘Q` 完全退出应用。正常退出后保存窗口位置、大小和最大化／全屏状态；重新启动时恢复，并按当前显示器可用区域校正位置。

预览只读，展示当前磁盘内容和工作区变更，不代表历史会话快照；通过「刷新」重新读取。文件内容上限 4 MiB，文本最多 10 万行且每行不超过 32 KiB；超限或不支持的文件会显示提示。搜索最多返回 500 项，并限制目录遍历范围。

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
