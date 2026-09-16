# ZCode Switcher 1.1.14 Release

发布于 2026-09-16。本地构建产物，对应 git commit `0633077`。

## 产物

| 文件 | 大小 | SHA-256 |
|---|---|---|
| `ZCodeSwitcher-1.1.14-x64-setup.exe` | 4.0 MB | `7fc0f0cd3b8cf260df636c5c72217a7483849f5532fefb31078309d5305c3939` |
| `latest.json` | – | Tauri updater 清单 |
| `SHA256SUMS.txt` | – | 上述 sha256 |

> Windows ARM64 (`aarch64-pc-windows-msvc`) 在本机工具链上未构建（无 cross
> toolchain）；如需，参考 `.github/workflows/release.yml` 在
> `windows-latest` 上用 `npm run tauri build -- --target
> aarch64-pc-windows-msvc` 走 CI 路径。

## 安装与使用

1. 双击 `ZCodeSwitcher-1.1.14-x64-setup.exe` → 默认装到
   `%LOCALAPPDATA%\ZCode Switcher\`。
2. 启动后登录 ZCode 账号；按 `ZCode Switcher` 文档配置。

## 自测命令

```powershell
zcode-switcher.exe --quota-probe           # 拉一次真实额度（需外网 + ~/.zcode/v2/credentials.json）
zcode-switcher.exe --guard-selftest        # 网关拦截链路三态（暂停 / 恢复 / 放行）
zcode-switcher.exe --plan-switch-probe start-plan    # 套餐切换 + setting.json 落盘验证
zcode-switcher.exe --plan-switch-probe coding-plan
```

详细断言矩阵见 `docs/verification-1.1.14.md`。

## 主要变化（摘自 `docs/changelog.md`）

- 低额度自动切换改为"先切套餐、后切账号"：判定对象改为 ZCode 当前选中的套餐入口，当前入口余额不足时先切换到同账号内仍有余额的另一个套餐入口（Start Plan 入口 ↔ GLM Coding Plan 入口），账号内所有套餐都低于阈值后才切换账号。
- 新增"低额度时动作"设置：除自动切换外，可选择"暂停执行"——余额低于阈值时本地网关拦截所有模型请求（非流式 429 / 流式 SSE error），同时停止自动切换并提醒，**等待人工操作才解除**。
- 实测 ZCode 不监听 setting.json 外部修改：套餐切换优先通过 CDP 注入渲染层 settingService 实时生效（无需重启），不可用时退回写配置文件并在开启自动重启时自动重启 ZCode。
- 新增真机自测命令：`--guard-selftest`（网关拦截链路）、`--plan-switch-probe`（套餐切换落盘验证）。
- 审计修正：暂停模式不再污染全局 `autoSwitchPaused` 标志（该标志语义是"所有账号 Token/积分均低于阈值"）；`isCurrentEntryLow` 在当前入口无数据时回退到整账号判定，避免账号只订阅单套餐时自动切换失灵；不再以可能过期的 Coding Plan API Key 本地快照阻断套餐切换。

## 已知问题 / 注意事项

- **本地 tauri build 失败**：当前 `zcode-switcher.exe` 进程（PID 103732）占着
  `src-tauri/target/release/zcode-switcher.exe`，cargo 无法覆盖。本 release
  采用的是上一轮（17:50 前后）已经成功构建的产物，对应 `0633077`。
- **签名**：`latest.json` 的 `signature` 字段为空——本地无 `TAURI_SIGNING_PRIVATE_KEY`。
  CI release.yml 用 tauri-action 自动签；本目录产物**未签名**，仅供本地
  验证；分发给真实用户走 GitHub Release 上带签名的 `latest.json`。
- **Windows GNU 工具链**：仍受 `w64devkit` 缺 `libgcc_eh` 影响，
  `cargo test --lib` 必须切 MSVC（`cargo +stable-x86_64-pc-windows-msvc
  test --lib`）。见 `docs/environment.md` §7。

## 关联文档

- `docs/environment.md` — 跨平台环境搭建与构建手册
- `docs/macos.md` — macOS 专属（签名 / 公证 / DMG）
- `docs/verification-1.1.14.md` — 1.1.14 真机验收清单
- `INSTALL.md` — 1.1.12 时期的 Windows 安装实录（保留作历史）
