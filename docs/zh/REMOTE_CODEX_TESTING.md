# 试用 SSH 远程 Codex

当前版本支持通过本地系统 SSH 只读浏览 Linux 服务器上的 Codex 会话。服务器无需安装 Yes Sessions 或读取程序；OpenCode 等其他远程 provider 暂未接入。

## 使用

1. 打开「设置 → 远程」，或顶部「本机 → 连接远程 Codex」。
2. 输入 SSH 配置中的 Host 别名，或 `ubuntu@your-server`。shell alias 不能直接填入：如果 `vibe` 展开为 `ssh ubuntu@your-server`，应填写 `ubuntu@your-server`。
3. 数据目录保留 `~/.codex`，或填写远程 Codex 数据根的绝对路径。该目录应包含 `sessions` 子目录；不支持把整个家目录或单个 JSONL 文件当作数据根。
4. 如果需要密码，点击「在终端登录」，在系统 Terminal 中完成 SSH 密码／主机密钥确认，然后回到应用点击「连接」。若已配置可直接使用的密钥认证，可以直接连接。
5. 在会话列表中打开详情；通过侧栏「刷新」更新列表和当前会话。顶部来源菜单可以切回本机。

应用通过临时的本地 `.command` 文件打开 Terminal，登录流程退出后删除该文件，不需要 Apple Events 自动化控制权限。密码仅由系统 SSH 处理，应用不保存密码或私钥。SSH Host 配置、跳板和 IdentityFile 由 OpenSSH 处理；应用不执行 shell alias 或 shell function。

「在终端登录」建立应用专用的 SSH ControlMaster 连接，空闲约 10 分钟后退出。普通的 `ssh server` 登录没有建立这个专用连接，不能替代此按钮。出现 `Permission denied (publickey,password)` 时重新通过按钮登录；应用不会自动绕过未知或变化的主机密钥。

顶部显示当前 SSH 目标，底部彩色状态栏显示远程模式和读取状态；只读与手动刷新说明集中在侧栏。切回本机后隐藏远程状态栏。

## 支持范围

- 复用本地 SSH 配置；自定义端口和跳板请放入 `~/.ssh/config` 并使用对应 Host 别名。
- Codex 会话摘要、详情、同一会话分段历史、子会话导航及 usage 读取。
- 远程搜索仅匹配摘要；打开详情后仍可在当前会话中搜索。
- 手动刷新，切换来源时清除选择和导航状态。刷新连接失败时保留已有视图并显示错误。
- 远程工作区预览、Git、用量仪表盘、本地 CLI 恢复和本地文件链接均不开放。嵌入的数据附件可显示，远程文件附件暂不下载。

首次连接只返回有界摘要，打开详情后才下载对应 JSONL。远程依赖 Linux 上的 `/bin/bash` 和常见 GNU 工具：find、stat、head、tail、base64、tr、wc。shell 启动脚本应避免向非交互连接的 stdout 输出欢迎文字，否则协议会明确报错。

本次验证版的上限：最多 2,000 个 JSONL 文件；每文件列表读取前 256 KiB 和末尾 64 KiB；索引文件最多 1 MiB；单个详情文件最多 32 MiB；单次编码响应最多 64 MiB；单次 SSH 读取超时 45 秒。超过上限会报错，不静默展示截断的详情。会话很多时，首次摘要传输仍可能较大，增量索引后续再做。

本地只保存 SSH 目标和数据根配置，不在启动时自动连接远程。会话的临时镜像位于当前用户私有的 `/tmp/ys-*` 目录，正常释放来源后清理。进程异常退出可能遗留临时数据；这不是加密离线缓存。

## 本地验证

```bash
cargo fmt --all --check
cargo check --workspace
cargo test --workspace
cargo run -p yes-sessions --example visual_capture -- --remote
./scripts/package-macos.sh
codesign --verify --deep --strict "target/macos/Yes Sessions.app"
```

协议测试使用本地 fixture，不会连接真实服务器；macOS 的 shell fixture 将 stat 格式转换为系统 BSD stat 的等价格式，Linux 使用系统 GNU 工具。视觉工具另会尝试连接本机测试地址 `visual-test@127.0.0.1`，用于捕获错误状态，不使用真实服务器配置。截图写入 `target/visual/remote-settings-{zh,en}.png` 和 `target/visual/remote-bar-{light,dark}.png`。

测试包生成于 `target/macos/Yes Sessions.app`。无分发凭据时使用 ad-hoc 签名，未公证；如系统拦截首次启动，可在「系统设置 → 隐私与安全性」选择「仍要打开」。不需要关闭 Gatekeeper。
