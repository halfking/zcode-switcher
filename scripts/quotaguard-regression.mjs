// isCurrentEntryLow / pickPlanSwitchTarget / entryHealth / currentPlanEntry /
// firstVerifiedTarget 单元测试。复盖三个关键回退：
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
  import { isCurrentEntryLow, pickPlanSwitchTarget, currentPlanEntry, entryHealth, firstVerifiedTarget }
    from ${JSON.stringify(posix(join(process.cwd(), "src/lib/quotaGuard.ts")))};
  globalThis.__qg = { isCurrentEntryLow, pickPlanSwitchTarget, currentPlanEntry, entryHealth, firstVerifiedTarget };
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
const {
  isCurrentEntryLow,
  pickPlanSwitchTarget,
  currentPlanEntry,
  entryHealth,
  firstVerifiedTarget,
} = globalThis.__qg;

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

// ---- 6. firstVerifiedTarget：逐一验证切换目标 -------------------------------
//
// 目标语义（任务：切前必须确认待切换的套餐/帐户确有余额）：
//   - 按给定顺序逐个用最新拉取的余额复核，第一个通过的成为切换目标；
//   - 复核不合格（最新余额跌破阈值 / 拉取失败）→ 跳过，继续验证下一个；
//   - 全部不通过 → 返回 null，调用方绝不执行切换。

// isSwitchableCandidate 的镜像判定（glm52.ts 同语义，测试内联避免多打一个 bundle）：
// 每个已知量纲都必须严格高于阈值，且至少一个量纲有数据。
function candidateOk(quota) {
  if (!quota || quota.error) return false;
  const token = quota.token ?? null;
  const point = quota.point ?? null;
  if (token === null && point === null) return false;
  if (token !== null && token <= TOKEN_THRESHOLD_WAN * 10_000) return false;
  if (point !== null && point <= POINT_THRESHOLD) return false;
  return true;
}

{
  // 场景 A：首个候选最新余额仍充足 → 直接选中。串行模式（concurrency=1）下
  // 还要求"通过即停"：不再发起后面的余额请求。并发模式下的停止语义由场景 E 锁定。
  const refreshed = [];
  const candidates = [{ id: "A" }, { id: "B" }];
  const picked = await firstVerifiedTarget(
    candidates,
    async (item) => {
      refreshed.push(item.id);
      return { id: item.id, token: 800_000, point: 5_000 }; // 都高于阈值
    },
    (_item, fresh) => candidateOk(fresh),
    { concurrency: 1 }
  );
  assert.equal(picked?.id, "A", "首个候选通过时应被选中");
  assert.deepEqual(refreshed, ["A"], "通过即停：不应继续刷新后续候选");
  console.log("[6] 逐一验证：首个候选通过即停 PASS");
}

{
  // 场景 B：缓存说候选 A 有余额，但最新余额已耗尽 → 必须跳过 A、验证 B
  // 并选中 B。这是"绝不从一个没余额的帐户切到另一个没余额的帐户"的核心
  // 回归点：凭过期缓存切到 A 就是切到了空帐户。
  const refreshed = [];
  const latest = {
    A: { token: 0, point: 0 }, // 缓存充足、实际已耗尽
    B: { token: 900_000, point: 2_000 },
  };
  const candidates = [{ id: "A" }, { id: "B" }];
  const picked = await firstVerifiedTarget(
    candidates,
    async (item) => {
      refreshed.push(item.id);
      return { ...latest[item.id], id: item.id };
    },
    (_item, fresh) => candidateOk(fresh)
  );
  assert.equal(picked?.id, "B", "首个候选复核不合格时应继续验证下一个");
  assert.deepEqual(refreshed, ["A", "B"], "A、B 都应被逐一刷新验证");
  console.log("[7] 逐一验证：复核不合格跳过并验证下一个 PASS");
}

