# Yes Sessions Electron 版本研究文档（归档）

> 本文只记录 9.x Electron 版本，不能作为当前架构或实现依据。当前事实以根目录 `README.md`、`AGENTS.md`、`DESIGN.md` 与 `crates/` 源码为准。

**版本**: 9.0.0
**更新日期**: 2026-05-25
**用途**: 旧版迁移追溯与视觉/功能比对

## 1. 项目定位

Yes Sessions 是一个 Electron 桌面应用，用于浏览、管理和恢复 AI CLI 工具的本地会话历史。当前重点不是供应商配置管理，而是会话读取、会话详情展示、文件上下文预览、Git 变更查看和终端恢复。

## 2. 当前技术栈

| 层级         | 技术                                            |
| ------------ | ----------------------------------------------- |
| 桌面框架     | Electron 30                                     |
| 前端         | React 18 + TypeScript                           |
| 构建         | Vite                                            |
| UI           | Tailwind CSS + shadcn/ui + Lucide React         |
| 状态         | Zustand + TanStack Query                        |
| 国际化       | i18next，运行时资源位于 `src/lib/i18n/index.ts` |
| 主进程持久化 | electron-store                                  |
| 外部会话读取 | JSONL 文件，OpenCode SQLite 数据库              |
| 终端         | Ghostty / Kitty / Terminal.app launcher         |

## 3. 功能范围

### 会话管理

- 支持 Claude Code、OpenCode、Codebuddy、Codex CLI。
- 会话列表支持按日期/目录查看、虚拟滚动和详情加载。
- 会话详情支持 Markdown、代码高亮、工具调用、子 Agent、模型信息和思考内容显示控制。

### 文件与 Git 上下文

- 根据会话工作目录展示文件树。
- 支持文本、图片、diff 和 Git 变更预览。
- 主进程提供 `tree:*`、`file:*`、`git:*` IPC handler；文件读取只允许访问已通过 `tree:get` 注册过的目录根内的 realpath。
- Git watcher 只监听当前活动目录，并通过 `git:changed` 通知渲染进程刷新。

### 终端恢复

- 支持外部终端 Ghostty、Kitty 和 Terminal.app。
- 当前已移除内置终端和 PTY/xterm 运行路径，恢复会话统一由主进程 `electron/services/terminal/launcher.ts` 启动外部终端。

### 设置

- 设置持久化在 electron-store。
- 共享设置类型在 `src/types/index.ts`。
- 设置归一化和版本号在 `src/lib/settings/migration.ts`，主进程配置存储与 renderer settings store 共用该逻辑。
- 渲染进程初始化时通过 preload 同步读取 `__INITIAL_SETTINGS__`，Zustand store 再负责 UI 状态和写回。
- 文案实际从 `src/lib/i18n/index.ts` 的内联 `enTranslations`、`zhTranslations` 加载。

## 4. 当前分层

```text
src/
  pages/Sessions/                 # 主页面编排
  components/                     # 应用级 UI
  components/sessions/            # 会话、文件、Git、对话详情 UI
  components/sessions/ConversationView/
  hooks/                          # 查询、设置、Git watch、布局 resize
  stores/                         # Zustand 客户端状态
  lib/api/                        # 渲染进程 IPC wrapper
  lib/i18n/                       # 运行时翻译资源
  lib/settings/                   # 设置迁移、归一化和版本号
  lib/terminal/                   # 终端恢复命令构建
  types/                          # 前后端共享类型

electron/
  main.ts                         # 应用生命周期、主窗口和 splash
  handlers/                       # app/settings/file/file-preview/session/git/tree IPC
  handlers/app.ts                 # 应用级 handler 聚合注册入口
  services/session/               # 各 AI 工具会话读取服务
  services/terminal/              # 外部终端启动服务
  services/performance/           # 启动性能日志
  ipc/registry.ts                 # IPC 注册与统一响应包装
  utils/                          # 配置和错误工具
```

## 5. 维护注意事项

- 新增 UI 文案必须同步更新 `src/lib/i18n/index.ts` 的中英文对象。
- `src/locales/*.json` 不参与运行时加载，更新它们不能修复缺失翻译。
- 新增设置项时必须更新 `AppSettings`、`DEFAULT_SETTINGS` 和 `src/lib/settings/migration.ts`；`InitialSettings` 由共享类型派生，store 字段应继续从 `AppSettings` 派生，避免重复定义。
- 主进程 handler 应通过 `ipcRegistry.register` 注册，保持统一响应格式。
- `sessions:getDetail` 的返回值校验会按 message type 检查语义必需字段；新增会话来源时应保证解析结果满足 `user/system/assistant/tool_use/tool_result` 的字段约定。
- 单元/集成测试集中放在 `tests/`：`tests/src/` 镜像 renderer/shared 代码，`tests/electron/` 镜像主进程代码；`e2e/` 独立保存 Playwright Electron 烟测。
- `npm run typecheck` 会同时检查应用源码和集中测试目录；测试相关 TypeScript 配置位于 `tsconfig.test.json`。
- IPC contract 的基础行为由 `tests/electron/ipc/registry.test.ts` 和 `tests/electron/handlers/app.test.ts` 覆盖；新增 handler 分组时应同步补注册测试。
- 外部终端恢复的选择和启动参数由 `tests/src/lib/terminal/externalTerminal.test.ts` 覆盖；真实 GUI 终端启动不纳入 E2E，避免自动化环境被系统应用状态污染。
- 会话列表交互由 `tests/src/components/sessions/VirtualSessionList.test.tsx` 覆盖；修改列表分组、折叠或选择行为时应同步更新测试。
- 会话详情基础交互由 `tests/src/components/sessions/ConversationView/index.test.tsx` 覆盖；修改 turn 渲染或 tool 展开行为时应同步更新测试。
- Electron E2E smoke 由 `e2e/electron-smoke.e2e.ts` 覆盖生产入口启动、核心 IPC、设置弹窗基础 tab 交互、隔离 Codebuddy fixture 的列表到详情路径、文件预览窗口的文件树和内容读取路径，以及 Git diff 面板的变更选择和内容渲染路径。
- 旧非 Electron 迁移残留已从工作区清理；当前源码边界以 Electron + React + TypeScript 为准。
