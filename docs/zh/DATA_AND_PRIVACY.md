# 数据与隐私

[文档首页](README.md) · 中文 · [English](../en/DATA_AND_PRIVACY.md)

## 会话从哪里来

Yes Sessions 读取当前 macOS 用户目录中的本地数据，不需要导入文件或提供模型 API Key。

| 数据源 | 默认位置 |
| --- | --- |
| CodeBuddy CLI | `~/.codebuddy/projects/<project>/*.jsonl` |
| CodeBuddy CN | `~/Library/Application Support/CodeBuddyExtension/Data/<account>/CodeBuddyIDE/<user>/history/<workspace>/<session>/` |
| Claude Code | `~/.claude/projects/<project>/*.jsonl` |
| OpenCode | `~/.local/share/opencode/opencode.db` |
| Codex CLI | `~/.codex/sessions/**/*.jsonl` 与 `~/.codex/session_index.jsonl` |

列表采用轻量读取；打开详情时才加载完整消息。跨会话搜索也可能读取正文。OpenCode 同时解析受支持的 v1/v2 数据并对迁移记录去重。

### CodeBuddy CN 的范围

列表读取 IDE 保存的索引，详情按索引读取消息，展示已保存的思考、工具调用与结果、图片和请求级用量。工作区路径通过 CodeBuddy CN 的 `User/workspaceStorage` 信息还原。没有可展示内容的空会话不出现在列表中。

IDE 注入的环境与规则默认折叠，原始消息仍可复制。这只是阅读方式的调整，不会重写源记录。未在本地保存的消息、已清理的历史或缺失附件无法恢复。

## 应用保存什么

应用配置目录为 `~/Library/Application Support/yes-sessions/`，包括：

- `settings.json`：语言、主题、阅读布局、终端偏好等设置。
- `window-state.json`：窗口位置与尺寸等状态。

打开内嵌附件时，应用可能在系统临时目录生成文件供外部程序打开；「另存为」会写入你选择的位置。

## 读取与外部操作的边界

会话浏览、搜索和项目预览在本机进行，预览不修改会话源文件或项目文件。应用没有用于同步会话的云端账号服务。

打开远程附件或链接会交给外部程序，可能访问对应网络地址。恢复会话会启动原 CLI；后续模型请求、费用和工作区修改遵循该工具的配置与权限。安装与更新需要从 GitHub 或 Homebrew 下载发布文件。

## 分享问题前

会话可能包含源代码、路径、终端输出和凭据。提交截图或复现样例前，请删除 API Key、个人信息和私有代码；优先提供最小化、脱敏后的片段，不要直接上传整个历史目录。

卸载应用不会代替原工具清理历史。Homebrew 的 `--zap` 会清理 Cask 列出的应用设置等文件，使用前请阅读 [卸载说明](HOMEBREW_CASK_GUIDE.md)。
