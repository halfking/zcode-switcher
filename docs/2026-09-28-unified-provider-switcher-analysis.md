# 四项目统一模型供应商切换工具：合并可行性与方案

**日期**：2026-09-28
**分析对象**：`zcode-switcher`（本项目）、`codex-tools`、`cc-switch`、`orca`
**文档性质**：可行性分析 + 方案设计（尚未实施）
**版本**：v2（经独立审计修订，修订点见 §10）

> ⚠️ **本文档所有 git 状态均为 2026-09-28 快照。cc-switch 的 fork 正在被并发会话改动，orca 的 preset 层正在重构。动手前必须重新核对 §0.2 与 §8。**

---

## 0. 摘要（Executive Summary）

### 0.1 结论一句话

**可以做，但不应该"合并代码库"；正确形态是：以 `cc-switch` 为唯一内核，其余三项目降级为「能力插件」向其输送，ZCode 作为新增的 app-type 接入。**

**但这个结论有一个前置条件**：必须先处理 cc-switch 与上游 22 个 commit 的差距，因为上游正在重构切换引擎本身（§3.2）。**跳过这一步直接开发，会导致方案 §3 的核心设计建立在即将消失的代码结构上。**

### 0.2 四个项目的本质（这是全文最重要的认知）

| 项目 | 本质 | 体量 | 维护状态 |
|------|------|------|----------|
| **zcode-switcher** | **ZCode（Z.ai/智谱）账号池 + 配额熔断** | 10,015 行 Rust | 无 LICENSE，最后提交 2026-09-18 |
| **codex-tools** | **Codex 协议级网关**（不是切换器） | 45,430 行 Rust | MIT，最后提交 2026-09-08 |
| **cc-switch** | **通用 provider 切换器**（已是超集） | 218,762 行 Rust | MIT，**落后上游 22 个 commit，领先 13 个** |
| **orca** | **Electron 侧的平行 preset 实现**（更年轻、更弱） | ~392 万行 TS | 并发重构中，20 文件未提交 |

**关键事实**：zcode-switcher 与 MiniMax Code **毫无关系**（`grep -ri "minimax\|claude\|codex" src-tauri/src src/ scripts` = **0 命中**）。它管的是 Z.ai 的 ZCode。

### 0.3 五个关键判断

| # | 判断 | 依据强度 |
|---|------|----------|
| 1 | **三者并非同类竞品**，是三种不同形态：ZCode 账号池、Codex 协议网关、通用切换器 | 高（代码级，已复核） |
| 2 | **cc-switch 覆盖面已是超集**：10 个 app-type、SQLite 17 表、77k 行代理层 | 高（代码级，已复核） |
| 3 | **cc-switch 与上游的架构级重构冲突是最大风险**，优先于任何功能开发 | 高（git 实测） |
| 4 | **IDE 是四家全零的真空白**，但"生态上无人做"属未验证推断 | 中（本地已验证 / 生态未验证） |
| 5 | **orca 的 preset 层不可直接复用**：4 条红线里 3 条 cc-switch 早已内建 | 高（代码级，已复核） |

### 0.4 推荐路线（详见 §7）

```
Phase 0  上游同步 + 冲突裁决          ← 阻塞项，不完成则后续全部作废
Phase 1  ZCode 作为第 11 个 app-type
Phase 2  补齐 IDE 维度
Phase 3  orca 知识固化（仅剩 1 条真缺口）
```

---

## 1. 现状分析

### 1.1 体量与许可证

| 项目 | 技术栈 | Rust LOC | TS LOC | 版本 | 许可证 |
|------|--------|---------|--------|------|--------|
| **zcode-switcher** | Tauri 2 + **React 19** | 10,015 (17 文件) | 8,420 (22 文件) | 1.1.17+47 | **无 LICENSE 文件、无 license 字段** |
| **codex-tools** | Tauri 2 + **React 19** | 45,430 (48 文件) | 18,489 (37 文件) | 2.9.0 | MIT (170-carry) |
| **cc-switch** | Tauri 2 + **React 18.2** | 218,762 (237 文件) | 105,903 (356 文件) | 3.20.4 | MIT (Jason Young) |
| **orca** | **Electron** + React + TS | — | ~3,919,864 (22,954 文件) | — | 见 LICENSE |

三家都是 Tauri 2（`@tauri-apps/api` ^2.8 / ^2.10 / ^2）—— 这是唯一有利的合并前提。但 React 版本不同（cc-switch 停在 18.2，另两家 19.x），壳体并非完全一致。

### 1.2 zcode-switcher：ZCode 账号池

**定位纠偏**：与 MiniMax Code 无关，管的是 Z.ai（智谱）的 **ZCode** 客户端。

硬编码端点证据：
- `src-tauri/src/quota.rs:12`、`oauth_cli.rs:10-11`、`captcha.rs:12` → `https://zcode.z.ai/api/v1/...`
- `src-tauri/src/oauth.rs:19-23` → `chat.z.ai/api/oauth/authorize`、`bigmodel.cn/login`
- `src/lib/glm52.ts:5` → 监控模型 `glm-5.3` / `glm-5.3-flash`

