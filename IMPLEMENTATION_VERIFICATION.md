# 实现验证报告

## 任务目标
优化切换的检查与方式：
1. 检查当前账号是否**所有**余额都到阈值（不能只是其中一个），因为可以在账号的各个套餐之间先切换，最后才是切换账号
2. 增加pause操作方式：除了切换账号和套餐外，还可以在余额低于阈值时暂停所有执行，等待人工操作
3. 在ZCode账户下的套餐之间切换

## 实现验证清单

### ✅ 需求1：账号内套餐优先切换，所有套餐都低才切账号

#### 1.1 套餐切换逻辑
**位置**: `src/store.ts:528-591`
```typescript
// 第 1 步：帐号内先切套餐
const target: PlanEntryTarget | null = pickPlanSwitchTarget(
  activeQuota,
  currentEntry,
  tokenWan,
  pointThreshold
);
if (target) {
  // 找到同账号其他高于阈值的套餐 → 切换套餐
  await api.switchPlan(target);
  // ...
  return; // 停止，不继续切账号
}
// 第 2 步：没找到 → 才切账号
```

**验证**: ✅ 逻辑正确，先尝试套餐切换，只有返回null才继续账号切换

#### 1.2 套餐选择算法
**位置**: `src/lib/quotaGuard.ts:pickPlanSwitchTarget()`
```typescript
export function pickPlanSwitchTarget(...): PlanEntryTarget | null {
  const acc = accounts.find(a => a.id === quota.account_id);
  if (!acc) return null;

  // 遍历同账号的所有套餐
  for (const candidate of acc.plans) {
    if (candidate.id === currentEntry.planId) continue; // 跳过当前套餐
    
    const tokenRemaining = primaryQuotaRemaining(candidate);
    const pointsRemaining = candidate.balancePoints ?? 0;
    
    // 检查是否高于阈值
    if (tokenRemaining >= tokenWan || pointsRemaining >= pointThreshold) {
      return { account: acc, plan: candidate }; // 找到目标
    }
  }
  
  return null; // 所有套餐都低于阈值
}
```

**验证**: ✅ 遍历所有套餐，只返回高于阈值的套餐，全部低才返回null

#### 1.3 ZCode套餐切换实现
**位置**: `src-tauri/src/profile.rs:1437-1499 switch_plan_internal()`
```rust
pub async fn switch_plan_internal(...) -> Result<(), String> {
  let mut params = json!({
    "modelProviderFamilySelectedKeys": {
      "chatglm": plan_key  // "glm-coding-5h" 或 "glm-coding-start"
    }
  });
  
  // 通过CDP调用ZCode的settingService.update()
  let result = zcode_cdp::call_method(
    &mut ws, 
    "settingService.update", 
    params
  ).await?;
  
  Ok(())
}
```

**验证**: ✅ 使用ZCode官方API `settingService.update()` 修改 `modelProviderFamilySelectedKeys`

#### 1.4 账号级余额判定
**位置**: `src/lib/glm52.ts:accountHeadroom()`
```typescript
export function accountHeadroom(acc: Account): number {
  let max = 0;
  acc.plans.forEach(p => {
    const remaining = primaryQuotaRemaining(p);
    if (remaining > max) max = remaining;
  });
  return max;
}
```

**验证**: ✅ 返回账号所有套餐的最大剩余量

---

### ✅ 需求2：增加pause动作模式

#### 2.1 配置类型定义
**位置**: `src/store.ts:42,54,94`
```typescript
export type LowQuotaAction = "switch" | "pause";
const DEFAULT_LOW_QUOTA_ACTION: LowQuotaAction = "switch";

// Store状态
glm52LowQuotaAction: LowQuotaAction;
```

**验证**: ✅ 定义了两种模式类型

#### 2.2 前端UI
**位置**: `src/components/SettingsPanel.tsx:555-573`
```tsx
<SettingGroup
  title={t.glmLowQuotaActionTitle}
  description={
    glm52LowQuotaAction === "pause"
      ? t.glmLowQuotaActionDescPause
      : t.glmLowQuotaActionDescSwitch
  }
>
  <select
    value={glm52LowQuotaAction}
    onChange={(e) => setGlm52LowQuotaAction(e.target.value as LowQuotaAction)}
  >
    <option value="switch">{t.glmLowQuotaActionSwitch}</option>
    <option value="pause">{t.glmLowQuotaActionPause}</option>
  </select>
</SettingGroup>
```

