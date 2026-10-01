// 重置卡特性回归：决策规则（纯函数）与 store 的自动用卡布线。
//
//   R1  decideResetCard 纯规则：
//       - 5h 见底、周充足 → FIVE_HOUR；5h 见底、周 ≤5% → WEEK；
//       - 周见底（5h 还行）→ WEEK（周窗口打死 5h 卡救不了）；
//       - 两窗口健康 → 不动卡；月积分见底 → 不动卡（卡救不了月窗口）；
//       - 无积分窗口 → 不触发；只有 5h 窗口见底 → FIVE_HOUR。
//   R2  自动用卡（5h 路径）：当前账号 5h 窗口剩 0.5% → 用一张 5 小时卡
//       → 立即刷新额度、不切套餐不切账号、不进入耗尽暂停。
//   R3  无可用卡：库存为空 → 不打 use、直接走常规切换（切到健康账号）。
//   R4  自动用卡（周路径）：5h 见底且周剩 3%（≤5%）→ 用周卡。
//   R5  use 失败：报错后落回常规流程进入暂停；10 分钟失败冷却让第二轮
//       不再重试（use 恰好调用一次）。
//
// 套路同 auto-switch-regression.mjs：esbuild 临时 bundle + 子进程逐场景。
import { build } from "esbuild";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { spawnSync } from "node:child_process";

const cacheRoot = join(process.cwd(), "node_modules", ".cache", "resetcard-test");
await mkdir(cacheRoot, { recursive: true });
const dir = await mkdtemp(cacheRoot + "-");
const posix = (p) => p.split("\\").join("/");

const coreStub = `
  export async function invoke(command, args) {
    const rec = (globalThis.__calls ||= []);
    rec.push({ cmd: command, args });
    if (command === "fetch_quota") {
      const id = args && args.id;
      const counts = (globalThis.__fetchCounts ||= {});
      counts[id] = (counts[id] ?? 0) + 1;
      const script = (globalThis.__quotaScripts || {})[id];
      const empty = { plan_name: null, plan_description: null, plan_status: null, plan_ends_at: null, balances: [] };
      return script ? Promise.resolve(script(counts[id])) : Promise.resolve(empty);
    }
    if (command === "switch_to") {
      (globalThis.__switches ||= []).push(args);
      return { id: (args && args.id) || "", name: (args && args.id) || "" };
    }
    if (command === "switch_plan") {
      (globalThis.__switchPlans ||= []).push(args);
      return { selected_key: "coding-plan:builtin:bigmodel-coding-plan", applied_live: true };
    }
    if (command === "set_quota_guard") {
      return { paused: !!(args && args.paused), reason: null, updated_at: 0 };
    }
    if (command === "reset_card_inventory") {
      (globalThis.__inventoryCalls ||= []).push(args);
      return Promise.resolve(
        (globalThis.__resetInventory ||= { five_hour: [], week: [] })
      );
    }
    if (command === "use_reset_card_cmd") {
      (globalThis.__useCalls ||= []).push(args);
      if (globalThis.__failUseCard) {
        return Promise.reject(new Error("重置卡使用失败（测试注入）"));
      }
      return Promise.resolve({
        record_id: 1, expire_time: "2026-10-30 01:35:04", grant_type: "DIRECT", available: false,
      });
    }
    if (command === "zcode_running") {
      return null;
    }
    return null;
  }
`;
const windowStub = `
  export function getCurrentWindow() {
    return { setBackgroundColor: async () => {}, listen: async () => () => {} };
  }
`;
await writeFile(join(dir, "tauri-core-stub.mjs"), coreStub);
await writeFile(join(dir, "tauri-window-stub.mjs"), windowStub);