**目录解析有四层回退，不是简单的 `~/.zcode/v2/`**（`src-tauri/src/profile.rs:53-73`，源码注释详述）：
```
setting.json    → home/.zcode/v2                    （永远 home 基址）
credentials.json → dataBaseDir()/.zcode/v2
config.json      → dataBaseDir()/.zcode/v2
dataBaseDir()    = setting.json#dataBaseDir ‖ ZCODE_DATA_BASE_DIR ‖ HOME ‖ USERPROFILE
```
> **源码注释原文警告**：曾因写死 home 导致 credentials/config 落到 ZCode 看不到的盘，**表现为"任何账号都切不了"**。任何移植都必须完整复刻这四层回退。

**独有能力**（其他项目没有）：
1. **CDP 免重启热切换** —— `zcode_cdp.rs:29-123,225-282`：遍历 React Fiber（上限 600,000 节点，`:86`）调用 `refreshCodingPlanApiKey` / `settingService.update`。
2. **配额熔断网关** —— `proxy.rs:425-905`：本地 axum + 账号池轮转 + 阿里云验证码求解；耗尽时非流式返 429、流式返 SSE `event: error`（`:655-676`）。
3. **凭据加密** —— `crypto.rs:1-58`：AES-256-GCM `enc:v1:`，与 ZCode 自身格式兼容，改了就切不了。
4. **`config.json` 必须原地 truncate 写** —— `profile.rs:675-691` 源码明写：ZCode 用 fs/watch，rename 换 inode 会漏事件。**这与 cc-switch 的原子写策略直接冲突。**

**规模定位**：10k 行 Rust、51 个 `#[test]`（已复核），但 `proxy.rs` / `zcode_cdp.rs` / `captcha.rs` **零测试覆盖**——恰是风险最高的两块。模型/套餐名硬编码为子串匹配（`"glm-5.3"`、`"start"`、`"coding"`），ZCode 改名会静默失效。

### 1.3 codex-tools：Codex 协议网关

**核心是 12,732 行的 `proxy_service.rs`**（占全仓 Rust 28%）。不是改配置，而是**协议级模拟**：`/v1/responses` + WebSocket、`/v1/chat/completions`、Anthropic `/v1/messages`、图像生成、`/v1/responses/compact`。

**耦合 Codex 版本的确切位置**：
- UA 伪装在 `src-tauri/src/proxy_service/model_catalog.rs:3-5`：`CODEX_CLIENT_VERSION = "0.153.4"` / `CODEX_USER_AGENT = "codex_cli_rs/0.153.4"`
- 模型目录 `MODELS` 同文件 `:7+`（含 `gpt-6-astra`）

**技术债**：`ApiProxyPanel.tsx` 3,879 行、`useCodexController.ts` 3,225 行；6 份重复的原子写实现；`state_5.sqlite` 直接 SQL UPDATE（`provider_sync/mod.rs:662-723`）跨版本高危。

**Windows Store 路径其实不脆弱**（审计纠正）：`cli/windows_identity.rs:4-19` 是**版本无关**的，只匹配 `windowsapps/openai.codex_*/app/{chatgpt,codex,codex desktop}.exe` 四段尾部；版本号字面量出现在 `:28,38-40` 且位于 `#[cfg(test)]` 内，其测试名恰是 `both_running_old_store_and_new_launch_version_have_desktop_identity`——**断言两版本都能匹配**。

### 1.4 cc-switch：已是超集，但抽象是反模式

**覆盖矩阵**（`README.md:316-327`，代码验证于 `app_config.rs:396-464`）：

| 工具 | 供应商切换 | 本地路由 | 托盘 | MCP | Skills | Prompts | Sessions | Usage |
|------|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Claude Code | ✓ | ✓ | ✓ | ✓ | ✓ | CLAUDE.md | ✓ | ✓ |
| Claude Desktop | ✓ | 仅模型映射 | – | – | – | – | – | 仅映射 |
| Codex | ✓ | ✓ | ✓ | ✓ | ✓ | AGENTS.md | ✓ | ✓ |
| Gemini CLI | ✓ | ✓ | ✓ | ✓ | ✓ | GEMINI.md | ✓ | ✓ |
| Grok Build | ✓ | ✓ | ✓ | ✓ | ✓ | AGENTS.md | ✓ | ✓ |
| OpenCode | 共存 | – | – | ✓ | ✓ | AGENTS.md | ✓ | ✓ |
| OpenClaw | 共存 | – | – | – | – | 工作区编辑 | ✓ | – |
| Hermes | 共存 | – | – | ✓ | ✓ | Memory | ✓ | – |
| Pi | 共存 | – | – | – | ✓ | AGENTS.md | ✓ | ✓ |
| **MiniMax Code** | 共存 | – | – | ✓ | ✓ | AGENTS.md | ✓ | ✓ |

两条正交轴：`is_additive_mode()`（`:435-440`）、`supports_local_proxy()`（`:442-447`，仅 Claude/Codex/Gemini/GrokBuild）。

**结构问题：`AppType` 是扁平枚举，不是 trait 注册表。** 全 `src-tauri/src` 中 `AppType::` 出现 **1,424 次**，其中 `services/provider/mod.rs` 275 次（分布在 269 行）、`services/proxy.rs` 177 次、`app_config.rs` 155 次、`skill.rs` 95 次。写盘统一走 `write_live_snapshot`（`live.rs:1587-1748`）的大 `match`。全仓唯一的 trait 是 `proxy/providers/adapter.rs:16`，且它属于**本地路由代理**，不是配置管理。

