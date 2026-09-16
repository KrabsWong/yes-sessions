# 发布流程

[返回首页](../../README.md) · [English](../en/RELEASE_PIPELINE.md) · [Homebrew 安装指南](HOMEBREW_CASK_GUIDE.md)

本文面向维护者，说明从合并 PR 到发布 macOS 安装包的流程。普通使用者请参阅 [Homebrew 安装指南](HOMEBREW_CASK_GUIDE.md) 或下载 [最新版本](https://github.com/KrabsWong/yes-sessions/releases/latest)。

## 发布方式

常规改动通过功能分支和 PR 进入 `main`。合并前，为 PR 添加且仅添加一个版本标签：

| 标签 | 用途 |
| --- | --- |
| `major` | 重大版本升级，例如不兼容变更 |
| `minor` | 新功能 |
| `patch` | 修复和小幅改进 |
| `skip-release` | 无需发布的文档、配置等改动 |

不要依赖工作流在缺少标签时默认选择 `patch` 的行为。PR 版本标签控制升版；`vMAJOR.MINOR.PATCH` 格式的 Git tag 触发安装包发布，两者不是同一种标签。

## 自动发布链路

1. PR 合并后，[版本工作流](../../.github/workflows/auto-version-bump.yml) 检查 `skip-release`；带此标签时不发版。
2. 如果 `Cargo.toml` 中的当前版本尚无对应 Git tag，直接发布该版本；如果已有 tag，则按 PR 标签递增版本并更新锁文件。
3. 工作流使用 `WORKFLOW_PAT` 原子推送版本提交和 Git tag。
4. [发布工作流](../../.github/workflows/release.yml) 校验 tag 与 Cargo 版本一致，运行格式检查、编译检查和测试，然后构建 Apple Silicon 安装包。
5. [打包脚本](../../scripts/package-macos.sh) 生成并校验应用签名；配置完整 Apple 凭据时，还会完成公证和票据装订（stapling）。
6. 将 `Yes-Sessions-<版本>-arm64.dmg` 发布到主仓库的 GitHub Release，再将同一文件上传到 [Homebrew Tap 的 Release](https://github.com/KrabsWong/homebrew-yes-sessions/releases)。
7. 按最终 DMG 的 SHA256 生成 Cask、同步 Tap 首页，并推送 Tap 的 `main`。

当前分发目标为 macOS 13 及以上的 Apple Silicon Mac，没有 Intel 安装包。只有发布工作流成功、Release 附件可下载且 Tap 更新完成，才算完成整个发布流程。

## 仓库配置

在 GitHub Actions Secrets 中配置下列凭据，不要将凭据写入仓库。

| Secret | 用途 |
| --- | --- |
| `WORKFLOW_PAT` | 向主仓库推送版本提交和 Git tag；仓库规则须允许此操作 |
| `HOMEBREW_TAP_TOKEN` | 向 `KrabsWong/homebrew-yes-sessions` 上传 Release 附件并推送 Cask |
| `MACOS_CERTIFICATE` | Base64 编码的 Developer ID Application `.p12` 证书 |
| `MACOS_CERTIFICATE_PASSWORD` | 上述证书的导出密码 |
| `CODESIGN_IDENTITY` | Developer ID Application 签名身份 |
| `APPLE_ID` | 公证使用的 Apple 账号 |
| `APPLE_TEAM_ID` | Apple 开发者团队 ID |
| `APPLE_APP_PASSWORD` | 公证使用的 App 专用密码 |

前两项是当前自动发布链路的必需项。六项 Apple 签名凭据须全部配置或全部留空；部分配置会中止发布。全部留空时生成 ad-hoc 签名、未经 Apple 公证的安装包，Release 说明会注明签名状态。

`WORKFLOW_PAT` 用于确保推送 Git tag 后能触发后续发布工作流，不应直接替换成默认 `GITHUB_TOKEN`。Tap 仓库和附件需要允许安装用户访问；面向公众分发时，下载链接不应要求用户认证。

## 合并前验证

在 macOS 开发环境的仓库根目录运行：

```bash
cargo fmt --all --check
cargo check --workspace --locked
cargo test --workspace --locked
./scripts/package-macos.sh
codesign --verify --deep --strict "target/macos/Yes Sessions.app"
```

本地打包默认使用 ad-hoc 签名，输出应用到 `target/macos/Yes Sessions.app`，输出 DMG 到 `release/`。这可以验证构建和打包，不代表 GitHub 权限、跨仓库上传或 Apple 公证已验证成功。

同时确认 PR 标签正确、版本号没有意外修改，且将推送的完整 Git 历史不含密钥或凭据。不要提前用本地安装包的 SHA256 更新线上 Tap：Cask 必须对应最终发布的附件。

## 失败恢复

- **版本工作流失败：** 检查 token 权限、仓库规则、版本和已有 tag，再重跑失败任务。不要移动或覆盖已经发布的 Git tag。
- **构建、签名或公证失败：** 修复原因后重跑发布任务。已有正确 tag 时，也可在 Actions 手动运行 Release，填写该 tag，无需再次升版。
- **主仓库已发布，Tap 更新失败：** 修复 Tap 权限或上传问题，再重跑同一版本。工作流允许覆盖同名 Tap 附件；Cask 没有变化时不会重复提交。

重跑会重新构建安装包，应让整个后续上传及校验和更新流程完成，避免附件与 Cask 不一致。不要重跑旧版本来覆盖当前 Tap，也不要并行发布不同版本；当前工作流未设置发布并发锁。

## 维护入口

- [版本工作流](../../.github/workflows/auto-version-bump.yml)
- [发布工作流](../../.github/workflows/release.yml)
- [macOS 打包脚本](../../scripts/package-macos.sh)
- [Cask 模板](../../packaging/yes-sessions.rb.in) 与 [生成脚本](../../scripts/render-homebrew-cask.py)
- [Tap 首页模板](../../packaging/homebrew-README.md)
