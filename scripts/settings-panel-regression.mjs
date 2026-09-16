import assert from "node:assert/strict";
import { build } from "esbuild";
import { JSDOM } from "jsdom";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const dir = await mkdtemp(join(tmpdir(), "zcode-settings-test-"));
// Windows 路径反斜杠内嵌进生成的源码字符串会破坏语法（\U 等被当成转义），统一转成正斜杠
const posix = (p) => p.replace(/\\/g, "/");
const entry = join(dir, "entry.tsx");
const source = `
  import React from "${posix(join(process.cwd(), "node_modules/react/index.js"))}";
  import { createRoot } from "${posix(join(process.cwd(), "node_modules/react-dom/client.js"))}";
  import SettingsPanel from ${JSON.stringify(posix(join(process.cwd(), "src/components/SettingsPanel.tsx")))};
  globalThis.renderSettings = () => createRoot(document.getElementById("root")).render(React.createElement(SettingsPanel));
`;
await (await import("node:fs/promises")).writeFile(entry, source);

globalThis.calls = [];
globalThis.scan = [{ path: "/Applications/ZCode.app", target: "ZCode.app", arguments: "", hasFlag: false }];
globalThis.enableResult = { modified: 1, already: 0, total: 1 };
const virtual = new Map();
function mock(name, contents) { const p = join(dir, name); virtual.set(p, contents); return p; }
const mocks = {
  "@tauri-apps/api/core": mock("tauri-core.ts", `export async function invoke(command) { globalThis.calls.push(command); if (command === "zcode_launcher_scan") return globalThis.scan; if (command === "zcode_launcher_enable") { globalThis.scan = [{ ...globalThis.scan[0], hasFlag: true }]; return globalThis.enableResult; } if (command === "zcode_launcher_disable") { globalThis.scan = [{ ...globalThis.scan[0], hasFlag: false }]; return 1; } }`),
  "@tauri-apps/api/app": mock("tauri-app.ts", `export const getVersion = async () => "test";`),
  "@tauri-apps/plugin-opener": mock("opener.ts", `export const openUrl = async () => {};`),
  "@tauri-apps/plugin-process": mock("process.ts", `export const relaunch = async () => {};`),
  "@tauri-apps/plugin-updater": mock("updater.ts", `export const check = async () => null;`),
  "react": join(process.cwd(), "node_modules/react/index.js"),
  "react-dom/client": join(process.cwd(), "node_modules/react-dom/client.js"),
  "../store": mock("store.ts", `import { useSyncExternalStore } from "${posix(join(process.cwd(), "node_modules/react/index.js"))}"; const listeners = new Set(); const notify = () => listeners.forEach(f => f()); let state = { quotaRefreshIntervalMinutes: 0, setQuotaRefreshIntervalMinutes(){}, activeQuotaRefreshIntervalMinutes: 1, setActiveQuotaRefreshIntervalMinutes(){}, glm52AutoSwitchEnabled: false, setGlm52AutoSwitchEnabled(){}, glm52AutoSwitchThresholdWan: 10, setGlm52AutoSwitchThresholdWan(){}, autoRestart: false, setAutoRestart(){}, tryNoRestartSwitch: true, setTryNoRestartSwitch(v){ state = { ...state, tryNoRestartSwitch: v }; notify(); }, theme: "dark", setTheme(){}, floatingWindowMode: false, setFloatingWindowMode(){}, language: "en", toast(text){ globalThis.toasts.push(text); } }; export function useStore(selector) { useSyncExternalStore((f) => { listeners.add(f); return () => listeners.delete(f); }, () => (selector ? selector(state) : state)); return selector ? selector(state) : state; }`),
  "../lib/api": mock("api.ts", `export const api = { openConfigDir: async () => {} };`),
  "./Modal": mock("modal.tsx", `export function UpdateModal(){ return null; }`),
};
const iconNames = ["Download", "ExternalLink", "FolderOpen", "Info", "ShieldCheck", "Power", "Zap", "Clock", "Moon", "Sun", "Check"];
const lucide = iconNames.map((n) => `export const ${n} = () => null;`).join("\n");
const bundle = join(dir, "bundle.mjs");
await build({ entryPoints: [entry], bundle: true, format: "esm", outfile: bundle, platform: "browser", jsx: "automatic", plugins: [{ name: "mocks", setup(b) { b.onResolve({ filter: /.*/ }, (args) => { if (mocks[args.path]) return { path: mocks[args.path] }; if (args.path === "lucide-react") return { path: mock("lucide.ts", lucide) }; return undefined; }); b.onLoad({ filter: new RegExp(dir.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")) }, (args) => virtual.has(args.path) ? { contents: virtual.get(args.path), loader: args.path.endsWith(".tsx") ? "tsx" : "ts" } : undefined); } }] });

const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost" });
Object.assign(globalThis, { window: dom.window, document: dom.window.document, toasts: [] });
Object.defineProperty(globalThis, "navigator", { value: dom.window.navigator, configurable: true });
Object.defineProperty(dom.window.navigator, "userAgent", { value: "Macintosh; Intel Mac OS X" });
// Windows 下动态 import 需要 file:// URL，绝对路径（C:\...）会报 ERR_UNSUPPORTED_ESM_URL_SCHEME
await import(pathToFileURL(bundle).href);

// React 19 并发调度在 jsdom 里可能让 effect / 重渲染晚于固定 sleep 完成，
// 用轮询代替一次性等待（曾出现 ~20% 概率 100ms 不够导致的偶发失败）。
const toggleEl = () => document.querySelector('[role="switch"]');
async function waitFor(cond, desc, timeoutMs = 5000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    if (cond()) return;
    if (Date.now() > deadline) throw new Error(`waitFor 超时：${desc}`);
    await new Promise((r) => setTimeout(r, 10));
  }
}

globalThis.renderSettings();
await waitFor(
  () => toggleEl()?.getAttribute("aria-checked") === "false",
  "scan 结果把禁用的 ZCode 开关关掉"
);
toggleEl().click();
await waitFor(() => {
  assert.ok(globalThis.calls.includes("zcode_launcher_enable"), "enable is invoked through the toggle UI");
  return toggleEl()?.getAttribute("aria-checked") === "true";
}, "enable 成功后开关打开");
toggleEl().click();
await waitFor(
  () => toggleEl()?.getAttribute("aria-checked") === "false",
  "disable 后端成功后开关关闭"
);
assert.deepEqual(globalThis.calls.slice(-2), ["zcode_launcher_disable", "zcode_launcher_scan"]);
console.log("PASS settings panel macOS launcher toggle regression");
await rm(dir, { recursive: true, force: true });
