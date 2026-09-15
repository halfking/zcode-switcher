#!/usr/bin/env bash
# scripts/run-tests.sh —— GNU 工具链下运行 cargo 单元测试的辅助脚本。
#
# 背景（INSTALL.md「故障排查」同款问题）：主 exe 的 comctl32 v6 依赖由
# tauri-winres 在构建 bin 目标时嵌入 manifest；cargo test 的测试 exe 不经过
# 那条路，Windows 会加载 comctl32 5.82，缺 tao 引用的 TaskDialogIndirect
# 导出，进程启动即崩（0xc0000139 STATUS_ENTRYPOINT_NOT_FOUND）。
#
# 解法：Windows SxS 在 exe 没有内嵌 manifest 时会自动查找旁边的
# `<exe名>.manifest`。本脚本先 `cargo test --no-run` 编译，再把带 comctl32 v6
# 依赖的 manifest 放到每个测试 exe 旁边，然后直接运行测试。
#
# 用法：
#   source scripts/dev-env.sh
#   scripts/run-tests.sh                    # 全部单元测试
#   scripts/run-tests.sh quota::            # 按过滤条件运行
#   scripts/run-tests.sh quota:: --nocapture
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT/src-tauri"

# 1. 编译测试（不运行）
cargo test --lib --no-run "$@" >/dev/null

# 2. 找出最新的测试 exe，旁边放 manifest
MANIFEST="$ROOT/scripts/testexe.manifest"
INSTANTIATED=$(ls -t target/debug/deps/zcode_switcher_lib-*.exe 2>/dev/null | head -1 || true)
if [ -z "$INSTANTIATED" ]; then
  echo "[run-tests] 未找到测试可执行文件" >&2
  exit 1
fi
cp "$MANIFEST" "$INSTANTIATED.manifest"

# 3. 直接运行测试 exe（harness 参数原样透传）
echo "[run-tests] 运行 $INSTANTIATED $*"
"$INSTANTIATED" "$@"
