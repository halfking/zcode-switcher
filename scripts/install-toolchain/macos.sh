#!/usr/bin/env bash
# scripts/install-toolchain/macos.sh —— zcode-switcher macOS 工具链安装/状态脚本
#
# 用途：在一台裸 macOS 上装好构建 zcode-switcher 所需的所有工具。
#   - Homebrew（必需；macOS 上几乎所有工具都靠它）
#   - Node.js 20 LTS（brew formula）
#   - Rust 1.98 stable（rustup-init，brew 不直接装 toolchain）
#   - Xcode Command Line Tools（tauri-osascript / codesign / xcrun notarytool）
#
# 不装的东西：
#   - NSIS：macOS 打 DMG，不依赖 NSIS
#   - Visual Studio Build Tools：只 Windows 需要
#
# 用法：
#   bash scripts/install-toolchain/macos.sh             # 检查 + 缺啥装啥
#   bash scripts/install-toolchain/macos.sh --status    # 只检查，exit 0/1
#   bash scripts/install-toolchain/macos.sh --self-contained  # 跳过 Xcode CLT 安装提示
#   bash scripts/install-toolchain/macos.sh --arch=apple-silicon  # 强制按 aarch64 走（默认自动探测）
#
# 任意阶段失败立即退出。输出明确写出每一步结果。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# ---- 解析参数 ---------------------------------------------------------------

STATUS=0
SELF_CONTAINED=0
FORCE_ARCH=""
for arg in "$@"; do
  case "$arg" in
    --status) STATUS=1 ;;
    --self-contained) SELF_CONTAINED=1 ;;
    --arch=*) FORCE_ARCH="${arg#--arch=}" ;;
    -h|--help)
      sed -n '2,18p' "$0"
      exit 0
      ;;
    *) echo "[FATAL] 未知参数：$arg" >&2; exit 2 ;;
  esac
done

# ---- 工具函数 ----------------------------------------------------------------

log()  { printf '%s\n' "$*"; }
hdr()  { printf '\n=== %s ===\n' "$*"; }
err()  { printf '[FATAL] %s\n' "$*" >&2; }
warn() { printf '[WARN] %s\n' "$*" >&2; }
ok()   { printf '[OK] %s\n' "$*"; }
need_root_for_brew() {
  # brew 在 Apple Silicon 上不需要 sudo（装到 /opt/homebrew），Intel 上也是
  # /usr/local。除非用户改过路径，否则全程不要求 sudo。
  if [ -d "/opt/homebrew" ] || [ -d "/usr/local/Homebrew" ]; then
    return 0
  fi
  return 1
}

detect_arch() {
  if [ -n "$FORCE_ARCH" ]; then
    echo "$FORCE_ARCH"
    return
  fi
  local m
  m="$(uname -m)"
  case "$m" in
    arm64|aarch64) echo "apple-silicon" ;;
    x86_64)        echo "intel" ;;
    *)             echo "unknown" ;;
  esac
}

# ---- 状态表 ------------------------------------------------------------------

emit_status() {
  local brew node rustc cargo xcode_status
  brew="$(command -v brew || true)"
  if [ -n "$brew" ]; then
    ok "brew：$brew"
  else
    warn "brew：未安装"
  fi

  node="$(command -v node || true)"
  if [ -n "$node" ]; then
    ok "node：$(${node##*/} --version)  at $node"
  else
    warn "node：未安装"
  fi

  rustc="$(command -v rustc || true)"
  if [ -n "$rustc" ]; then
    local ver active
    ver="$($rustc --version | awk '{print $2}')"
    active="$(rustup show active-toolchain 2>/dev/null || echo "system")"
    ok "rustc：$ver  ($active)"
  else
    warn "rustc：未安装"
  fi

  xcode_status="$(xcode-select -p 2>/dev/null || true)"
  if [ -n "$xcode_status" ] && [ -d "$xcode_status" ]; then
    ok "xcode-clt：$xcode_status"
  else
    warn "xcode-clt：未安装（tauri build / codesign / notarytool 需要）"
  fi
}

# ---- 安装步骤 ----------------------------------------------------------------

install_xcode_clt() {
  if [ "$SELF_CONTAINED" -eq 1 ]; then
    warn "跳过 Xcode CLT 安装（--self-contained）。"
    return
  fi
  if xcode-select -p >/dev/null 2>&1; then
    ok "Xcode CLT 已安装：$(xcode-select -p)"
    return
  fi
  warn "Xcode Command Line Tools 未装；弹出系统对话框后请点「安装」。"
  xcode-select --install
  # 等用户装完
  until xcode-select -p >/dev/null 2>&1; do
    sleep 2
  done
  ok "Xcode CLT 已就绪：$(xcode-select -p)"
}

install_homebrew() {
  if command -v brew >/dev/null 2>&1; then
    ok "Homebrew 已装：$(brew --version | head -1)"
    return
  fi
  warn "Homebrew 未装；开始安装（需要联网，会提示输入密码）。"
  /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
  # 把 brew 加入当前 shell PATH（Apple Silicon 装到 /opt/homebrew）
  if [ -f /opt/homebrew/bin/brew ]; then
    eval "$(/opt/homebrew/bin/brew shellenv)"
  elif [ -f /usr/local/bin/brew ]; then
    eval "$(/usr/local/bin/brew shellenv)"
  fi
  ok "Homebrew 安装完成：$(brew --version | head -1)"
}

install_node() {
  if command -v node >/dev/null 2>&1; then
    ok "node 已装：$(node --version)"
    return
  fi
  brew install node@20
  brew link --force --overwrite node@20 || true
  ok "node 安装完成：$(node --version)"
}

install_rust() {
  if command -v rustc >/dev/null 2>&1; then
    ok "rustc 已装：$(rustc --version)"
    return
  fi
  warn "rust 未装；运行 rustup-init（装到 ~/.cargo/）。"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --no-modify-path
  # 让当前 shell 找得到 cargo
  # shellcheck source=/dev/null
  source "$HOME/.cargo/env" 2>/dev/null || true
  ok "rust 安装完成：$(rustc --version)"
}

# ---- 主流程 ------------------------------------------------------------------

hdr "zcode-switcher macOS 工具链检查 ($(detect_arch))"
emit_status

if [ "$STATUS" -eq 1 ]; then
  # 只检查：缺任何关键工具退出 1
  if ! command -v brew >/dev/null 2>&1 \
     || ! command -v node >/dev/null 2>&1 \
     || ! command -v rustc >/dev/null 2>&1 \
     || ! xcode-select -p >/dev/null 2>&1; then
    err "关键工具缺失；运行 bash scripts/install-toolchain/macos.sh 安装"
    exit 1
  fi
  ok "全部就绪。"
  exit 0
fi

hdr "安装（幂等：已装跳过）"
install_xcode_clt
install_homebrew
install_node
install_rust

hdr "安装完成"
emit_status

cat <<'NEXT'

下一步：
  1. 重启 shell（让 brew shellenv 与 cargo env 生效）
  2. npm install
  3. npm run tauri build -- --target aarch64-apple-darwin --bundles dmg
     详见 docs/macos.md。

NEXT
