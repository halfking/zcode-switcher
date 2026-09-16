# ZCode Switcher 1.1.16+45 Release

发布于 2026-09-17。本地构建产物（`npm run tauri build`，build.count=45），
已在构建机上完成真机部署验证。

## 产物

| 文件 | 大小 | SHA-256 |
|---|---|---|
| `ZCodeSwitcher-1.1.16+45-x64-setup.exe` | 4.0 MB | `69921e7ac26089e697d68f3e4a9c444db3e1fdad5a4deeb86267ab8a8c3d905b` |
| `latest.json` | – | Tauri updater 清单（本地构建无签名） |
| `SHA256SUMS.txt` | – | 上述 sha256 |

注：tauri 打包目录里的原始文件名带 stamp 前值（`+44`），安装后的 exe
元数据为 `1.1.16+45`，发布产物按安装后版本命名。`+N` 为 semver 构建
元数据，自动更新只比较 `1.1.16`。

## 本版更新（摘自 `docs/changelog.md`）

- **低额度自动切换的目标余额强制复核**：切套餐前重新拉取当前账号最新余额，
  复核通过才执行切套餐（复核发现当前入口已恢复则不动，目标入口复核不合格
  则直接落入切账号流程）；切账号时候选按余量排出验证顺序后逐一重新拉取
  最新余额，只有 token/积分仍全部高于阈值的账号才允许成为目标，全部验证
  不通过则进入自动暂停——杜绝凭过期缓存"从一个没余额的账号切到另一个
  没余额的账号"。
- 修复设置面板回归脚本的偶发失败：固定 sleep 等待 React 重渲染在负载高时
  不够（实测 ~20% 失败率），改为轮询等待。
- 修复 `windows-regression.ps1` 在 Windows PowerShell 5.1 下的解析失败：
  脚本含中文但无 UTF-8 BOM，5.1 按 GBK 误读导致 ParserError；补 BOM 修复。

## 真机验证（2026-09-17，构建机）

- 静默安装（NSIS `/S`）覆盖 1.1.15+43 成功，exe 元数据
  `ProductVersion = 1.1.16+45`。
- `windows-regression.ps1 -ExpectedVersion 1.1.16`：9/10 通过
  （版本 / 会话归属 / 进程 / credentials 加密形态 / 档案快照全部 PASS）。
- 唯一 FAIL 项 `zcode.cdp`（9229 端口）为环境状态：验证时机上 ZCode 由
  用户直接启动、未带 `--remote-debugging-port=9229`，与本版改动无关；
  经 Switcher 的增强启动快捷方式（带 flag）重启 ZCode 后该项即恢复。
- 前端检查：`tsc --noEmit` 通过；`test:quotaguard` 9/9（含 4 项逐一验证
  新用例）；`test:settings` 连续 6 次稳定通过。

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
  带签名的 `latest.json`（推送 `v1.1.16` tag 触发）。
- Windows ARM64 仍需走 CI 路径构建（参考 `.github/workflows/release.yml`）。

## 关联文档

- `docs/environment.md` — 跨平台环境搭建与构建手册
- `docs/changelog.md` — 完整更新日志
