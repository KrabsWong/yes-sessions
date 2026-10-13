# 试用 SSH 远程 Codex

当前版本支持通过本地系统 SSH 只读浏览 Linux 服务器上的 Codex 会话。服务器无需安装 Yes Sessions 或读取程序；OpenCode 等其他远程 provider 暂未接入。

## 使用

1. 打开「设置 → 远程」，或顶部「本机 → 管理远程服务器」。
2. 点击「添加服务器」，填写不重复的显示别名（例如「开发机」）。输入 SSH 配置中的 Host 别名，或 `ubuntu@your-server`。shell alias 不能直接填入：如果 `vibe` 展开为 `ssh ubuntu@your-server`，应填写 `ubuntu@your-server`。
3. 数据目录保留 `~/.codex`，或填写远程 Codex 数据根的绝对路径。该目录应包含 `sessions` 子目录；不支持把整个家目录或单个 JSONL 文件当作数据根。
4. 点击行内勾号保存，然后点击该行「连接」，自动尝试 SSH 密钥认证；如需密码或私钥口令，在应用内输入。首次连接会单独显示主机指纹，请核对后确认。认证成功后自动读取会话。
5. 可以仅点击勾号保存配置而不连接；保存后服务器按行排列，点击行右侧的修改图标原地编辑三个字段，勾号保存、叉号取消，删除图标移除配置，底部按钮新增服务器。旧版本的单台配置会自动迁移为 `Server 1`，可以改名。
6. 在会话列表中打开详情；通过侧栏「刷新」更新列表和当前会话。顶部来源菜单按显示别名选择其他服务器，也可以切回本机。切换时关闭上一台连接。

应用继续使用系统 OpenSSH、SSH Host 配置、跳板和 IdentityFile，不执行 shell alias 或 shell function。认证弹窗通过应用自身的本地 ASKPASS 子进程和私有 Unix socket 与 SSH 通信；密码不写配置、文件、环境变量或命令参数，提交／取消后清空输入及撤销历史，不保存到钥匙串。主机身份由系统 known_hosts 管理，不自动接受变化的主机密钥。

远程模式期间保持应用专用主连接，每 30 秒发送 SSH 保活，连续 3 次无回应后连接会失效。网络故障按 2、4、8、16、30 秒间隔重试；电脑唤醒或网络恢复后可自动重新连接。需要认证时再次弹窗；取消或认证失败后暂停重试，需点击「重新连接」。只恢复连接，不自动重复刷新已加载的会话，以保留阅读位置；未完成的首次读取会在恢复后重试。

侧栏「断开」关闭本应用连接。切换服务器或退出应用同样会清理主连接；本地辅助进程负责在应用异常退出时终止关联的 SSH 进程。它不安装常驻服务，也不在远程部署任何程序。

顶部显示自定义服务器别名；SSH Target 输入框默认遮蔽，可用眼睛图标显示／隐藏。底部彩色状态栏显示远程模式和读取状态；只读与手动刷新说明集中在侧栏。切回本机后隐藏远程状态栏。

## 支持范围

- 复用本地 SSH 配置；自定义端口和跳板请放入 `~/.ssh/config` 并使用对应 Host 别名。
- Codex 会话摘要、详情、同一会话分段历史、子会话导航及 usage 读取。
- 远程搜索仅匹配摘要；打开详情后仍可在当前会话中搜索。
- 手动刷新，切换来源时清除选择和导航状态。刷新连接失败时保留已有视图并显示错误。
- 远程工作区预览、Git、用量仪表盘、本地 CLI 恢复和本地文件链接均不开放。嵌入的数据附件可显示，远程文件附件暂不下载。

首次连接只返回有界摘要，打开详情后才下载对应 JSONL。远程依赖 Linux 上的 `/bin/bash` 和常见 GNU 工具：find、stat、head、tail、base64、tr、wc。shell 启动脚本应避免向非交互连接的 stdout 输出欢迎文字，否则协议会明确报错。

本次验证版的上限：最多 2,000 个 JSONL 文件；每文件列表读取前 256 KiB 和末尾 64 KiB；索引文件最多 1 MiB；单个详情文件最多 32 MiB；单次编码响应最多 64 MiB；单次 SSH 读取超时 45 秒。超过上限会报错，不静默展示截断的详情。会话很多时，首次摘要传输仍可能较大，增量索引后续再做。

本地只保存服务器别名、SSH 目标和数据根配置，不在启动时自动连接远程。会话的临时镜像位于当前用户私有的 `/tmp/ys-*` 目录，正常释放来源后清理。进程异常退出可能遗留临时数据；这不是加密离线缓存。

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
