# 原生应用发布与 Homebrew

## 自动发布链路

1. PR 合并到 `main` 后，`auto-version-bump.yml` 检查 `skip-release` 标签。
2. 如果 Cargo 当前版本尚无对应 Git tag，直接发布该预设版本，例如预设的 `10.0.0`。已有 tag 时按 `major` / `minor` / `patch` 递增，未指定默认 patch，major 优先。
3. 使用 `WORKFLOW_PAT` 原子推送版本提交和 tag。不要改成普通 `GITHUB_TOKEN` 推送，否则 tag push 不会自动触发后续工作流。
4. `release.yml` 在 macOS runner 上检查版本、运行 Rust 验证并构建 arm64 应用。
5. `package-macos.sh` 使用锁文件构建，默认使用 ad-hoc 签名；配置完整 Apple 凭据时完成 Developer ID 签名、公证、stapling。生成 `Yes-Sessions-<version>-arm64.dmg`。
6. 发布主仓库 Release，再将同一个 DMG 发布到 `KrabsWong/homebrew-yes-sessions` 的 Release。
7. 使用最终 DMG 的 SHA256 和 `packaging/yes-sessions.rb.in` 生成 Cask，同时同步 `packaging/homebrew-README.md`，最后推送 Tap 的 `main`。

Tap 的版本与校验和只能在正式安装包发布成功后更新。不要用本地 ad-hoc 签名包的校验和提前修改 Tap，也不要将旧版本安装包对应的 Cask 先改为新的应用包名。

## 合并前检查

- 检查 Cargo 版本与远程 tag：当前 `11.0.0` 已有 `v11.0.0`，本次修复合并后默认递增到 `11.0.1`；不要添加 `major` 或 `minor` 标签。
- 需要发布的 PR 不添加 `skip-release`。
- 主仓库配置 `WORKFLOW_PAT`，具有推送 main/tag 的权限；仓库规则须允许发布机器人执行该操作。
- 可选：完整配置 `MACOS_CERTIFICATE`、`MACOS_CERTIFICATE_PASSWORD`、`CODESIGN_IDENTITY` 和 `APPLE_ID`、`APPLE_TEAM_ID`、`APPLE_APP_PASSWORD`。证书须为 Developer ID Application。
- 配置 `HOMEBREW_TAP_TOKEN`，允许向 `KrabsWong/homebrew-yes-sessions` 推送 main 并上传 Release 附件。
- Tap 与下载附件需对安装用户可访问；普通公开的 `brew install` 使用无需认证的下载链接。
- 没有证书时将六项 Apple Secrets 全部留空，正常发布 ad-hoc 安装包；只配置部分时中止发布。

## Cask 约定

- 应用包名：`Yes Sessions.app`。
- 架构：`:arm64`；最低系统：macOS 13 Ventura。
- Bundle ID：`com.yessessions.app`。
- 配置目录：`~/Library/Application Support/yes-sessions`。
- 不通过 postflight 移除 quarantine。未公证版本首次启动被拦截时，在「系统设置 → 隐私与安全」中选择「仍要打开」。

`/Users/krabswang/Personal/homebrew-yes-sessions` 是本地 checkout。Actions 会克隆 GitHub 上的 Tap 更新，无需依赖这台电脑或该绝对路径。发布完成后在本地 Tap 执行 `git pull` 即可同步。

## 安装与更新

```bash
brew tap krabswong/yes-sessions
brew install --cask yes-sessions
# 已安装用户
brew update
brew upgrade --cask yes-sessions
```

## 失败恢复

构建或签名失败时修复原因，在 Actions 中重跑失败任务。已有有效 tag 时，也可手动运行 Release 并填写该 tag；不需要再次递增版本。发布到了两个仓库但 Cask 更新失败时，可重跑同一版本，附件上传允许覆盖，Cask 无变化时不会重复提交。

不要对旧 tag 手动重新发布以覆盖当前 Tap 版本。重跑期间不要并行发布其他版本。

## 本地验证边界

本地可以检查 YAML、shell、Cask Ruby 语法及模板参数，并生成 ad-hoc 构建。实际运行 Actions 后才能确认 GitHub token 权限及跨仓库发布链路；可选的 Developer ID 签名和公证还需要真实 Apple 凭据验证。
