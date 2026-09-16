# Yes Sessions

**在一个 macOS 应用中，浏览、搜索并继续你的 AI 编程会话。**

中文 · [English](README.en.md)

Yes Sessions 将 CodeBuddy CLI、CodeBuddy CN、Claude Code、OpenCode 和 Codex CLI 的本地历史集中到一个原生界面。找回之前的提问、查看工具执行记录，或回到终端继续工作，无需逐个翻找日志文件。

<img width="700" alt="image" src="https://github.com/user-attachments/assets/1cffc250-0b2f-4614-a86f-f8d8a1edd13c" />

[下载安装](https://github.com/KrabsWong/yes-sessions/releases/latest) · [使用指南](docs/zh/USER_GUIDE.md) · [常见问题](docs/zh/FAQ.md) · [反馈问题](https://github.com/KrabsWong/yes-sessions/issues)

## 你可以用它做什么

- **找回上下文**：按项目和日期浏览会话，搜索当前会话或当前工具的历史，直接跳转到匹配消息。
- **读懂执行过程**：查看 Markdown、代码、思考内容、工具调用、附件和子会话；展开附加上下文，查看 IDE 注入的环境与规则。
- **查看代码与变更**：在侧边面板浏览项目文件、预览图片，对比已暂存、未暂存和未跟踪的 Git 变更。
- **继续之前的工作**：将支持的 CLI 会话交给 Ghostty、Kitty 或系统终端恢复。
- **舒适地阅读长会话**：通过右侧导航快速定位提问，阅读时保留位置，按需跳转到新消息；支持中英文、明暗主题和阅读布局设置。

## 支持的工具

| 工具 | 浏览与搜索本地历史 | 在终端恢复 |
| --- | --- | --- |
| CodeBuddy CLI | 支持 | 支持 |
| CodeBuddy CN（IDE） | 支持 | 不支持 |
| Claude Code | 支持 | 支持 |
| OpenCode | 支持，兼容 OpenCode 1 与 OpenCode 2 beta 数据 | 支持，需安装对应 CLI |
| Codex CLI | 支持 | 支持 |

展示内容取决于原工具实际保存的记录。Yes Sessions 不会补全缺失的消息，也不提供模型服务；恢复会话需要已安装并配置好对应 CLI。

## 安装

需要 **Apple Silicon Mac，macOS 13 Ventura 或更高版本**。当前不提供 Intel、Windows 或 Linux 安装包。

### Homebrew

```bash
brew tap krabswong/yes-sessions
brew install --cask yes-sessions
```

更新已安装的应用：

```bash
brew update
brew upgrade --cask yes-sessions
```

### 手动下载

从 [GitHub Releases](https://github.com/KrabsWong/yes-sessions/releases/latest) 下载 `Yes-Sessions-<版本>-arm64.dmg`，打开后将 `Yes Sessions.app` 拖入「应用程序」。

签名与公证状态以对应版本的发布说明为准。未公证版本若被 macOS 拦截，确认下载来源后，可在「系统设置 → 隐私与安全」中选择「仍要打开」。受管理的电脑可能限制此操作。

其他安装方式见 [安装脚本说明](docs/zh/INSTALL_SCRIPT.md)。

## 开始使用

1. 先在支持的工具中产生本地会话记录，然后打开 Yes Sessions。
2. 选择工具，在会话列表中按项目与日期找到记录，点击查看详情。
3. 用 `⌘F` 搜索当前会话，用 `⇧⌘F` 搜索当前工具的会话历史。
4. 需要继续对话时，使用会话恢复入口；需要查看项目代码时，打开标题栏右侧的文件与变更面板。

通过 `⌘,` 打开设置，切换界面语言、主题、阅读布局和首选终端。详见 [使用指南](docs/zh/USER_GUIDE.md)。

## 数据与隐私

会话浏览与搜索直接读取本机历史文件或数据库，无需向 Yes Sessions 提供模型 API Key。应用设置与窗口状态保存在本机。会话来源、CodeBuddy CN 的读取范围及外部操作边界见 [数据与隐私](docs/zh/DATA_AND_PRIVACY.md)。

文件与 Git 面板为只读预览，展示的是**当前工作区**，不是会话发生时的文件快照。恢复会话后，CLI 的网络访问、文件修改和费用由对应工具决定。

## 文档与贡献

- [文档首页](docs/zh/README.md)：安装、使用、常见问题与维护文档。
- [开发与贡献](docs/zh/CONTRIBUTING.md)：本地运行、验证、提交问题和贡献代码。
- [发布流程](docs/zh/RELEASE_PIPELINE.md)：面向维护者的版本、签名与分发说明。

项目使用 Rust 与 GPUI Kit 构建原生 macOS 界面，仅 Mermaid 图表使用系统 WebKit 渲染，不附带浏览器内核。

## 许可证

项目在 [Cargo.toml](Cargo.toml) 中声明使用 MIT 许可证。第三方组件保留各自许可证，相关声明见 [第三方许可目录](crates/yes-app/third-party/)。
