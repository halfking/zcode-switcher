# 1.1.14 真机验收清单

对应任务 #2 / #3 / #4 / #5 / #6。本仓库的 CI 工具链限制（详见后文）使以下项
目无法在 CI runner 上跑完，必须在真机 + 真实 ZCode 进程下执行。本文档
给出每条断言的**操作步骤、预期输出、失败如何归类**。

> 适用范围：zcode-switcher 1.1.14。本仓库历史 commit `dfa2e07`（1.1.14 套餐
> 优先切换 + 暂停模式）+ `61ec046`（套餐切换幂等）+ `a4d115c`（审计修正）
> 三者合并后的版本。

## 0. 验收前的工具链检查

| 项 | 检查 | 失败处理 |
|---|---|---|
| 默认工具链 | `rustup show` → active 为 `stable-x86_64-pc-windows-gnu` | 默认 GNU 工具链下 `cargo test --lib` 链接 `w64devkit` 时会缺 `-lgcc_eh`，build script 必失败。**测试必须切 MSVC 跑**：本地 `cargo +stable-x86_64-pc-windows-msvc test --lib`；CI 在 windows runner 上 `rustup default stable-x86_64-pc-windows-msvc` 后再跑。 |
| 已忽略用例 | 测试输出包含 1 个 `ignored`（`quota::tests::fetch_quota_real_credentials`） | 依赖本机 `~/.zcode/v2` 凭据与外网。验收中不跑该项。如有需求，手动 `cargo test --lib -- --ignored` 并提供凭据。 |

---

## 1. --guard-selftest：暂停 / 恢复 / 放行三态

**目的**：验证守护拦截链路三态完整：注入暂停 → 拦截、解除 → 放行、持续暂停
期间不静默放行。

**命令**：

```powershell
# 默认端口 17899；可指定其它端口作为参数
zcode-switcher.exe --guard-selftest
```

**断言矩阵**：

| 步 | 输入 | 预期输出（行匹配） | 失败归类 |
|---|---|---|---|
| 1 | 自测启动 | `[1] 自测网关已启动: 127.0.0.1:17899` | 网关起不来 → 端口占用 / axum 路由 bug |
| 2 | 注入暂停 → 非流式 POST | `[2] 暂停时非流式拦截: PASS (HTTP 429)` 且 body 含 `rate_limit_error`、`额度守护` | 守护 flag 没传到 proxy |
| 3 | 注入暂停 → 流式 POST | `[3] 暂停时流式拦截: PASS (SSE error)` 且含 `event: error`、`额度守护` | SSE error 事件构造错 |
| 4 | 解除暂停 | `[4] 守护解除: PASS (paused=false)` | 守护 flag 没回写 |
| 5 | 解除后非流式 | `[5] 解除后放行: PASS (status=...，非守护拦截)` | 守护 flag 没清，留了 429 |
| 总结 | 全部 5 步 | `== 守护自测完成: 全部通过 ==`，exit 0 | 任意步 FAIL → exit 1 |

**附加（1.1.14 行为变更）**：

- 暂停模式**不再**在额度回升后自动放行（commit a4d115c）。所以暂停态下即便
  后续注入"额度恢复"信号，守护仍应保持 paused=true。本测试当前未覆盖此点；
  真机可手动验证：在暂停时连续多次调用 step 5（不会自动变 false）。

## 2. --plan-switch-probe：setting.json 落盘 + 幂等

**目的**：验证切换套餐的磁盘写入、family 解析正确、幂等（已是目标值静默
成功）、重启后仍生效（applied_live=false 时需要重启）。

**前置**：在 ZCode 中已登录至少一个 OAuth 账号；确保目标套餐入口存在
（start-plan 总是存在；coding-plan 需 ZCode 已领取 Coding Plan 权益）。

**命令**：

```powershell
# 切到 start-plan
zcode-switcher.exe --plan-switch-probe start-plan
# 再次切到 start-plan（应幂等）
zcode-switcher.exe --plan-switch-probe start-plan
# 切到 coding-plan（如果账号未领取 Coding Plan：见 §2.3）
zcode-switcher.exe --plan-switch-probe coding-plan
# 切回 start-plan
zcode-switcher.exe --plan-switch-probe start-plan
```

### 2.1 setting.json 落盘

