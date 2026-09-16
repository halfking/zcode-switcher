# ZCode Switcher 1.1.15+43 Release

发布于 2026-09-17。本地构建产物（`npm run tauri build`，build.count=43），
已在构建机上完成真机部署验证。

## 产物

| 文件 | 大小 | SHA-256 |
|---|---|---|
| `ZCodeSwitcher-1.1.15+43-x64-setup.exe` | 4.0 MB | `d17e2c72e6998ed940ba63b6d83efabc8a151c9a67d17038578dcd873074647b` |
| `latest.json` | – | Tauri updater 清单（本地构建无签名） |
| `SHA256SUMS.txt` | – | 上述 sha256 |

## 本版修复（摘自 `docs/changelog.md`）

- **托盘双图标修复**：tauri.conf.json 的声明式 `trayIcon` 与代码里的
  `setup_tray` 重复创建托盘，移除配置项，只保留带「显示/退出」菜单的程序化托盘。
- **重复账号自动合并**：同一账号"一条档案存了邮箱、另一条只有用户 ID"曾被
  判定为两个账号（旧版按单一身份键去重漏判）。现改为身份信号（邮箱/手机号/
  用户 ID 任一重叠）并查集分组，主档案吸收空缺字段与缺失的 apiKey 快照；
  应用启动时自动迁移收敛一次，合并前自动备份到 `account-backups/`。
- 保存/导入/切换/活跃判定全部改用身份信号重叠匹配，重新捕获同账号不再产生新档案。
- **版本号引入编译次数**：显示为「版本+编译次数」（如 1.1.15+43），每次
  `tauri build` 自动递增（`scripts/stamp-build.mjs` + `build.count`）；
  `+N` 为 semver 构建元数据，不影响自动更新的版本比较。
- 设置面板版本显示规范化为 `v1.1.15+43`。

## 真机验证（2026-09-17，构建机）

- 静默安装（NSIS `/S`）覆盖 1.1.14 成功，exe 元数据 `ProductVersion = 1.1.15+43`。
- 设置面板「关于」显示 `版本 v1.1.15+43`。
- 账号列表收敛为 2 条（重复的 pcwelcl0 卡片消失），在用会话凭据保持不变。
- 单实例：再次启动 exe 不产生第二个进程，仅唤起已有窗口。
- 托盘仅 1 个图标。

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
  带签名的 `latest.json`（推送 `v1.1.15` tag 触发）。
- Windows ARM64 仍需走 CI 路径构建（参考 `.github/workflows/release.yml`）。

## 关联文档

- `docs/environment.md` — 跨平台环境搭建与构建手册
- `docs/changelog.md` — 完整更新日志
