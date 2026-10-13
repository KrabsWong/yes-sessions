# SSH 远程会话读取方案（调研稿）

日期：2026-10-10。状态：设计建议，尚未实现或连接真实远程主机验证。

后续实施决定：用户选择先验证 Codex 的免部署模式，当前实现与试用边界见 [远程 Codex 试用说明](REMOTE_CODEX_TESTING.md)。下文保留多 provider 的长期方案比较，不代表首版需要部署 reader。

建议采用「系统 OpenSSH + 按连接运行的 Rust reader + 现有 provider」。本地只保存 SSH 别名和会话目录选择，不复制主机、端口、私钥配置。远程 reader 不监听端口、不注册系统服务、不启动任何 agent CLI，只在 SSH 连接期间读取用户选定的数据。

这条路线的明确代价是首次连接需要部署一个用户权限的二进制文件。它是方案选择，不是当前已经获得的远程安装授权。若产品要求服务器完全不能放置程序，应改选下文的无 reader 路线，并接受 SQLite 和传输性能方面的限制。

## 1. 范围与用户流程

首版建议本地仍仅支持 macOS 13+，远程先支持 Linux x86_64 / aarch64；远程 Windows、容器内自动发现、远程编辑和继续运行 agent 不包含在首版。

用户流程：

1. 在数据来源中选择「添加 SSH 服务器」。从本地配置得到别名候选，也允许直接输入已有 SSH 别名。
2. 测试连接，显示实际目标的用户、主机、端口和认证状态。
3. 首次使用时说明并部署 reader；也接受用户自行预装的受支持版本。
4. 选择「自动发现」「指定远程家目录」「按 agent 指定会话目录／数据库文件」。
5. 展示发现结果、路径和格式支持状态，保存选中的数据根。
6. 切换「本机 / 服务器」，再选择 agent。首版一次展示一个来源，不做跨服务器聚合。

远程目录输入不是本地文件选择器。可以输入绝对路径或相对于指定远程 home 的 `~/...`；后续可提供受限远程目录浏览。错误要区分连接失败、目录不存在、权限不足、不支持的格式和暂时没有会话。

## 2. 当前代码的适配情况

| 位置 | 已有能力 / 约束 | 建议改动 |
| --- | --- | --- |
| `crates/yes-core/src/providers/mod.rs` | `SessionProvider` 分离列表和详情；`ProviderRegistry` 只以 `AppType` 索引 | 保留解析接口；在其上增加来源和 reader 调用层 |
| `crates/yes-core/src/providers/codex.rs`、`codebuddy.rs` | 已有自定义 root，列表采用有限前缀读取 | 远程复用；目录输入增加明确的路径类型 |
| `crates/yes-core/src/providers/opencode.rs` | `with_database_path`、只读 SQLite、两套表结构探测 | 在远程主机上直接执行现有查询，补只读事务和超时 |
| `crates/yes-core/src/providers/codebuddy_cn.rs` | 默认是 macOS IDE 扩展的数据目录 | 不当作中文 CLI 的实现；按真实样本验证 CLI 兼容性 |
| `crates/yes-core/src/model.rs` | `Session` 和附件含本地 `PathBuf`，无来源 ID | 增加来源封装，区分远程引用和本地可访问文件 |
| `crates/yes-core/src/settings.rs` | serde 默认值兼容旧配置 | 新增来源配置，老用户默认只有本机 |
| `crates/yes-app/src/app.rs` | provider 选择、详情、子会话导航按 agent / session ID 工作 | 将来源带入选择、异步请求、导航及缓存键 |
| `crates/yes-app/src/app.rs` | UI 直接读取文件 metadata / WAL 签名，并每 5 秒刷新 | 变更检测下移到 source；远程刷新采用独立策略 |
| `crates/yes-core/src/search.rs` | 全文搜索可能逐个加载详情 | 远程搜索不得沿用逐会话往返；首版只开放摘要搜索 |
| `workspace.rs`、`git.rs`、`terminal.rs` 及附件视图 | 假定路径在本机，存在本地命令调用 | 远程来源首版禁用本地工作区、Git、Finder、恢复 CLI 等入口 |

不要给每个 provider 填入一套 SSH 分支，也不要把远程目录伪装为本地缓存目录后交给所有现有功能。前者重复实现，后者容易误读本机同名路径。

## 3. 连接方案选择

| 方案 | 优点 | 成本 / 局限 | 建议 |
| --- | --- | --- | --- |
| 系统 SSH + Rust reader | SSH 配置兼容性好；复用 provider；远程读取 SQLite；按需传输 | 需要分发 Linux 二进制和协议版本管理 | 推荐 |
| 系统 SSH/SFTP + 本地解析 | 不必部署 reader，JSONL 原型容易开始 | 大量元数据和文件往返；数据库需要一致性快照；不能靠下载活跃 db 保证正确 | 不允许部署程序时的备选 |
| Rust SSH 库直接连接 | 进程内管理方便 | 要验证 OpenSSH 配置、跳板、agent、macOS 特有配置等兼容性 | 首版不选 |
| SSHFS / 定时 rsync 全目录 | 可把远程文件映射成类似本地路径 | 引入挂载或同步依赖；过量复制；SQLite 一致性仍未解决 | 不选 |