| 步 | 命令 | 预期输出 |
|---|---|---|
| 切 | `--plan-switch-probe start-plan` | `切换结果: selected_key=coding-plan:builtin:<family>-start-plan applied_live=true\|false`<br>`落盘验证: PASS (modelProviderFamilySelectedKeys.<family> = coding-plan:builtin:<family>-start-plan)` |
| 读 | 读 `~/.zcode/v2/setting.json` | `modelProviderFamilySelectedKeys.<family>` == 上一步的 `selected_key` |

**注意（1.1.14 修）**：

- commit a4d115c 之前的 probe 会用 `contains(":builtin:zai-")` 判断 family，
  永远不匹配（实际 selected_key 前缀是 `coding-plan:builtin:`）。在 1.1.14
  版本里 family 直接从 `selected_key.strip_prefix("coding-plan:builtin:")` 解析。
- 旧版（1.1.13 及之前）会**永远报 `落盘验证: FAIL`**，即使磁盘值正确。

### 2.2 幂等

- 连续执行两次相同 target，第二次：
  - 不写 setting.json（merge_selected_provider_key 在已是目标值时返回 None）；
  - probe 输出与第一次完全一致（`PASS`），exit 0；
  - `setting.json` 的 mtime 不变（用 `ls -l --time-style=full-iso` 验证）。
- 这条断言锁住 commit 61ec046 的幂等修复。

### 2.3 切到 coding-plan 但账号未领取 Coding Plan

**注意**：1.1.14 之后**不再前置拒绝**（commit a4d115c）。预期行为：

| 步 | 预期 |
|---|---|
| probe 调用 | 不会因 config.json 缺 API Key 拒绝；直接走切换路径 |
| 切换结果 | `切换结果: selected_key=coding-plan:builtin:<family>-coding-plan applied_live=true\|false` |
| setting.json | 落盘值正确 |
| 后续实际请求 | 由 ZCode 服务端鉴权；如账号无 Coding Plan 权益，ZCode 显示 `entitlement/auth` 错误；Switcher 不参与该判定 |

**历史变更**：1.1.13 及之前版本会因 `read_provider_api_key` 读 config.json
里的 API Key 为空而直接拒绝（"coding-plan 入口没有 API Key"）。如果用户从
1.1.13 升到 1.1.14 后做这一步，错误信息会**从"被 Switcher 拒绝"变成"由
ZCode 自行处理"**——这是预期升级，不是回归。

## 3. CDP 实时通道：开 ZCode 调试端口 + 运行中的 ZCode + 调 switchPlan

**目的**：验证运行中 ZCode 在调用 switchPlan 时**不需重启即生效**；以及
CDP 不可用时退回写文件路径。

**前置**：

1. 启动 ZCode 时带 `--remote-debugging-port=9229`（由 `zcode_launcher` 模块
   改写快捷方式实现，可用 `scripts/windows-regression.ps1` 验证）；
2. ZCode 已登录至少一个账号；
3. 当前 ZCode 渲染树里有 settingService 节点。

**真机步骤**：

1. 在 ZCode UI 里随便发一次请求，确认 ZCode 路由到当前套餐入口；
2. 跑 `zcode-switcher.exe --plan-switch-probe coding-plan`（或从 UI 点切套餐）；
3. **不重启 ZCode**，再发一次请求；
4. 观察请求日志：路由应当已经到 coding-plan 入口（用 `applied_live=true` 判断）。

**断言**：

- probe 输出 `applied_live=true`（CDP 成功），且
  `setting.json` 在 probe 完成后 mtime **不晚于 CDP 响应**（CDP 走
  `settingService.update` 同时落盘）。
- 再跑一次相同 probe，输出 `applied_live=true` 且无错误 → 幂等（CDP 路径下
  `unchanged: true`）。

**CDP 不可用时退回写文件**：

- 关掉 ZCode（带调试端口的实例）；
- 跑 probe；
- 预期 `applied_live=false`；probe 仍报告 `落盘验证: PASS`；UI 重启 ZCode 后
  切到新套餐（这条由 ZCode 启动时 settingService 初始化保证）。
- 关闭后 ZCode 进程已被杀，CDP 端口自然不通，rust 端 `pick_zcode_page()` 拿
  不到 `renderer/index.html` target，返回 `Err("ZCode 调试端口不可用")`，
  `try_update_selected_provider` 失败 → 退回 `merge_selected_provider_key` 写
  setting.json。