**验证**: ✅ UI下拉框允许选择switch/pause

#### 2.3 前端pause逻辑
**位置**: `src/store.ts:497-512`
```typescript
if (state.glm52LowQuotaAction === "pause") {
  if (!currentLow) {
    // 额度充足时不放行，等待人工操作
    return;
  }
  // 额度低 → 启用网关拦截
  void api.setQuotaGuard(true, t.glmGuardReason).catch(() => {});
  
  if (!guardPauseNotified) {
    guardPauseNotified = true;
    state.toast(t.glmGuardPaused, "warn");
  }
  return; // 不执行切换逻辑
}
```

**验证**: ✅ pause模式下调用网关拦截，不执行切换

#### 2.4 后端网关拦截
**位置**: `src-tauri/src/proxy.rs:660-676`
```rust
// 额度守护：暂停期间拦截所有模型请求
if let Some(reason) = quota_guard_is_paused() {
    let message = if reason.is_empty() {
        "额度守护已暂停：账号余额低于阈值，请人工切换账号/套餐或等待额度恢复。"
    } else {
        format!("额度守护已暂停：{}", reason)
    };
    
    let stream = /* 检查是否流式请求 */;
    
    return Err(provider_error_response(
        stream,
        StatusCode::TOO_MANY_REQUESTS,
        "rate_limit_error",
        message,
    ));
}
```

**验证**: ✅ 后端拦截所有请求，返回429错误

#### 2.5 持久化存储
**位置**: `src/store.ts:321-326,1038-1049`
```typescript
function loadGlm52LowQuotaAction(): LowQuotaAction {
  return localStorage.getItem("zcs:glm52LowQuotaAction") === "pause"
    ? "pause"
    : "switch";
}

setGlm52LowQuotaAction: (v) => {
  localStorage.setItem("zcs:glm52LowQuotaAction", v);
  set({ glm52LowQuotaAction: v });
}
```

**验证**: ✅ 配置持久化到localStorage

---

### ✅ 需求3：完整的切换流程

#### 3.1 切换触发条件
**位置**: `src/store.ts:493-496,522-526`
```typescript
const currentLow = isCurrentEntryLow(activeQuota, tokenWan, pointThreshold);

// 当前入口余额充足 → 无需动作
if (!currentLow) {
  if (state.autoSwitchPaused) state.setAutoSwitchPaused(false);
  return;
}
```

**验证**: ✅ 只在当前套餐入口低于阈值时触发

#### 3.2 完整切换流程
**位置**: `src/store.ts:528-627`
```typescript
// 第 1 步：账号内切套餐
const target = pickPlanSwitchTarget(...);
if (target) {
  // 冷却期检查
  if (Date.now() - lastPlanSwitchAttemptAt < cooldown) return;
  
  lastPlanSwitchAttemptAt = Date.now();
  glm52AutoSwitching = true;
  
  try {
    state.toast(t.glmAutoSwitchingPlan.replace("{plan}", targetPlanName));
    await api.switchPlan(target);
    lastPlanSwitchFailed = false;
    state.toast(t.glmPlanSwitched, "success");
  } catch (err) {
    lastPlanSwitchFailed = true;
    state.toast(t.glmPlanSwitchFailed, "error");
  } finally {
    glm52AutoSwitching = false;
  }
  return; // 成功或失败都停止，不继续切账号
}

// 第 2 步：切账号（所有套餐都低时）
const nextAcc = pickAccountTarget(...);
if (nextAcc) {
  await api.switchAccount(nextAcc);
} else {
  // 所有账号都低
  state.setAutoSwitchPaused(true);
}
```

**验证**: ✅ 三级决策：套餐切换 → 账号切换 → 全部暂停

---

## 国际化支持

**位置**: `src/i18n.ts:165-169,497-502,828-833`

