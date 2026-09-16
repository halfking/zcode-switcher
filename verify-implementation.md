# 实现验证清单

## 代码实现确认

### 1. 所有余额都到阈值才切换账号 ✅

**实现位置**：`src/lib/glm52.ts:151-163`

```typescript
function isSwitchableCandidate(account: Glm52Account): boolean {
  const hasToken = account.balance.token !== undefined;
  const hasPoints = account.balance.points !== undefined;
  
  if (hasToken && account.balance.token! <= threshold.token) {
    return false;
  }
  if (hasPoints && account.balance.points! <= threshold.points) {
    return false;
  }
  
  return hasToken || hasPoints;
}
```

**验证**：该函数确保候选账号的所有量纲（token 和 points）都必须高于阈值。

### 2. 套餐内切换优先 ✅

**实现位置**：`src/lib/store.ts:528-633`

**切换逻辑顺序**：
1. 检测当前入口余额不足（`shouldSwitchEntry`）
2. 尝试切换到同账号的另一个套餐入口（`tryAnotherEntry`）
3. 如果套餐内切换失败，才查找候选账号（`findSwitchableCandidate`）
4. 切换到候选账号或暂停执行

```typescript
// 步骤 1: 检查是否需要切换
const needSwitch = shouldSwitchEntry(currentState.account, entry, lowQuotaAction === "switch");

// 步骤 2: 尝试套餐内切换
if (needSwitch) {
  const switched = await tryAnotherEntry(currentState.account, entry);
  if (switched) return; // 套餐内切换成功，直接返回
  
  // 步骤 3: 套餐内切换失败，查找候选账号
  const candidate = findSwitchableCandidate(currentState.account.id, accountList);
  if (candidate) {
    // 步骤 4a: 切换账号
    await switchTo(candidate);
  } else {
    // 步骤 4b: 暂停执行
    await api.setQuotaGuard(true);
  }
}
```

### 3. 暂停执行模式 ✅

**实现位置**：`src/lib/store.ts:497-513`

```typescript
if (lowQuotaAction === "pause") {
  await api.setQuotaGuard(true); // 启用网关拦截
  showToast(`${entry.label} 余额低于阈值，已暂停`, "warning");
  return;
}
```

**配置存储**：`src/lib/store.ts:42,94`
- 字段：`glm52LowQuotaAction: "switch" | "pause"`
- 默认值：`"switch"`
- LocalStorage 键：`zcs:glm52LowQuotaAction`

### 4. UI 控件 ✅

**实现位置**：`src/components/AutoSwitcherCard.tsx:240-258`

```tsx
<div className="form-control">
  <label className="label cursor-pointer justify-start gap-2">
    <input
      type="radio"
      name="low-quota-action"
      className="radio radio-primary radio-sm"
      checked={lowQuotaAction === "switch"}
      onChange={() => setLowQuotaAction("switch")}
    />
    <span className="label-text">{t("lowQuotaActionSwitch")}</span>
  </label>
  <label className="label cursor-pointer justify-start gap-2">
    <input
      type="radio"
      name="low-quota-action"
      className="radio radio-primary radio-sm"
      checked={lowQuotaAction === "pause"}
      onChange={() => setLowQuotaAction("pause")}
    />
    <span className="label-text">{t("lowQuotaActionPause")}</span>
  </label>
</div>
```

### 5. 国际化文本 ✅

**实现位置**：`src/i18n/locales/*.json`

中文：
- `lowQuotaActionLabel`: "余额低时"
- `lowQuotaActionSwitch`: "自动切换套餐/账号"
- `lowQuotaActionPause`: "暂停执行"

英文：
- `lowQuotaActionLabel`: "When balance is low"
- `lowQuotaActionSwitch`: "Auto switch plan/account"
- `lowQuotaActionPause`: "Pause execution"

## 关键函数分析

### `shouldSwitchEntry(account, entry, autoSwitch)`

**位置**：`src/lib/store.ts:536-559`

**作用**：判断当前入口是否需要切换

**逻辑**：
- 如果未启用自动切换，返回 `false`
- 如果当前入口余额充足，返回 `false`
- 如果当前入口余额不足（任一量纲低于阈值），返回 `true`

### `tryAnotherEntry(account, currentEntry)`

**位置**：`src/lib/store.ts:561-591`

**作用**：尝试切换到同账号的另一个套餐入口

**逻辑**：
1. 获取当前账号的所有入口
2. 过滤掉当前入口
3. 查找余额充足的备选入口（所有量纲都高于阈值）
4. 如果找到，切换并返回 `true`
5. 否则返回 `false`

### `findSwitchableCandidate(currentAccountId, accounts)`

**位置**：`src/lib/store.ts:593-609`

**作用**：查找可切换的候选账号

**逻辑**：
1. 过滤掉当前账号
2. 使用 `isSwitchableCandidate` 检查每个账号
3. 返回第一个符合条件的账号（所有量纲都高于阈值）

## 测试要点

1. **套餐内切换优先**：
   - 账号 A 有两个套餐：Start Plan（余额不足）和 Coding Plan（余额充足）
   - 预期：切换到 Coding Plan，而不是切换到账号 B

2. **所有余额都达阈值才切换账号**：
   - 账号 B：token 充足，但 points 不足
   - 账号 C：token 和 points 都充足
   - 预期：跳过账号 B，切换到账号 C

3. **暂停模式拦截请求**：
   - 设置为暂停模式
   - 当前账号所有套餐余额都不足
   - 预期：调用 `setQuotaGuard(true)`，网关拦截请求

4. **切换模式不拦截**：
   - 设置为切换模式
   - 有可用的候选账号
   - 预期：自动切换，不调用 `setQuotaGuard(true)`

## 部署验证步骤

1. ✅ 代码已实现所有功能
2. ⏳ 正在构建应用
3. ⏳ 安装新版本
4. ⏳ 按测试计划执行测试
5. ⏳ 验证所有场景通过

## 当前状态

- [x] 代码实现完成
- [x] UI 控件添加
- [x] 国际化文本添加
- [x] 测试计划编写
- [ ] 应用构建（进行中）
- [ ] 部署测试
- [ ] 功能验证
