# 2026-09-16 修复记录：低额度自动切换改为“先切套餐、后切账号”（1.1.14）

## 现象

1.1.12 起的低额度自动切换只切账号：当前账号两个套餐入口（Start Plan
token 桶 / GLM Coding Plan 积分桶）只要其中之一低于阈值就切到另一个账号，
导致明明 Coding Plan 积分还有 800/2000、Start Plan 今日 token 已用完
的场景下被强制切账号（白白消耗新账号的 5 小时积分窗口）。

2. 切套餐（切换 `modelProviderFamilySelectedKeys[family]`）通过直接写
   `setting.json` 实现，但实测 ZCode 3.11.x 渲染层不监听该文件外部修改，
   当前运行中的 ZCode 不会切到新套餐，只能等重启；用户体验差。

## 根因

1. 切换判定的粒度过粗：把“单套餐入口低”混同于“整账号不可用”，需要拆开。
2. ZCode 设置切换有两套等价路径：渲染层 `settingService.update({...})`（与
   UI 点击同一条路径，内存即落盘、运行中即时生效）与直接改
   `setting.json`（仅下次启动生效）。1.1.12 走的是后者。

## 修复内容（1.1.14）

1. **后端新增套餐入口切换**
   - `src-tauri/src/profile.rs`
     - `plan_selected_key_for_family`：把 `start-plan` / `coding-plan`
       两种入口名拼接成 `coding-plan:builtin:<family>-<entry>` 形式的
       selected key（与 ZCode 自身命名一致），并拒绝其它目标。
     - `merge_selected_provider_key`：纯函数版 `setting.json` 合并，仅
       更新 `modelProviderFamilySelectedKeys[family]`，其它键与其它
       family 原样保留；已是目标值时返回 `None`（不写盘）；损坏 JSON
       时返回 `None`（宁可不切，也不覆盖用户配置）。
     - `ensure_start_plan_entry_credentials`：切到 `start-plan` 入口
       时，若该入口无 API Key，把当前 credentials 里的
       `zcodejwttoken` 写进去（与切号流程同语义）。
     - `switch_plan_internal`：切换到 Start Plan 时确保当前 credentials 里的
       `zcodejwttoken` 已写入入口；随后走两条切换路径：
       1. **CDP 实时通道**（`zcode_cdp::try_update_selected_provider`）：
          通过渲染层 `settingService.update({...})` 让运行中的 ZCode
          立即路由到新套餐入口；
       2. **文件写入兜底**：patch `setting.json`，配
          `applied_live=false`，提示“需重启 ZCode 生效”。
       Coding Plan 不以 config.json 的 API Key 快照作前置拒绝条件：该
       Key 由 ZCode 在切换后按当前 OAuth 权益刷新，旧/过期快照不能用来
       判断套餐是否实际可用。
   - `src-tauri/src/zcode_cdp.rs`
     - 新增 `try_update_selected_provider`：从 React fiber 树定位
       携带 `settingService.get/update` 的节点，缓存到
       `window.__zcsSettingsRegistry`；命中后调用
       `svc.update({modelProviderFamilySelectedKeys:{...}})` 并读回
       验证，匹配才返回成功。找不到服务、版本不匹配、JS 异常时回
       `Err`，让调用方退回文件写入路径。
   - `src-tauri/src/proxy.rs`
     - 新增 `QUOTA_GUARD` 暂停标志与 `set_quota_guard /
       quota_guard_status` 命令；`/v1/messages` 在暂停时按
       `stream` 字段分流：非流式返回 HTTP 429 + `rate_limit_error`
       + 中文原因；流式返回 200 + SSE `event: error` 事件（Agent
       侧表现是干净的一次失败并停止）。
     - 抽出 `serve_on(port, gateway_key) -> (port, shutdown)` 公共
       函数供 `start_proxy` 与 `--guard-selftest` 共用。
   - `src-tauri/src/lib.rs`
     - 新增 `--guard-selftest`（守护拦截链路真机自测）与
       `--plan-switch-probe <start-plan|coding-plan>`（套餐切换落
       盘验证）两条无窗口 CLI。