新增 app-type 需同步改 8–12 处：`AppType` → `all()` → `FromStr` → `write_live_snapshot` → `read_live_settings` → `mcp_servers` 加列 → `user_version` 迁移（`database/schema.rs:456-580`）→ TS union。

**已存在的枚举漂移**（已复核）：
- Rust `AppType` 10 个 variant
- `FromStr` 错误文案**只列 9 个，漏了 `mcode`**（`app_config.rs:485-486`）
- TS union `src/types/usage.ts:192-199` 只有 **7** 个，漏 `claude-desktop`/`openclaw`/`hermes`
  > **审计纠正**：`usage.ts:176-192` 有 17 行注释**说明这是有意的**（`claude-desktop` 被后端 `folded_app_type_sql` 折叠进 claude，避免看板显示误导性数字）。**这是语义决策不是 bug，不能"自动修掉"**——但 registry 化后应显式建模这个折叠关系。

**存储**：`~/.cc-switch/cc-switch.db`，SQLite（rusqlite 0.31），17 张表，手工 `user_version` 迁移阶梯 0→19。

**写盘库**：`toml 0.8` + **`toml_edit 0.22`**（保留 TOML 注释/格式），`serde_json` + `preserve_order`；原子写 + 0600（`config.rs:383-392`）。

> **⚠️ 备份机制的重要澄清（审计发现，原文档表述错误）**：`app_config.rs:706-718` 的备份只针对 **cc-switch 自己的 `~/.cc-switch/config.json`**，与 live 应用配置无关。真正的 live 备份走 DB 表 `proxy_live_backup`（`database/dao/proxy.rs:788-797`），生产调用方**全部在 `services/proxy.rs`**（代理接管路径）。`services/provider/mod.rs` 中的 `save_live_backup` 调用**全部位于 `:114` 起的 `#[cfg(test)] mod tests` 内**。
> **结论：非代理切换路径、以及全部 5 个 additive 模式 app-type（OpenCode/OpenClaw/Hermes/Pi/Mcode）目前没有写前备份。** 而 upstream `15c0b3ce` 更是**主动删除了代理路径的 live 备份**（"No backups"，改为 `live-state.json` 记录 direct/proxy 模式）。

**无 CLI 二进制**（`crate-type = ["staticlib","cdylib","rlib"]`，无 `[[bin]]`），但有 `ccswitch://v1/import?resource=...` 深链（仅导入非切换）与托盘菜单。

**monolith**：`services/proxy.rs` 10,769 行、`codex_config.rs` 9,005 行、`services/provider/mod.rs` 7,631 行、`services/skill.rs` 6,983 行。全仓 Rust 仅 1 处 TODO——债是结构性的。

### 1.5 orca：更年轻的平行实现

| 维度 | orca preset | cc-switch |
|------|-------------|-----------|
| 目标 agent | 3 个（`codex`/`claude`/`opencode`） | 10 个 app-type |
| 存储 | `GlobalSettings.customProviders` | SQLite `providers` 表 |
| Codex 写盘 | **正则文本合并**（`codex-apply-provider-preset.ts:98-130`） | `toml_edit` 结构化编辑 |
| 备份 | **无** | 仅代理路径有（见上） |
| 校验 | 无 | 有 |

**orca 内置 kaixuan 端点**（`src/shared/provider-preset-types.ts:86-107`）：`kaixuan-local` = `127.0.0.1:8782`，`kaixuan-kxpms` = `llm.kxpms.cn`，共享 11 个模型 id（`:70-82`）。

**与 cc-switch 本地 fork 深度撞车**（审计发现，比原判断更深）：不只是"实现同一个东西"，而是**用了完全相同的两个端点**和**高度重叠的 11 个 model id**（orca `claude-opus-4-8`/`glm-5.1`/`glm-5.2`/`gpt-5.4`/`gpt-5.5`/`mimo-v2.5*`/`minimax-m2.7*`/`minimax-m3` vs cc-switch `codexProviderPresets.ts`）。**任一侧改模型目录，另一侧会静默漂移。**

cc-switch 本地 fork 对应实现：`env_expand.rs`（env 展开）、`gateway_health.rs`（健康探针，417 行）、`provider_bundle.rs`（一键 bundle，1,443 行），端口默认 8782。

**当前状态（审计时快照）**：分支 `feat/kaixuan-v4-custom-providers`，HEAD `ab9f00a50`，**20 个文件未提交修改**（含 6 个 i18n、4 个 test）。v4 迁移的调用点（`:133,193,195,196`）与 bridge 签名（`provider-preset-bridge.ts:9-33`）**已一致**，重构接近收尾。

> ⚠️ **此层此刻不稳定，不可作为依赖基线。**

---

## 2. 合并可行性分析

### 2.1 为什么"合并代码库"是错的

**证据 1：重复壳体成本。** 三家都是 Tauri 2，合库意味着人工搬运约 273k 行 Rust + 133k 行 TS，产出混合三种设计哲学（账号池思维、协议网关思维、配置注册表思维）的 25 万行单体。收益接近零——它们的壳体本来就一样。

**证据 2：fork 同步成本已经失控。**（审计实测，**原文档方向写反已修正**）

```
git rev-list --count HEAD..upstream/main        → 22   （落后 22 个 commit）
git rev-list --count upstream/main..HEAD        → 13   （领先 13 个 commit）
upstream/main tip = 846de29c 2026-09-28        （就是今天）
工作区：git status --porcelain 为空（原文档称的 14 个未提交文件已被并发会话提交）
```