中文：
- `glmLowQuotaActionTitle: "低额度时动作"`
- `glmLowQuotaActionSwitch: "自动切换套餐/账号"`
- `glmLowQuotaActionDescSwitch: "账号内先切换到有余额的套餐；所有套餐都低于阈值后才切换账号。"`
- `glmLowQuotaActionPause: "暂停执行"`
- `glmLowQuotaActionDescPause: "额度低于阈值时通过网关拦截所有请求；需人工切换账号/套餐后恢复。"`

英文、俄文翻译完整。

**验证**: ✅ 完整的多语言支持

---

## 测试场景

### 场景1：套餐切换优先
1. 账号A有两个套餐：coding-5h（当前使用，余额低）、coding-start（余额充足）
2. 触发检查
3. **预期**: 切换到coding-start，不切换账号

### 场景2：所有套餐都低才切账号
1. 账号A所有套餐都低于阈值
2. 账号B有余额
3. 触发检查
4. **预期**: 先尝试账号A内切换（失败），然后切换到账号B

### 场景3：pause模式
1. 设置"低额度时动作"为"暂停执行"
2. 当前套餐余额低于阈值
3. 发送模型请求
4. **预期**: 
   - 前端显示toast警告
   - 后端返回429错误
   - 不自动切换账号或套餐

### 场景4：switch模式（默认）
1. 设置"低额度时动作"为"自动切换套餐/账号"
2. 当前套餐余额低于阈值
3. **预期**: 按照套餐优先 → 账号的顺序自动切换

---

## 代码质量检查

### 类型安全
- ✅ TypeScript严格模式
- ✅ LowQuotaAction枚举类型
- ✅ PlanEntryTarget接口

### 错误处理
- ✅ 套餐切换失败有toast提示
- ✅ 切换失败后有冷却期重试机制
- ✅ CDP调用有错误捕获

### 用户体验
- ✅ 所有操作有toast通知
- ✅ UI有清晰的说明文案
- ✅ 配置持久化
- ✅ 冷却期防止频繁切换

---

## 手动测试步骤

由于开发环境限制（Cargo不在PATH），无法执行自动化构建和GUI测试。建议按以下步骤进行手动测试：

### 1. 构建应用
```bash
# 在配置了Rust工具链的环境中
npm run tauri build
```

### 2. 测试pause模式
1. 打开设置面板
2. 找到"低额度时动作"下拉框
3. 选择"暂停执行"
4. 模拟低余额场景（可通过阈值设置）
5. 验证：
   - [ ] 显示toast警告
   - [ ] 后续请求被拦截
   - [ ] 不自动切换账号/套餐

### 3. 测试套餐优先切换
1. 设置"低额度时动作"为"自动切换套餐/账号"
2. 准备账号A（两个套餐：一个低一个高）
3. 使用余额低的套餐
4. 触发切换
5. 验证：
   - [ ] 显示"正在切换到..."toast
   - [ ] 切换到同账号的高余额套餐
   - [ ] 不切换账号

### 4. 测试账号切换
1. 让账号A所有套餐都低于阈值
2. 准备账号B有充足余额
3. 触发切换
4. 验证：
   - [ ] 先尝试账号A内切换（无高余额套餐）
   - [ ] 然后切换到账号B

### 5. 测试配置持久化
1. 选择"暂停执行"
2. 重启应用
3. 验证：
   - [ ] 设置保持为"暂停执行"

---

## 总结

### 已完成
✅ 所有代码实现已完成并提交到main分支
✅ 所有需求通过代码审查验证
✅ 国际化支持完整
✅ 错误处理和用户体验良好

### 待验证（需要实际运行环境）
⏳ 构建应用
⏳ GUI交互测试
⏳ 端到端场景测试

### 代码位置汇总
- 前端逻辑: `src/store.ts` (套餐优先切换、pause模式)
- 套餐选择: `src/lib/quotaGuard.ts` (pickPlanSwitchTarget)
- 后端拦截: `src-tauri/src/proxy.rs` (quota_guard_is_paused)
- 后端切换: `src-tauri/src/profile.rs` (switch_plan_internal)
- UI界面: `src/components/SettingsPanel.tsx`
- 国际化: `src/i18n.ts`
