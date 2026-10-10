# 使用 Homebrew 安装

[返回首页](../../README.md) · [English](../en/HOMEBREW_CASK_GUIDE.md) · [发布流程](RELEASE_PIPELINE.md)

Yes Sessions 通过项目维护的 [Homebrew Tap](https://github.com/KrabsWong/homebrew-yes-sessions) 分发。系统要求为 **macOS 13 Ventura 及以上、Apple Silicon 芯片**。使用以下命令前，需要已安装 Homebrew；也可以直接下载 [DMG 安装包](https://github.com/KrabsWong/yes-sessions/releases/latest)。

## 安装

```bash
brew tap krabswong/yes-sessions
brew trust krabswong/yes-sessions
brew install --cask yes-sessions
```

如果 Homebrew 提示 tap 不受信任，先运行 `brew trust krabswong/yes-sessions`，再重试安装。

安装后，从「应用程序」打开 **Yes Sessions**。

如果首次启动时 macOS 提示无法验证应用是否包含恶意软件并建议移到废纸篓，请关闭提示窗口，打开「系统设置 → 隐私与安全」，在安全性区域找到应用被阻止的信息并选择「仍要打开」，再按提示确认。操作前请确认安装包来自项目的 [GitHub Releases](https://github.com/KrabsWong/yes-sessions/releases)。部分版本使用 ad-hoc 签名，未经 Apple 公证；受组织管理的设备可能限制此操作。

## 更新

退出 Yes Sessions 后运行：

```bash
brew update
brew upgrade --cask yes-sessions
```

完成后重新打开应用。可以用以下命令查看当前 Cask 信息和已安装版本：

```bash
brew info --cask yes-sessions
brew list --cask --versions yes-sessions
```

## 卸载

```bash
brew uninstall --cask yes-sessions
brew untap krabswong/yes-sessions
```

普通卸载保留应用设置。如果希望同时清除 Yes Sessions 的设置和窗口状态，请在移除 Tap 前改用：

```bash
brew uninstall --cask --zap yes-sessions
```

当前 Cask 的 `zap` 范围为：

- `~/Library/Application Support/yes-sessions`
- `~/Library/Preferences/com.yessessions.app.plist`
- `~/Library/Saved Application State/com.yessessions.app.savedState`

这些操作不会清理 Claude Code、Codex 或其他来源工具自己的会话目录。

## 安装遇到问题

- **找不到 Cask：** 确认已执行 `brew tap krabswong/yes-sessions`，再运行 `brew update`。
- **Tap 不受信任：** 运行 `brew trust krabswong/yes-sessions` 后重试安装。
- **下载失败：** 检查能否访问 [Tap 的 Release 页面](https://github.com/KrabsWong/homebrew-yes-sessions/releases/latest)，也可以从主仓库下载 DMG 手动安装。
- **校验和不匹配：** 不要跳过校验。记录版本、命令和错误信息，提交到 [问题反馈](https://github.com/KrabsWong/yes-sessions/issues)。
- **架构不支持：** 当前安装包仅支持 Apple Silicon，尚未提供 Intel 版本。

## 维护者参考

安装源是项目自己的 Tap，不依赖向官方 `homebrew-cask` 仓库提交。Cask 由 [发布工作流](../../.github/workflows/release.yml) 自动更新，使用最终发布 DMG 的 SHA256；不需要使用者手动修改版本或校验和。

维护 Cask 时修改主仓库中的 [模板](../../packaging/yes-sessions.rb.in)，维护 Tap 首页时修改 [首页模板](../../packaging/homebrew-README.md)。完整的凭据配置、发布顺序和故障恢复见 [发布流程](RELEASE_PIPELINE.md)。
