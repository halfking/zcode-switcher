# 实现验证清单（1.1.17 低额度切换余额复核：并发/超时/进度）

> 本文件在 2026-09-17 审计时重写：旧版引用的 `shouldSwitchEntry` /
> `tryAnotherEntry` / `findSwitchableCandidate` / `AutoSwitcherCard.tsx` /
> `i18n/locales/*.json` 均与实际代码不符，已按真实实现修正。
> 1.1.17 迭代：候选验证从串行改为限并发 + 单候选超时，进度暴露到 UI。

## 代码实现确认

### 1. 目标余额强制复核（切套餐 + 切账号）✅

**实现位置**：`src/store.ts` `maybeSwitchGlm52Account`（store 级布线）
+ `src/lib/quotaGuard.ts` `firstVerifiedTarget`（验证助手，支持并发/超时/进度）
+ `src/lib/glm52.ts` `isSwitchableCandidate`（双阈值合格判定）

- 切套餐：执行 `switch_plan` 前重新拉取当前账号最新余额（`refreshQuota`），
  复核当前入口仍低、且目标入口复核仍通过才执行；复核发现余额恢复则不动，
  目标复核不合格则落入切账号流程。
- 切账号：候选先用缓存余量排出优先级顺序（`accountHeadroom` 降序），然后
  用最新余额复核——限并发（默认 3）、单候选限时（默认 15 秒，超时/拉取
  失败按不合格跳过），胜者恒为"验证通过里优先级最高"的账号（排在前面的
  候选未出结果时，后面的先通过也要等）；全部不通过则进入自动暂停
  （`autoSwitchPaused`），不做任何切换。
- 验证进度：`switchVerifyProgress`（total/done/failed/active 账号名）实时
  更新，主界面在验证期间显示横幅（i18n `glmVerifyProgress`），结束即清空。
- 后端数据源：`profile.rs` 的 `fetch_quota(Some(id))` 读取**目标档案自己的
  凭据副本**查询余额，保证验证的是目标账号的真实余额。

### 2. 套餐内切换优先 ✅

**实现位置**：`src/store.ts`（第 1 步）+ `src/lib/quotaGuard.ts`
`pickPlanSwitchTarget` / `isCurrentEntryLow` / `entryHealth`

当前入口低于阈值时先切同账号的另一个入口（Start Plan ↔ Coding Plan）；
入口不可识别（`active_provider` 缺失/非法）或另一入口不可评估时跳过切套餐，
直接进入切账号判定。

### 3. 暂停执行模式 ✅

**实现位置**：`src/store.ts`（`glm52LowQuotaAction === "pause"` 分支）

低于阈值时 `setQuotaGuard(true)` 让本地网关拦截模型请求并停止一切自动切换；
配置持久化在 localStorage `zcs:glm52LowQuotaAction`，默认 `"switch"`。
切换模式下复核不合格进入的是 `autoSwitchPaused`（语义："所有账号均低于
阈值"），与网关拦截的暂停模式相互独立。

### 4. 关键护栏 ✅

- `isSwitchableCandidate`：候选已知的每个量纲必须**严格高于**阈值且至少
  一个量纲有数据；无数据/拉取出错的账号一律不合格（保守不切）。
- `switch_plan` 抛错（典型：coding-plan 入口缺 API Key）视同该入口耗尽，
  落入切账号流程；失败后进入 10 分钟长冷却。
- 套餐切换冷却时间戳只在**真正执行** `switchPlan` 时记录；复核轮次
  （目标已耗尽/余额已恢复）不消耗冷却（2026-09-17 审计修复）。
- 执行切换前检查 `busy`，验证期间用户开始手动操作则不抢动作。

## 自动化回归

| 命令 | 覆盖 |
|---|---|
| `npm run test:quotaguard` | quotaGuard 纯函数 10 项：入口判定回退、pickPlanSwitchTarget、firstVerifiedTarget 串行/并发/超时/进度语义 |
| `npm run test:autoswitch` | store 布线 6 场景：S1 逐一验证选首个合格者、S2 全部不合格暂停、S3S4 复核不动+不消耗冷却、S5 切套餐失败落入切号、S6 并发上限+进度暴露+优先级不变、S7 挂死候选按超时跳过 |
| `npm run test:settings` | 设置面板 macOS launcher 开关（轮询等待，无偶发失败） |
| `npm run test:windows` | 实机回归（版本/会话/凭据加密形态/CDP；CDP 仅在 ZCode 由增强启动拉起时要求，`-RepairCdp` 一键修复） |

## 部署状态

- [x] 代码实现完成（含 2026-09-17 审计修复）
- [x] 自动化回归通过（见上）
- [x] 1.1.16+45 已构建并静默安装，实机回归 9/10（唯一 FAIL 为环境 CDP 状态）
- [x] 审计修复后重新构建部署 1.1.16+46：静默安装 + 实机回归 9/10（唯一 FAIL 为环境 CDP 状态）
- [x] 1.1.17：候选验证限并发+单候选超时+进度 UI；windows-regression 的 CDP 项改为条件要求并支持 `-RepairCdp`
