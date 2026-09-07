# 一键安装脚本

提供 curl 安装脚本，自动下载、安装并移除 macOS quarantine 标记。

## macOS 快速安装

### 方式一：使用 curl（推荐）

**安装最新版本（自动检测）：**

```bash
curl -fsSL https://raw.githubusercontent.com/KrabsWong/yes-sessions/main/scripts/install.sh | bash
```

**安装指定版本：**

```bash
curl -fsSL https://raw.githubusercontent.com/KrabsWong/yes-sessions/main/scripts/install.sh | bash -s -- -v 10.0.0
```

**通过环境变量指定版本：**

```bash
YS_VERSION=10.0.0 curl -fsSL https://raw.githubusercontent.com/KrabsWong/yes-sessions/main/scripts/install.sh | bash
```

### 方式二：先下载再运行

```bash
# 下载脚本
curl -fsSL -o install.sh https://raw.githubusercontent.com/KrabsWong/yes-sessions/main/scripts/install.sh

# 查看脚本内容（可选）
cat install.sh

# 运行（安装最新版）
bash install.sh

# 或安装指定版本
bash install.sh -v 10.0.0
```

---

## 脚本功能

✅ **自动检测架构** - 自动识别 Apple Silicon 或 Intel  
✅ **自动下载** - 从 GitHub Releases 下载最新版本  
✅ **自动挂载** - 挂载 DMG 文件  
✅ **自动安装** - 复制到 /Applications  
✅ **自动清理** - 移除 quarantine 标记，无需手动 `xattr -c`  
✅ **自动卸载旧版本** - 检测到旧版本时提示是否替换  
✅ **自动清理** - 删除临时文件和 DMG  
✅ **可选启动** - 安装完成后可选择立即打开应用

---

## 脚本行为说明

### 安装流程

```
1. 检测操作系统 (仅支持 macOS)
2. 检测系统架构 (arm64/x86_64)
3. 获取版本号：
   - 优先使用用户指定的版本（命令行参数或环境变量）
   - 否则自动从 GitHub API 获取最新版本
4. 检查是否已安装旧版本
5. 从 GitHub 下载对应架构的 DMG
6. 挂载 DMG
7. 复制 .app 到 /Applications
8. 执行 xattr -cr 移除 quarantine
9. 卸载 DMG
10. 清理临时文件
11. 完成！
```

### 需要用户交互的地方

- **替换旧版本**：如果检测到已安装，会询问是否替换
- **立即启动**：安装完成后询问是否立即打开应用

其他步骤全自动，无需干预。

---

## 与 Homebrew 对比

| 特性                | 安装脚本          | Homebrew Cask       |
| ------------------- | ----------------- | ------------------- |
| 一键安装            | ✅                | ✅                  |
| 自动获取最新版本    | ✅                | ✅                  |
| 安装指定版本        | ✅ `-v 10.0.0`    | ❌                  |
| 自动移除 quarantine | ✅                | ✅                  |
| 自动更新            | ❌                | ✅ `brew upgrade`   |
| 卸载管理            | ❌                | ✅ `brew uninstall` |
| 依赖检查            | ❌                | ✅                  |
| popularity          | 任意用户          | 需要 GitHub stars   |
| 维护难度            | ✅ 无需维护版本号 | 中等                |

**建议**：

- **普通用户**：提供安装脚本（最简单）
- **开发者用户**：同时提供 Homebrew（更专业）

---

## 版本选择

脚本支持三种方式指定版本：

### 1. 自动获取最新版本（默认）
```bash
curl -fsSL https://raw.githubusercontent.com/KrabsWong/yes-sessions/main/scripts/install.sh | bash
```
脚本会自动调用 GitHub API 获取最新 release 版本号。

### 2. 命令行参数指定版本
```bash
curl -fsSL https://raw.githubusercontent.com/KrabsWong/yes-sessions/main/scripts/install.sh | bash -s -- -v 10.0.0
```

### 3. 环境变量指定版本
```bash
YS_VERSION=10.0.0 curl -fsSL https://raw.githubusercontent.com/KrabsWong/yes-sessions/main/scripts/install.sh | bash
```

### 优先级
命令行参数 > 环境变量 > 自动获取最新版本

---

## 安全提示

⚠️ **关于 curl | bash 的安全性**

运行 `curl | bash` 之前，建议先查看脚本内容：

```bash
# 下载并查看
curl -fsSL -o install.sh https://raw.githubusercontent.com/KrabsWong/yes-sessions/main/scripts/install.sh
cat install.sh

# 确认安全后再运行
bash install.sh
```

本脚本仅执行以下操作：

1. 下载 GitHub Release 的 DMG 文件
2. 挂载/复制/卸载 DMG
3. 移除 macOS quarantine 属性
4. 清理临时文件

**不会**：上传数据、修改系统配置、安装其他软件。

---

## 故障排除

### 问题：curl 命令无响应

**解决**：检查网络连接，或尝试使用代理：

```bash
curl -fsSL -x http://proxy:port https://raw.githubusercontent.com/... | bash
```

### 问题：提示 "Permission denied"

**解决**：确保对 /Applications 有写入权限：

```bash
sudo bash install.sh
```

### 问题：下载速度慢

**解决**：脚本使用 GitHub 官方链接，如果慢可以：

1. 手动从 [Releases](https://github.com/KrabsWong/yes-sessions/releases) 下载 DMG
2. 双击挂载 DMG
3. 拖动应用到 Applications 文件夹
4. 运行 `xattr -cr "/Applications/Yes Sessions.app"`

---

## 版本历史

- **v1.0** - 初始版本，支持自动检测架构、下载、安装、移除 quarantine
````
