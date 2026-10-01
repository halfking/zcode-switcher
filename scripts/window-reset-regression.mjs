// 重置窗口（5 小时/周）特性回归：卡片计数、到点待刷新判定、倒计时文案
// 与 store 的 refreshQuotaForWindowReset 布线。
//
//   W1  countWindowCards：只统计积分卡（point），token/工具桶不计数，
//       多账号累加。
//   W2  pendingWindowResets：缓存早于重置时刻的桶才算"待重置"；
//       已反映（fetched_at >= reset）、错误态、无 next_reset_at、
//       过期超 7 天的异常时间戳都不算。
//   W3  formatResetCountdown：秒 → "1m/3h12m/2d2h" 短文案，过期/非法返回空。
//   W4  composeWindowCardsLine：zh/en/ru 三语文案与零卡片隐藏。
//   W5  refreshQuotaForWindowReset（核心场景）：全部账号耗尽进入自动暂停后，
//       非当前账号的 5h 窗口到点重置 → 刷新该账号 → 重评自动切换：
//       解除暂停、清除网关拦截、切到恢复的账号。普通 refreshQuota 不触发
//       重评，这正是新 action 存在的理由。
//   W6  refreshQuotaForWindowReset 传入未知 id：不拉取、不抛错。
//
// 套路同 auto-switch-regression.mjs：esbuild 临时 bundle + 子进程逐场景运行
// （store 模块有跨调用状态，场景间必须隔离）。失败 exit 1 并保留 bundle 目录。
import { build } from "esbuild";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";

