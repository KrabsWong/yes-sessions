# Yes Sessions Electron 版本 review 记录（归档）

> 本文只记录 9.x Electron 版本的历史整改，不能作为当前 Rust/GPUI 实现依据。当前事实以根目录 `README.md`、`AGENTS.md`、`DESIGN.md` 与 `crates/` 源码为准。

**版本**: 9.0.0
**更新日期**: 2026-05-25
**用途**: 旧版迁移追溯

## 已处理

- `chatLayout` 已补入共享 `AppSettings` 和 `DEFAULT_SETTINGS`，避免设置重置/导入导出时丢失该布局配置。
- 删除未被引用的 `src/hooks/usePerformance.ts`，减少调试型死代码。
- 移除多处运行路径上的调试 `console.log`，主进程 Git watcher 改用 `electron-log`。
- README 已注明实际 i18n 资源位于 `src/lib/i18n/index.ts`，`src/locales/*.json` 只是参考。
- 已移除内置终端特性，包括 renderer xterm 客户端、主进程 PTY manager、`node-pty`/`@xterm/*` 依赖、设置中的 builtin 选项和打包白名单；历史 `builtin` 配置会迁移回默认 `auto`。
- `src/lib/api/` 已继续按领域拆分为 `sessions/settings/app/files/git` 等模块，并删除无额外抽象价值的 barrel 入口 `src/lib/api/index.ts`；调用方直接依赖对应领域 wrapper。
- `electron/main.ts` 已把应用级 IPC 注册、文件预览窗口、文件读取、tree/git handler 注册拆到 `electron/handlers/app.ts`。
- `electron/handlers/app.ts` 已继续收敛为聚合注册入口，settings/file-preview/file/git handler 已拆成独立模块。
- 已补最小 Vitest 覆盖：API response helper、应用配置一致性、默认设置形状。`npx vitest run` 现在可作为有效质量门。
- 已补 session message parser 测试，覆盖 OpenCode 文件块、Claude 清理和 Codebuddy 清理。
- 已补 ConversationView 纯逻辑测试，覆盖 Claude XML 解析、消息 turn 分组、延迟 tool result 匹配、工具分类/摘要、语言识别、时间/值格式化和 diff 计算。
- 已补 file/git API wrapper 测试，覆盖 IPC channel、参数透传、成功/失败响应处理、Git watch 监听和清理。
- 已补 settings store mutation 测试，覆盖默认值补齐、局部初始设置合并、通用更新、布局/侧栏/theme toggle 和主进程同步失败时的本地状态行为。
- `SettingsDialog` 的体验设置 toggle UI 已收敛为 `ToggleSettingItem`，减少重复 JSX。
- `InitialSettings`、`IElectronAPI`、`AccentColor` 已收敛到共享类型；settings store 的状态字段从 `AppSettings` 派生，减少新增设置时的重复维护点。
- 设置迁移逻辑已收敛到 `src/lib/settings/migration.ts`，主进程配置存储和 renderer settings store 共用同一套归一化规则；electron-store 会记录 `settingsVersion`，启动、读取、更新、导入时都会补齐缺失字段并过滤非法持久化值。
- Git handler 已从 shell 字符串拼接改为 `execFile` 参数数组调用，并对传入的 repo 文件路径做相对路径约束，降低路径注入和越界读取风险。
- 文件读取 handler 已增加目录根约束：`tree:get` 会注册 realpath 后的允许根目录，`file:read` 和 `file:readImage` 只允许读取该根目录内的文件，并用 realpath 防止路径穿越或 symlink 越界。
- 已补 Electron IPC contract 测试，覆盖 `ipcRegistry` 统一成功/失败响应包装、注册覆盖、注销/清理，以及 `registerAppHandlers()` 聚合注册入口。
- 已补会话列表组件级交互测试，覆盖 `VirtualSessionList` 的日期/目录分组渲染、session 选择回调、全部折叠和展开行为。
- 已补会话详情组件级交互测试，覆盖 `ConversationView` 的 system/user/assistant 渲染、tool call 展示以及展开后输入/输出展示。
- 本文档和 `docs/PROJECT_RESEARCH.md` 已从旧的 CC Switch/Tauri 重写语境更新为当前 Yes Sessions/Electron 项目事实。
- README、安装脚本、Homebrew 和版本管理文档中的示例版本已同步到当前 `package.json` 版本语境，支持矩阵和应用描述已统一为当前支持/暂未接入状态。
- 运行时 i18n 和备份 locale 中的不支持应用文案已统一为“暂未接入 / Not Connected”，并删除未被当前源码使用的 `APP_TYPES` 兼容导出别名。
- 已移除当前源码未引用的历史依赖，包括旧版 `xterm` 包、CodeMirror/@uiw 编辑器链路、表单/图表/代理/自动更新相关包，并同步清理打包白名单中的无效条目。
- 已继续清理无引用的 shadcn switch 组件、对应 Radix 依赖、未使用的 `png-to-ico` 依赖，以及 Vite Electron external 中已不存在的 `keytar` 残留。
- 已继续清理确认无引用的组件/工具和依赖：`HelpDialog`、`ConfirmDialog`、`EmptyState`、`Badge`、`Spinner`、`ScrollArea`、`src/lib/session-files.ts`、`@radix-ui/react-scroll-area`，并删除旧非 Electron 迁移日志。
- `src/locales/en.json` 和 `src/locales/zh.json` 已从运行时 `src/lib/i18n/index.ts` 重新生成，移除旧的通用 CRUD/empty state 备份文案，保持备份/参考文件与实际加载资源一致。
- 已删除确认无入口的旧会话上下文组件；文件预览/Git diff 已统一走独立预览窗口，相关未引用的旧搜索和 context panel 翻译也已裁剪。
- Git、文件树、会话统计、支持状态和终端信息等 IPC payload 类型已收敛到 `src/types/ipc.ts`，renderer API wrapper 和 Electron handler 共用同一套类型定义。
- `ipcRegistry.register()` 已支持可选运行时参数校验；文件、Git、settings、session 等本地能力 IPC 通道已补基础参数校验，并补充 registry/validation contract 测试。
- 旧的 CC Switch 重写计划、Session Viewer 早期计划、Bad Cases 和 `docs/superpowers/` 历史计划材料已压缩为短归档说明，避免旧依赖、旧任务指令和旧路线图被误认为当前事实。
- 文件读取允许根已从全局集合改为按 `webContents` sender 隔离：窗口必须先通过 `tree:get` 注册目录，才能在同一窗口内调用 `file:read`/`file:readImage` 读取该目录下文件，并已补 handler 测试覆盖跨窗口拒绝访问。
- `ipcRegistry.register()` 已支持可选返回值校验；文件树、Git status/file diff、session stats/support status 和 terminal info 等稳定 IPC payload 已补轻量 schema 校验和 contract 测试。
- `sessions:getAll` 和 `sessions:getDetail` 已补 `Session[]` / `SessionDetail | null` 轻量返回值校验，覆盖会话基础字段、消息数组和消息基础字段，保留各来源 metadata 扩展。
- 已补 Playwright Electron smoke：`npm run test:e2e` 会先执行 `tsc && vite build`，再以生产入口启动 Electron，并验证主窗口 preload、核心 settings/session IPC 和可选 session detail 路径。
- 主进程运行时对 `electron` 模块的导入已改为 default import 后解构，修复生产构建产物在 `electron .` 启动时对 CommonJS named export 不兼容的问题。
- 已删除未提交的旧非 Electron 迁移残留，避免无关产物继续污染源码边界。
- 已将分散在 `src/` 和 `electron/` 下的 Vitest 文件集中迁移到 `tests/src/` 与 `tests/electron/`，`e2e/` 保持独立，并同步更新测试 import 与文档。
- 已新增 `tsconfig.test.json`，并让 `npm run typecheck` 同时覆盖集中测试目录、E2E 和测试配置文件；format 脚本也已纳入 `tests/`、`e2e/` 和根配置文件。
- Electron E2E 已补设置弹窗路径：覆盖主窗口打开设置、General/Experience/Terminal tab 切换和弹窗关闭，避免设置 UI 回归只由组件/手工验证兜底。
- `SessionDetail` IPC 返回值校验已从基础字段类型推进到按消息类型校验语义必需字段：普通消息需要内容/推理/脱敏内容，`tool_use` 需要工具名和输入，`tool_result` 需要工具名和输出。
- `AppSupportSummary.status` 已从裸 `string` 收紧为共享 `AppSupportStatus` 枚举，并在 IPC result 校验中拒绝未知支持状态。
- 已移除 settings store 中无引用的旧 `useExperienceStore` 兼容导出，避免后续代码继续依赖已合并的旧 store 名称。
- 已删除无入口的手工调试脚本，避免未纳入 package 脚本/文档的会话探查和 Mermaid smoke 代码继续作为维护负担。
- preload 中重复的 `Window.electronAPI` / `__INITIAL_SETTINGS__` 全局声明已移除，renderer 全局类型统一由 `src/types/electron.d.ts` 维护。
- 已移除 `package.json` 中与 `electron-builder.yml` 重复且不一致的旧打包配置，Electron Builder 配置事实来源统一为 `electron-builder.yml`。
- 已移除未直接使用的顶层 `javascript-obfuscator` devDependency；生产混淆仍通过 `vite-plugin-javascript-obfuscator` 及其自带依赖执行。
- 已将纯类型包 `@types/react-syntax-highlighter` 从运行时 dependencies 移到 devDependencies，生产依赖集合只保留实际运行时的 `react-syntax-highlighter`。
- Tailwind content 扫描范围已从模板遗留的根级 `pages/components/app` 目录收敛到当前实际入口 `index.html`、`file-preview.html` 和 `src/**/*`。
- 应用会话支持状态已从主进程 `sessions:getSupportStatus` 的重复硬编码表收敛到共享 `src/config/apps.ts`，renderer 和 Electron handler 共用同一份 `APP_SESSION_SUPPORT`。
- `IElectronAPI` 已从任意字符串 channel 收紧为共享 `IpcChannel` / `IpcEventChannel`，renderer 侧 IPC 调用和事件监听能在类型检查阶段发现拼写错误。
- 已从 `IpcChannel` 删除没有 renderer API wrapper、也没有 Electron handler 的旧通道声明：`app:checkForUpdates`、`app:quit`、`app:minimize`、`config:backup`。
- `SessionStatsSummary` 已改为复用业务层 `SessionStats` 类型，删除无引用的 `AppSessionSupport` 类型；`sessionsApi` 的 appType 参数也已从裸 `string` 收紧为 `AppType` 并补 wrapper 测试。
- `SessionServiceRegistry` 已删除无生产调用的 `getAll()` / `has()` 方法和旧 `SessionServiceInterface` 类型别名，`SessionService.getStats()` 也改为复用共享 `SessionStats`。
- Git diff 预览中剩余的关闭按钮 title、图片 alt 和行数单位已迁入运行时 `diff.*` i18n key，并同步重新生成备份 locale 文件。
- 主题快速切换按钮的 hover title 已迁入运行时 `settings.*` i18n key，并同步重新生成备份 locale 文件。
- 已删除当前无引用的 `Card`、`Input`、`Label` shadcn 组件文件，并移除 `Layout` 中重复的 i18n 副作用导入；i18n 初始化保留在应用入口 `src/main.tsx` 和文件预览入口 `src/file-preview.tsx`。
- 安装脚本、README、安装文档和 Homebrew 指南中的 GitHub Release/Raw URL 已修正到当前仓库 `KrabsWong/yes-sessions` 和公开 Homebrew 产物仓库 `KrabsWong/homebrew-yes-sessions`，并修正安装脚本 help 里的旧版本示例与 `docs/INSTALL_SCRIPT.md` 中错位的 Markdown 片段。
- 主进程错误工具已移除旧 `CCError` 命名和未使用的 Result/sync wrapper/helper 导出，保留当前 IPC registry 实际使用的 `AppRuntimeError`、validation/input 工厂和 async IPC 错误包装。
- 已删除无 UI 入口的 config import/export 和 renderer shell API，包括对应 IPC channel、handler、wrapper 与 config-store 导入导出方法；外链打开仍由主进程窗口策略处理。
- 会话消息 parser 已从动态注册表收敛为静态内置 parser 表，删除未使用的 register/unregister/get parser API，当前只暴露生产使用的解析和能力判断函数。
- `ConversationView` 目录入口已停止 re-export 内部子组件、类型和工具函数，页面侧只通过该入口消费主组件，内部测试按需直连具体模块。
- 应用配置模块已删除仅测试使用的描述表、支持/不支持列表 helper 和聚合信息 helper；`isAppSupported()` 直接从共享 `APP_SESSION_SUPPORT` 派生。
- hooks 导出面已继续收窄：`settingsKeys`、toast reducer、顶层 toast 函数和 toast 类型不再对模块外导出，只保留当前组件实际消费的 hook。
- 已删除应用自有 `yes-sessions.db` 管理器和 schema：当前没有业务读写该库，设置持久化走 `electron-store`，`better-sqlite3` 仅保留给 OpenCode 外部 SQLite 会话读取。
- 性能监控和 Git watcher 服务导出面已收窄：删除未使用的内存日志方法、`PerformanceMonitor` class export 和 `getGitWatcher()`。
- 共享 `ErrorCode` 已收窄到当前 IPC 错误包装实际产生的 `UNKNOWN_ERROR`、`VALIDATION_ERROR`、`INVALID_INPUT`，删除未使用的通用错误码和 `ERROR_CODES` 导出。
- 文件预览 IPC/API 已删除未被预览入口读取的 `appType` 参数，保留实际使用的目录和会话标题。
- 各会话来源服务已停止导出 class 构造器，只保留 handler 注册实际消费的单例服务导出。
- 文件预览独立入口和错误边界中的用户可见兜底文案已迁入运行时 i18n 资源，并同步备份 locale。
- 会话详情头部的会话 ID、更新时间、工作目录、未命名会话、新消息提示和复制 title 已迁入运行时 i18n 资源；更新时间格式跟随当前语言。
- 会话列表卡片的未命名会话兜底和目录视图日期格式已改为复用 `sessions.untitledSession` 和当前 i18n 语言。
- Electron E2E 已补临时 Codebuddy fixture 路径：在隔离 HOME 中创建最小 JSONL 会话，覆盖会话列表发现、选择、详情加载和 user/tool/assistant 内容渲染。
- Electron E2E 已补文件预览窗口路径：复用隔离 Codebuddy fixture，从会话详情打开独立文件预览窗口，覆盖文件树渲染、项目文件选择和内容读取。
- Electron E2E 已补 Git diff 用户路径：在临时 git 仓库 fixture 中制造已提交文件的工作区修改，覆盖文件预览窗口 Git tab、变更文件选择和 diff 内容渲染。
- 外部终端恢复逻辑已拆出纯函数测试：覆盖 Ghostty/Kitty/Terminal.app 的选择优先级、不可用偏好回退、工作目录存在/缺失时的启动参数，以及各应用 resume 命令映射；`getTerminalInfo()` 现在返回实际有效终端，避免偏好终端不可用时 UI 禁用 Resume。
- 会话详情、工具调用、Mermaid 图表、文件预览和虚拟列表中的剩余硬编码用户可见文案已迁入运行时 i18n 资源，并同步重新生成备份 locale。
- 会话列表日期分组已从本地化显示字符串改为稳定的 `YYYY-MM-DD` group key，展示时再按当前 i18n 语言格式化；折叠状态、排序和语言显示不再耦合，`VirtualSessionList` 也不再需要外部传入翻译函数。
- 会话组件内部已移除未被项目消费的 default export，统一使用 named export；代码块组件中的剩余用户可见文案也已迁入运行时 i18n 资源并同步备份 locale。
- Renderer API 入口已删除未提供额外抽象价值的聚合 `api` 对象，调用方改为直接使用 `filePreviewApi`、`fileApi` 等具名 API wrapper，减少重复入口和间接依赖。
- Renderer API wrapper 已停止 re-export 业务/IPC payload 类型；组件和测试统一从 `src/types` 引用共享类型，API 模块只保留调用封装职责。
- IPC registry 已移除未被生产代码消费的 `has()`、`getChannels()`、`unregister()` 和内部类型导出，只保留注册 handler 与应用退出清理所需的公开能力。
- 配置存储模块已停止导出 `ConfigStore` class 和 `StoreSchema` 类型，只保留生产代码实际消费的 `configStore` 单例。
- 终端 launcher 已停止导出仅模块内部使用的 Ghostty/Kitty 安装检测函数；外部只保留恢复会话和读取终端信息两个业务入口。
- 路径边界工具已停止导出单根判断 helper，只保留生产代码实际消费的多根校验入口 `isPathInsideAnyRoot()`。
- 主题颜色工具已移除无消费者的 `defaultAccentColor` 导出，并将仅内部使用的 `ColorOption` 类型收为模块私有。
- Git IPC handler 已停止导出仅注册函数内部调用的 `getGitStatus()`、`getGitDiff()` 和 `getFileDiffContent()`；主进程错误工具中的 `AppRuntimeError` 也已收为模块私有。
- Session service registry 已停止导出仅测试 mock 使用的 `SessionService` 接口；hooks 中也已将完整 `sessionsKeys` 收为模块私有，只暴露设置更新需要复用的 `terminalInfoQueryKey()`。
- ConversationView 代码块主题对象 `tokyoNightTheme` 已收为 `CodeBlock.tsx` 模块私有，避免内部语法高亮样式成为组件公共 API。
- 测试文件已统一位于 `tests/` 和 `e2e/` 下，当前 `src/` 与 `electron/` 中没有散落的 `.test` / `.spec` / `__tests__` 文件；继续保留集中测试目录结构。
- 设置弹窗和会话页中已确认存在运行时 i18n key 的旧硬编码 fallback 已移除，用户可见文案继续以 `src/lib/i18n/index.ts` 为事实来源。
- ThemeProvider 已移除旧“兼容” action wrapper，context 直接暴露 settings store 的 async theme/accent action；仅 `resetAccentColor()` 继续封装额外 DOM 变量清理。
- 已移除无 UI 入口的会话搜索过滤/高亮死路径，包括 `ConversationView.searchQuery` 传递链、`HighlightedText` 组件和对应自维持测试；README 与研究文档也已停止宣称当前支持全文搜索。
- ConversationView 已移除被常量永久关闭的代码块折叠、长回复截断和工具输出折叠分支，并清理对应未使用常量与 i18n 文案；仅保留实际生效的超长代码禁用语法高亮保护。
- 主题切换模块已删除无消费者的 `ThemeToggleButton` 备用按钮，并清理仅该按钮使用的 `themeSwitchToLight` / `themeSwitchToDark` 文案。
- 语言、主题和主色调设置组件中已确认存在运行时 i18n key 的旧 fallback 文案已移除，设置 UI 文案继续统一来自 `src/lib/i18n/index.ts`。
- Renderer API 调用方已从 `@/lib/api` barrel 改为直接导入 `@/lib/api/{app,files,git,sessions,settings}`，并删除 `src/lib/api/index.ts` 这个重复入口。
- IPC `appTypeArg()` 校验已改为复用 `src/config/apps.ts` 的 `APP_ORDER`，删除 validation 模块里重复手写的应用类型集合。
- 设置归一化逻辑已改为复用 `src/config/apps.ts` 的 `APP_ORDER` 校验默认应用，并将语言类型收敛到当前实际提供的 `en` / `zh` 两种运行时资源。

