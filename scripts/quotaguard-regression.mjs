// isCurrentEntryLow / pickPlanSwitchTarget / entryHealth / currentPlanEntry
// 单元测试。复盖三个关键回退：
//   1. 单套餐账号回退：active_provider=start-plan，但 balances 里只有
//      coding-plan 积分（账号只订阅 Coding Plan）→ 入口数据不充分，
//      仍能基于整账号判定触发切号（不要被"看似充足"误挡住）。
//   2. 双套餐正常判定：两个入口都有余额，单独按入口算阈值。
//   3. active_provider 缺失/非法 → 整账号判定。
//
// 不引入新测试框架；用 node 22 + esbuild 走与 scripts/settings-panel-regression.mjs
// 相同的临时 bundle 套路。失败时 exit 1 + 输出差异。

import assert from "node:assert/strict";
import { build } from "esbuild";
import { JSDOM } from "jsdom";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

// jsdom 让 document / window 存在，但 quotaGuard.ts 不直接用 DOM，写它只是
// 防止有人未来在 quotaGuard 里加 DOM 引用时本测试不至于抛 ReferenceError。
const dom = new JSDOM("<!doctype html><html><body></body></html>");
globalThis.window = dom.window;
globalThis.document = dom.window.document;

const dir = await mkdtemp(join(tmpdir(), "zcode-quotaguard-test-"));
const posix = (p) => p.replace(/\\/g, "/");

// esbuild 临时 bundle：把 quotaGuard.ts 与其依赖打到单一 ESM，
// 并把 @tauri-apps/api/core 替换为 noop，避免 node 找不到 tauri 运行时。
const apiStub = `
  export function invoke(_command, _args) { return Promise.resolve(null); }
`;
const apiStubPath = join(dir, "tauri-api-stub.mjs");
const { writeFile } = await import("node:fs/promises");
await writeFile(apiStubPath, apiStub);

const entryPath = join(dir, "entry.mjs");
const entry = `
  import { isCurrentEntryLow, pickPlanSwitchTarget, currentPlanEntry, entryHealth }
    from ${JSON.stringify(posix(join(process.cwd(), "src/lib/quotaGuard.ts")))};
  globalThis.__qg = { isCurrentEntryLow, pickPlanSwitchTarget, currentPlanEntry, entryHealth };
`;
await writeFile(entryPath, entry);

const outPath = join(dir, "bundle.mjs");
await build({
  entryPoints: [entryPath],
  bundle: true,
  format: "esm",
  platform: "node",
  target: "node22",
  alias: {
    "@tauri-apps/api/core": apiStubPath,
  },
  outfile: outPath,
  logLevel: "warning",
});

await import(pathToFileURL(outPath).href);
const { isCurrentEntryLow, pickPlanSwitchTarget, currentPlanEntry, entryHealth } = globalThis.__qg;

// ---- fixtures --------------------------------------------------------------

const TOKEN_THRESHOLD_WAN = 50; // 50 万 token
const POINT_THRESHOLD = 1000;  // 1000 积分

function pointItem(remaining, name = "GLM Coding 5h") {
  return {
    show_name: name,
    used_units: 0,
    total_units: remaining,
    remaining_units: remaining,
    unit_type: "point",
    period: "5h",
    plan_id: "personal:glm-coding",
  };
}
function tokenItem(remaining, name = "GLM-5.3") {
  return {
    show_name: name,
    used_units: 0,
    total_units: remaining,
    remaining_units: remaining,
    unit_type: "token",
    period: "weekly",
    plan_id: "start-plan",
  };
}

// ---- 1. 单套餐账号回退（任务 #4 主目标） -----------------------------------