const harness = `
import { useStore } from ${JSON.stringify(posix(join(process.cwd(), "src/store.ts")))};
import { decideResetCard, pointWindowPercent } from ${JSON.stringify(posix(join(process.cwd(), "src/lib/glm52.ts")))};

function assert(cond: unknown, msg: string): asserts cond {
  if (!cond) throw new Error("断言失败: " + msg);
}

// pt(period, remain, total)：积分桶；tok(wan)：token 桶。
const pt = (period: string, remain: number, total: number) => ({
  show_name: period === "5h" ? "5小时积分" : period === "weekly" ? "周积分" : "月积分",
  used_units: total - remain, total_units: total, remaining_units: remain,
  unit_type: "point", period, plan_id: "personal:glm-coding", next_reset_at: null,
});
const tok = (wan: number) => ({
  show_name: "GLM-5.3", used_units: 0, total_units: wan * 10_000,
  remaining_units: wan * 10_000, unit_type: "token", period: "weekly", plan_id: "start-plan",
});
const quotaOf = (balances: any[]) => ({
  plan_name: null, plan_description: null, plan_status: null, plan_ends_at: null, balances,
});

const profile = (id: string, active: boolean) => ({
  id, name: id, user_id: id, email: id + "@t.local", phone: "", avatar: "",
  cred_hash: id, cred_file: id + ".json", created_at: 0, updated_at: 0,
  active, short_id: id,
});

async function arm(ids: string[], activeId: string, scripts: Record<string, (call: number) => any>) {
  (globalThis as any).__quotaScripts = scripts;
  const st = useStore.getState();
  st.setGlm52AutoSwitchEnabled(true);
  st.setGlm52AutoSwitchThresholdWan(50);
  st.setGlm52AutoSwitchPointThreshold(1000);
  st.setGlm52ExhaustRestartZcode(false);
  st.setGlm52AutoResetCardEnabled(true);
  useStore.setState({
    profiles: ids.map((id) => profile(id, id === activeId)),
    quotas: {},
    autoSwitchPaused: false,
  });
}
const cycle = () => useStore.getState().refreshAllQuota();
const state = () => useStore.getState();
const calls = (cmd: string) =>
  ((globalThis as any).__calls || []).filter((c: any) => c.cmd === cmd);

const scenarios: Record<string, () => Promise<void>> = {
  async R1() {
    const q = (balances: any[]) => quotaOf(balances);
    // 5h 见底、周充足 → 5 小时卡
    assert(decideResetCard(q([pt("5h", 10, 2000), pt("weekly", 5000, 10000)]), 1, 5)?.type === "FIVE_HOUR",
      "5h 0.5%/周 50% 应选 5 小时卡");
    // 5h 见底、周 ≤5% → 周卡（同时回满 5h）
    assert(decideResetCard(q([pt("5h", 10, 2000), pt("weekly", 300, 10000)]), 1, 5)?.type === "WEEK",
      "5h 0.5%/周 3% 应选周卡");
    // 周见底（5h 还行）→ 周卡
    assert(decideResetCard(q([pt("5h", 1000, 2000), pt("weekly", 20, 10000)]), 1, 5)?.type === "WEEK",
      "周 0.2% 应选周卡");
    // 两窗口健康 → 不动卡
    assert(decideResetCard(q([pt("5h", 1000, 2000), pt("weekly", 5000, 10000)]), 1, 5) === null,
      "两窗口健康不应动卡");
    // 月积分见底 → 卡救不了，不触发
    assert(decideResetCard(q([pt("5h", 10, 2000), pt("weekly", 5000, 10000), pt("monthly", 1, 6000)]), 1, 5) === null,
      "月窗口见底时不应动卡");
    // 无积分窗口 → 不触发
    assert(decideResetCard(q([tok(0)]), 1, 5) === null, "无积分窗口不应触发");
    // 只有 5h 窗口见底 → 5 小时卡
    assert(decideResetCard(q([pt("5h", 5, 2000)]), 1, 5)?.type === "FIVE_HOUR",
      "仅 5h 见底应选 5 小时卡");
    // 阈值可调：触发线 10% 时 8% 也触发
    assert(decideResetCard(q([pt("5h", 160, 2000), pt("weekly", 5000, 10000)]), 10, 5)?.type === "FIVE_HOUR",
      "触发阈值 10% 时 8% 应触发");
    // 百分比辅助：total<=0 视为 0%
    assert(pointWindowPercent(q([pt("5h", 0, 0)]), "5h") === 0, "total=0 应为 0%");
    assert(pointWindowPercent(q([tok(5)]), "5h") === null, "无该窗口应为 null");
  },

  async R2() {
    (globalThis as any).__resetInventory = {
      five_hour: [{ record_id: 1, expire_time: "2026-10-30 01:35:04", grant_type: "DIRECT", available: true }],
      week: [],
    };
    await arm(["A"], "A", {
      A: (n: number) => n === 1
        ? quotaOf([tok(900), pt("5h", 10, 2000), pt("weekly", 5000, 10000)])
        : quotaOf([tok(900), pt("5h", 2000, 2000), pt("weekly", 5000, 10000)]),
    });
    await cycle();
    const use = (globalThis as any).__useCalls || [];
    assert(use.length === 1 && use[0].resetType === "FIVE_HOUR" && use[0].id === "A",
      "应自动用一张 5 小时卡，实际 " + JSON.stringify(use));
    assert(calls("switch_to").length === 0, "用卡成功后不应切账号");
    assert(calls("switch_plan").length === 0, "用卡成功后不应切套餐");
    assert(state().autoSwitchPaused === false, "用卡成功后不应进入暂停");
    const counts = (globalThis as any).__fetchCounts || {};
    assert(counts.A === 2, "用卡后应立即刷新当前账号额度，实际拉取 " + (counts.A ?? 0));
  },

  async R3() {
    (globalThis as any).__resetInventory = { five_hour: [], week: [] };
    await arm(["A", "B"], "A", {
      A: () => quotaOf([tok(0), pt("5h", 10, 2000), pt("weekly", 5000, 10000)]),
      B: () => quotaOf([tok(900)]),
    });
    await cycle();
    assert(((globalThis as any).__useCalls || []).length === 0,
      "没有可用卡时不得打 use 接口");
    const sw = calls("switch_to");
    assert(sw.length === 1 && sw[0].args.id === "B",
      "无卡时应走常规切换到 B，实际 " + JSON.stringify(sw));
  },

  async R4() {
    (globalThis as any).__resetInventory = {
      five_hour: [],
      week: [{ record_id: 7, expire_time: "2026-10-30 01:35:04", grant_type: "DIRECT", available: true }],
    };
    await arm(["A"], "A", {
      A: (n: number) => n === 1
        ? quotaOf([tok(900), pt("5h", 10, 2000), pt("weekly", 300, 10000)])
        : quotaOf([tok(900), pt("5h", 2000, 2000), pt("weekly", 10000, 10000)]),
    });
    await cycle();
    const use = (globalThis as any).__useCalls || [];
    assert(use.length === 1 && use[0].resetType === "WEEK",
      "周 3%（≤5%）应用周卡，实际 " + JSON.stringify(use));
    assert(calls("switch_to").length === 0, "用卡成功后不应切账号");
  },

  async R5() {
    (globalThis as any).__failUseCard = true;
    (globalThis as any).__resetInventory = {
      five_hour: [{ record_id: 1, expire_time: "2026-10-30 01:35:04", grant_type: "DIRECT", available: true }],
      week: [],
    };
    await arm(["A"], "A", {
      A: () => quotaOf([tok(0), pt("5h", 10, 2000), pt("weekly", 5000, 10000)]),
    });
    await cycle();
    assert(((globalThis as any).__useCalls || []).length === 1, "第一轮应尝试用卡一次");
    assert(state().autoSwitchPaused === true, "用卡失败且无候选应进入自动暂停");
    await cycle();
    assert(((globalThis as any).__useCalls || []).length === 1,
      "失败冷却期内第二轮不得重试用卡（10 分钟冷却）");
    (globalThis as any).__failUseCard = false;
  },
};

export async function runScenario(name: string) {
  const fn = scenarios[name];
  if (!fn) throw new Error("未知场景: " + name);
  await fn();
  console.log("[" + name + "] PASS");
}
`;
const harnessPath = join(dir, "harness.ts");
await writeFile(harnessPath, harness);