2. **前端重写额度守护与切换流程**
   - `src/lib/quotaGuard.ts`：把“百分比守护桶”拆成
     `entryOfBalance` / `entryHealth` / `currentPlanEntry` /
     `isCurrentEntryLow` / `pickPlanSwitchTarget` 五个语义函数；
     积分条目（`unit_type=point`）归 `coding-plan` 入口、监控
     token 桶（GLM-5.3/5.3-Flash）归 `start-plan` 入口、其它条目
     不参与判定。
   - `src/store.ts`
     - 新增 `glm52LowQuotaAction` 持久化字段（`switch` / `pause`），
       默认 `switch`。
     - `maybeSwitchGlm52Account` 重写：先判当前入口
       `isCurrentEntryLow`，再选动作——
       - `switch`：先 `pickPlanSwitchTarget` 找另一个有余额的入口
         切（CDP 优先，兜底写文件；切了之后立即刷新一次当前账
         号的额度），失败进 10 分钟长冷却；同账号内所有套餐都低
         才切账号（候选账号要求其已知的 token/积分量纲全部高于
         阈值，避免在耗尽账号之间循环）。
       - `pause`：把暂停标志同步到后端网关，停止自动切换，仅
         发一次 toast 提醒。
     - 切号 / 关闭自动切换 / 切回 `switch` 模式时都会清掉网关拦
       截标志，避免守护状态残留。
   - `src/components/SettingsPanel.tsx`：新增“低额度时动作”下拉
     框（自动切换套餐/账号、暂停执行）。
   - `src/lib/api.ts`：新增 `switch_plan` / `set_quota_guard` /
     `quota_guard_status` / `QuotaGuardStatus` / `PlanEntryTarget`
     / `PlanSwitchOutcome` 类型与命令。
3. **多语言文案**：zh / en / ru 新增 12 个 key（含 `glmGuardPaused`
   / `glmPlanSwitchedLive` / `glmPlanSwitchedNeedRestart` 等）。
4. **README + docs/usage.md**：特性列表与使用说明同步描述“先切套
   餐、后切账号”和“暂停执行”两种模式；特别强调暂停模式仅在
   本地网关运行且 ZCode 走网关时拦截请求，ZCode 直连时仍会停
   止自动切换并弹窗提醒。

## 审计修正（提交前发现）

- **store.ts 暂停模式误改全局 `autoSwitchPaused`**：`autoSwitchPaused`
  字段语义是“所有账号 Token/积分均低于阈值”（与 d2cf2c9 改的
  `glmAutoSwitchPaused` 文案对齐）。暂停模式是用户显式选择的另
  一种动作，用 `setAutoSwitchPaused(true)` 表达会污染全局 UI 文
  案。修正：暂停模式仅通过网关拦截 + toast 表达暂停，不调用
  `setAutoSwitchPaused`。
- **quotaGuard.ts `isCurrentEntryLow` 漏掉“当前入口无数据”回退**：
  当账号只订阅了 Coding Plan（无 token 桶）但当前入口是
  `start-plan` 时，`entryHealth(...).evaluable=false` → `.low=false`
  → `isCurrentEntryLow` 误判为“充足”，自动切换失灵。修正：
  `evaluable=false` 时回退到 `isAccountLow` 整账号判定，与
  `currentPlanEntry=null` 的回退语义一致。

## 验证

- `cargo test --lib`：通过（含 `plan_selected_key_builds_entry_and_validates_target`
  / `merge_selected_provider_key_merges_and_is_idempotent` 等新增
  单测）。
- `tsc --noEmit` + `npm run build`：通过。
- 真机：账号 pcwelcl0（Coding Lite）下，`--quota-probe` 验证
  Start Plan 入口 5h+周积分桶正确返回，quota/limit 数据未变。
