#!/bin/bash

# Yes Sessions 安装脚本
# 一键下载并安装经过签名和公证的发布包

set -euo pipefail

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# 检查操作系统
if [[ "$OSTYPE" != "darwin"* ]]; then
    echo -e "${RED}========================================${NC}"
    echo -e "${RED}  ❌ 不支持的操作系统${NC}"
    echo -e "${RED}========================================${NC}"
    echo ""
    echo -e "${YELLOW}Yes Sessions 目前仅支持 macOS。${NC}"
    echo ""
    echo "检测到您的操作系统: $OSTYPE"
    echo ""
    echo "当前版本不提供其他平台支持。"
    echo ""
    echo -e "${BLUE}访问 GitHub 获取更多信息:${NC}"
    echo "https://github.com/KrabsWong/yes-sessions"
    exit 1
fi

# 配置
REPO="KrabsWong/yes-sessions"
APP_NAME="Yes-Sessions"
APP_BUNDLE_NAME="Yes Sessions.app"
INSTALL_DIR="/Applications"

# 获取最新版本号
get_latest_version() {
    local version
    # 尝试从 GitHub API 获取最新 release 版本
    if command -v curl &> /dev/null; then
        version=$(curl -s "https://api.github.com/repos/${REPO}/releases/latest" | grep -o '"tag_name": "[^"]*"' | grep -o '[0-9]\+\.[0-9]\+\.[0-9]\+' | head -1)
    elif command -v wget &> /dev/null; then
        version=$(wget -qO- "https://api.github.com/repos/${REPO}/releases/latest" | grep -o '"tag_name": "[^"]*"' | grep -o '[0-9]\+\.[0-9]\+\.[0-9]\+' | head -1)
    fi
    
    if [ -z "$version" ]; then
        echo ""
        return 1
    fi
    
    echo "$version"
}

# 解析命令行参数
VERSION=""
while [[ $# -gt 0 ]]; do
    case $1 in
        -v|--version)
            VERSION="$2"
            shift 2
            ;;
        -h|--help)
            echo "Yes Sessions 安装脚本"
            echo ""
            echo "用法: $0 [选项]"
            echo ""
            echo "选项:"
            echo "  -v, --version <版本>    指定安装版本 (例如: 10.0.0)"
            echo "  -h, --help              显示此帮助信息"
            echo ""
            echo "示例:"
            echo "  $0                      # 安装最新版本"
            echo "  $0 -v 10.0.0            # 安装指定版本"
            echo ""
            echo "环境变量:"
            echo "  YS_VERSION              指定版本号 (优先级低于命令行参数)"
            echo ""
            exit 0
            ;;
        *)
            echo "未知选项: $1"
            echo "使用 '$0 --help' 查看帮助"
            exit 1
            ;;
    esac
done

# 优先使用命令行参数，其次是环境变量，最后自动获取最新版本
if [ -z "$VERSION" ]; then
    if [ -n "${YS_VERSION:-}" ]; then
        VERSION="$YS_VERSION"
        echo -e "${BLUE}📌 使用环境变量指定的版本: ${VERSION}${NC}"
    else
        echo -e "${BLUE}🔍 正在获取最新版本...${NC}"
        VERSION=$(get_latest_version)
        if [ -z "$VERSION" ]; then
            echo -e "${RED}❌ 无法获取最新版本号${NC}"
            echo ""
            echo "可能的原因："
            echo "  1. 网络连接问题"
            echo "  2. GitHub API 限制"
            echo ""
            echo "解决方案："
            echo "  1. 手动指定版本安装:"
            echo "     curl -fsSL ... | bash -s -- -v 10.0.0"
            echo ""
            echo "  2. 设置环境变量:"
            echo "     YS_VERSION=10.0.0 curl -fsSL ... | bash"
            echo ""
            exit 1
        fi
        echo -e "${GREEN}✓ 最新版本: ${VERSION}${NC}"
    fi
fi

# 检测架构
ARCH=$(uname -m)
if [ "$ARCH" = "arm64" ]; then
    DMG_FILE="${APP_NAME}-${VERSION}-arm64.dmg"
    echo -e "${BLUE}检测到 Apple Silicon (M1/M2/M3/M4) 架构${NC}"
else
    echo -e "${RED}当前原生版本仅支持 Apple Silicon，检测到架构: $ARCH${NC}"
    exit 1
fi

# 下载 URL
DOWNLOAD_URL="https://github.com/${REPO}/releases/download/v${VERSION}/${DMG_FILE}"
TEMP_DIR=$(mktemp -d)
DMG_PATH="${TEMP_DIR}/${DMG_FILE}"
MOUNT_POINT=""

cleanup() {
    if [ -n "$MOUNT_POINT" ] && [ -d "$MOUNT_POINT" ]; then
        hdiutil detach "$MOUNT_POINT" -quiet 2>/dev/null || true
    fi
    if [ -n "$TEMP_DIR" ] && [ -d "$TEMP_DIR" ]; then
        rm -rf "$TEMP_DIR"
    fi
}
trap cleanup EXIT

echo -e "${BLUE}========================================${NC}"
echo -e "${BLUE}  正在安装 Yes Sessions v${VERSION}${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""

# 检查是否已安装。旧 Electron 版本使用连字符 bundle 名，原生版本使用空格。
INSTALLED_BUNDLES=()
for installed_bundle in "${INSTALL_DIR}/${APP_BUNDLE_NAME}" "${INSTALL_DIR}/${APP_NAME}.app"; do
    if [ -d "$installed_bundle" ]; then
        INSTALLED_BUNDLES+=("$installed_bundle")
    fi