{
  // 场景 C：所有候选的最新余额都不达标 → 返回 null（绝不切换），
  // 且每个候选都必须被验证过（逐一，不允许只看第一个就放弃）。
  const refreshed = [];
  const candidates = [{ id: "A" }, { id: "B" }, { id: "C" }];
  const picked = await firstVerifiedTarget(
    candidates,
    async (item) => {
      refreshed.push(item.id);
      return { id: item.id, token: 0, point: 0 };
    },
    (_item, fresh) => candidateOk(fresh)
  );
  assert.equal(picked, null, "全部复核不合格时应返回 null（不切换）");
  assert.deepEqual(refreshed, ["A", "B", "C"], "每个候选都应逐一验证");
  console.log("[8] 逐一验证：全部不合格返回 null PASS");
}

{
  // 场景 D：余额拉取失败（网络/429 等）视为不可验证 → 不合格，继续下一个；
  // refresh 抛错不允许让整个验证流程中断。
  const refreshed = [];
  const candidates = [{ id: "A" }, { id: "B" }];
  const picked = await firstVerifiedTarget(
    candidates,
    async (item) => {
      refreshed.push(item.id);
      if (item.id === "A") throw new Error("请求超时");
      return { id: item.id, token: 900_000, point: 2_000 };
    },
    (_item, fresh) => candidateOk(fresh)
  );
  assert.equal(picked?.id, "B", "拉取失败的候选应被视为不合格并跳过");
  assert.deepEqual(refreshed, ["A", "B"], "抛错后应继续验证下一个候选");
  console.log("[9] 逐一验证：拉取失败视为不可验证 PASS");
}

{
  // 场景 E：并发模式不改变选择语义 —— 低优先级候选先通过时，必须等更高
  // 优先级的候选判定完：高优先级也通过 → 高优先级胜出。同时锁定：
  //   - 并发上限（同时在飞的 refresh ≤ concurrency）；
  //   - 单候选超时（挂死的候选按不合格跳过）；
  //   - onProgress 进度回报（done/failed/activeIndexes）与胜者确定后停止回报。
  const progress = [];
  let inflight = 0;
  let maxInflight = 0;
  const candidates = [{ id: "A" }, { id: "B" }, { id: "C" }];
  const picked = await firstVerifiedTarget(
    candidates,
    async (item) => {
      inflight += 1;
      maxInflight = Math.max(maxInflight, inflight);
      try {
        if (item.id === "A") {
          // 高优先级但验证慢：B 先完成也不能抢跑。
          await new Promise((resolve) => setTimeout(resolve, 120));
          return { id: item.id, token: 700_000, point: 3_000 };
        }
        if (item.id === "B") {
          await new Promise((resolve) => setTimeout(resolve, 10));
          return { id: item.id, token: 900_000, point: 2_000 };
        }
        // C：挂死，只能靠超时判出局。
        await new Promise(() => {});
        return { id: item.id };
      } finally {
        inflight -= 1;
      }
    },
    (_item, fresh) => candidateOk(fresh),
    { concurrency: 2, timeoutMs: 250, onProgress: (p) => progress.push({ ...p, active: [...p.activeIndexes] }) }
  );
  assert.equal(picked?.id, "A", "并发下胜者仍应是优先级最高且合格的 A");
  assert.equal(maxInflight, 2, "在飞验证数不得超过并发上限 2");
  assert.ok(progress.length > 0, "应回报验证进度");
  assert.ok(
    progress.every((p) => p.active.length <= 2),
    "进度里的在飞候选数不得超过并发上限"
  );
  assert.equal(progress[0].total, 3, "进度 total 应等于候选数");
  // 胜者确定后不再回报进度：最后的进度事件必须早于（或等于）A 出结果，
  // 且 done 不会把胜者确定后仍在飞的候选算进去。
  const last = progress[progress.length - 1];
  assert.ok(last.done >= 2, "胜者确定前 A、B 应已出结果");
  console.log("[10] 并发验证：优先级语义 / 并发上限 / 超时 / 进度 PASS");
}

// ---- 清理 ------------------------------------------------------------------

await rm(dir, { recursive: true, force: true });
console.log("== quotaGuard 单元测试：全部通过 ==");