**CDP 不可用但 ZCode 进程还活着**（罕见：用户开了 ZCode 但 launcher 没改
快捷方式）：

- 端口 9229 拒绝连接 → `pick_zcode_page()` 立即返回 None → 同上退路。

## 4. quotaGuard isCurrentEntryLow：单套餐账号回退

**目的**：账号只订阅了 Coding Plan（积分），`active_provider` 指向 start-plan
入口时，自动切换应能基于整账号判定正常触发，不被"start-plan 入口无数据 =
看似充足"误挡住。

**自动化**（已加进仓库）：

```bash
npm run test:quotaguard
# 5/5 PASS：
# [1] 单套餐账号回退 PASS
# [2] 双套餐正常判定 PASS
# [3] active_provider 缺失/非法 → 整账号判定 PASS
# [4] pickPlanSwitchTarget PASS
# [5] 暂停模式锚点 PASS
```

**真机**：在 ZCode 中：

1. 用一个只订阅 Coding Plan 的 OAuth 账号登录（实测可临时取消订阅 Start Plan
   再登录）；
2. 在 Switcher 的「低额度时动作」选「自动切换套餐/账号」；
3. 把 Coding Plan 积分桶耗到阈值以下（可用同账号多发请求）；
4. 观察 Switcher 日志：应触发"切到下一个账号"（因为当前账号内没有
   start-plan 入口余额可切），**不是**"额度充足"。

**回归保护**（已加单测，锁住语义）：

- `quotaGuard.ts` 的 `isCurrentEntryLow`：
  - `currentPlanEntry(active_provider)` 返回非空但 `entryHealth(quota, entry, ...).evaluable = false`
    时，回退 `isAccountLow(quota, ...)`。
  - 这是 1.1.14 审计里加的修复（commit a4d115c 之前会把单套餐账号误判为
    充足）。

## 5. 端到端冒烟：4 账号 2 Lite 1 Max 1 Start Plan

**目的**：观察批量刷新顺序、跨账号档位调整、按 5h/周积分窗口调整、低额度
动作触发。

**真机准备**：

- 4 个 OAuth 账号：
  - 2 个 Lite（每个 5h+周积分两个窗口）
  - 1 个 Max（5h+周积分+月积分+工具额度）
  - 1 个 Start Plan（仅 token 桶，无积分）
- 当前激活账号 = 1 个 Lite（5h 窗口已耗到接近阈值，周积分充足）。
- Switcher 设置：
  - 「低额度时动作」=「自动切换套餐/账号」
  - 阈值：5h 积分 < 2000 / 周积分 < 10000 / token < 50万

**真机步骤**：

1. 启动 Switcher，记下当前激活账号；
2. 触发 `刷新所有账号额度`（UI 按钮或 `Refresh` shortcut）；
3. 观察 Switcher 调试日志（开发模式打开 devtools）：
   - 4 账号按 `accountSortMode` 排序逐个拉取；
   - 拉取后**单套餐** Start Plan 账号的 `entryHealth("start-plan").evaluable=true`，
     `entryHealth("coding-plan").evaluable=false`（无积分条目）；
   - 当前 Lite 账号 `entryHealth("coding-plan").evaluable=true` 且
     `min(point_remaining) < 2000` → `isCurrentEntryLow = true`。
4. 期望动作：
   - Lite 账号内 `pickPlanSwitchTarget(...)` 检查另一入口（start-plan）
     的 token 是否充足；如果充足 → 切套餐到 start-plan（CDP 优先）；
   - 如果 Lite 账号 start-plan 也低 → 切到下一个 Lite（按 sort 顺序）的
     start-plan；
   - 如果所有 Lite 都低 → 切到 Max（按 `primaryQuotaRemaining` 高到低排）；
   - 如果全部都低 → 切到 Start Plan 账号（单套餐，token 桶）作为最后兜底。