done

if [ "${#INSTALLED_BUNDLES[@]}" -gt 0 ]; then
    echo -e "${YELLOW}⚠️  检测到已安装的旧版本${NC}"
    read -p "是否先卸载旧版本? (y/n) " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        echo -e "${BLUE}正在卸载旧版本...${NC}"
        for installed_bundle in "${INSTALLED_BUNDLES[@]}"; do
            rm -rf "$installed_bundle"
        done
        echo -e "${GREEN}✓ 旧版本已卸载${NC}"
    fi
    echo ""
fi

# 下载
echo -e "${BLUE}📥 正在下载...${NC}"
echo "URL: ${DOWNLOAD_URL}"

if command -v curl &> /dev/null; then
    curl -fL --progress-bar -o "${DMG_PATH}" "${DOWNLOAD_URL}"
elif command -v wget &> /dev/null; then
    wget --progress=bar:force -O "${DMG_PATH}" "${DOWNLOAD_URL}"
else
    echo -e "${RED}错误: 需要 curl 或 wget${NC}"
    exit 1
fi

if [ ! -f "${DMG_PATH}" ]; then
    echo -e "${RED}错误: 下载失败${NC}"
    exit 1
fi

echo -e "${GREEN}✓ 下载完成${NC}"
echo ""

hdiutil verify "${DMG_PATH}" >/dev/null

# 计算文件大小
FILE_SIZE=$(du -h "${DMG_PATH}" | cut -f1)
echo -e "${BLUE}📦 文件大小: ${FILE_SIZE}${NC}"
echo ""

# 挂载 DMG
echo -e "${BLUE}📂 正在挂载 DMG...${NC}"
MOUNT_POINT="${TEMP_DIR}/mounted"
mkdir -p "$MOUNT_POINT"
hdiutil attach "${DMG_PATH}" -nobrowse -quiet -mountpoint "$MOUNT_POINT"

if [ ! -d "$MOUNT_POINT" ]; then
    echo -e "${RED}错误: 无法挂载 DMG${NC}"
    exit 1
fi

echo -e "${GREEN}✓ 已挂载到: ${MOUNT_POINT}${NC}"
echo ""

# 查找 DMG 根目录中的 App。使用 shell glob，避免依赖 macOS BSD find
# 不支持的 GNU `-maxdepth` 参数。
APP_PATH=""
for candidate in "$MOUNT_POINT"/*.app; do
    if [ -d "$candidate" ]; then
        APP_PATH="$candidate"
        break
    fi
done

if [ -z "$APP_PATH" ]; then
    echo -e "${RED}错误: 在 DMG 中未找到应用${NC}"
    hdiutil detach "$MOUNT_POINT" -quiet 2>/dev/null || true
    exit 1
fi

APP_BASENAME=$(basename "$APP_PATH")
echo -e "${BLUE}📝 找到应用: ${APP_BASENAME}${NC}"
echo ""

codesign --verify --deep --strict --verbose=2 "$APP_PATH"
spctl --assess --type execute --verbose=2 "$APP_PATH"

# 复制到 Applications
echo -e "${BLUE}📋 正在安装到 ${INSTALL_DIR}...${NC}"
cp -R "${APP_PATH}" "${INSTALL_DIR}/"

if [ ! -d "${INSTALL_DIR}/${APP_BASENAME}" ]; then
    echo -e "${RED}错误: 安装失败${NC}"
    hdiutil detach "$MOUNT_POINT" -quiet 2>/dev/null || true
    exit 1
fi

echo -e "${GREEN}✓ 应用已复制${NC}"
echo ""

# 卸载 DMG
echo -e "${BLUE}📤 正在卸载 DMG...${NC}"
hdiutil detach "$MOUNT_POINT" -quiet
MOUNT_POINT=""
echo -e "${GREEN}✓ DMG 已卸载${NC}"
echo ""

# 清理临时文件
echo -e "${BLUE}🧹 正在清理临时文件...${NC}"
rm -rf "$TEMP_DIR"
TEMP_DIR=""
echo -e "${GREEN}✓ 清理完成${NC}"
echo ""

# 安装完成
echo -e "${GREEN}========================================${NC}"
echo -e "${GREEN}  ✅ 安装成功!${NC}"
echo -e "${GREEN}========================================${NC}"
echo ""
echo -e "${BLUE}应用已安装到: ${INSTALL_DIR}/${APP_BASENAME}${NC}"
echo ""
echo -e "${YELLOW}你可以:${NC}"
echo -e "  1. 在 Launchpad 中找到 Yes Sessions"
echo -e "  2. 在 Applications 文件夹中双击打开"
echo -e "  3. 使用 Spotlight (Cmd+Space) 搜索 'Yes Sessions'"
echo ""
echo -e "${BLUE}首次启动可能需要几秒钟加载数据库...${NC}"
echo ""

# 询问是否立即打开
read -p "是否立即打开应用? (y/n) " -n 1 -r
echo
if [[ $REPLY =~ ^[Yy]$ ]]; then
    open "${INSTALL_DIR}/${APP_BASENAME}"
    echo -e "${GREEN}🚀 正在启动 Yes Sessions...${NC}"
fi

echo ""
echo -e "${GREEN}感谢使用 Yes Sessions!${NC}"
echo -e "${BLUE}GitHub: https://github.com/${REPO}${NC}"
