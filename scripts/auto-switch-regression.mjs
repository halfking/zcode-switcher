// store 级集成回归：低额度自动切换的目标余额验证布线。
//
// 覆盖（对应 docs/fixes-2026-09-17-balance-verify.md 审计结论 + 并发/超时迭代）：
//   S1   候选按余量排序后逐一用最新拉取的余额复核：缓存充足但复核耗尽的
//        候选被跳过，切到第一个验证通过的账号（绝不凭过期缓存切换）。
//   S2   所有候选复核不合格 → 不执行任何切换，进入自动暂停。
//   S3S4 复核发现当前入口余额已恢复 → 什么都不做；且复核轮次不消耗套餐
//        切换冷却，紧接着的下一轮能正常执行真正的切套餐（冷却回归防护）。
//   S5   切套餐失败（入口缺 API Key 等）→ 视同耗尽，落入切账号逐一验证。
//   S6   并发上限：候选验证按 candidateVerifyConcurrency 并行（在飞数
//        ≤上限且 >1），优先级语义不变——切到的仍是"有余额的最高优先级
//        候选"；验证进度经 switchVerifyProgress 实时暴露并在结束后清空。
//   S7   单候选超时：复核请求挂死的候选在 candidateVerifyTimeoutMs 后按
//        不合格跳过，切换落 to 后续合格候选，整体流程不被挂死拖住。
//   S8a  切套餐不得影响当前正在执行的任务（CDP live 路径）：
//        applied_live=true 时 kill_zcode_for_switch 和 restart_zcode
//        调用次数都必须为 0。
//   S8b  切套餐不得影响当前正在执行的任务（in-place 路径）：
//        applied_live=false 且 autoRestart=true 时 store 会调
//        restartZcode 让配置生效，但 kill_zcode_for_switch 仍必须为 0。
//        这是切套餐 ≠ 切号这条不变量在回归测试层的强制约束。
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
      // 在飞计数：给 S6 断言"确实并发且不超过上限"用。
      const inflight = (globalThis.__inflight ||= { n: 0, max: 0 });
      inflight.n += 1;
      inflight.max = Math.max(inflight.max, inflight.n);
      const script = (globalThis.__quotaScripts || {})[id];
      const empty = { plan_name: null, plan_description: null, plan_status: null, plan_ends_at: null, balances: [] };
      // __fetchDelayMs：人为拉长每次拉取，制造可观测的并发窗口（默认 0）。
      const delayMs = globalThis.__fetchDelayMs || 0;
      const run = script
        ? Promise.resolve(script(counts[id]))
        : Promise.resolve(empty);
      return new Promise((resolve, reject) => {
        run.then(
          (value) =>
            setTimeout(() => {
              inflight.n -= 1;
              resolve(value);
            }, delayMs),
          (error) =>
            setTimeout(() => {
              inflight.n -= 1;
              reject(error);
            }, delayMs)
        );
      });
    }
    if (command === "switch_to") {
      (globalThis.__switches ||= []).push(args);
      return { id: (args && args.id) || "", name: (args && args.id) || "" };
    }
    if (command === "switch_plan") {
      (globalThis.__switchPlans ||= []).push(args);
      if (globalThis.__failSwitchPlan) throw new Error("coding-plan 入口没有 API Key（测试注入）");
      const live = globalThis.__planNotLive ? false : true;
      return { selected_key: "coding-plan:builtin:bigmodel-coding-plan", applied_live: live };
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

  async S6() {
    // 并发上限 + 进度暴露：4 个缓存都充足的候选按余量排序 B>C>D>E，
    // 复核时 B、C 已耗尽（跳过），D、E 合格 → 胜者必须是优先级更高的 D
    // （并发不改变"有余额的最高优先级候选"语义）。
    (globalThis as any).__fetchDelayMs = 25;
    useStore.setState({ candidateVerifyConcurrency: 3 });
    const progressSeen: any[] = [];
    const unsub = useStore.subscribe((s: any) => {
      if (s.switchVerifyProgress) {
        progressSeen.push({
          total: s.switchVerifyProgress.total,
          done: s.switchVerifyProgress.done,
          failed: s.switchVerifyProgress.failed,
          active: [...s.switchVerifyProgress.active],
        });
      }
    });
    try {
      await arm(["A", "B", "C", "D", "E"], "A", {
        A: () => ({ balances: [tok(0)] }),
        B: (n: number) => ({ balances: [tok(n === 1 ? 900 : 0)] }),
        C: (n: number) => ({ balances: [tok(n === 1 ? 800 : 0)] }),
        D: () => ({ balances: [tok(700)] }),
        E: () => ({ balances: [tok(600)] }),
      });
      await cycle();
    } finally {
      unsub();
      (globalThis as any).__fetchDelayMs = 0;
    }
    const sw = calls("switch_to");
    assert(sw.length === 1 && sw[0].args.id === "D",
      "并发验证下应切到有余额的最高优先级候选 D，实际 " + JSON.stringify(sw));
    const inflight = (globalThis as any).__inflight || { max: 0 };
    assert(inflight.max === 3,
      "在飞验证数应恰为并发上限 3（批量刷新是串行的），实际 max=" + inflight.max);
    assert(progressSeen.length > 0, "验证期间应通过 switchVerifyProgress 暴露进度");
    assert(progressSeen.some((p) => p.active.length === 3),
      "应能观察到 3 个候选同时验证中，进度快照 " + JSON.stringify(progressSeen));
    const last = progressSeen[progressSeen.length - 1];
    assert(last.total === 4, "进度 total 应等于候选数 4，实际 " + JSON.stringify(last));
    assert(last.done === 3 && last.failed === 2,
      "B/C 复核耗尽 + D 通过后即决出胜者：done=3 failed=2，实际 " + JSON.stringify(last));
    assert(state().switchVerifyProgress === null, "验证结束后进度应清空");
  },

  async S7() {
    // 单候选超时：B 复核请求挂死 → candidateVerifyTimeoutMs 后按不合格跳过，
    // 切到后续合格的 C，整个流程不被挂死拖住。
    (globalThis as any).__fetchDelayMs = 0;
    await arm(["A", "B", "C"], "A", {
      A: () => ({ balances: [tok(0)] }),
      // 第 1 次调用是批量刷新（返回充足缓存），第 2 次是验证：永远不返回。
      B: (n: number) => (n === 1 ? { balances: [tok(900)] } : new Promise(() => {})),
      C: () => ({ balances: [tok(800)] }),
    });
    useStore.setState({ candidateVerifyTimeoutMs: 400 });
    const timeoutGuard = new Promise((_, reject) =>
      setTimeout(() => reject(new Error("单候选超时未生效：验证被挂死的候选拖住")), 8000)
    );
    await Promise.race([cycle(), timeoutGuard]);
    const sw = calls("switch_to");
    assert(sw.length === 1 && sw[0].args.id === "C",
      "挂死候选应被超时跳过并切到 C，实际 " + JSON.stringify(sw));
    assert(state().autoSwitchPaused === false, "切到合格候选后不应进入自动暂停");
  },

  async S8a() {
    // 切套餐不得影响当前正在执行的任务（CDP live 路径）：
    // kill_zcode_for_switch / restart_zcode 调用次数都必须为 0。
    // 单帐号 + 当前 start-plan 入口低、coding-plan 入口充足，
    // stub 返回 applied_live=true；store 应只调 switch_plan。
    await arm(["A"], "A", {
      A: () => ({
        balances: [tok(0), pt(5000)],
        active_provider: "coding-plan:builtin:bigmodel-start-plan",
      }),
    });
    await cycle();
    assert(calls("switch_plan").length === 1,
      "S8a 应执行切套餐，实际 " + JSON.stringify(calls("switch_plan")));
    assert(calls("kill_zcode_for_switch").length === 0,
      "S8a CDP live 路径不得调用 kill_zcode_for_switch，实际 " +
        JSON.stringify(calls("kill_zcode_for_switch")));
    assert(calls("restart_zcode").length === 0,
      "S8a CDP live 路径不得调用 restart_zcode，实际 " +
        JSON.stringify(calls("restart_zcode")));
  },

  async S8b() {
    // 切套餐不得影响当前正在执行的任务（in-place 路径）：
    // applied_live=false 且 autoRestart=true 时 store.ts:630-633
    // 会调 restartZcode 让配置生效，但 kill_zcode_for_switch 仍必须为 0。
    // 这是当前实现编码的不变量：切套餐 ≠ 切号，kill 永远不进切套餐路径。
    (globalThis as any).__planNotLive = true;
    await arm(["A"], "A", {
      A: () => ({
        balances: [tok(0), pt(5000)],
        active_provider: "coding-plan:builtin:bigmodel-start-plan",
      }),
    });
    useStore.setState({ autoRestart: true, tryNoRestartSwitch: false });
    try {
      await cycle();
    } finally {
      (globalThis as any).__planNotLive = false;
    }
    assert(calls("switch_plan").length === 1,
      "S8b in-place 路径应执行切套餐，实际 " + JSON.stringify(calls("switch_plan")));
    // 关键不变量：即便走 restart 分支，kill 也必须为 0。
    assert(calls("kill_zcode_for_switch").length === 0,
      "S8b in-place 路径也绝对不得调用 kill_zcode_for_switch（切套餐≠切号），实际 " +
        JSON.stringify(calls("kill_zcode_for_switch")));
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
for (const name of ["S1", "S2", "S3S4", "S5", "S6", "S7", "S8a", "S8b"]) {
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