**15 个文件两边都改过**，且冲突是**语义级**的——upstream 的 22 个 commit 包含一个架构级重构（见 §3.2），正面冲击本地 fork 的 inert-merge 逻辑（`live.rs:711-843`）。

**证据 3：许可证不对称。** codex-tools 与 cc-switch 都是 MIT。**zcode-switcher 没有 LICENSE 文件**，`package.json`/`Cargo.toml` 也无 license 字段——严格说是保留所有权利。它自己的代码可搬进自己的产品；反向（MIT 代码并进来再整体开源）需先解决授权。

### 2.2 为什么"cc-switch 作内核"是对的

1. **覆盖面已是超集**：10 个 app-type > orca 3 + ZCode 1。且**已内置 MiniMax Code**（`AppType::Mcode` → `data_dir()/config.yaml`，`mcode_config.rs:12-61`，支持 `MINIMAX_DATA_DIR`/`MAVIS_DATA_DIR` 覆盖）。
2. **基础设施完备度**：`src-tauri/tests/` 14 个集成测试 / 8,858 行，用 `CC_SWITCH_TEST_HOME` 重置**真实文件系统** + **真实 SQLite**，Cargo 无 mockall/wiremock/mockito。对比 codex-tools 只有 3 个 `node --test`，其中 `heatmapScale.test.ts` **是孤儿测试，package.json 与 CI 都没引用，永远不跑**（已复核）。
3. **写盘正确性更高**：`toml_edit` vs orca 的正则；orca 三个 apply 路径**全部无备份**，且 `JSON.parse` 失败会**静默把整个文件丢成 `{}`**（`claude-apply-provider-preset.ts:120-124`、`opencode-apply-provider-preset.ts:128-132`）——明确的数据丢失路径。
4. **需要的能力天然是插件形态**：ZCode 的 CDP 熔断、Codex 网关，都是"针对某 app 的特殊增强"，落在 `ProviderService::switch`（`services/provider/mod.rs:5712`）入口 + `write_live_snapshot` 分派 + `provider_endpoints` 表。

> **待验证**：本次分析**未运行任何测试**。"基础设施完备度"是数量级判断，但若 cc-switch 测试套件当前是红的，该判断反转。**Phase 0 必须先跑一次。**

### 2.3 能力矩阵与缺口

| 能力 | zcode-switcher | codex-tools | cc-switch | orca | 合并后归属 |
|------|:---:|:---:|:---:|:---:|------|
| Claude Code provider | ✗ | ✗ | ✓ | ✓ | cc-switch |
| Codex provider | ✗ | ✓(仅账号) | ✓ | ✓ | cc-switch |
| Gemini / Grok / OpenCode | ✗ | 部分 | ✓ | 部分 | cc-switch |
| **MiniMax Code** | ✗ | ✗ | ✓ | ✗ | cc-switch |
| **ZCode (Z.ai)** | ✓ | ✗ | ✗ | ✗ | **待新增 app-type** |
| 多账号凭据池 | ✓ | ✓ | 部分 | ✗ | cc-switch + 插件 |
| 配额/用量展示 | ✓ | ✓ | ✓ | ✗ | cc-switch |
| 配额熔断(429/SSE) | ✓ | ✗ | ✗ | ✗ | **待移植** |
| CDP 免重启热切换 | ✓ | ✗ | ✗ | ✗ | **待移植** |
| Codex 协议网关 | ✗ | ✓ | 部分(77k行) | ✗ | cc-switch proxy |
| 私有 DB 改写 | ✗ | ✓(高危) | ✗ | ✗ | **建议不做** |
| MCP / Skills / Prompts | ✗ | ✗ | ✓ | 部分 | cc-switch |
| 深链导入 | ✗ | ✗ | ✓ | ✗ | cc-switch |
| **CLI 一键切换** | ✗ | ✓ (`ctc`) | ✗ | ✗ | **待补齐** |
| **IDE (VS Code/Cursor)** | ✗ | ✗* | ✗ | ✗ | **真空白** |

\* `codex-tools/editor_apps.rs` 全 192 行只有 `EDITOR_SPECS`（`:20-63`）+ 重启（`:108-114`），**不写任何配置**。已复核。

### 2.4 唯一的真空白：IDE

**CLI 维度已覆盖**（8 个 CLI 都已是 app-type）。

**IDE 维度四家全零**（已逐仓全树 grep 复核）：cc-switch `src-tauri/src` + `src`、zcode-switcher、orca 均无 IDE provider 级集成。

> **⚠️ 审计纠正（重要）**：原文写"**上游没人做**"属**推断**，只由 4 个本地仓库的 grep 支撑。而 §8 第 1 条又自认"Cursor/VS Code 是否存在稳定可写的 provider 入口**未验证**"。即**排第 2 的 Phase 2 建立在一个自认未验证的前提上**。
> **修正表述**：**"这四家都没做 IDE provider 切换"是已验证事实；"生态上无人做"是未验证推断。** IDE 方向的增量价值仍显著高于其他方向，但应在 Phase 0 完成后先做技术预研再定投入。

---

## 3. 架构方案

### 3.1 目标形态

