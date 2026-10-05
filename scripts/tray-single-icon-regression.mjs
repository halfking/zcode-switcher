// 系统托盘单图标回归：macOS 菜单栏曾经出现两块（一个标准图标 + 一个白图标加 "ZCode Switcher" 文字）。
//
// 机制（Tauri v2）：`App::build` 阶段会为配置里的 `app.trayIcon` 自动创建一个 id 固定 "main"
// 的托盘（tauri-2.x src/app.rs：`if let Some(tray_config) = &config.app.tray_icon { … tray.build() }`）。
// 同时 lib.rs 的 setup 又用 `TrayIconBuilder::with_id("zcode-switcher-tray")` 建了第二个。
// 两个 id 不同 ⇒ 两个独立 NSStatusItem ⇒ 菜单栏两块。配置那块的 `title` 让 macOS 在图标旁
// 渲染 "ZCode Switcher" 文字，`iconAsTemplate: true` 让它变成单色（浅色菜单栏下看起来是"白图标"）。
//
//   T1  所有 tauri 配置文件都不含 trayIcon / tray-icon 声明（trayIcon 只能有一个来源：代码）。
//   T2  Rust 侧托盘构建点唯一（`TrayIconBuilder::with_id` 全仓恰好 1 处，且有明确 id）。
//   T3  保留的那个托盘功能完整：有 Show / Quit 菜单项，防止"为去重把交互删了"。
//   T4  macOS 打包仍走 tauri.macos.conf.json（说明 T1 守的正是 macOS 生效的那份配置），
//       且该文件仍是合法 JSON、仍产出 dmg。
//
// 失败 exit 1。
//
// 用法：node scripts/tray-single-icon-regression.mjs [root]
// 可选 root 用于对仓库副本跑同一批断言（负控验证：把托盘改回两块，门必须转红），
// 避免为了证明门有效而去改真实工作区。
import { readFileSync, readdirSync } from "node:fs";
import { join, resolve } from "node:path";

const root = process.argv[2] ? resolve(process.argv[2]) : resolve(import.meta.dirname, "..");
const tauriDir = join(root, "src-tauri");

let failed = 0;
const check = (id, name, ok, detail) => {
  if (ok) {
    console.log(`  ok   ${id}  ${name}`);
  } else {
    failed += 1;
    console.log(`  FAIL ${id}  ${name}\n       ${detail}`);
  }
};

// ---------------------------------------------------------------- T1
// 扫所有 tauri 配置文件（不写死文件名，将来新增 macos/linux 专用配置也自动纳入）。
const confFiles = readdirSync(tauriDir).filter((f) => /^tauri.*\.conf\.json$/.test(f));
if (confFiles.length === 0) {
  failed += 1;
  console.log("  FAIL T1  找到 tauri 配置文件\n       src-tauri 下没有任何 tauri*.conf.json");
} else {
  for (const f of confFiles) {
    const text = readFileSync(join(tauriDir, f), "utf8");
    // 只看键名，不看注释/字符串值：声明式托盘的三种写法都要拦住。
    const decl = text.match(/"tray-?[Ii]con"/);
    check(
      "T1",
      `${f} 不声明 trayIcon`,
      !decl,
      `发现声明式托盘 ${decl?.[0]}；Tauri 会自动再建一个 id="main" 的托盘，与代码里的托盘重复。移除它。`,
    );
  }
}

// ---------------------------------------------------------------- T2
const srcDir = join(tauriDir, "src");
const rustFiles = [];
const walk = (dir) => {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, entry.name);
    if (entry.isDirectory()) walk(p);
    else if (entry.name.endsWith(".rs")) rustFiles.push(p);
  }
};
walk(srcDir);

const builders = [];
for (const f of rustFiles) {
  const lines = readFileSync(f, "utf8").split(/\r?\n/);
  lines.forEach((line, i) => {
    // 注释里提到 TrayIconBuilder 不算构建点。
    if (/^\s*(\/\/|\/\*|\*)/.test(line)) return;
    if (line.includes("TrayIconBuilder::with_id")) {
      builders.push(`${f.slice(root.length + 1)}:${i + 1}`);
    }
  });
}
check(
  "T2",
  "Rust 侧托盘构建点唯一",
  builders.length === 1,
  `期望恰好 1 处 TrayIconBuilder::with_id，实际 ${builders.length} 处：${builders.join(", ") || "(无)"}`,
);
const trayRs = readFileSync(join(srcDir, "tray.rs"), "utf8");
check(
  "T2",
  "托盘构建在 tray.rs 且 id 明确",
  /TrayIconBuilder::with_id\("zcode-switcher-tray"\)/.test(trayRs),
  "tray.rs 里应保留 with_id(\"zcode-switcher-tray\") 的构建点",
);
// 显式 title 会在 macOS 菜单栏渲染文字；唯一托盘不应带 title。
const titleInTray = trayRs
  .split(/\r?\n/)
  .find((line) => !/^\s*(\/\/|\/\*|\*)/.test(line) && line.includes(".title("));
check(
  "T2",
  "程序化托盘不设 title（避免菜单栏出现文字）",
  !titleInTray,
  `tray.rs 不应对托盘调用 .title(...)，否则菜单栏图标旁会多出一段文字：${titleInTray}`,
);

// ---------------------------------------------------------------- T3
const menuIds = [...trayRs.matchAll(/MenuItem::with_id\([^,]+,\s*"([^"]+)"/g)].map((m) => m[1]);
check(
  "T3",
  "托盘菜单含 show / quit",
  menuIds.includes("show") && menuIds.includes("quit"),
  `期望菜单项包含 show 与 quit，实际：${menuIds.join(", ") || "(无)"}`,
);
check(
  "T3",
  "setup_tray 仍被 lib.rs 调用",
  /tray::setup_tray\(/.test(readFileSync(join(srcDir, "lib.rs"), "utf8")),
  "lib.rs 的 setup 里应调用 tray::setup_tray，否则关窗后无法从托盘唤回",
);

// ---------------------------------------------------------------- T4
const workflow = readFileSync(join(root, ".github", "workflows", "build-macos.yml"), "utf8");
check(
  "T4",
  "macOS 打包使用 tauri.macos.conf.json",
  workflow.includes("src-tauri/tauri.macos.conf.json"),
  "build-macos.yml 应通过 --config 指定 macOS 专用配置；否则 T1 守的配置文件在 macOS 上不生效",
);
const macConf = JSON.parse(readFileSync(join(tauriDir, "tauri.macos.conf.json"), "utf8"));
check(
  "T4",
  "macOS 配置仍是合法 JSON 且产出 dmg",
  Array.isArray(macConf?.bundle?.targets) && macConf.bundle.targets.includes("dmg"),
  `bundle.targets 应含 dmg，实际：${JSON.stringify(macConf?.bundle?.targets)}`,
);

console.log(failed === 0 ? "\ntray-single-icon: 全部通过" : `\ntray-single-icon: ${failed} 项失败`);
process.exit(failed === 0 ? 0 : 1);
