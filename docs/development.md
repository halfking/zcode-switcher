# 开发说明

> ⚠️ **精简版**。完整的工具链安装、跨平台差异、故障排查见
> [`docs/environment.md`](environment.md)。本文件保留作为快速参考。
> macOS 专属（签名 / 公证 / DMG）见 [`docs/macos.md`](macos.md)。

## 安装依赖

```powershell
# Windows
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\install-toolchain\windows.ps1

# macOS
bash scripts/install-toolchain/macos.sh

# Linux
bash scripts/install-toolchain/linux.sh
```

## 启动开发模式

```powershell
npm run tauri dev
```

## 构建前端

```powershell
npm run build
```

## 构建 Windows 安装包

```powershell
npm run tauri build
```

## 构建 macOS 安装包

macOS DMG 必须在 Mac 或 GitHub macOS Runner 中构建：

```bash
npm ci
npm run tauri build -- \
  --target aarch64-apple-darwin \
  --bundles dmg \
  --config src-tauri/tauri.macos.conf.json
```

GitHub Actions 入口为 `.github/workflows/build-macos.yml`。构建产物位于
`src-tauri/target/aarch64-apple-darwin/release/bundle/dmg/`，详细说明见
`docs/macos.md`。

## 构建 Linux 裸二进制

Linux 上没有安装包目标，只产裸 release 二进制：

```bash
bash scripts/install-toolchain/linux.sh
npm run tauri build    # → src-tauri/target/release/zcode-switcher
```

## 本地目录约定

- 源码修改、依赖安装、构建和测试应在开发目录进行。
- GitHub 发布目录只保留准备提交的干净文件，不放 `node_modules`、`dist`、`src-tauri/target`、安装包、日志和临时截图。