```
┌──────────────────────────────────────────────────────────────┐
│  cc-switch（唯一内核）                                         │
│  ├─ 切换引擎（等上游 key-field 重构合入后再定形态）            │
│  ├─ SQLite SSOT                                               │
│  ├─ 写盘引擎（toml_edit + 原子写 + per-app write_strategy）     │
│  ├─ CLI: `ccs`（新增 bin target）                              │
│  └─ 插件槽位                                                     │
│      ├─ zcode-quota-guard   （来自 zcode-switcher）           │
│      └─ ide-provider-bridge （新建，填补空白）                │
└──────────────────────────────────────────────────────────────┘
         ▲                        ▲
         │ 知识固化                  │ 数据互通
┌────────┴─────────┐      ┌───────┴────────┐
│  orca            │      │ zcode-switcher │
│  （不复用写盘）    │      │ codex-tools    │
└──────────────────┘      └────────────────┘
```

### 3.2 🔴 核心改造：先等上游 key-field 重构（阻塞项）

**这是本文档最重要的技术判断，且是首版遗漏的。**

upstream 待合入的 22 个 commit 里包含一个**架构级重构**，直接冲击 §3.2 原方案的存在理由：

| commit | 内容 |
|--------|------|
| `81df5a08` | `feat: add a key-field write engine for client config files` |
| `63ed5002` / `0e6430ab` / `fb564537` / `143bc461` | Claude / Codex / Gemini+Grok **改为只替换 key 字段** |
| `15c0b3ce` | `feat(proxy): keep a per-app direct/proxy mode **instead of backing up live files**` |
| `bcb83d7c` | `test: add golden tests that lock live-config behavior **before the switching refactor**` |
| `b0875f4c` / `a79d9ff1` | live-write 修复：外部编辑中断写入的向前推进 |
| `69ff69dd` | `refactor: share editor, write and lookup helpers across apps` |

**含义**：
1. upstream **正在把 `write_live_snapshot` 的大 match 拆掉**——这正是原文 `AppAdapter` trait 想做的事。**如果自行设计 trait，会与上游重构正面冲突。**
2. upstream **主动删除了代理路径的 live 文件备份**（"No backups"，改用 `live-state.json` 记录 direct/proxy 模式）。本地 fork 的 `proxy_live_backup` 机制会与之冲突。
3. `bcb83d7c` 的 golden tests 是**上游自己为重构设的行为锁**——本地 fork 的 inert-merge（`live.rs:711-843`）若与之语义不符，会在合入时红。

**因此方案调整为**：

```
Phase 0  同步 upstream 22 个 commit + 裁决 inert-merge vs key-field engine 的冲突
         → 重新评估 AppType 抽象的最终形态（上游很可能已部分完成）
Phase 1+ 基于合入后的真实结构设计 ZCode / IDE 适配
```

**不做的事**：不要在合入上游之前动手写 `AppAdapter` trait。

### 3.3 ZCode 接入方案（Phase 1）

**为什么优先**：zcode-switcher 是三者中唯一有"别人完全没有的能力"（CDP 热切换 + 配额熔断），且 ZCode 在 cc-switch 里完全缺席。

**命名**：`AppType::Zcode`。**注意与 `AppType::Mcode`（MiniMax Code）严格区分**——建议枚举值用 `zai-code` 或 `zcode`，文档与代码注释必须显式警告此混淆风险。

**接入前必须先定的设计问题（原文遗漏）**：**ZCode 属于 switch 模式还是 additive 模式？**
`is_additive_mode()`（`app_config.rs:435-440`）决定它走 `write_live_snapshot` 还是 `opencode_config::set_typed_provider`。
- 若 additive → 无备份机制（见 §1.4 澄清）
- 若 switch → 需处理 ZCode 特有的原地写要求

**配置映射**：

| 项 | 值 | 依据 |
|---|---|---|
| 设置目录（setting.json） | `home/.zcode/v2`（**永远 home 基址**） | `profile.rs:86-89` |
| 数据目录（credentials/config） | `dataBaseDir()/.zcode/v2`，**四级回退** | `profile.rs:53-73` |
| OAuth | `zcode.z.ai/api/v1/oauth/token` | `oauth_cli.rs:10-11` |
| 模型 | `glm-5.3` / `glm-5.3-flash` | `src/lib/glm52.ts:5` |
| 凭据加密 | `enc:v1:` AES-256-GCM（不可改格式） | `crypto.rs:1-58` |

**写盘特殊要求（必须照搬）**：
1. `config.json` **原地 truncate 写**而非 rename——ZCode 用 fs/watch，rename 换 inode 会漏事件（`profile.rs:675-691`）。
   > **⚠️ 这与 cc-switch 的原子写策略直接冲突，且原地 truncate 崩溃即残缺**（源码注释自承）。而 cc-switch 的回滚机制**只在代理路径生效**（§1.4）。**Phase 1 需要先补一个 per-app 写前快照机制**，否则"崩溃可恢复"无从谈起。这不是 R3 那一行"🟡 中"能盖住的。
2. 写盘前备份到 `~/.zcode/v2/account-backups/`（`profile.rs:993-1018`）。
3. OAuth 切换后删除 `coding-plan-cache.json` 强制刷新权益（`profile.rs:1494-1501`）。

### 3.4 IDE 支持方案（Phase 2）