SSH 无 reader 备选也应限定读取已知会话文件，不能同步整个 agent home。OpenCode 可要求远程提供 `sqlite3` 并生成一致性快照后下载；缺少能力时明确显示不支持，不能降级成复制活跃 `.db`。这条路线仍需维护 shell、工具版本和快照清理逻辑。

### 复用 SSH 配置的边界

- 使用 `/usr/bin/ssh`，将别名交给 OpenSSH；`Host`、`Include`、`Match`、`IdentityFile`、`IdentityAgent`、`ProxyJump` 等由它处理。
- 别名候选提取只是 UI 辅助：展开有限的 Include，去重，忽略通配符和否定项，限制递归；不能穷举动态 Host / Match 的目标，必须允许手输。
- 对用户选中的别名调用 `ssh -G` 预览有效配置；不要在启动时对所有候选执行它。配置中的 `Match exec` 等可能执行本地程序，不能把配置评估当成纯文本解析。
- 首版后台连接使用 `BatchMode=yes`、`StrictHostKeyChecking=yes`。未知主机需要用户先在终端完成主机身份确认；密钥口令应交给本地 agent / Keychain。密码和交互式 MFA 首版不承诺后台支持。
- Finder 启动的应用与终端可能有不同的 `SSH_AUTH_SOCK` 和 PATH；测试连接需给出准确诊断，不承诺“终端连得上，应用必然连得上”。
- 应用数据通道覆盖影响协议的配置：不分配 TTY、不启用 agent/X11/端口转发、不执行 LocalCommand，并处理已有 RemoteCommand。代理认证配置继续复用。
- 首版每个活动来源保持一条 SSH stdio 连接，明确超时和断开清理。为隔离用户的既有 master 生命周期，可对数据通道禁用共享 ControlPath；后续再评估应用专有连接复用。

