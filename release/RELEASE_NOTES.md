# ZCode Switcher 1.1.17+47 Release

发布于 2026-09-17。本地构建产物（`npm run tauri build`，build.count=47），
已在构建机上完成真机部署验证。

## 产物

| 文件 | 大小 | SHA-256 |
|---|---|---|
| `ZCodeSwitcher-1.1.17+47-x64-setup.exe` | 4.0 MB | `d4a5f7fc5fdf93cc2f89d1564134df47549289cbab7b70e33e711afa30059e61` |
| `latest.json` | – | Tauri updater 清单（本地构建无签名） |
| `SHA256SUMS.txt` | – | 上述 sha256 |

注：tauri 打包目录里的原始文件名带 stamp 前值（`+46`），安装后的 exe
元数据为 `1.1.17+47`，发布产物按安装后版本命名。`+N` 为 semver 构建
元数据，自动更新只比较 `1.1.17`。

## 本版更新（摘自 `docs/changelog.md`）

- **候选余额验证限并发 + 单候选超时**：切账号前的余额验证从纯串行改为
  限并发（默认 3 个同时在飞），单候选验证限时 15 秒——超时/拉取失败按
  不合格跳过，挂死的请求不再拖住整轮切换；并发不改变选择语义，切到的
  仍是"验证通过里余量优先级最高"的账号。
- **验证进度暴露到 UI**：验证期间主界面显示进度横幅（已验证/总数与正在
  验证的账号名，中/英/俄三语），验证结束自动消失。
- **`windows-regression.ps1` 的 zcode.cdp 检查改为条件要求**：仅当 ZCode
  由增强启动（命令行带 `--remote-debugging-port=9229`）拉起时才检查 CDP；
  用户手动启动的 ZCode 无调试端口记为 SKIP 不再判 FAIL。新增 `-RepairCdp`
  一键修复：用增强参数重启 ZCode 后复查。
- 回归扩容：`test:quotaguard` 10 项（新增并发优先级/上限/超时/进度用例）；
  `test:autoswitch` 6 场景（新增 S6 并发上限+进度+优先级、S7 超时跳过）。

## 真机验证（2026-09-17，构建机）

- 静默安装（NSIS `/S`）覆盖 1.1.16+46 成功，exe 元数据
  `ProductVersion = 1.1.17+47`；安装后拉起新版 Switcher 驻留托盘。
- `windows-regression.ps1 -ExpectedVersion 1.1.17`：**9/9 通过、0 失败**。
  本次 ZCode 未带调试参数运行（用户手动启动），`zcode.cdp` 按新规则记为
  SKIP 并提示 `-RepairCdp`——该环境状态不再阻塞回归结论。
- 前端检查：`tsc --noEmit` 通过；`test:quotaguard` 10/10；
  `test:autoswitch` 6/6（S1-S5 原场景无回归，S6/S7 新语义锁定）；
  `test:settings` 稳定通过；`cargo` 单元测试 46/46（1 项真机凭据用例按
  设计 ignored）。

## 自测命令

```powershell
zcode-switcher.exe --quota-probe           # 拉一次真实额度（需外网 + ~/.zcode/v2/credentials.json）
zcode-switcher.exe --guard-selftest        # 网关拦截链路三态（暂停 / 恢复 / 放行）
zcode-switcher.exe --plan-switch-probe start-plan    # 套餐切换 + setting.json 落盘验证
zcode-switcher.exe --plan-switch-probe coding-plan
```

## 已知问题 / 注意事项

- **签名**：`latest.json` 的 `signature` 字段为空——本地无
  `TAURI_SIGNING_PRIVATE_KEY`。CI release.yml 用 tauri-action 自动签；
  本目录产物**未签名**，仅供本地验证；分发给真实用户走 GitHub Release 上
  带签名的 `latest.json`（推送 `v1.1.17` tag 触发）。
- Windows ARM64 仍需走 CI 路径构建（参考 `.github/workflows/release.yml`）。

## 关联文档

- `docs/environment.md` — 跨平台环境搭建与构建手册
- `docs/changelog.md` — 完整更新日志
