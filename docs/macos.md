# macOS 构建与分发

> 配套 `docs/environment.md` §3（macOS 安装）和 §5（macOS 构建命令）。本文只
> 写 macOS 特有事项：arch 选择、签名、公证、DMG 验证。

## 1. 架构选择

| Mac | target | 备注 |
|---|---|---|
| Apple Silicon (M1/M2/M3/M4) | `aarch64-apple-darwin` | 默认；自家机出自家二进制 |
| Intel | `x86_64-apple-darwin` | Intel Mac 唯一可用 |

**不能交叉编译**——Apple 工具链限制。CI 用 macOS runner（`macos-latest`
是 Apple Silicon）产 aarch64 二进制；要产 x86_64 需要 Intel runner（已
下架）或者用 `macos-13` runner。

universal binary：当前 Tauri 2 + zcode-switcher 还没启用。Apple Silicon 上
跑 x86_64 二进制靠 Rosetta，**不要**把这种运行模式当作分发目标。

## 2. 本机构建

```bash
# 1. 装工具链
bash scripts/install-toolchain/macos.sh

# 2. 拉依赖 + 编译
npm ci
npm run tauri build -- \
  --target aarch64-apple-darwin \
  --bundles dmg \
  --config src-tauri/tauri.macos.conf.json

# 3. 产物
ls -lh src-tauri/target/aarch64-apple-darwin/release/bundle/dmg/
# → ZCode Switcher_1.1.14_aarch64.dmg
```

`src-tauri/tauri.macos.conf.json` 是一份**覆盖**配置，与主 `tauri.conf.json`
合并使用。差异点：

- `productName`：`ZCode Switcher`（一致）
- `bundle.identifier`：`com.zcode.switcher`（一致）
- `bundle.macOS.minimumSystemVersion`：`11.0`（Big Sur+；WKWebView 稳定）
- `bundle.macOS.dmg.background`、`iconSize`、`windowSize`：可按需定制
- 不带 `bundle.resources/WebView2Loader.dll`（macOS 用 WKWebView，不需要）

示例：

```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "ZCode Switcher",
  "version": "1.1.14",
  "identifier": "com.zcode.switcher",
  "bundle": {
    "active": true,
    "targets": ["dmg"],
    "macOS": {
      "minimumSystemVersion": "11.0",
      "exceptionDomain": "",
      "frameworks": []
    },
    "icon": [
      "icons/32x32.png",
      "icons/128x128.png",
      "icons/128x128@2x.png",
      "icons/icon.icns"
    ]
  }
}
```

## 3. 签名

> 跳过签名也能跑（右键 → 打开一次解除 Gatekeeper），但分发给真实用户必须签。

需要：

1. **Apple Developer ID**（$99/yr，不是免费账号）
2. **Developer ID Application** 证书（Xcode → Accounts → Manage Certificates）
3. **App-Specific Password**（appleid.apple.com → App-Specific Passwords）
4. **`.p12` 凭据**（cert + private key），导出时设密码

环境变量：

```bash
export APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"
export APPLE_ID="you@example.com"
export APPLE_APP_SPECIFIC_PASSWORD="abcd-efgh-ijkl-mnop"
export APPLE_TEAM_ID="TEAMID10"
```

`tauri build` 会自动用这些变量调 `codesign --deep --force --options runtime
--sign "$APPLE_SIGNING_IDENTITY"`。

## 4. 公证

签名后还要公证（notarize），否则 Gatekeeper 在第一次启动时会做"无法打开，
因为它来自身份不明的开发者"拦截。

```bash
# 1. 提交 DMG
xcrun notarytool submit \
  "src-tauri/target/aarch64-apple-darwin/release/bundle/dmg/ZCode Switcher_1.1.14_aarch64.dmg" \
  --apple-id "$APPLE_ID" \
  --password "$APPLE_APP_SPECIFIC_PASSWORD" \
  --team-id "$APPLE_TEAM_ID" \
  --wait

# 2. 拿到 submission-id 后查日志
xcrun notarytool log <submission-id> \
  --apple-id "$APPLE_ID" \
  --password "$APPLE_APP_SPECIFIC_PASSWORD" \
  --team-id "$APPLE_TEAM_ID"

# 3. Staple ticket 到 DMG（离线验证必需）
xcrun stapler staple \
  "src-tauri/target/aarch64-apple-darwin/release/bundle/dmg/ZCode Switcher_1.1.14_aarch64.dmg"
```