**技术现实**：VS Code 官方没有"模型供应商切换"概念（Copilot 走自有认证）；真正可切的是第三方扩展/provider 配置。Cursor 有 `~/.cursor/mcp.json`，但格式非公开、无稳定 schema。

**分两级**：
- **L1（可靠）**：IDE 侧 **MCP server 一键切换**。cc-switch 已有 MCP 子系统（`src-tauri/src/mcp/`，2,674 行）+ 深链（3,179 行）。
  > **⚠️ 动手前必查（审计发现）**：orca 已有 `.mcp.json` 的写路径（`McpConfigSection.tsx:193`，且带注释说明为何不猜 per-agent 目录布局）。**需先确认 cc-switch 的 MCP 子系统为何只覆盖 10 个 app-type 而无 project-scoped 输出**，否则会重复造 orca 已有的东西。
- **L2（探索）**：直接写 Cursor provider 配置。**必须先做逆向验证**（读本机 `~/Library/Application Support/Cursor/User/settings.json` 与 `~/.cursor/` 实际结构），确认存在稳定可写入口后再动手。**不要凭想象写代码。**

**可复用**：codex-tools 的 IDE 检测/重启逻辑（`editor_apps.rs:20-114`，MIT 兼容）——检测已安装 IDE + 220ms settle + 重启。

### 3.5 orca 整合方案（Phase 3）

**不要整合代码，整合知识。**

可提取的纯函数层（零 Electron/store/i18n 耦合）：`applyProviderToClaudeSettings`、`applyProviderToOpenCodeConfig`、`renderCodexConfigForProvider`、`provider-preset-types.ts` 类型。**但它们都不如 cc-switch 的实现**（无备份、正则 vs toml_edit、无校验），抽过来是降级。

**真正价值是踩坑知识**——但**审计发现 4 条红线里 3 条 cc-switch 早已内建**：

| 坑 | cc-switch 是否已有 | 证据 |
|---|---|---|
| `requires_openai_auth = true` 劫持 ChatGPT 登录态 → 401 | ✅ 已有 keyless 安全闸 | `src/config/codexProviderPresets.ts:71`「会被后端 keyless 安全闸拒绝切换」 |
| `api_key` 非 schema 字段，应为 `experimental_bearer_token` | ✅ 已有专用抽取函数+注释 | `codex_config.rs:3012-3040` |
| ClaudeCode env 无 `${VAR}` 展开 | ❓ 未见对应处理 | — |
| OpenCode 需 `options.baseURL` + `models` map | ⚠️ baseURL 有校验，**models map 未见** | `services/stream_check.rs:333-356` |

**因此 Phase 3 缩小为**：只补 1–2 条真缺口（重点查 OpenCode `models` map），其余转为 cc-switch 内的注释 + 契约测试。

**契约测试标准**：把 cc-switch 生成的配置**喂给真 CLI**（`codex` / `claude` / `opencode`），断言它们认不认——**不是字符串包含断言**。orca 现有测试全是 `expect(written).toContain(...)` 这类自证式断言，不能作为参考标准。

**时序约束**：orca preset 层正在被并发重构（20 文件未提交），**必须等 v4 落地后再启动**。

### 3.6 CLI 方案

cc-switch 与 zcode-switcher **都没有 CLI 二进制**。这是 codex-tools 的真优势，值得移植。

新增 `[[bin]] name = "ccs"`：
```
ccs list                      # 列出所有 app-type 及其 provider
ccs switch <app> <provider>   # 一键切换（核心诉求：简捷）
ccs current                   # 显示各 app 当前生效 provider
ccs quota <app>               # 配额
ccs --json                    # 机器可读
```
**必须支持 `--json`**：codex-tools 的 `ctc` 已有 `switch --best`（`src/utils/accountRanking.ts`），说明脚本化切换有真实需求。

---

## 4. 与 codex-tools 的关系：明确"不合并协议网关"

**建议：不把 `proxy_service.rs`（12,732 行）搬进 cc-switch。**

1. **功能重叠**：cc-switch `src-tauri/src/proxy/` 已有 77,471 行 / 68 文件，且已实现 Codex 与 Anthropic 协议转换。两个 Codex 网关并存比一个更糟。
2. **耦合 Codex 版本**：UA 伪装 `0.153.4`（`proxy_service/model_catalog.rs:3-5`），上游升版即断。
3. **不建议移植**：`state_5.sqlite` 直接 SQL UPDATE（`provider_sync/mod.rs:662-723`）、rollout JSONL 改写——私有 DB 无版本兼容承诺。

**建议保留的价值**：`ctc` CLI 交互设计、托盘配额渲染（`tray_visual.rs` 1,731 行 / `windows_taskbar_widget.rs` 1,877 行）、账号 warm-up 与 session 亲和负载均衡的**设计思路**。

**codex-tools 最终定位**：**独立发行，数据互通**。cc-switch 可导入其 `accounts.json`，但不接管其运行。

---

## 5. 风险登记

