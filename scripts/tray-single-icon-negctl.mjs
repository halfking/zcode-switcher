// 负控（手动运行，不参与 CI）：node scripts/tray-single-icon-negctl.mjs
//
// 作用：证明 tray-single-icon-regression.mjs 不是恒绿的门（哑门）。做法是在 .scratch 下的
// 临时仓库副本里把「双托盘」注入回去，门必须转红；还原后必须重新转绿（对照组）。
// 刻意只改副本、不碰真实工作区 —— 负控状态若落在共享工作区，会被并发会话的
// `git add -A && commit` 原样带进主干。
import { cpSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const root = resolve(import.meta.dirname, "..");
const scratch = join(root, ".scratch", "tray-negctl");
rmSync(scratch, { recursive: true, force: true });
mkdirSync(join(scratch, "src-tauri", "src"), { recursive: true });
mkdirSync(join(scratch, ".github", "workflows"), { recursive: true });

cpSync(join(root, "src-tauri", "tauri.conf.json"), join(scratch, "src-tauri", "tauri.conf.json"));
cpSync(join(root, "src-tauri", "tauri.macos.conf.json"), join(scratch, "src-tauri", "tauri.macos.conf.json"));
cpSync(join(root, "src-tauri", "src", "tray.rs"), join(scratch, "src-tauri", "src", "tray.rs"));
cpSync(join(root, "src-tauri", "src", "lib.rs"), join(scratch, "src-tauri", "src", "lib.rs"));
cpSync(join(root, ".github", "workflows", "build-macos.yml"), join(scratch, ".github", "workflows", "build-macos.yml"));

const run = (label) => {
  const r = spawnSync(process.execPath, [join(root, "scripts", "tray-single-icon-regression.mjs"), scratch], {
    encoding: "utf8",
  });
  console.log(`--- ${label} (exit=${r.status}) ---`);
  console.log((r.stdout || "").split("\n").filter((l) => l.trim()).join("\n"));
  return r.status;
};

const macConfPath = join(scratch, "src-tauri", "tauri.macos.conf.json");
const trayPath = join(scratch, "src-tauri", "src", "tray.rs");
const macConfOrig = readFileSync(macConfPath, "utf8");
const trayOrig = readFileSync(trayPath, "utf8");

let bad = 0;

// 负控 1：把声明式 trayIcon 加回 macOS 专用配置（= 1.1.15 那次修复漏掉的那一步）
writeFileSync(
  macConfPath,
  macConfOrig.replace(
    '    "security": {',
    '    "trayIcon": {\n      "iconPath": "icons/32x32.png",\n      "iconAsTemplate": true,\n      "menuOnLeftClick": false,\n      "title": "ZCode Switcher"\n    },\n    "security": {',
  ),
);
if (run("neg1 trayIcon back in macos.conf") === 0) {
  console.log("FAIL neg1 did not go red - the gate is mute");
  bad++;
}
writeFileSync(macConfPath, macConfOrig);

// 负控 2：代码里再建第二个托盘
writeFileSync(
  trayPath,
  trayOrig.replace(
    "    tray.build(app)?;",
    '    TrayIconBuilder::with_id("second-tray").build(app)?;\n    tray.build(app)?;',
  ),
);
if (run("neg2 second TrayIconBuilder in code") === 0) {
  console.log("FAIL neg2 did not go red - the gate is mute");
  bad++;
}
writeFileSync(trayPath, trayOrig);

// 负控 3：给唯一托盘加 title（菜单栏图标旁会多出文字）
writeFileSync(
  trayPath,
  trayOrig.replace(
    '    let mut tray = TrayIconBuilder::with_id("zcode-switcher-tray")',
    '    let mut tray = TrayIconBuilder::with_id("zcode-switcher-tray")\n        .title("ZCode Switcher")',
  ),
);
if (run("neg3 .title() on the only tray") === 0) {
  console.log("FAIL neg3 did not go red - the gate is mute");
  bad++;
}
writeFileSync(trayPath, trayOrig);

// 对照组：还原后必须重新转绿，否则说明门恒红
if (run("control restored") !== 0) {
  console.log("FAIL control did not go green");
  bad++;
}

rmSync(scratch, { recursive: true, force: true });
console.log(bad === 0 ? "\nnegctl passed: gate catches all three double-tray regressions" : `\nnegctl failed: ${bad}`);
process.exit(bad === 0 ? 0 : 1);
