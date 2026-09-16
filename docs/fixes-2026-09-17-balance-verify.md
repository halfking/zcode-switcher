# 审计 handoff：低额度切换的目标余额逐一验证（2026-09-17）

## 结论 / 根因

**结论**：1.1.16 的"切换前强制复核目标余额"主链路正确（切套餐复核 + 切账号
逐一验证 + 全部不合格暂停），数据源正确（`fetch_quota(Some(id))` 用目标档案
自己的凭据副本查询）。审计发现 2 个问题并已修复：

1. **冷却时间被验证轮次误消耗**（行为缺陷）：`maybeSwitchGlm52Account` 在
   余额复核**之前**就写入 `lastPlanSwitchAttemptAt`。一次"复核不动"
   （目标入口已耗尽或当前入口已恢复）也会消耗 60 秒冷却，期间冷却检查
   `return` 会把**切账号步骤一起阻塞**，导致恢复后最多晚切一个冷却周期。
   **根因**：把"冷却"从"防切换抖动"错误扩大到"防验证执行"。修复：时间戳
   移到真正执行 `api.switchPlan` 之前，复核轮次不再消耗冷却。
2. **核心布线无回归保护**（测试缺口）：`firstVerifiedTarget` 只有助手级
   单测，store 级布线（逐一刷新候选 → 复核 → 切换/暂停/冷却/失败回落）
   完全没有自动化测试，冷却这类时序回归只能靠手工发现。**根因**：低额度
   切换逻辑深埋在 zustand store 里，此前没有可注入 tauri 命令的测试入口。
   修复：新增 `scripts/auto-switch-regression.mjs`（真实 store + 桩
   invoke，4 场景分进程运行），其中 S3S4 正是冷却修复的回归防护——旧代码
   在该用例下必失败。

**审计确认无问题的部分**：`isSwitchableCandidate` 对无数据/出错账号保守
拒绝；切套餐失败视同耗尽落入切号；暂停模式与 `autoSwitchPaused` 语义分离；
`refreshQuota` 失败会把错误写回缓存从而阻断下一次自动切换（保守方向正确）。

## 改动文件与关键行为

| 文件 | 改动 |
|---|---|
| `src/store.ts` | 冷却时间戳移入"复核通过、即将执行 switchPlan"分支；注释说明理由 |
| `scripts/auto-switch-regression.mjs` | 新增，store 级集成回归（S1/S2/S3S4/S5，`npm run test:autoswitch`） |
| `package.json` | 新增 `test:autoswitch` script |
| `docs/changelog.md` | 1.1.16 补审计修复与集成回归条目 |
| `TEST_PLAN.md` | 新增场景 7（逐一复核防过期缓存）/ 场景 8（全部不合格暂停）+ 自动化对照表 |
| `verify-implementation.md` | 重写：旧版引用的函数/文件与实际代码不符，已按真实实现修正 |
| `docs/fixes-2026-09-17-balance-verify.md` | 本文档 |

## 测试命令与结果

```
npm run test:quotaguard   # 9/9 PASS（含逐一验证 4 项）
npm run test:autoswitch   # S1/S2/S3S4/S5 全部 PASS（S3S4 在修复前的代码上必失败）
npm run test:settings     # PASS（轮询等待，连续多次稳定）
tsc --noEmit              # 通过
npm run tauri build       # 1.1.16+46 构建成功
# 静默安装 /S 后：
scripts/windows-regression.ps1 -ExpectedVersion 1.1.16   # 9/10 PASS
```

## 遗留风险

1. **验证到执行之间的竞态**：候选 A 复核通过到 `switchTo` 落盘之间（秒级）
   余额仍可能耗尽——物理上不可消除，只能靠下一轮监测兜底。
2. **CDP 依赖环境状态**：实机回归的 `zcode.cdp` 项取决于 ZCode 上次是否
   经带 `--remote-debugging-port=9229` 的快捷方式启动；用户直启 ZCode 时
   该项 FAIL（与本版改动无关），实时套餐切换会退回写配置+重启路径。
3. **验证轮次的额外余额请求**：逐一复核会对每个候选多拉一次余额；候选很多
   且网络不佳时验证可能持续较分钟级（有 `glm52AutoSwitching` 防重入 +
   `busy` 抢占检查兜底）。
4. **重打包产物命名滞后一位**：tauri bundler 取 stamp 前的配置版本命名
   安装包文件名，安装后 exe 版本为 stamp 后值（+1），发布产物按安装后版本
   命名；`+N` 仅构建元数据，不影响更新比较。

## 下一轮提示词（如需继续）

> 对 zcode-switcher 低额度自动切换做下一轮迭代：1) 为"逐一验证候选"增加
> 并发上限与单候选超时（目前串行、无单次超时），验证期间向 UI 暴露验证
> 进度；2) 把 `scripts/windows-regression.ps1` 的 `zcode.cdp` 检查项改为
> "仅当 zcode-switcher 由增强启动拉起 ZCode 时才要求"，或提供一键修复；
> 3) 跑全量回归 + 重新构建部署 + 更新 release/ 产物，提交推送主分支。