const entry = `
import { JSDOM } from "jsdom";
const dom = new JSDOM("<!doctype html><html><body></body></html>", { url: "http://localhost" });
globalThis.window = dom.window;
globalThis.document = dom.window.document;
globalThis.localStorage = dom.window.localStorage;
const { runScenario } = await import(${JSON.stringify(posix(harnessPath))});
runScenario(process.argv[2]).then(
  () => process.exit(0),
  (e) => { console.error((e && e.message) || String(e)); process.exit(1); }
);
`;
const entryPath = join(dir, "entry.mjs");
await writeFile(entryPath, entry);

const bundle = join(dir, "bundle.mjs");
await build({
  entryPoints: [entryPath],
  bundle: true,
  format: "esm",
  platform: "node",
  target: "node22",
  alias: {
    "@tauri-apps/api/core": join(dir, "tauri-core-stub.mjs"),
    "@tauri-apps/api/window": join(dir, "tauri-window-stub.mjs"),
  },
  external: ["jsdom"],
  outfile: bundle,
  logLevel: "warning",
});

let failed = 0;
for (const name of ["R1", "R2", "R3", "R4", "R5"]) {
  const r = spawnSync(process.execPath, [bundle, name], { encoding: "utf8" });
  if (r.status !== 0) {
    failed += 1;
    console.error(`[${name}] FAIL`);
    if (r.stdout) console.error(r.stdout);
    if (r.stderr) console.error(r.stderr);
  } else {
    console.log(r.stdout.trim());
  }
}
if (failed) {
  console.error(`== reset-card 回归：${failed} 个场景失败 ==`);
  console.error(`bundle 目录（保留供排查）: ${dir}`);
  process.exit(1);
}
await rm(dir, { recursive: true, force: true });
console.log("== reset-card 回归：全部通过 ==");