`ssh -G` 和非 TTY 传输行为见 [OpenSSH ssh 手册](https://man.openbsd.org/ssh)，配置语义见 [ssh_config 手册](https://man.openbsd.org/ssh_config)。实际实现必须以最低支持 macOS 附带的 OpenSSH 版本做兼容测试，不能假定最新 OpenBSD 的所有选项都存在。

## 4. 推荐架构与数据模型

```text
GPUI 原生界面
  └─ SessionSourceService（选中来源、超时、缓存、请求代次）
      ├─ LocalSource → 现有 ProviderRegistry
      └─ SshSource → /usr/bin/ssh → stdio 协议
                                  └─ yes-session-reader（远程 Rust）
                                      └─ yes-core provider → JSONL / SQLite
```

reader 新增为独立 crate，只依赖领域模型和读取能力，不依赖 GPUI / WebKit，不运行 Node.js 或 Python。先验证 `yes-core` 在 Linux 的编译和行为，只在确有需要时用 feature / cfg 隔离 macOS 服务，不提前拆分大量 crate。

建议的概念模型：

```text
SourceConfig = Local | Ssh { id, label, ssh_alias, home_override?, roots[] }
ProviderRoot = { id, format, path_kind, remote_path }
SessionKey   = { source_id, root_id, app_type, provider_session_id }
SessionRef   = { key, summary, remote_locator?, revision? }
```

`root_id` 避免同一服务器上多个安装目录中相同 session ID 冲突。父子会话关系仍保留 provider 原始 ID，但解析时始终限定在同一来源和数据根。不能仅修改列表 key，详情、搜索、导航历史、token usage 和附件缓存都必须带上来源。

配置仅保存稳定的来源 ID、SSH 别名、可选 home 和路径选择；不保存私钥内容、密码，也不复制解析出来的主机和端口。SSH 配置改变后重新评估目标，清理旧连接；目标发生变化时使原缓存失效，避免把旧服务器内容展示为新服务器内容。

异步后台任务承载远程请求，GPUI 主线程不等待 SSH。给每次来源切换一个 generation；旧请求返回时，若 generation 不匹配就丢弃结果。

## 5. 目录发现

查找顺序建议：用户显式路径优先；未指定时，探测 reader 进程可见且受支持的 agent 环境变量；最后按选定 home 的默认规则探测。若用户显式指定 home，则发现范围以该 home 为准，不悄悄使用登录账户其他目录的环境覆盖。

只探测已知目录并检查格式特征，不对整个 home 或 `/` 做递归扫描。不要求 agent 可执行文件已安装，只要求存在受支持的数据。

| 数据类型 | 默认候选 | 当前证据与边界 |
| --- | --- | --- |
| CodeBuddy CLI | `<home>/.codebuddy/projects` | 当前 provider 使用 `.codebuddy` 根；官方中英文 CLI 文档也描述 projects 数据 |
| 中文 CodeBuddy CLI | 同样先探测 `.codebuddy/projects` | 不能凭语言创建第二份重复数据源；有不同根时分别配置，格式差异用样本判定 |
| Codex CLI | `<home>/.codex/sessions` | 当前 provider 的读取位置；历史归档等位置按实际支持版本另测，不能假定已支持 |
| OpenCode | `<home>/.local/share/opencode/opencode.db` | 当前 provider 读取此 db，支持已实现的两类 schema；旧版文件存储不自动宣称兼容 |
| Claude Code | `<home>/.claude/projects` | 当前 provider 的默认位置，可沿用同一方案 |
| CodeBuddy CN IDE | 当前 macOS `Library/Application Support/CodeBuddyExtension/Data` | 不是 Linux CLI 默认目录；不要直接迁移这条规则 |

CodeBuddy 官方说明支持 `CODEBUDDY_CONFIG_DIR`，并记录了 projects 下的会话相关目录，见[环境变量参考](https://www.codebuddy.cn/docs/cli/env-vars)。OpenCode 官方说明其数据默认位于 `~/.local/share/opencode/`，见[存储与排障文档](https://opencode.ai/docs/troubleshooting/)；具体格式仍以 provider schema 检查为准。

实现时应支持经当前版本文档／样本验证的 `CODEX_HOME`、`XDG_DATA_HOME` 等覆盖规则。SSH 非交互进程不保证拥有用户交互 shell 中的变量，因此自动发现失败必须能手动覆盖，不自动 source 任意 shell 初始化文件。

路径类型必须明确：agent 数据根、sessions/projects 目录、单个项目会话目录和 SQLite 文件不能混用。由路径适配器转换为 provider 所需 root；无法可靠推断时要求选择类型，不用反复向上遍历猜测。用户输入的 `~` 由 reader 按远程 home 展开，不能由 macOS 展开。

部分摘要还依赖同级辅助文件：Codex 使用 `session_index.jsonl`，CodeBuddy 使用 `sessions/*.json` 获取标题，也会读取计划文件。选择完整 agent 数据根时可以在授权边界内复用；只选择单个 sessions/projects 目录时，不自动扩大读取边界，缺失的标题等信息应降级显示。实现前要列出每个 provider 实际读取的辅助文件白名单。

## 6. reader 协议与读取策略

最小方法：`hello`、`discover`、`list_sessions`、`get_session_detail`。协议包含版本、请求 ID、结构化错误、长度上限和数据根标识。建议长度前缀 JSON 帧，stdout 仅传协议，stderr 传诊断；对 shell 启动噪声给出握手失败提示，禁止无界扫描或吞掉任意输出。

列表只传摘要，支持分批返回。不要把当前同步 `Vec<Session>` 接口直接等同于高性能分页：可以先复用内部扫描并分批发送，后续按基准结果改为增量索引。详情与子会话 usage 可在 reader 内一次聚合，避免每个子会话一次网络往返。

JSONL：沿用轻量列表策略，只在打开详情后解析完整内容；并发写入时只接受完整记录，文件截断／替换时重新读取，明确响应是否完整。mtime、size 和文件身份用于缓存失效提示，不把它们当成严格内容哈希。给详情响应和解析任务设置上限与取消，超限显示明确状态，后续再加入消息分页。

SQLite：在远程主机直接以只读模式连接，使用短读事务保证一次详情中多次查询的视图一致，设置 busy timeout；不执行 migration、checkpoint 或写入 agent 表。活跃 WAL 数据不能使用 `immutable=1` 绕过锁。只读打开在某些权限／WAL 条件下可能失败，应报告而非修改源库权限。

不能复制活跃 `.db` 后在本机查询，也不能把分别下载 db、wal、shm 视为一致快照。SQLite 的 [WAL 文档](https://www.sqlite.org/wal.html) 和 [在线备份 API](https://www.sqlite.org/backup.html) 说明了相关机制；如果未来确需快照，应由远程 SQLite 一致性备份机制生成。

首版手动刷新，连接中可加低频轮询，断开后停止；不承诺远程文件监视实时性。首版摘要搜索和当前详情内搜索可用；跨会话全文搜索后续在远程执行并只返回命中摘要，不能逐个把全部历史拉回本机。

本地默认只缓存摘要和已打开详情的内存数据。持久离线缓存作为后续显式选项，需要容量上限、清理入口和来源隔离；断线时明确显示已缓存／未缓存，不能把连接失败显示成零条会话。

## 7. 路径和动作边界

- 本地以参数数组启动 ssh，禁止拼接到本地 shell；严格校验 SSH 目标，拒绝以 `-` 开头及控制字符等异常输入。
- 远程命令仍会经过远程 shell，仅使用固定 bootstrap / reader 命令和正确引用的安装路径。用户目录、搜索词、session ID 通过 stdin 协议传入，不放入远程命令字符串。
- reader 只接受已登记的数据根和受限方法，不提供任意命令执行或任意文件读取 RPC。
- 对真实路径和 symlink 边界做校验；对可变 symlink 要使用目录句柄相对打开等方式防止检查与使用之间逃逸，不能仅依赖一次 canonicalize。
- provider 的辅助文件读取也要审计，例如计划文件、tool-results 和图片；即使没有通用 read-file RPC，解析器也可能跟随会话中的路径读取额外文件。
- 远程 `AttachmentSource::LocalPath` 不能直接进入本地附件解析器。首版仅显示已经嵌入的内容，远程文件引用展示占位；后续通过有边界的下载接口处理。
- 首版隐藏远程来源的 Finder 打开、工作区文件树、Git 操作和本地 CLI 恢复入口。后续支持远程继续会话必须单独设计 SSH 终端动作。
- 不整目录读取 `auth.json`、凭据或 SSH 私钥；会话内容本身也可能包含秘密，日志不记录正文或完整连接配置。

## 8. reader 分发

建议构建 Linux x86_64 / aarch64 的独立 release 产物，优先评估 musl 构建并用实际发行版验证 bundled SQLite 和依赖兼容性。不能因使用 Rust 就宣称已经跨平台。

首次连接探测 OS / arch，用户同意后部署到用户私有、按版本隔离的目录；无需 sudo。由本地经 SSH 上传，远程无需联网下载。安装路径、架构不匹配、磁盘不足、noexec、仅 SFTP / forced-command 账户均需明确失败。

采用可信发布清单中的校验值验证产物，临时文件写入后原子替换，校验目录所有权和权限。协议握手确认兼容性；不兼容则要求更新 reader，不能静默按旧结构解释新字段。断连／stdin EOF 后 reader 退出，异常断线由超时兜底。

CI 增加 Linux reader 构建、协议与 provider 测试；桌面保持当前签名、公证和 DMG 发布流程。reader 是新增发布产物，实施时评估体积和许可证，不在本次调研中修改工作流。

## 9. 实施顺序和验收

| 阶段 | 交付 | 验收 |
| --- | --- | --- |
| P0 技术验证 | Linux core/reader 编译；真实 CLI 脱敏样本；SSH stdio 原型 | 经别名和跳板连接，正确读取一个 JSONL 详情和正在写入的 OpenCode WAL 数据；验证中文 CLI 格式 |
| P1 来源模型 | SourceConfig、SessionKey、来源隔离和兼容迁移 | 本机原有行为不变；两个来源相同 ID 不串会话；旧请求不能覆盖新来源 |
| P2 reader 和传输 | 目录发现、只读列表/详情、协议、部署与错误状态 | 自定义路径、空格/中文/引号、symlink、超时、取消、坏帧、版本不匹配测试通过 |
| P3 原生 UI | SSH 来源设置、目录选择、连接状态、来源切换 | 中英文完整；本地动作不会误用于远程路径；断网不清空既有视图 |
| P4 发布验证 | 真实 Linux 矩阵、性能基准、reader 产物与文档 | workspace fmt/check/test、macOS 打包及 codesign 验证通过，Linux 构建和集成测试通过 |

性能验收建议使用 1 万会话、单文件 100 MB、模拟 100 ms RTT 的数据集：列表阶段不得全量解析／下载大型 JSONL；一次列表刷新不能每个文件启动一个 SSH；详情只传选中会话及必要子会话；取消后停止继续传输。记录首屏耗时、字节数、远程 CPU 和内存，再制定具体耗时预算，当前不提供未经测量的性能承诺。

还需覆盖：未知／变化的主机密钥、加密私钥、GUI 启动环境、ProxyJump、Include/Match、缺少 agent、非交互 shell 噪声、JSONL 半行、SQLite busy、路径越界、会话删除和服务器重装。只读测试应确认不执行 agent 命令、不修改原始会话内容。

## 10. 实施前保留的产品决策

推荐默认值已经给出，但开始编码前应确认：是否接受首次部署 reader；远程首版是否仅限 Linux；中文 CodeBuddy CLI 的实际版本和脱敏样本。它们不阻碍当前方案调研，但会影响正式支持范围。

本次已完成代码只读检查和官方资料核对，未连接用户服务器、未读取用户 SSH 配置或凭据、未部署 reader，未运行构建或集成测试。后续实施应按 feature 分支和 PR 流程推进；该功能若按计划交付，建议使用 `minor` 版本标签。
