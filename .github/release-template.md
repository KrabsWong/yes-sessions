## 安装指南

### macOS

#### Apple Silicon (M1/M2/M3/M4)

1. 下载 `Yes-Sessions-5.6.0-arm64.dmg`
2. 打开终端，移除安全隔离属性：
   ```bash
   xattr -c ~/Downloads/Yes-Sessions-5.6.0-arm64.dmg
   ```
3. 双击 DMG 安装

#### Intel Mac

1. 下载 `Yes-Sessions-5.6.0-x64.dmg`
2. 打开终端，移除安全隔离属性：
   ```bash
   xattr -c ~/Downloads/Yes-Sessions-5.6.0-x64.dmg
   ```
3. 双击 DMG 安装

---

## 新版本特性

### 新增功能

- Toast 通知系统：所有设置变更都有视觉反馈
- Thinking 内容开关：可选择在会话详情中显示/隐藏 AI 思考过程
- 默认应用选择：设置启动时默认展示的 AI 应用
- 界面优化：设置页面布局改进，颜色选择器更简洁

### 改进

- 优化 Toast 样式，支持多种级别（成功/信息/警告/错误）
- 默认应用列表显示不支持的应用并标记"即将推出"
- 国际化支持增强

---

## 系统要求

- **macOS**: macOS 13.0 (Ventura) 或更高版本

---

## 关于 macOS "文件已损坏" 提示

由于应用未经过 Apple 官方签名，macOS 会显示"文件已损坏"。这是因为：

1. 从网络下载的文件会被 macOS 标记为 `com.apple.quarantine`
2. 未签名的应用会被 Gatekeeper 拦截

**解决方法**：安装前运行 `xattr -c` 命令移除安全标记（详见上方安装说明）

**长期方案**: 考虑申请 Apple Developer 账号进行官方签名

---

## 文件校验

| 文件名                       | SHA256 |
| ---------------------------- | ------ |
| Yes-Sessions-5.6.0-arm64.dmg | 待补充 |
| Yes-Sessions-5.6.0-x64.dmg   | 待补充 |

---

## 已知问题

- macOS 首次安装需要移除 quarantine 属性（见安装说明）
- 深色/浅色主题切换需要重启应用才能完全生效

---

## 相关链接

- 完整文档: https://github.com/KrabsWong/yes-sessions#readme
- 问题反馈: https://github.com/KrabsWong/yes-sessions/issues
- 功能建议: https://github.com/KrabsWong/yes-sessions/discussions