| # | 风险 | 等级 | 缓解 |
|---|------|:---:|------|
| R1 | **upstream key-field 重构冲击本地 fork 的 inert-merge，语义冲突** | 🔴 阻塞 | **Phase 0 首要任务**；不完成则 §3.2 全部作废 |
| R2 | upstream `15c0b3ce` 删除 live 备份，与本地 `proxy_live_backup` 冲突 | 🔴 高 | Phase 0 一并裁决 |
| R3 | `AppType` trait 化涉及 1,424 处引用 | 🔴 高 | **等上游重构后再评估**，勿自研 |
| R4 | ZCode 原地 truncate 写崩溃即残缺，且无现成回滚机制可挂靠 | 🔴 高 | Phase 1 需**先补 per-app 写前快照** |
| R5 | orca 与 cc-switch fork 用相同端点 + 重叠 model id，会静默漂移 | 🟡 中 | Phase 3 建 SSOT + 同步检查 |
| R6 | zcode-switcher 无 LICENSE，并入 MIT 项目后再开源有授权问题 | 🟡 中 | 对外发布前补 MIT 或重新授权 |
| R7 | ZCode 模型/套餐名硬编码子串匹配 | 🟡 中 | 迁入时改为配置项 |
| R8 | zcode-switcher 的 `proxy.rs`/`zcode_cdp.rs` 零测试覆盖 | 🟡 中 | 移植时**必须补测试** |
| R9 | fork 领先 13 个 commit 中 5 个是 kaixuan 专属定制，上游不会接受 | 🟡 中 | 每合一次上游要先剥离一次定制，需计入长期成本 |
| R10 | TS union 少 3 variant 是**有意的语义折叠**，不能当 bug 修 | 🟡 中 | registry 化时显式建模折叠关系 |
| R11 | IDE provider 配置无公开 schema | 🟢 低-中 | L2 先实测再决定，不承诺 |
| R12 | 三方枚举漂移（10/9/7） | 🟢 低 | registry 化后自然收敛 |

---

## 6. 分阶段执行计划

### Phase 0：上游同步与冲突裁决（阻塞项）
1. **先跑一次 `cargo test`**，确认 cc-switch 当前测试基线（本文档未验证）
2. 读 upstream 22 个 commit 的 diff，重点 `live.rs` / `codex_config.rs`
3. 裁决三件事：
   - inert-merge（`live.rs:711-843`）是否仍需要，还是 upstream 已用更好方式解决
   - `proxy_live_backup` 与 upstream 的 `live-state.json` direct/proxy 模式如何取舍
   - key-field engine 是否已部分实现了 §3.2 想要的抽象
4. 合入上游，剥离 5 个 kaixuan 专属定制
5. **基于合入后的真实结构，重新评估 §3.2 的抽象方案**
**门槛**：不通过则整个方案暂停

### Phase 1：ZCode 作为第 11 个 app-type
**前置**：补 per-app 写前快照机制（R4）
**产出**：`AppType::Zcode` + 适配器 + 集成测试 + 四级目录回退完整复刻
**验收**：
- cc-switch 内可见 ZCode 账号/套餐
- 切换后 ZCode **无需重启**即生效（CDP 路径通）
- 写入中断可从快照还原

### Phase 2：IDE 维度（L1）
**前置**：先查 cc-switch MCP 子系统为何无 project-scoped 输出（§3.4）
**产出**：`AppType::VsCode`/`Cursor` 的 MCP 适配器 + 编辑器重启（复用 codex-tools）
**验收**：检测已安装 IDE，一键切换其 MCP 配置并重启生效

### Phase 3：orca 知识固化（缩小版）
**前置**：orca v4 提交落地
**产出**：OpenCode `models` map 缺口修补 + 真 CLI 契约测试
**验收**：cc-switch 生成的配置喂给真 `codex`/`claude`/`opencode`，三者均接受

### Phase 4（可选）：配额熔断与网关
`zcode-quota-guard` 插件；Codex 网关**不做合并**。前置：Phase 1 稳定运行后。

---

## 7. 明确不建议的方案

| 方案 | 为什么否决 |
|------|-----------|
| **新建 unified-provider-switcher 仓库** | 搬运 273k 行 Rust；混合三种设计哲学；从此背三份上游同步负担。cc-switch 已是超集 |
| **把 cc-switch 合进 zcode-switcher** | 体量倒挂 22 倍；后者无 SQLite、无 MCP、无测试基建、无 IDE 能力 |
| **把 zcode-switcher 合进 codex-tools** | 后者 Codex 专用（`README.md:3`），无 Gemini/OpenCode/MiniMax Code，方向不对 |
| **直接复用 orca 的 preset 写盘** | 无备份 + 正则改 TOML + JSON 解析失败丢整个文件，比 cc-switch 现有实现更差 |
| **自研 `AppAdapter` trait** | **upstream 正在做同一件事**（key-field engine），会正面冲突 |
| **移植 `state_5.sqlite` 直接改写** | Codex 私有 DB，无版本兼容承诺 |
| **现在去动 orca** | 20 文件未提交 + 并发会话正在写 |
| **把 TS union 的 3 个缺项当 bug 修** | 是有意的语义折叠，会改坏设计决策 |

---

## 8. 待验证事项（实施前必须实测）

以下均为**推断**，不是已验证事实：