## 5. 验证

```bash
# 签名校验
codesign -dv --verbose=4 \
  "src-tauri/target/aarch64-apple-darwin/release/bundle/macos/ZCode Switcher.app"
# 期望：Authority=Developer ID Application: ...  ; flags=...runtime

# 公证校验
xcrun stapler validate \
  "src-tauri/target/aarch64-apple-darwin/release/bundle/dmg/ZCode Switcher_1.1.14_aarch64.dmg"
# 期望：The validate action worked!

# Gatekeeper 模拟（要求有真实 mac）
spctl --assess --verbose \
  "src-tauri/target/aarch64-apple-darwin/release/bundle/dmg/ZCode Switcher_1.1.14_aarch64.dmg"
# 期望：accepted
```

## 6. CI

`.github/workflows/build-macos.yml`：

- 触发：push to main / PR
- runner：`macos-latest`
- 步骤：`npm ci` → `npm run tauri build` → 上传 DMG 到 workflow artifact
- **不**做签名 / 公证（仓库里没配 `APPLE_*` secret）

要让 DMG 进 GitHub Release：

1. repo Settings → Secrets and variables → Actions → 新增：
   - `APPLE_SIGNING_IDENTITY`
   - `APPLE_ID`
   - `APPLE_APP_SPECIFIC_PASSWORD`
   - `APPLE_TEAM_ID`
   - `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`（updater）
2. 在 `build-macos.yml` 加 `tauri-apps/tauri-action` 步骤（同 `release.yml`）
3. 在 `release.yml` 的 matrix 加 `macos-latest`，args 带上 `--target` + DMG

## 7. 已知差异（macOS vs Windows）

| 项 | Windows | macOS |
|---|---|---|
| 入口对象 | `zcode-switcher.exe` + `WebView2Loader.dll` | `ZCode Switcher.app/Contents/MacOS/zcode-switcher` |
| 安装位置 | `%LOCALAPPDATA%\ZCode Switcher\` | `/Applications/ZCode Switcher.app` |
| 卸载 | `uninstall.exe` (NSIS) | 拖入废纸篓 |
| 用户数据 | `%LOCALAPPDATA%\com.zcode.switcher\` | `~/Library/Application Support/com.zcode.switcher/` |
| WebView2 数据 | `EBWebView/` | `WKWebView/`（macOS 自管） |
| 快捷方式改写 | `zcode_launcher/windows.rs` | `zcode_launcher/macos.rs`（`zcode-switcher-macos-launcher.json`） |
| 自动更新通道 | NSIS | DMG + Sparkle 风格的 `latest.json`（需要签名私钥） |

## 8. 故障排查

| 现象 | 原因 / 处理 |
|---|---|
| `xcrun: error: unable to find utility "notarytool"` | Xcode < 13.3。升 Xcode / 跑 `softwareupdate --install-auxiliary-tools` |
| `xcode-select: error: unable to get active developer directory` | 没装 CLT。`xcode-select --install` |
| `codesign` 报 "no identity found" | 没装 Developer ID Application 证书。Xcode → Accounts → Manage Certificates |
| `notarytool submit` 卡在 "Waiting for processing" | Apple 服务器抽风，10-30 分钟是常态；带 `--wait` 不要打断 |
| `notarytool` 报 `Package Invalid` | 通常是 hardened runtime 没开（`--options runtime`），或 entitlements 缺关键项 |
| DMG 双击后 `unacceptable package` | 没 staple ticket；跑 `xcrun stapler staple ...dmg` |
| 启动报 "App is damaged" | 签名但没公证；要么公证，要么 `xattr -cr /Applications/ZCode\ Switcher.app`（用户侧临时绕过） |
