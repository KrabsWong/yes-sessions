# 安装脚本

[文档首页](README.md) · 中文 · [English](../en/INSTALL_SCRIPT.md)

大多数用户可直接使用 [Homebrew 或 DMG](../../README.md#安装)。`scripts/install.sh` 是可选的交互式安装方式，适用于 Apple Silicon macOS，并要求安装包通过系统安全评估。

## 下载并运行

```bash
curl -fsSL -o install.sh https://raw.githubusercontent.com/KrabsWong/yes-sessions/main/scripts/install.sh
less install.sh
bash install.sh
```

脚本会询问是否删除旧应用、是否立即启动。请在终端中直接运行下载的脚本，避免管道输入影响交互。

指定已发布版本时，将下面的 `X.Y.Z` 替换为 Releases 中的版本号，不带 `v`：

```bash
bash install.sh --version X.Y.Z
# 或通过环境变量指定
YS_VERSION=X.Y.Z bash install.sh
```

版本优先级为：`--version` / `-v` 参数、`YS_VERSION`、GitHub 最新 Release。可用 `bash install.sh --help` 查看帮助。

## 实际行为

1. 检查 macOS 和 arm64 架构，确定版本。
2. 检测 `/Applications/Yes Sessions.app`；若同意删除旧版本，会在下载新包前删除旧应用。
3. 下载主仓库 Release 的 DMG，并用 `hdiutil verify` 验证镜像。
4. 挂载后运行 `codesign --verify --deep --strict` 和 `spctl --assess`。
5. 验证通过后复制应用到 `/Applications`，卸载镜像并清理临时文件。
6. 询问是否启动应用。

脚本不会移除 quarantine 属性，也不会自动绕过 Gatekeeper。ad-hoc 签名包可能通过签名完整性校验，但无法通过 `spctl`；此时脚本会停止，不会安装新包。

## 更新与失败处理

更新前退出应用。当前脚本不是原子替换：同意删除旧版后，如果下载或验证失败，旧应用也已经移除；拒绝删除则不会中止后续安装。需要可控替换时，请使用 Homebrew 或手动 DMG 安装。

无法获取最新版时，检查 GitHub 网络连接或 API 限制，也可指定版本。下载失败时，确认该 Release 存在 arm64 附件。没有 `/Applications` 写权限时，优先通过 Finder 手动安装，按系统提示处理权限。

若系统评估失败，请查看发布说明中的签名状态和 [首次启动说明](FAQ.md)，不要把签名完整性校验通过视为已公证。
