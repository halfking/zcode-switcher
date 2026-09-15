# zcode-switcher 本地构建与安装配置

本机器（Windows 10 x64）原本没有 Node.js、Rust、MSVC 工具链。
本项目最终在「免管理员 / 无 Visual Studio Build Tools」的环境下完成端到端构建与安装包产出。
本文件说明工具链的来源、位置、复用方式，以及产物布局。

## 工具链

| 组件 | 版本 | 安装位置 | 来源 | 大小 |
| --- | --- | --- | --- | --- |
| Node.js | 20.19.5 LTS | `C:\workspace\zcode-switcher\.tools\node-v20.19.5-win-x64\` | nodejs.org 便携 zip | ~30 MB |
| npm | 10.8.2 | 同 Node 安装根目录 | Node 自带 | – |
| Rust | 1.98.1 (stable, GNU target) | `C:\Users\86133\.cargo\` | `rustup-init.exe`，默认 host `x86_64-pc-windows-gnu` | ~250 MB |
| MinGW-w64 GCC | 14.2.0 | `C:\workspace\zcode-switcher\.tools\mingw64\` | winlibs UCRT 便携 zip | ~1 GB |
| NSIS | 3.11 | `C:\Users\86133\AppData\Local\tauri\NSIS\` | 手动预下载（避免 Tauri bundler 网络超时） | ~5 MB |

> ⚠ Rust 默认 host 是 `x86_64-pc-windows-gnu`，因此 `webview2-com-sys 0.38.2` 走动态链接，
> `target/release/WebView2Loader.dll` 是运行时的依赖。
> 已通过修改 `src-tauri/build.rs` + `src-tauri/tauri.conf.json` 把该 dll 打入 NSIS 安装包。
> 详见仓库根目录 `docs/fixes-2026-09-14.md` 与 `src-tauri/build.rs`。

## 激活工具链

启动 Git Bash / MSYS 风格 shell 后，运行：

```bash
source scripts/dev-env.sh
```

该脚本会：

1. 把 `.tools/node-v20.19.5-win-x64/`、`~/.cargo/bin/`、`.tools/mingw64/bin/` 加入 `PATH`（置顶）
2. 打印当前 node / npm / rustc / cargo / gcc 版本
3. 设置 `MSYS_NO_PATHCONV=1`，避免 Git Bash 把路径里的冒号 / 空格误转换为 Windows 风格

激活后即可直接运行：

```bash
npm install
npm run build
npm run tauri build
cargo --version
```

## 项目源码版本

| 文件 | 字段 | 值 |
| --- | --- | --- |
| `package.json` | name / version | `zcode-switcher` / `1.1.12` |
| `src-tauri/Cargo.toml` | name / version | `zcode-switcher` / `1.1.12` |
| `src-tauri/tauri.conf.json` | productName / version / identifier | `ZCode Switcher` / `1.1.12` / `com.zcode.switcher` |

## 构建产物

| 路径 | 大小 | 说明 |
| --- | --- | --- |
| `src-tauri/target/release/zcode-switcher.exe` | 36 MB | Tauri release 可执行文件 |
| `src-tauri/target/release/WebView2Loader.dll` | 160 KB | webview2-com-sys 动态链接依赖 |
| `src-tauri/target/release/bundle/nsis/ZCode Switcher_1.1.12_x64-setup.exe` | 6.7 MB | NSIS 安装包，与 GitHub Release 命名一致 |

## 安装目录（运行版）

NSIS 默认安装到 `%LOCALAPPDATA%\ZCode Switcher\`：

```
C:\Users\86133\AppData\Local\ZCode Switcher\
├── zcode-switcher.exe    36 MB   （主程序）
├── WebView2Loader.dll    160 KB  （WebView2 loader，来自安装包 bundle.resources）
└── uninstall.exe         79 KB   （NSIS 卸载器）
```

WebView2 用户数据目录（首次启动时创建，不在安装包内）：

```
C:\Users\86133\AppData\Local\com.zcode.switcher\EBWebView\
```

卸载时**安装目录**会被清空，**用户数据目录**按 Tauri 默认行为保留。

## 关键 npm 脚本

```bash
npm run dev                       # vite 开发服务器
npm run build                     # tsc + vite build（仅前端）
npm run tauri build               # 完整 release 构建并打 NSIS 安装包
npm run build:captcha-runtime     # 拷贝 captcha 求解器脚本并装 jsdom
npm run test:settings             # 前端设置面板回归（node）
npm run notice:check              # 校验 public/notice.json（CI 模式）
```

## PowerShell 端到端回归（实机）

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows-regression.ps1
```

要求**真实桌面会话**（Session 1）。该脚本检查项：

1. 已安装的 `zcode-switcher.exe` ProductVersion
2. 进程归属（应在交互登录用户会话下）
3. `~/.zcode/v2/credentials.json` 是否为 `enc:v1:` 密文
4. 账号池档案与凭据快照加密完整性
5. ZCode CDP（9229）上是否存在 renderer 页面

非交互式 SSH/服务会话下第 2/5 项会失败 —— 这是会话限制，不是构建问题。

## 故障排查

| 现象 | 原因 / 处理 |
| --- | --- |
| `npm install` 报 `lucide-react@1.18.0 notarget` | lockfile 还锁着错误的版本。`rm package-lock.json && npm install` |
| `npm run build` 报 daisyui 报错 | `package.json` 的 daisyui 应该是 `^4.12.10`（不是 `^5.x`） |
| `tauri build` 在 `Downloading NSIS` 超时 | 手工预下载到 `%LOCALAPPDATA%\tauri\NSIS\`，再 `tauri build` |
| 安装后启动报「找不到 WebView2Loader.dll」 | 见「工具链」注释。已修复，重新 `npm run tauri build` 即可 |
| `cargo build` 报 `linker not found` | 没激活工具链或 gcc 不在 PATH。`source scripts/dev-env.sh` |
| 刷新套餐额度失败 | 2026-09-15 已修复（billing 接口要求 `X-Device-Mid` 头，见 `docs/fixes-2026-09-15.md`）。验证：`zcode-switcher.exe --quota-probe` |
| `cargo test` 测试 exe 启动失败 (0xc0000139) | GNU 工具链下测试 exe 无 comctl32 v6 manifest。改用 `bash scripts/run-tests.sh`：先 `--no-run` 编译，再把 `scripts/testexe.manifest` 放到测试 exe 旁边（Windows SxS 会自动加载），然后直接运行测试 |
