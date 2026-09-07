# Homebrew Cask 提交指南

## 方案一：提交到官方 Homebrew Cask（推荐）

### 步骤

1. **Fork Homebrew Cask 仓库**

   ```bash
   # 访问 https://github.com/Homebrew/homebrew-cask
   # 点击 Fork 按钮
   ```

2. **克隆你的 Fork**

   ```bash
   git clone https://github.com/YOUR_USERNAME/homebrew-cask.git
   cd homebrew-cask
   ```

3. **创建 Cask 文件**

   ```bash
   # 使用 homebrew 提供的命令生成模板
   brew create --cask https://github.com/KrabsWong/homebrew-yes-sessions/releases/download/v10.0.0/Yes-Sessions-10.0.0-arm64.dmg

   # 或者手动创建
   mkdir -p Casks
   cp /path/to/your/yes-sessions.rb Casks/yes-sessions.rb
   ```

4. **计算 SHA256 校验值**

   ```bash
   # 下载文件并计算校验值
   curl -L -o yes-sessions.dmg https://github.com/KrabsWong/homebrew-yes-sessions/releases/download/v10.0.0/Yes-Sessions-10.0.0-arm64.dmg
   shasum -a 256 yes-sessions.dmg

   # 或者使用 brew 命令
   brew fetch --cask yes-sessions
   ```

5. **编辑 Cask 文件**（更新 SHA256）

   ```ruby
   cask "yes-sessions" do
     version "10.0.0"
     sha256 "YOUR_SHA256_HERE"  # 替换为实际计算的值

     url "https://github.com/KrabsWong/homebrew-yes-sessions/releases/download/v#{version}/Yes-Sessions-#{version}-arm64.dmg"
     name "Yes Sessions"
     desc "AI Session Manager - Browse and resume your AI conversations"
     homepage "https://github.com/KrabsWong/yes-sessions"

     livecheck do
       url :url
       strategy :github_latest
     end

     app "Yes Sessions.app"

     depends_on macos: ">= :ventura"

     zap trash: [
       "~/Library/Application Support/yes-sessions",
       "~/Library/Preferences/com.yessessions.app.plist",
       "~/Library/Logs/yes-sessions",
       "~/Library/Saved Application State/com.yessessions.app.savedState",
     ]
   end
   ```

6. **测试 Cask**

   ```bash
   # 语法检查
   brew style --fix yes-sessions

   # 安装测试
   brew install --cask --verbose --debug ./Casks/yes-sessions.rb

   # 卸载测试
   brew uninstall --cask yes-sessions
   ```

7. **提交 PR**
   ```bash
   git add Casks/yes-sessions.rb
   git commit -m "Add Yes Sessions 10.0.0"
   git push origin main
   ```
   然后访问 GitHub 创建 Pull Request

---

## 方案二：创建个人 Tap（更快上线）

如果不想等待官方审核，可以创建自己的 Homebrew Tap。

### 步骤

1. **创建 Tap 仓库**
   - 在 GitHub 创建名为 `homebrew-yes-sessions` 的仓库
   - **注意**：命名必须是 `homebrew-` 开头

2. **创建 Cask 文件结构**

   ```
   homebrew-yes-sessions/
   ├── Casks/
   │   └── yes-sessions.rb
   └── README.md
   ```

3. **创建 Cask 文件**（见上方内容）

4. **用户安装方式**
   ```bash
   brew tap krabswong/yes-sessions
   brew install --cask yes-sessions
   ```

---

## 计算 SHA256 的自动化脚本

创建 `scripts/update-homebrew.sh`：

```bash
#!/bin/bash

VERSION="10.0.0"
REPO="KrabsWong/homebrew-yes-sessions"

# 计算 arm64 版本的 SHA256
curl -sL "https://github.com/${REPO}/releases/download/v${VERSION}/Yes-Sessions-${VERSION}-arm64.dmg" | shasum -a 256 | cut -d' ' -f1

# 计算 x64 版本的 SHA256
curl -sL "https://github.com/${REPO}/releases/download/v${VERSION}/Yes-Sessions-${VERSION}-x64.dmg" | shasum -a 256 | cut -d' ' -f1
```

运行：

```bash
chmod +x scripts/update-homebrew.sh
./scripts/update-homebrew.sh
```

---

## 常见问题

### Q: 为什么我的 Cask 被拒绝？

常见原因：

- 应用不够知名（GitHub stars < 30）
- 没有正确的签名
- 下载链接不稳定

### Q: 如何更新 Cask？

```bash
# 修改版本号和 SHA256
brew bump-cask-pr yes-sessions --version 8.2.2
```

### Q: 支持自动更新吗？

是的，可以在 Cask 中添加：

```ruby
auto_updates true
```

---

## 参考文档

- [Homebrew Cask 贡献指南](https://github.com/Homebrew/homebrew-cask/blob/master/CONTRIBUTING.md)
- [Cask 语言参考](https://docs.brew.sh/Cask-Cookbook)
- [常见 Cask 问题](https://docs.brew.sh/Common-Issues)