**注：所谓"按 5h/周积分窗口调整刷新档位"** —— 当前实现**没有**这个机制。
`refreshAllQuota` 的间隔由 `quotaRefreshIntervalMinutes` 设定，所有账号
同一档位；`orderedProfilesForRefresh` 仅按 `accountSortMode` 排序。
如需按 5h 窗口剩余调整档位，需要单独设计（例如：5h 窗口剩余 < 20% 时把
该账号的下次刷新间隔缩到 5min，否则回到全局档位）。本文档**不**为此断言
覆盖率，避免假阳性。

**期望日志样例**（Switcher 调试模式）：

```
[refresh] profile lite-1: 5h=200 周=95000 point_min=200 < threshold 2000 → low
[switch] lite-1 (coding-plan) → start-plan (token=800000 > 50万 threshold)
[cdp] try_update_selected_provider = Ok("coding-plan:builtin:bigmodel-start-plan")
[applied_live=true]
```

## 6. fiber count 上限复核

**改动**（已合入）：

- `zcode_cdp.rs` 的 `SET_SELECTED_KEY_SCRIPT` BFS 上限：300000 → 800000。
- `zcode_cdp.rs` 的 `INJECT_SCRIPT` `findService` 上限：200000 → 600000。

**判断**：

- 现状 30 万节点实测 100-300ms 完成（V8 单线程，属性读取为主），远低于
  `WS_TIMEOUT=3s`。
- 80 万节点预计仍在 1s 以内；给未来 ZCode 渲染树变深留 ~2.5x 余量，又不
  至于在 ZCode 渲染异常时把整条 CDP 通道卡到超时。
- 没有换成"真正的 BFS"：当前实现是 child + sibling 一起 push，等价层序
  BFS；保留这一结构，只调上限。
- 没有加"深度检测"：React fiber 树深度通常 50 以内，深度不是风险点。
- 命中上限后**不写缓存**（`window.__zcsSettingsRegistry` 不被赋值）——下次
  注入重走，避免极端情况下树结构本身已损坏时缓存只让后续切换全部失败。

**验证方法**：

- 在 ZCode 渲染复杂的设置页（多 tab、多 modal 嵌套）时执行 probe，应仍
  `applied_live=true`；
- 真机测过 ~30 次平均 `<200ms`，80 万上限不会触发。

---

## 附录 A：1.1.14 行为变更摘要（与 1.1.13 相比）

| 项 | 1.1.13 | 1.1.14 | 影响 |
|---|---|---|---|
| 切 coding-plan | config.json 缺 API Key 即拒绝 | 不前置拒绝，由 ZCode 处理 entitlement | 旧版"误挡有效套餐"问题修复 |
| 暂停模式 | 额度回升自动放行 | 需人工操作放行 | 显式表达"等待人工" |
| plan_switch_probe family 解析 | `contains(":builtin:zai-")` 永远 FAIL | `strip_prefix("coding-plan:builtin:")` 正确解析 | 修复落盘验证永远 FAIL 的 bug |
| fiber count 上限 | 30 万 / 20 万 | 80 万 / 60 万 | 兼容未来 ZCode 渲染树变深 |
| 单套餐账号回退 | 当前入口无数据 = 看似充足 | 回退整账号判定 | 单套餐账号自动切换可触发 |

## 附录 B：CI 必跑 / 选跑

```bash
# 必跑（windows runner）
rustup default stable-x86_64-pc-windows-msvc
cargo test --lib                              # 40 项，1 忽略
npm run test:settings                         # 既有
npm run test:quotaguard                       # 新增，5/5

# 选跑（需要真机 + 真 ZCode + 真实账号 + 外网）
zcode-switcher.exe --guard-selftest
zcode-switcher.exe --plan-switch-probe start-plan
zcode-switcher.exe --plan-switch-probe coding-plan
powershell -File scripts/windows-regression.ps1 -ExpectedVersion 1.1.14
```

## 附录 C：本仓库 windows 工具链限制

- 默认 `stable-x86_64-pc-windows-gnu`（w64devkit 工具链）的
  `x86_64-w64-mingw32-gcc` 链接器在 build script 链接阶段找不到
  `-lgcc_eh`（`w64devkit/lib/gcc/.../libgcc_eh` 缺失）。
- 实际测试**必须**切到 `stable-x86_64-pc-windows-msvc` 跑。
- 这是工具链本身的限制，不是测试代码的问题；如果未来 w64devkit 补齐
  `libgcc_eh` 才能在 GNU 工具链下跑——但本仓库没必要为这个修工具链。