const cacheRoot = join(process.cwd(), "node_modules", ".cache", "windowreset-test");
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
    if (command === "set_quota_guard") {
      return { paused: !!(args && args.paused), reason: null, updated_at: 0 };
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
import {
  countWindowCards,
  dueWindowResetProfileIds,
  pendingWindowResets,
  formatResetCountdown,
} from ${JSON.stringify(posix(join(process.cwd(), "src/lib/glm52.ts")))};
import { composeWindowCardsLine, getTexts } from ${JSON.stringify(posix(join(process.cwd(), "src/i18n.ts")))};

const NOW = 1_800_000_000; // 固定"当前时间"（秒）

function assert(cond: unknown, msg: string): asserts cond {
  if (!cond) throw new Error("断言失败: " + msg);
}

const bal = (over: any) => ({
  show_name: "x", used_units: 0, total_units: 0, remaining_units: 0,
  unit_type: "point", period: "5h", plan_id: null, next_reset_at: null, ...over,
});

const scenarios: Record<string, () => Promise<void>> = {
  async W1() {
    const quotas: any = {
      A: { balances: [
        bal({ period: "5h" }),
        bal({ period: "weekly" }),
        bal({ period: "monthly" }),
        bal({ unit_type: "token", period: "weekly" }),   // token 桶不计数
        bal({ unit_type: "tool", period: "5h" }),         // 工具桶不计数
        bal({ period: null }),                            // mcp 汇总无周期不计数
      ]},
      B: { balances: [bal({ period: "5h" }), bal({ period: "weekly" })]},
    };
    const counts = countWindowCards(quotas);
    assert(counts.fiveHour === 2, "5h 卡应 2 张（A1+B1），实际 " + counts.fiveHour);
    assert(counts.weekly === 2, "周卡应 2 张（A1+B1），实际 " + counts.weekly);
    assert(counts.monthly === 1, "月卡应 1 张，实际 " + counts.monthly);
  },

  async W2() {
    const quotas: any = {
      A: { fetched_at: NOW - 100, balances: [bal({ next_reset_at: NOW + 1000 })] },
      B: { fetched_at: NOW + 2000, balances: [bal({ next_reset_at: NOW + 1000 })] }, // 已反映
      C: { error: "boom", fetched_at: NOW - 100, balances: [bal({ next_reset_at: NOW + 100 })] },
      D: { fetched_at: NOW - 9 * 86400, balances: [bal({ next_reset_at: NOW - 8 * 86400 })] }, // 过期>7天
      E: { fetched_at: NOW - 100, balances: [bal({})] }, // 无 next_reset_at
    };
    const pending = pendingWindowResets(quotas, NOW);
    assert(pending.length === 1 && pending[0].profileId === "A" && pending[0].resetAt === NOW + 1000,
      "只有 A 的未来窗口待重置，实际 " + JSON.stringify(pending));
  },

  async W3() {
    assert(formatResetCountdown(0) === "", "0 应无文案");
    assert(formatResetCountdown(-5) === "", "负数应无文案");
    assert(formatResetCountdown(NaN) === "", "NaN 应无文案");
    assert(formatResetCountdown(20) === "<1m", "20s → <1m，实际 " + formatResetCountdown(20));
    assert(formatResetCountdown(59) === "1m", "59s 四舍五入 → 1m，实际 " + formatResetCountdown(59));
    assert(formatResetCountdown(90) === "2m", "90s → 2m（取整到分），实际 " + formatResetCountdown(90));
    assert(formatResetCountdown(11520) === "3h12m", "11520s → 3h12m，实际 " + formatResetCountdown(11520));
    assert(formatResetCountdown(86400) === "1d0h", "86400s → 1d0h，实际 " + formatResetCountdown(86400));
    assert(formatResetCountdown(180000) === "2d2h", "180000s → 2d2h，实际 " + formatResetCountdown(180000));
  },

  async W4() {
    const zh = getTexts("zh"), en = getTexts("en"), ru = getTexts("ru");
    assert(composeWindowCardsLine(zh, { fiveHour: 2, weekly: 2, monthly: 1 }) ===
      "重置窗口：5小时卡 ×2 · 周卡 ×2 · 月卡 ×1",
      "zh 完整文案不符：" + composeWindowCardsLine(zh, { fiveHour: 2, weekly: 2, monthly: 1 }));
    assert(composeWindowCardsLine(zh, { fiveHour: 0, weekly: 0, monthly: 0 }) === "",
      "零卡片应返回空串");
    assert(composeWindowCardsLine(en, { fiveHour: 0, weekly: 1, monthly: 0 }) ===
      "Reset windows: weekly cards ×1",
      "en 只保留非零窗口：" + composeWindowCardsLine(en, { fiveHour: 0, weekly: 1, monthly: 0 }));
    assert(composeWindowCardsLine(ru, { fiveHour: 1, weekly: 0, monthly: 0 }) ===
      "Окна сброса: 5-часовых ×1",
      "ru 文案不符：" + composeWindowCardsLine(ru, { fiveHour: 1, weekly: 0, monthly: 0 }));
  },

  async W5() {
    const THRESHOLD_WAN = 50;
    const POINT_THRESHOLD = 1000;
    const profile = (id: string, active: boolean) => ({
      id, name: id, user_id: id, email: id + "@t.local", phone: "", avatar: "",
      cred_hash: id, cred_file: id + ".json", created_at: 0, updated_at: 0,
      active, short_id: id,
    });
    const tok = (wan: number) => ({
      show_name: "GLM-5.3", used_units: 0, total_units: wan * 10_000,
      remaining_units: wan * 10_000, unit_type: "token", period: "weekly", plan_id: "start-plan",
    });
    // 5h 积分卡带未来重置时刻：第 1 次拉取（批量刷新）返回耗尽，
    // 第 2 次起（窗口重置刷新 + 候选复核）返回已恢复。
    const pt = (n: number, resetAt: number | null) => ({
      show_name: "5小时积分", used_units: 0, total_units: Math.max(n, 1),
      remaining_units: n, unit_type: "point", period: "5h",
      plan_id: "personal:glm-coding", next_reset_at: resetAt,
    });
    (globalThis as any).__quotaScripts = {
      A: () => ({ balances: [tok(0), pt(0, null)], active_provider: "coding-plan:builtin:bigmodel-start-plan" }),
      B: (n: number) => n === 1
        ? { balances: [tok(0), pt(0, 1799999999)] }
        : { balances: [tok(900), pt(5000, 1800003600)] },
    };
    const st = useStore.getState();
    st.setGlm52AutoSwitchEnabled(true);
    st.setGlm52AutoSwitchThresholdWan(THRESHOLD_WAN);
    st.setGlm52AutoSwitchPointThreshold(POINT_THRESHOLD);
    st.setGlm52ExhaustRestartZcode(false);
    useStore.setState({
      profiles: [profile("A", true), profile("B", false)],
      quotas: {},
      autoSwitchPaused: false,
    });
    await useStore.getState().refreshAllQuota();
    const state = () => useStore.getState();
    const calls = (cmd: string) =>
      ((globalThis as any).__calls || []).filter((c: any) => c.cmd === cmd);
    assert(state().autoSwitchPaused === true, "第 1 轮全部耗尽应进入自动暂停");
    assert(calls("set_quota_guard").some((c: any) => c.args.paused === true),
      "进入暂停应开启网关拦截");
    assert(calls("switch_to").length === 0, "耗尽期间不得切号");

    // 窗口到点：只刷新 B（普通 refreshQuota 不会重评切换，这正是
    // refreshQuotaForWindowReset 的意义）。
    await useStore.getState().refreshQuotaForWindowReset(["B"]);
    assert(state().autoSwitchPaused === false, "窗口重置后应解除自动暂停");
    const sw = calls("switch_to");
    assert(sw.length === 1 && sw[0].args.id === "B",
      "应切到窗口恢复的 B，实际 " + JSON.stringify(sw));
    assert(calls("set_quota_guard").some((c: any) => c.args.paused === false),
      "恢复后应清除网关拦截");
    const counts = (globalThis as any).__fetchCounts || {};
    assert((counts.B ?? 0) >= 2, "B 应被重置刷新 + 候选复核至少各拉取一次，实际 " + (counts.B ?? 0));
  },

  async W6() {
    const before = ((globalThis as any).__calls || []).length;
    await useStore.getState().refreshQuotaForWindowReset(["ghost-id"]);
    const after = ((globalThis as any).__calls || []).length;
    assert(before === after, "未知 id 不应触发任何后端调用");
  },

  async W7() {
    // App.tsx 到点调度用的收集函数：只挑"过点且超过 settle 余量"的账号，
    // 同账号多桶去重；未过 settle、未来窗口、已反映的都不算。
    const S = 1000;
    const quotas: any = {
      A: { fetched_at: NOW - 400, balances: [bal({ next_reset_at: NOW - 60 })] },   // 过点 60s → due
      B: { fetched_at: NOW - 400, balances: [bal({ next_reset_at: NOW - 5 })] },    // 过点 5s < settle 15s
      C: { fetched_at: NOW - 400, balances: [bal({ next_reset_at: NOW + 600 })] },  // 未来
      D: { fetched_at: NOW - 400, balances: [
        bal({ next_reset_at: NOW - 60 }),
        bal({ next_reset_at: NOW - 120 }),
      ]},                                                                             // 两桶均到点 → 去重一次
      E: { fetched_at: NOW + 10, balances: [bal({ next_reset_at: NOW - 60 })] },     // 已反映
    };
    const due = dueWindowResetProfileIds(quotas, NOW * S, 15 * S);
    assert(due.length === 2 && due.includes("A") && due.includes("D"),
      "只有 A 与 D 到点可刷（去重），实际 " + JSON.stringify(due));
    // settle 之外的口径复核：settle=0 时 B 也应入列。
    const due0 = dueWindowResetProfileIds(quotas, NOW * S, 0);
    assert(due0.length === 3 && due0.includes("B"),
      "settle=0 时 B 也应到点，实际 " + JSON.stringify(due0));
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
for (const name of ["W1", "W2", "W3", "W4", "W5", "W6", "W7"]) {
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
  console.error(`== window-reset 回归：${failed} 个场景失败 ==`);
  console.error(`bundle 目录（保留供排查）: ${dir}`);
  process.exit(1);
}
await rm(dir, { recursive: true, force: true });
console.log("== window-reset 回归：全部通过 ==");
