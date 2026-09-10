import assert from "node:assert/strict";
import { build } from "esbuild";
import { JSDOM } from "jsdom";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const dir = await mkdtemp(join(tmpdir(), "zcode-settings-test-"));
const entry = join(dir, "entry.tsx");
const source = `
  import React from "${join(process.cwd(), "node_modules/react/index.js")}";
  import { createRoot } from "${join(process.cwd(), "node_modules/react-dom/client.js")}";
  import SettingsPanel from ${JSON.stringify(join(process.cwd(), "src/components/SettingsPanel.tsx"))};
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
  "../store": mock("store.ts", `import { useSyncExternalStore } from "${join(process.cwd(), "node_modules/react/index.js")}"; const listeners = new Set(); const notify = () => listeners.forEach(f => f()); let state = { quotaRefreshIntervalMinutes: 0, setQuotaRefreshIntervalMinutes(){}, activeQuotaRefreshIntervalMinutes: 1, setActiveQuotaRefreshIntervalMinutes(){}, glm52AutoSwitchEnabled: false, setGlm52AutoSwitchEnabled(){}, glm52AutoSwitchThresholdWan: 10, setGlm52AutoSwitchThresholdWan(){}, autoRestart: false, setAutoRestart(){}, tryNoRestartSwitch: true, setTryNoRestartSwitch(v){ state = { ...state, tryNoRestartSwitch: v }; notify(); }, theme: "dark", setTheme(){}, floatingWindowMode: false, setFloatingWindowMode(){}, language: "en", toast(text){ globalThis.toasts.push(text); } }; export function useStore(selector) { useSyncExternalStore((f) => { listeners.add(f); return () => listeners.delete(f); }, () => (selector ? selector(state) : state)); return selector ? selector(state) : state; }`),
  "../lib/api": mock("api.ts", `export const api = { openConfigDir: async () => {} };`),
  "./Modal": mock("modal.tsx", `export function UpdateModal(){ return null; }`),
};
const iconNames = ["Download", "ExternalLink", "FolderOpen", "Info", "ShieldCheck", "Power", "Zap", "Clock", "Moon", "Sun", "Check"];
const lucide = iconNames.map((n) => `export const ${n} = () => null;`).join("\n");
const bundle = join(dir, "bundle.mjs");
await build({ entryPoints: [entry], bundle: true, format: "esm", outfile: bundle, platform: "browser", jsx: "automatic", plugins: [{ name: "mocks", setup(b) { b.onResolve({ filter: /.*/ }, (args) => { if (mocks[args.path]) return { path: mocks[args.path] }; if (args.path === "lucide-react") return { path: mock("lucide.ts", lucide) }; return undefined; }); b.onLoad({ filter: new RegExp(dir.replace(/[.*+?^${}()|[\\]\\]/g, "\\\\$&")) }, (args) => virtual.has(args.path) ? { contents: virtual.get(args.path), loader: args.path.endsWith(".tsx") ? "tsx" : "ts" } : undefined); } }] });

const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost" });
Object.assign(globalThis, { window: dom.window, document: dom.window.document, toasts: [] });
Object.defineProperty(globalThis, "navigator", { value: dom.window.navigator, configurable: true });
Object.defineProperty(dom.window.navigator, "userAgent", { value: "Macintosh; Intel Mac OS X" });
await import(bundle);
globalThis.renderSettings();
await new Promise((r) => setTimeout(r, 100));
const toggle = document.querySelector('[role="switch"]');
assert.equal(toggle.getAttribute("aria-checked"), "false", "disabled discovered ZCode must start off");
toggle.click();
await new Promise((r) => setTimeout(r, 20));
assert.ok(globalThis.calls.includes("zcode_launcher_enable"), "enable is invoked through the toggle UI");
assert.equal(toggle.getAttribute("aria-checked"), "true", "successful enable turns on");
toggle.click();
await new Promise((r) => setTimeout(r, 100));
assert.deepEqual(globalThis.calls.slice(-2), ["zcode_launcher_disable", "zcode_launcher_scan"]);
assert.equal(toggle.getAttribute("aria-checked"), "false", "disable turns off only after backend success");
console.log("PASS settings panel macOS launcher toggle regression");
await rm(dir, { recursive: true, force: true });