## 当前风险

本轮优化到此收口。下面两项不再在当前 goal 中继续展开，后续如果要继续提高可靠性，可以从这里恢复。

### 1. 测试覆盖仍需继续拓宽

`npx vitest run` 已可运行，覆盖纯逻辑、renderer API wrapper、settings store、IPC registry contract、部分主进程边界、会话列表和会话详情组件交互、外部终端选择和启动参数构建。`npm run test:e2e` 已覆盖生产入口 Electron 启动、核心 IPC smoke、设置弹窗 tab 交互、使用隔离 Codebuddy fixture 的列表到详情路径、文件预览窗口的文件树和内容读取路径，以及 Git diff 面板的变更选择和内容渲染路径。剩余缺口主要是不能在自动化里稳定验证真实 GUI 终端启动后的系统级行为。

### 2. 复杂 session payload 仍缺少业务级语义校验

主要 IPC payload 类型已集中到 `src/types/ipc.ts`，renderer API wrapper 和 Electron handler 已共享类型定义；高风险 IPC 通道的输入参数已补运行时校验，稳定结构化返回值和会话基础 payload 也已补轻量校验。`SessionDetail` 现在会校验不同 message type 的语义必需字段，`AppSupportSummary.status` 也会限制在已知支持状态内。剩余风险集中在更细的跨来源业务语义，例如不同工具的 message metadata、tool call/tool result 配对策略和特殊内容块含义，当前仍主要依赖 TypeScript 类型和 parser/组件测试。

## 建议的下一轮重构

1. 如需进一步验证外部终端恢复，只做手工 smoke 或引入可注入 launcher adapter；不要在 E2E 中真实启动 Terminal.app/Ghostty/Kitty。
2. 继续评估跨来源 session message 的 metadata、tool call/tool result 配对和特殊内容块是否需要更细的 parser 级校验。
3. 后续新增测试继续保持集中目录结构：Vitest 放在 `tests/`，Electron/浏览器级流程放在 `e2e/`，避免重新散落回业务源码目录。
