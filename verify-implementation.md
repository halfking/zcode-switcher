# 实现验证清单（1.1.16 低额度切换余额复核）

> 本文件在 2026-09-17 审计时重写：旧版引用的 `shouldSwitchEntry` /
> `tryAnotherEntry` / `findSwitchableCandidate` / `AutoSwitcherCard.tsx` /
> `i18n/locales/*.json` 均与实际代码不符，已按真实实现修正。

## 代码实现确认

### 1. 目标余额强制复核（切套餐 + 切账号）✅

**实现位置**：`src/store.ts` `maybeSwitchGlm52Account`（store 级布线）
+ `src/lib/quotaGuard.ts` `firstVerifiedTarget`（逐一验证助手）
+ `src/lib/glm52.ts` `isSwitchableCandidate`（双阈值合格判定）

- 切套餐：执行 `switch_plan` 前重新拉取当前账号最新余额（`refreshQuota`），
  复核当前入口仍低、且目标入口复核仍通过才执行；复核发现余额恢复则不动，
  目标复核不合格则落入切账号流程。
- 切账号：候选先用缓存余量排出验证顺序（`accountHeadroom` 降序），然后
  逐一重新拉取最新余额复核，第一个通过双阈值的账号才成为目标；全部不通过
  则进入自动暂停（`autoSwitchPaused`），不做任何切换。
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
| `npm run test:quotaguard` | quotaGuard 纯函数 9 项：入口判定回退、pickPlanSwitchTarget、firstVerifiedTarget 逐一验证语义 |
| `npm run test:autoswitch` | store 布线 4 场景：S1 逐一验证选首个合格者、S2 全部不合格暂停、S3S4 复核不动+不消耗冷却、S5 切套餐失败落入切号 |
| `npm run test:settings` | 设置面板 macOS launcher 开关（轮询等待，无偶发失败） |
| `npm run test:windows` | 实机回归（版本/会话/凭据加密形态/CDP） |

## 部署状态

- [x] 代码实现完成（含 2026-09-17 审计修复）
- [x] 自动化回归通过（见上）
- [x] 1.1.16+45 已构建并静默安装，实机回归 9/10（唯一 FAIL 为环境 CDP 状态）
- [x] 审计修复后重新构建部署 1.1.16+46：静默安装 + 实机回归 9/10（唯一 FAIL 为环境 CDP 状态）
