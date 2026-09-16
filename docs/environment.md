# zcode-switcher 环境搭建与跨平台手册

> 本文档是 zcode-switcher 1.1.14+ 的**环境搭建与跨平台构建**总入口。
> 历史文档 `INSTALL.md`、`docs/development.md`、`docs/release.md` 在 1.1.14
> 合并到本文之后，**仍保留并加指针**，老链接不破。

## 0. 目录

1. [技术栈与目标平台](#1-技术栈与目标平台)
2. [工具链矩阵](#2-工具链矩阵)
3. [一键安装](#3-一键安装)
4. [激活本地工具链](#4-激活本地工具链)
5. [构建命令](#5-构建命令)
6. [跨平台差异与限制](#6-跨平台差异与限制)
7. [已知坑位](#7-已知坑位)
8. [故障排查速查](#8-故障排查速查)
9. [CI](#9-ci)
10. [历史文档归档](#10-历史文档归档)

---

## 1. 技术栈与目标平台

| 层 | 选型 | 版本 |
|---|---|---|
| 前端 | React + Vite + TypeScript | React 19 / Vite 7 / TS 5.8 |
| 桌面运行时 | Tauri | 2.x |
| 系统语言 | Rust | 1.98 stable |
| WebView (Windows) | WebView2 (webview2-com-sys) | runtime 由用户 OS 提供 |
| WebView (macOS) | WKWebView (system) | macOS 11+ |
| WebView (Linux) | webkit2gtk-4.1 | 由包管理器提供 |
| 安装包 (Windows) | NSIS | 3.11 |
| 安装包 (macOS) | DMG (Tauri 自带) | macOS 11+ |
| 安装包 (Linux) | **不打包** | 见 [§6](#6-跨平台差异与限制) |

**目标平台矩阵**：

| 平台 | 状态 | 主要 CI |
|---|---|---|
| Windows x64 (MSVC) | ✅ 一等支持 | `.github/workflows/release.yml` |
| Windows ARM64 (MSVC) | ✅ 一等支持 | `.github/workflows/release.yml` |
| Windows x64 (GNU) | ⚠️ legacy，部分场景需切 MSVC | — |
| macOS Apple Silicon | ✅ 一等支持 | `.github/workflows/build-macos.yml` |
| macOS Intel | ✅ 一等支持 | `.github/workflows/build-macos.yml` |
| Linux x64 | 🟡 裸二进制 only | `.github/workflows/build-linux.yml` |

---

## 2. 工具链矩阵

| 组件 | Windows | macOS | Linux |
|---|---|---|---|
| Node.js | 20.19.5 LTS (便携 zip, `.tools/`) | 20 LTS (`brew install node@20`) | 20 LTS (`nvm`) |
| npm | 10.8.2 (Node 自带) | 10.x (Node 自带) | 10.x (Node 自带) |
| Rust | 1.98 stable (rustup-init, `~/.cargo/`) | 同左 | 同左 |
| 默认 host | `x86_64-pc-windows-msvc` (推荐) | `aarch64-apple-darwin` / `x86_64-apple-darwin` | `x86_64-unknown-linux-gnu` |
| C/C++ toolchain | MSVC: Visual Studio Build Tools 2022；GNU: MinGW-w64 14 (UCRT) | Xcode Command Line Tools | gcc / clang (包管理器) |
| WebView | WebView2 Runtime (用户 OS) | WKWebView (system) | webkit2gtk-4.1 |
| 安装包工具 | NSIS 3.11 | DMG (Tauri 自带) | **无** |
| 额外系统库 | – | – | libssl / libxdo / libayatana-appindicator3 / librsvg2 |

**总占用**：

- Windows 全套（含 NSIS）：约 1.3 GB
- macOS：`brew install` 约 1.5 GB（含 CLT 已有部分）
- Linux：apt `build-deps` 约 600 MB + rust 约 400 MB

---

## 3. 一键安装

每个平台一个幂等脚本。已装的工具自动跳过，不污染全局 PATH。

### Windows（PowerShell 5.1+）

```powershell
# 默认：装 Node + MinGW + Rust + NSIS
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\install-toolchain\windows.ps1

# 只检查现状（CI 健康探针）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\install-toolchain\windows.ps1 -Status

# 完全不联网：只装到 .tools/ 与 ~/.cargo
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\install-toolchain\windows.ps1 -SelfContained

# 切 MSVC 工具链（推荐，比 GNU 少 0xc0000139 / libgcc_eh 两个坑）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\install-toolchain\windows.ps1 -ForceMsvc
```

### macOS

```bash
bash scripts/install-toolchain/macos.sh             # 装 Homebrew + Node + Rust + Xcode CLT
bash scripts/install-toolchain/macos.sh --status    # 只检查
bash scripts/install-toolchain/macos.sh --arch=apple-silicon  # 显式指定
```

### Linux（apt / dnf / pacman 自动检测）

```bash
bash scripts/install-toolchain/linux.sh
bash scripts/install-toolchain/linux.sh --status
bash scripts/install-toolchain/linux.sh --skip-system   # 不动 /usr（容器场景）
```

### 跨平台状态总览（纯 Node）

```bash
node scripts/install-toolchain/check.mjs           # 人类可读
node scripts/install-toolchain/check.mjs --json    # CI 消费
node scripts/install-toolchain/check.mjs --strict  # 任一缺失即 exit 1
npm run env:check                                   # 上面的 npm 脚本别名
```

---

## 4. 激活本地工具链

### Windows（Git Bash / MSYS）

`scripts/dev-env.sh` 把 node、cargo、gcc 加入 PATH，**不**修改注册表 / 系统 PATH：

```bash
source scripts/dev-env.sh
node --version        # v20.19.5（如果用了 .tools）
rustc --version       # rustc 1.98.1 ...
gcc --version         # gcc 14.2.0 ...
```

PowerShell 等价：直接调用完整路径（`.tools/node-v20.19.5-win-x64/node.exe`），
或 `Install-Toolchain` 不污染 PATH，由调用方临时 prepend。

### macOS / Linux

`rustup-init` 装到 `~/.cargo/`，新 shell 自动 source `~/.cargo/env`。
nvm 装到 `~/.nvm/`，新 shell 自动 source `~/.nvm/nvm.sh`。
brew 装到 `/opt/homebrew`（Apple Silicon）或 `/usr/local`（Intel），新 shell
自动 source `/opt/homebrew/bin/brew shellenv`（Apple Silicon）。

如果你的 shell 是 zsh：把上述 source 命令加到 `~/.zshrc`。
如果是 bash：`~/.bashrc`。
如果是 fish：`~/.config/fish/config.fish`（brew 给出的提示里有现成 snippet）。

---

## 5. 构建命令

| 用途 | 命令 | 产物 |
|---|---|---|
| 前端开发 | `npm run dev` | http://localhost:1420（无 Tauri 窗口） |
| Tauri 开发（前端 + 桌面窗口） | `npm run tauri dev` | 可交互窗口，热重载 |
| 仅前端构建 | `npm run build` | `dist/` |
| 完整 release（Windows） | `npm run tauri build` | `src-tauri/target/release/zcode-switcher.exe` + `bundle/nsis/*.exe` |
| 完整 release（macOS aarch64） | `npm run tauri build -- --target aarch64-apple-darwin --bundles dmg` | `bundle/dmg/*.dmg` |
| 完整 release（macOS x86_64） | `npm run tauri build -- --target x86_64-apple-darwin --bundles dmg` | `bundle/dmg/*.dmg` |
| 完整 release（Linux） | `npm run tauri build` | `src-tauri/target/release/zcode-switcher`（裸二进制） |
| Windows ARM64 | `npm run tauri build -- --target aarch64-pc-windows-msvc` | `bundle/nsis/*-arm64-setup.exe` |
| 单元测试 | `npm run test:settings` `npm run test:quotaguard` | node 端 |
| Rust 单元测试（Windows MSVC） | `cargo +stable-x86_64-pc-windows-msvc test --lib` | `target/debug/deps/*.exe` |
| Rust 单元测试（Windows GNU） | `scripts/run-tests.sh` | 同上，绕过 comctl32 v6 manifest 坑 |
| Rust 单元测试（macOS / Linux） | `cargo test --lib` | 标准 cargo 行为 |

`npm run build:captcha-runtime` 拷贝验证码求解器脚本到 `public/`，是
`npm run tauri build` 的隐式前置；CI 已经处理。

---

## 6. 跨平台差异与限制

| 项 | Windows | macOS | Linux |
|---|---|---|---|
| 默认 Rust target | `x86_64-pc-windows-msvc` | `aarch64-apple-darwin` (Apple Silicon) | `x86_64-unknown-linux-gnu` |
| WebView 依赖打包 | WebView2Loader.dll（dynamic-link 时） | WKWebView（系统自带） | webkit2gtk-4.1（包管理器装） |
| 安装包格式 | NSIS `.exe` | DMG | **无**（仅裸二进制） |
| 公证 / 签名 | NSIS 不需要；updater 走 tauri signing | **需要** Apple Developer ID + notarytool（见 `docs/macos.md`） | 不需要 |
| 自动更新 | `tauri-apps/tauri-action` 默认开 | 同左 | 不适用（无 Release 渠道） |
| 沙箱 / entitlements | 不需要 | `com.apple.security.cs.*` 见 `docs/macos.md` | AppArmor / SELinux 用户侧处理 |
| ZCode 快捷方式 | `zcode_launcher/windows.rs`（注册表 / .lnk） | `zcode_launcher/macos.rs`（`zcode-switcher-macos-launcher.json`） | 不支持 |
| Tray 行为 | `tray-icon` feature | Dock reopen → 还原窗口（commit `02f684c`） | X11 / Wayland 视 DE 而定 |

### macOS 限制

- **未签名的 DMG** 在 macOS 10.15+ 会被 Gatekeeper 拦截；CI 默认只产未签名
  DMG，要分发给真实用户必须走公证流程（`docs/macos.md`）。
- `aarch64-apple-darwin` 只能从 Apple Silicon Mac 编译；`x86_64-apple-darwin`
  同理。**不能交叉编译**（Apple 工具链限制）。

### Linux 限制

- **Tauri 2 不支持**在 Linux 上打 NSIS / DMG / DEB / RPM 安装包。当前仓库
  的 Linux CI 只产裸二进制 + 上传 artifact，**不上 GitHub Release**。
- Linux 桌面集成（tray-icon、autostart）依赖具体的桌面环境（GNOME / KDE
  / XFCE），本工具不针对某个 DE 做适配。
- WebKitGTK 在不同发行版上的 ABI 不保证一致（webkit2gtk-4.0 vs 4.1），
  本项目用 **4.1**（Ubuntu 22.04+ / Fedora 36+ / Arch current）。

### Windows 工具链选择

- **MSVC**（推荐）：Visual Studio Build Tools 2022 + Windows SDK。`cargo test`
  跑得稳，零额外手脚。链接器对 libgcc_eh 之类没有 w64devkit 那种坑。
- **GNU**（legacy / 免 VS Build Tools）：用 w64devkit 自带的 GCC。两个已知坑：
  1. **缺 libgcc_eh**：build script 链接阶段必失败。**测试**可用 MSVC
     解决，但**生产构建**如果必须走 GNU，需要补 `libgcc_eh.a`。
  2. **测试 exe 缺 comctl32 v6 manifest** → 启动即崩 0xc0000139。仓库里
     `scripts/run-tests.sh` 把 SxS manifest 放到 exe 旁边绕过。

---

## 7. 已知坑位

| 现象 | 原因 | 解决 |
|---|---|---|
| `linker not found` (`x86_64-w64-mingw32-gcc`) | 没 source dev-env.sh | `source scripts/dev-env.sh` |
| `cannot find -lgcc_eh` (GNU 工具链) | w64devkit 缺 libgcc_eh | 切 MSVC：`rustup default stable-x86_64-pc-windows-msvc`；或补 `libgcc_eh.a` |
| `cargo test` 启动崩溃 0xc0000139 (GNU 工具链) | 测试 exe 缺 comctl32 v6 manifest | `scripts/run-tests.sh`（已自动处理） |
| `npm install` 报 `lucide-react@1.18.0 notarget` | lockfile 锁了错误版本 | `rm package-lock.json && npm install` |
| `npm run build` 报 daisyui 报错 | daisyui 升到 v5 了 | 把 daisyui 钉到 `^4.12.10`（已是当前版本） |
| `tauri build` 在 `Downloading NSIS` 超时 | sourceforge 抽风 | 走 install-toolchain/windows.ps1 预下载到 `%LOCALAPPDATA%\tauri\NSIS\`；或手动预下载 |
| 安装后启动报「找不到 WebView2Loader.dll」 | NSIS bundle 漏 dll | `build.rs` 已把 dll 拷到 `src-tauri/bin/`，`tauri.conf.json` 的 `bundle.resources` 引用之；重跑 `npm run tauri build` |
| macOS Gatekeeper 拦截 | DMG 未签名 / 未公证 | `docs/macos.md` §签名与公证 |
| `xcode-select: error: unable to get active developer directory` | Xcode CLT 未装 | `xcode-select --install`；或 `bash scripts/install-toolchain/macos.sh` |
| Linux: `webkit2gtk-4.1 not found` | 包名错（不同发行版不一样） | 见 `scripts/install-toolchain/linux.sh` 的包列表 |
| 刷新套餐额度失败 | billing 接口要求 `X-Device-Mid` 头 | `docs/fixes-2026-09-15.md`；`zcode-switcher.exe --quota-probe` 验证 |

---

## 8. 故障排查速查

按"症状 → 工具 → 期望输出"组织：

### Rust 测试是否通过

```bash
# MSVC（推荐）
cargo +stable-x86_64-pc-windows-msvc test --lib 2>&1 | tail -5
# 期望：test result: ok. 39 passed; 0 failed; 1 ignored

# GNU（需要脚本绕过 manifest 坑）
scripts/run-tests.sh 2>&1 | tail -5
```

### 前端测试

```bash
npm run test:settings
npm run test:quotaguard  # 5/5 PASS
```

### 工具链完整状态

```bash
node scripts/install-toolchain/check.mjs --json | jq '.tools[] | select(.status=="missing")'
```

### 真机端到端（仅 Windows）

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows-regression.ps1 -ExpectedVersion 1.1.14
# 见 scripts/windows-regression.ps1 顶部注释与 docs/verification-1.1.14.md
```

### 配额 / CDP 通道真机自测

```powershell
zcode-switcher.exe --quota-probe
zcode-switcher.exe --guard-selftest
zcode-switcher.exe --plan-switch-probe start-plan
# 详细断言矩阵见 docs/verification-1.1.14.md
```

---

## 9. CI

`.github/workflows/` 当前有：

| Workflow | 触发 | Runner | 产物 |
|---|---|---|---|
| `release.yml` | tag `v*` 或手动 dispatch | `windows-latest` ×2（x64 + ARM64） | NSIS 安装包 → GitHub Release |
| `build-macos.yml` | PR / push to main | `macos-latest` | DMG → workflow artifact |
| `build-linux.yml` | PR / push to main | `ubuntu-22.04` | 裸二进制 → workflow artifact |

> **macOS DMG 不进 Release**：完整 notarize 需要 Apple Developer ID + App-Specific
> Password；这两个 secret 没配时只产未签名 DMG（用户首次打开需绕过
> Gatekeeper）。要进 Release 需要把上述 secret 配到 repo settings。

---

## 10. 历史文档归档

- `INSTALL.md`：1.1.12 时期写的 Windows 安装记录。工具链位置 / 激活脚本
  仍然准确（脚本目录是 `scripts/dev-env.sh`），版本号引用过时（已升到
  1.1.14），最新故障排查见本文 §7-8。
- `docs/development.md`：开发模式 / 构建命令的精简版。命令部分与本文
  §5 一致；macOS / Linux 细节见本文。
- `docs/release.md`：发布流程骨架。macOS 一节已迁到 `docs/macos.md`。
- `docs/macos.md`：macOS 专属（签名 / 公证 / DMG），与本文互补。

**老文档不是 deprecated**——它们对 GitHub 文件浏览器里的直接访问仍然有效，
新增内容请优先写进本文。
