# zcode-switcher 本地构建工具链激活脚本
#
# 用法（在 Git Bash / WSL / MSYS 风格 shell 里）：
#   source scripts/dev-env.sh
#
# 之后即可直接使用 node / npm / rustc / cargo / gcc / tauri。
#
# 工具链位置：
#   Node.js 20.19.5 LTS   -> .tools/node-v20.19.5-win-x64
#   Rust 1.98.1 (GNU)     -> ~/.cargo (rustup 默认路径)
#   MinGW-w64 14.2.0 GCC  -> .tools/mingw64

# 解析脚本所在目录（无论从哪里 source 都正确）
_DEV_ENV_DIR="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")/.." && pwd)"

# 工具链目录（脚本已经在 MSYS 环境里，路径全部使用 unix 风格）
_NODE_HOME="${_DEV_ENV_DIR}/.tools/node-v20.19.5-win-x64"
_MINGW_HOME="${_DEV_ENV_DIR}/.tools/mingw64/bin"

# Rust 默认装在 $HOME/.cargo/bin
_CARGO_BIN="${HOME}/.cargo/bin"
if [ ! -d "$_CARGO_BIN" ]; then
  _CARGO_BIN="$(cygpath -u "$USERPROFILE")/.cargo/bin"
fi

export PATH="${_NODE_HOME}:${_CARGO_BIN}:${_MINGW_HOME}:${PATH}"

# 让 node / npm 找得到全局模块
export NODE_HOME="${_NODE_HOME}"

# 抑制 Git Bash 把空格 / 冒号当路径转换，避免 mingw gcc 调用时被误解析
export MSYS_NO_PATHCONV=1

echo "[dev-env] 已激活本地构建工具链"
echo "[dev-env] Node $(node --version)  Rust $(rustc --version | awk '{print $2}')"
echo "[dev-env] cargo $(cargo --version | awk '{print $2}')  gcc $(gcc --version | head -1 | awk '{print $NF}')"
echo "[dev-env] PATH 已加入："
echo "[dev-env]   ${_NODE_HOME}"
echo "[dev-env]   ${_CARGO_BIN}"
echo "[dev-env]   ${_MINGW_HOME}"