{
  // 账号只订阅了 Coding Plan（积分），active_provider 指向 start-plan 入口
  // （例如刚切过去但 ZCode 还没刷新，或者用户手动把它设到了无数据的入口）。
  // balances 完全是积分条目，没有 token。
  // 期望：currentPlanEntry 返回 "start-plan"，entryHealth 不可评估，
  //       isCurrentEntryLow 回退到 isAccountLow（只看积分），积分低于阈值 → true。
  const quota = {
    balances: [pointItem(500)], // 500 积分，低于阈值 1000
    active_provider: "coding-plan:builtin:bigmodel-start-plan",
  };
  const entry = currentPlanEntry(quota.active_provider);
  assert.equal(entry, "start-plan", "active_provider 应被识别为 start-plan 入口");
  const health = entryHealth(quota, entry, TOKEN_THRESHOLD_WAN, POINT_THRESHOLD);
  assert.equal(health.evaluable, false, "start-plan 入口无 token 数据应不可评估");
  const low = isCurrentEntryLow(quota, TOKEN_THRESHOLD_WAN, POINT_THRESHOLD);
  assert.equal(low, true, "单套餐账号回退：积分低于阈值应触发（不被 start-plan 误判为充足）");
  console.log("[1] 单套餐账号回退 PASS");
}

// ---- 2. 双套餐正常判定 -----------------------------------------------------

{
  // start-plan 有充足 token，coding-plan 积分不足。
  // 当前入口 = start-plan，isCurrentEntryLow 应返回 false（入口充足）。
  const quota = {
    balances: [tokenItem(800000), pointItem(200)],
    active_provider: "coding-plan:builtin:bigmodel-start-plan",
  };
  assert.equal(isCurrentEntryLow(quota, TOKEN_THRESHOLD_WAN, POINT_THRESHOLD), false);
  // 当前入口 = coding-plan 且积分低于阈值 → true
  quota.active_provider = "coding-plan:builtin:bigmodel-coding-plan";
  assert.equal(isCurrentEntryLow(quota, TOKEN_THRESHOLD_WAN, POINT_THRESHOLD), true);
  console.log("[2] 双套餐正常判定 PASS");
}

// ---- 3. active_provider 缺失 / 非法 → 整账号判定 ---------------------------

{
  const quota = {
    balances: [tokenItem(100000), pointItem(50)],
    active_provider: null,
  };
  // 入口识别不出 → 整账号：token 充足但积分远低 → 应触发
  assert.equal(isCurrentEntryLow(quota, TOKEN_THRESHOLD_WAN, POINT_THRESHOLD), true);

  quota.active_provider = "custom:apikey"; // 非套餐入口
  assert.equal(currentPlanEntry(quota.active_provider), null);
  assert.equal(isCurrentEntryLow(quota, TOKEN_THRESHOLD_WAN, POINT_THRESHOLD), true);
  console.log("[3] active_provider 缺失/非法 → 整账号判定 PASS");
}

// ---- 4. pickPlanSwitchTarget：当前入口低、另一入口高 → 切套餐 ---------------

{
  const quota = {
    balances: [tokenItem(10000), pointItem(5000)], // start-plan 低、coding-plan 高
    active_provider: "coding-plan:builtin:bigmodel-start-plan",
  };
  const target = pickPlanSwitchTarget(
    quota,
    "start-plan",
    TOKEN_THRESHOLD_WAN,
    POINT_THRESHOLD
  );
  assert.equal(target, "coding-plan", "另一入口充足时应返回切套餐目标");

  // 反过来：start-plan 高、coding-plan 低 → 当前入口够就切不到
  quota.balances = [tokenItem(800000), pointItem(200)];
  const target2 = pickPlanSwitchTarget(
    quota,
    "start-plan",
    TOKEN_THRESHOLD_WAN,
    POINT_THRESHOLD
  );
  assert.equal(target2, null, "当前入口充足时不应切套餐");
  console.log("[4] pickPlanSwitchTarget PASS");
}

// ---- 5. 暂停模式锚点：当前入口低 → isCurrentEntryLow 必须为 true -----------

{
  // 任务 #2 的暂停模式要求 maybeSwitchGlm52Account 在 currentLow=true 时
  // 注入 setQuotaGuard(true)。这里锁住"低"的判定语义不会被静默改回 false。
  const quota = {
    balances: [pointItem(0)], // 积分耗尽
    active_provider: "coding-plan:builtin:bigmodel-coding-plan",
  };
  assert.equal(isCurrentEntryLow(quota, TOKEN_THRESHOLD_WAN, POINT_THRESHOLD), true);
  console.log("[5] 暂停模式锚点 PASS");
}

// ---- 清理 ------------------------------------------------------------------

await rm(dir, { recursive: true, force: true });
console.log("== quotaGuard 单元测试：全部通过 ==");