1. **cc-switch `cargo test` 当前是否全绿** —— **本次分析未运行任何测试**。§2.2 的"基础设施完备度"判断依赖于此。
2. **upstream 22 个 commit 与本地 fork 的实际冲突范围** —— 仅读 commit 标题与部分 diff，未逐行审计。
3. **Cursor / VS Code 是否存在稳定可写的 provider 配置入口** —— 未验证。
4. **cc-switch MCP 子系统为何无 project-scoped 输出** —— 未查。
5. **ZCode 客户端当前版本** —— 基于 `zcode.z.ai` 端点与 `app_version 3.11.2`（`captcha.rs:11`）；若 ZCode 已改 OAuth 端点需重新确认（`docs/changelog.md` 1.1.8 曾记录旧端点失效）。
6. **ZCode 在 macOS 下的完整路径行为** —— `zcode_launcher/macos.rs`（387 行）未实测。
7. **"90+ 预设"** —— 首版引自 README，本次审计未找到计数出处，已从判断中移除依赖。

---

## 9. 附录：关键文件索引

### cc-switch（内核）
| 主题 | 路径 |
|------|------|
| AppType 枚举 | `src-tauri/src/app_config.rs:396-490` |
| 写盘分派 | `src-tauri/src/services/provider/live.rs:1587-1748` |
| 切换入口 | `src-tauri/src/services/provider/mod.rs:5712` |
| DB schema / 迁移 | `src-tauri/src/database/schema.rs:27-580` |
| 原子写 | `src-tauri/src/config.rs:383-392` |
| live 备份（仅代理路径） | `src-tauri/src/database/dao/proxy.rs:788-797` |
| mcode 配置 | `src-tauri/src/mcode_config.rs:12-61` |
| Codex 惰性表合并 | `src-tauri/src/services/provider/live.rs:711-843` |
| TS union（含折叠说明） | `src/types/usage.ts:176-199` |

### zcode-switcher（待移植）
| 主题 | 路径 |
|------|------|
| 四级目录回退 | `src-tauri/src/profile.rs:53-73` |
| CDP 热切换 | `src-tauri/src/zcode_cdp.rs:29-282` |
| 配额熔断网关 | `src-tauri/src/proxy.rs:425-905` |
| 凭据加密 | `src-tauri/src/crypto.rs:1-58` |
| 原地写策略 | `src-tauri/src/profile.rs:675-691` |
| 配额探测 | `src-tauri/src/quota.rs:106-260` |

### codex-tools（部分移植）
| 主题 | 路径 |
|------|------|
| CLI | `src-tauri/src/command_line.rs:40-1573` |
| IDE 检测/重启 | `src-tauri/src/editor_apps.rs:20-114` |
| 托盘渲染 | `src-tauri/src/tray_visual.rs`（1,731 行） |
| Codex UA 伪装 | `src-tauri/src/proxy_service/model_catalog.rs:3-5` |

### orca（知识固化）
| 主题 | 路径 |
|------|------|
| preset 类型 | `src/shared/provider-preset-types.ts:34-108` |
| Codex 应用 | `src/main/codex/codex-apply-provider-preset.ts:46-240` |
| Claude 应用 | `src/main/claude/claude-apply-provider-preset.ts:52-142` |
| OpenCode 应用 | `src/main/opencode/opencode-apply-provider-preset.ts:41-157` |
| IPC handler | `src/main/ipc/provider-preset-handlers.ts:113-218` |

---

## 10. v2 修订记录

本文档经独立审计后修订，以下为**实质性更正**：

| # | 首版说法 | 实际 | 性质 |
|---|---------|------|------|
| 1 | fork「22 ahead / 12 behind」 | **22 behind / 13 ahead**（方向相反） | 🔴 事实错误 |
| 2 | 工作区 14 个未提交文件 | **已清空**（并发会话已提交） | 🔴 事实错误 |
| 3 | 「cc-switch 每次写盘前备份」 | 仅 `~/.cc-switch/config.json` 自备份；live 备份**只在代理路径**，upstream `15c0b3ce` 更主动删除 | 🔴 事实错误 |
| 4 | 自研 `AppAdapter` trait | **upstream 正在做同一件事**（`81df5a08` key-field engine），会冲突 | 🔴 方案缺陷 |
| 5 | 「上游没人做 IDE」 | 仅 4 家本地 grep 支撑；且与 §8 自认未验证矛盾 | 🟡 过度断言 |
| 6 | 三家都是 React 19 | cc-switch 是 **React 18.2** | 🟡 事实错误 |
| 7 | 配置目录 `~/.zcode/v2/` | **四级回退**，写死会重现"任何账号都切不了"已修 bug | 🟡 事实错误 |
| 8 | orca 4 条红线需固化 | **3 条 cc-switch 已内建**，Phase 3 应缩小 | 🟡 重复劳动 |
| 9 | Windows Store 路径版本钉死 | **生产函数版本无关**；版本字面量在测试内 | 🟡 论据方向反 |
| 10 | TS union 缺 3 variant 是漂移 | **有意的语义折叠**（`usage.ts:176-192` 有注释说明） | 🟡 会改坏设计 |
| 11 | `proxy_service.rs:157-158` 是 UA 伪装 | 实际在 `proxy_service/model_catalog.rs:3-5` | 🟡 引用错误 |
| 12 | 269 处 AppType 引用是「match 分支」 | 全仓 **1,424 处**引用，绝大多数是表达式非分支 | 🟡 低估 blast radius |
| 13 | ZCode 落地模式未讨论 | 需先定 switch vs additive（决定有无备份） | 🟡 方案缺口 |
| 14 | 「90+ 预设」 | 无计数出处，已从判断中移除依赖 | 🟢 未证实 |

---

**文档结束**。基于 2026-09-28 快照，**未执行任何构建或测试**。动手前请先完成 Phase 0。
