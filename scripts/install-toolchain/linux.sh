#!/usr/bin/env bash
# scripts/install-toolchain/linux.sh —— zcode-switcher Linux 工具链安装/状态脚本
#
# 用途：在一台裸 Linux 上装好构建 zcode-switcher 所需的工具链。
# 自动检测发行版（apt / dnf / pacman），按各自源装依赖。
#
# ⚠ Tauri 2 在 Linux 上无法打 NSIS / DMG 安装包，本脚本只保证能跑：
#   - npm run tauri dev
#   - cargo run / cargo test
#   - 产裸 release 二进制（src-tauri/target/release/zcode-switcher）
# 想要发行包请走 Windows / macOS 的 CI。
#
# 用法：
#   bash scripts/install-toolchain/linux.sh
#   bash scripts/install-toolchain/linux.sh --status        # 只检查
#   bash scripts/install-toolchain/linux.sh --self-contained  # 只装到 ~/.local 与 ~/.cargo，不动 /usr
#   bash scripts/install-toolchain/linux.sh --skip-system    # 完全不调包管理器（用于容器/CI 装好基础镜像后只装 rust/node）
#
# 任意阶段失败立即退出。输出明确写出每一步结果。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

STATUS=0
SELF_CONTAINED=0
SKIP_SYSTEM=0
for arg in "$@"; do
  case "$arg" in
    --status)        STATUS=1 ;;
    --self-contained) SELF_CONTAINED=1 ;;
    --skip-system)   SKIP_SYSTEM=1 ;;
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
require_root() {
  if [ "$(id -u)" -ne 0 ]; then
    err "需要 root 权限调用包管理器；用 sudo 重跑，或加 --skip-system"
    exit 1
  fi
}

detect_distro() {
  if [ -f /etc/os-release ]; then
    . /etc/os-release
    case "${ID:-}" in
      ubuntu|debian|pop|linuxmint|elementary) echo "debian" ;;
      fedora|rhel|centos|rocky|almalinux|nobara) echo "fedora" ;;
      arch|manjaro|endeavouros) echo "arch" ;;
      *) echo "unknown" ;;
    esac
  else
    echo "unknown"
  fi
}

# ---- 状态表 ------------------------------------------------------------------

emit_status() {
  local distro node rustc cargo
  distro="$(detect_distro)"
  log "发行版：$distro"
  for tool in node npm rustc cargo pkg-config; do
    if command -v "$tool" >/dev/null 2>&1; then
      local v
      v="$("$tool" --version 2>/dev/null | head -1 || echo '?')"
      ok "$tool：$v"
    else
      warn "$tool：未安装"
    fi
  done
  # webkit2gtk 是 tauri 2 在 linux 上的硬依赖
  if pkg-config --exists webkit2gtk-4.1 2>/dev/null; then
    ok "webkit2gtk-4.1：$(pkg-config --modversion webkit2gtk-4.1)"
  else
    warn "webkit2gtk-4.1：未安装（tauri 编译会失败）"
  fi
}

# ---- 安装系统包 -------------------------------------------------------------

install_system_deps() {
  if [ "$SKIP_SYSTEM" -eq 1 ] || [ "$SELF_CONTAINED" -eq 1 ]; then
    warn "跳过系统包安装（$( [ "$SKIP_SYSTEM" -eq 1 ] && echo --skip-system || echo --self-contained )）"
    return
  fi
  require_root
  local distro
  distro="$(detect_distro)"
  case "$distro" in
    debian)
      apt-get update
      # 工具链本体
      apt-get install -y --no-install-recommends \
        build-essential curl wget file pkg-config \
        libssl-dev libxdo-dev \
        libwebkit2gtk-4.1-dev \
        libayatana-appindicator3-dev librsvg2-dev
      ;;
    fedora)
      dnf install -y \
        gcc gcc-c++ make curl wget file pkgconf-pkg-config \
        openssl-devel libxdo-devel \
        webkit2gtk4.1-devel \
        libappindicator-gtk3-devel librsvg2-devel
      ;;
    arch)
      pacman -Sy --needed --noconfirm \
        base-devel curl wget file pkgconf \
        openssl xdotool \
        webkit2gtk-4.1 \
        libappindicator-gtk3 librsvg
      ;;
    *)
      err "未识别发行版；Tauri Linux 依赖见 https://v2.tauri.app/start/prerequisites/"
      err "（至少需要 webkit2gtk-4.1 + libayatana-appindicator3 + librsvg2）"
      exit 1
      ;;
  esac
  ok "系统包已就位"
}

# ---- node / rust -------------------------------------------------------------

install_node() {
  if command -v node >/dev/null 2>&1; then
    ok "node 已装：$(node --version)"
    return
  fi
  if [ "$SKIP_SYSTEM" -eq 1 ]; then
    # 容器场景：假设 node 已通过其他方式装好（multi-stage build 之类）
    err "找不到 node；先装 node 20 LTS 再重跑"
    exit 1
  fi
  # 用 nvm（per-user，不污染 /usr）；也可换 nvs / nodesource
  warn "用 nvm 装 node 20 LTS（装到 ~/.nvm）"
  curl -fsSL https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh | bash
  # 让当前 shell 找得到
  # shellcheck source=/dev/null
  \. "$HOME/.nvm/nvm.sh"
  nvm install 20
  nvm use 20
  ok "node 安装完成：$(node --version)"
}

install_rust() {
  if command -v rustc >/dev/null 2>&1; then
    ok "rustc 已装：$(rustc --version)"
    return
  fi
  warn "用 rustup 装 stable（装到 ~/.cargo）"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --no-modify-path
  # shellcheck source=/dev/null
  . "$HOME/.cargo/env"
  ok "rust 安装完成：$(rustc --version)"
}

# ---- 主流程 ------------------------------------------------------------------

hdr "zcode-switcher Linux 工具链检查"
emit_status

if [ "$STATUS" -eq 1 ]; then
  if ! command -v node >/dev/null 2>&1 \
     || ! command -v rustc >/dev/null 2>&1 \
     || ! pkg-config --exists webkit2gtk-4.1 2>/dev/null; then
    err "关键工具缺失；运行 bash scripts/install-toolchain/linux.sh 安装"
    exit 1
  fi
  ok "全部就绪。"
  exit 0
fi

hdr "安装（幂等：已装跳过）"
install_system_deps
install_node
install_rust

hdr "安装完成"
emit_status

cat <<'NEXT'

下一步：
  1. 重启 shell（让 cargo env / nvm 生效）
  2. npm install
  3. npm run tauri dev          # 调试模式（产裸二进制到 target/debug）
  4. npm run tauri build        # 产裸 release 二进制（无安装包）

NEXT

if [ "$(detect_distro)" = "debian" ] || [ "$(detect_distro)" = "fedora" ] || [ "$(detect_distro)" = "arch" ]; then
  cat <<'NOTE'
注意：Tauri 2 在 Linux 上只能产裸二进制，没有 NSIS/DMG/DEB/RPM 安装包。
发行版安装包请走 Windows / macOS CI。
NOTE
fi
