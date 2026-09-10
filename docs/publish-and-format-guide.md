# 发布与格式统一说明

本文档统一说明发布流程、更新日志、施工日志和公告格式。

## 发布流程

1. 修改 `package.json` 和 `src-tauri/tauri.conf.json` 中的版本号。
2. 在 `docs/changelog.md` 顶部添加对应版本的小节。
3. 运行基础验证：

```powershell
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo check --manifest-path src-tauri/Cargo.toml
npm run notice:check
```

4. 需要正式安装包时再运行：

```powershell
npm run tauri build
```

5. 创建并推送版本标签：

```powershell
git tag v1.1.8
git push origin v1.1.8
```

GitHub Actions 会读取 `docs/changelog.md` 里同版本小节作为 Release 说明，并上传安装包和 `latest.json`。

## 更新日志格式

文件：`docs/changelog.md`

格式：

```markdown
# 更新日志

## 1.1.8

- 修复手动切换后首次额度加载提示不明显的问题。
- 优化公告编辑与预览流程。
```

要求：

- 版本标题必须是 `## x.y.z`，不要加 `v`。
- 每条只写用户能理解的变化，不写内部实现流水账。
- GitHub Release 会自动抽取当前 tag 对应的小节。

## 施工日志格式

文件：`progress.md`

每轮实际改动仓库文件后，在文件末尾追加：

```markdown
## 2026-06-26 - Task: 简短任务名

### What was done

- 写本轮完成的业务动作或结果。

### Testing

- 写实际运行过的验证命令和结果。
- 未验证的地方必须明确说明。

### Notes

- `src/store.ts`：说明本轮改了什么。
- 回滚方式：说明如何撤回本轮改动。
```

要求：

- 只追加，不改写历史。
- 纯问答或只读检查不需要写。
- 有代码改动就必须写验证证据。

## 公告格式

文件：`public/notice.json`

公告会优先读取 GitHub 仓库 `main/public/notice.json`，读取失败时回退本地内置文件。软件启动时会拉取一次，运行中每 15 分钟自动拉取一次，手动打开公告窗口时也会尝试刷新；这不是服务端实时推送。

字段说明：

- `enabled`：总开关，`false` 时不显示任何公告。
- `id`：公告唯一 ID，改内容后建议换新 ID，否则已读用户可能不会再次弹出。
- `kind`：`system` 或 `temporary`。
- `title`：标题，建议使用 `{ "zh": "..." }`，当前公告编辑器只维护中文内容。
- `body`：正文，建议使用 `{ "zh": "..." }`，当前公告编辑器只维护中文内容。
- `level`：`info`、`warn` 或 `error`。
- `date`：显示日期文本。
- `showOnce`：`true` 表示用户关闭后不再自动弹出；`false` 表示每次启动都可弹。
- `showOnStartup`：`true` 表示启动时自动弹出。

正文支持的轻量标记：

```text
<red>重要文字</red>
<link url="https://example.com">链接文字</link>
<link url="https://example.com" />
<link url="https://example.com/file.zip" browser="false">获取</link>
https://example.com
```

链接说明：

- 不写 `browser` 时默认打开浏览器。
- `browser="false"` 时不打开浏览器，点击后先显示文件名、类型和大小供确认，再选择下载目录；软件会优先按响应头自动识别文件名和后缀，下载完成后在软件内提示。
- `browser="false"` 不依赖显示文本，按钮文字可以写成“获取”“下载”“说明书”等任意内容。
- 自闭合 `<link url="..." />` 会隐藏真实链接名，普通链接显示为“打开链接”，后台下载链接显示为“获取”。

示例：

```json
{
  "enabled": true,
  "notices": [
    {
      "id": "system-2026-06-26",
      "kind": "system",
      "title": {
        "zh": "版本提醒"
      },
      "body": {
        "zh": "<red>重要：</red>新版已发布，详情见 <link url=\"https://github.com/halfking/zcode-switcher/releases\">发布页</link>。安装包可点 <link url=\"https://github.com/halfking/zcode-switcher/releases/latest\" browser=\"false\">获取</link>。"
      },
      "level": "info",
      "date": "2026-06-26",
      "showOnce": true,
      "showOnStartup": true
    }
  ]
}
```

## 公告预览与编辑

启动可编辑预览：

```powershell
npm run notice:preview
```

也可以在项目根目录双击 `notice-preview.cmd`，它会自动启动同一个预览服务并打开浏览器。

打开后是中文可视化公告编辑器。支持：

- 新增系统公告或临时公告。
- 用表单编辑公告 ID、类型、等级、日期、弹出策略、中文标题和中文正文。
- 在正文中一键插入红色重点、普通链接、隐藏链接名和后台下载链接。
- 右侧按软件内公告弹窗样式实时预览，tab、公告卡片、链接样式和“知道了”按钮会尽量与正式界面一致。
- 展开“高级 JSON”后可查看或手动修改原始 JSON，也可以打开 `/json` 使用旧版 JSON 编辑器。
- 点击“保存”或按 `Ctrl+S` 写回 `public/notice.json`。

路径与生效规则：

- 默认编辑文件按脚本自身定位为项目根目录的 `public/notice.json`，不依赖当前命令行目录。
- `--input` 指定自定义文件时，相对路径仍按当前命令行目录解析。
- 预览脚本是源码仓库工具，不会随普通安装后的软件提供给最终用户。
- 软件在线时优先读取 GitHub 仓库 `main/public/notice.json`；只在本地保存不会让已安装软件立刻看到新公告。
- 正式发布公告需要把 `public/notice.json` 推送到 GitHub；安装包内置公告仅作为远程读取失败时的兜底。
- 如果 `showOnce` 为 `true` 且公告 ID 没变，用户关闭过后不会再次自动弹出；需要再次自动弹出时请换一个新的 `id`。

只校验不打开页面：

```powershell
npm run notice:check
```

指定文件、端口或语言：

```powershell
node scripts/preview-notice.mjs --input public/notice.json --port 4178 --lang zh
```

## 发布后检查

发布完成后检查：

- Release 页面中是否有 Windows 安装包。
- Release 附件中是否有 `latest.json`。
- 访问 `https://github.com/halfking/zcode-switcher/releases/latest/download/latest.json` 是否不是 404。
- 软件内检测更新是否能看到 `docs/changelog.md` 对应版本说明。
- 公告按钮是否能读取最新 `public/notice.json`。
