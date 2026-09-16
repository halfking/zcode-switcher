// store 级集成回归：低额度自动切换的目标余额逐一验证布线。
//
// 覆盖（对应 docs/fixes-2026-09-17-balance-verify.md 审计结论）：
//   S1   候选按余量排序后逐一用最新拉取的余额复核：缓存充足但复核耗尽的
//        候选被跳过，切到第一个验证通过的账号（绝不凭过期缓存切换）。
//   S2   所有候选复核不合格 → 不执行任何切换，进入自动暂停。
//   S3S4 复核发现当前入口余额已恢复 → 什么都不做；且复核轮次不消耗套餐
//        切换冷却，紧接着的下一轮能正常执行真正的切套餐（冷却回归防护）。
//   S5   切套餐失败（入口缺 API Key 等）→ 视同耗尽，落入切账号逐一验证。
//
// store 模块内有跨调用的冷却/守护状态，每个场景在独立子进程中运行，
// 保证互不污染。失败时 exit 1（并保留临时 bundle 目录供排查）。
import { build } from "esbuild";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";

// 临时目录放在项目 node_modules/.cache 下：被 git 忽略，且子进程能从
// 这里向上解析到项目的 node_modules（jsdom 以 external 形式运行时加载）。
const cacheRoot = join(process.cwd(), "node_modules", ".cache", "autoswitch-test");
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
      if (!script) {
        return { plan_name: null, plan_description: null, plan_status: null, plan_ends_at: null, balances: [] };
      }
      return script(counts[id]);
    }
    if (command === "switch_to") {
      (globalThis.__switches ||= []).push(args);
      return { id: (args && args.id) || "", name: (args && args.id) || "" };
    }
    if (command === "switch_plan") {
      (globalThis.__switchPlans ||= []).push(args);
      if (globalThis.__failSwitchPlan) throw new Error("coding-plan 入口没有 API Key（测试注入）");
      return { selected_key: "coding-plan:builtin:bigmodel-coding-plan", applied_live: true };
    }
    if (command === "set_quota_guard") {
      return { paused: !!(args && args.paused), reason: null, updated_at: 0 };
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

// 阈值：token 50 万 / 积分 1000。tok(x) 单位"万"；pt(n) 单位积分。
const harness = `
import { useStore } from ${JSON.stringify(posix(join(process.cwd(), "src/store.ts")))};

const THRESHOLD_WAN = 50;
const POINT_THRESHOLD = 1000;

function profile(id: string, active: boolean) {
  return {
    id, name: id, user_id: id, email: id + "@t.local", phone: "", avatar: "",
    cred_hash: id, cred_file: id + ".json", created_at: 0, updated_at: 0,
    active, short_id: id,
  };
}
const tok = (wan: number) => ({
  show_name: "GLM-5.3", used_units: 0, total_units: wan * 10_000,
  remaining_units: wan * 10_000, unit_type: "token", period: "weekly", plan_id: "start-plan",
});
const pt = (n: number) => ({
  show_name: "5小时积分", used_units: 0, total_units: n, remaining_units: n,
  unit_type: "point", period: "5h", plan_id: "personal:glm-coding",
});

async function arm(ids: string[], activeId: string, scripts: Record<string, (call: number) => any>) {
  (globalThis as any).__quotaScripts = scripts;
  const st = useStore.getState();
  st.setGlm52AutoSwitchEnabled(true);
  st.setGlm52AutoSwitchThresholdWan(THRESHOLD_WAN);
  st.setGlm52AutoSwitchPointThreshold(POINT_THRESHOLD);
  useStore.setState({
    profiles: ids.map((id) => profile(id, id === activeId)),
    quotas: {},
    autoSwitchPaused: false,
  });
}
async function cycle() {
  await useStore.getState().refreshAllQuota();
}
const state = () => useStore.getState();
const calls = (cmd: string) =>
  ((globalThis as any).__calls || []).filter((c: any) => c.cmd === cmd);
function assert(cond: unknown, msg: string): asserts cond {
  if (!cond) throw new Error("断言失败: " + msg);
}

const scenarios: Record<string, () => Promise<void>> = {
  async S1() {
    await arm(["A", "B", "C"], "A", {
      A: () => ({ balances: [tok(0)] }),
      B: (n) => ({ balances: [tok(n === 1 ? 900 : 0)] }),
      C: () => ({ balances: [tok(800)] }),
    });
    await cycle();
    const sw = calls("switch_to");
    assert(sw.length === 1 && sw[0].args.id === "C",
      "应切到验证通过的 C，实际 " + JSON.stringify(sw));
    const counts = (globalThis as any).__fetchCounts || {};
    assert(counts.B === 2, "B 应被批量刷新+逐一验证各拉取一次，实际 " + counts.B);
    assert(state().profiles.find((p: any) => p.id === "C")?.active === true, "C 应成为活跃账号");
  },

  async S2() {
    await arm(["A", "B", "C"], "A", {
      A: () => ({ balances: [tok(0)] }),
      B: (n) => ({ balances: [tok(n === 1 ? 900 : 0)] }),
      C: (n) => ({ balances: [tok(n === 1 ? 800 : 0)] }),
    });
    await cycle();
    assert(calls("switch_to").length === 0, "全部候选复核不合格时不得切换账号");
    assert(calls("switch_plan").length === 0, "全部候选复核不合格时不得切套餐");
    assert(state().autoSwitchPaused === true, "应进入自动暂停");
  },

  async S3S4() {
    await arm(["A"], "A", {
      A: (n) => ({
        balances: n === 2 ? [tok(800)] : [tok(0), pt(5000)],
        active_provider: "coding-plan:builtin:bigmodel-start-plan",
      }),
    });
    await cycle(); // 第 1 轮：目标 coding-plan，复核（第 2 次拉取）发现当前入口已恢复
    assert(calls("switch_plan").length === 0, "复核发现余额恢复后不得切套餐");
    assert(calls("switch_to").length === 0, "复核发现余额恢复后不得切账号");
    await cycle(); // 第 2 轮：复核轮次未消耗冷却 → 正常执行切套餐
    assert(calls("switch_plan").length === 1,
      "复核轮次不应消耗冷却：第 2 轮应执行切套餐");
    assert(calls("switch_plan")[0].args.target === "coding-plan", "应切到 coding-plan 入口");
    assert(calls("switch_to").length === 0, "切套餐成功后不得切账号");
  },

  async S5() {
    (globalThis as any).__failSwitchPlan = true;
    await arm(["A", "B"], "A", {
      A: () => ({
        balances: [tok(0), pt(5000)],
        active_provider: "coding-plan:builtin:bigmodel-start-plan",
      }),
      B: () => ({ balances: [tok(900)] }),
    });
    await cycle();
    assert(calls("switch_plan").length === 1, "应先尝试切套餐");
    const sw = calls("switch_to");
    assert(sw.length === 1 && sw[0].args.id === "B", "切套餐失败应逐一验证并切到 B");
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

// jsdom 必须先于 store 模块初始化（模块顶层就读 document/localStorage），
// 所以 harness 用动态 import 引入：esbuild 不拆包时动态导入保持惰性执行。
// jsdom 是 CJS 大包，必须 external（打进 ESM bundle 会抛 Dynamic require），
// 运行时由子进程从项目的 node_modules 解析。
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
for (const name of ["S1", "S2", "S3S4", "S5"]) {
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
  console.error(`== auto-switch 集成回归：${failed} 个场景失败 ==`);
  console.error(`bundle 目录（保留供排查）: ${dir}`);
  process.exit(1);
}
await rm(dir, { recursive: true, force: true });
console.log("== auto-switch 集成回归：全部通过 ==");
