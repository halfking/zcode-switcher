// scripts/install-toolchain/check.mjs —— 跨平台工具链状态总览。
//
// 与各平台 install 脚本的 --status 模式等价，但用纯 node 跑：
//   - 不依赖 PowerShell / bash
//   - 输出结构化 JSON 方便 CI 消费（--json）
//   - 同一份代码适配 windows / macos / linux（CI 上同时跑三种 runner 也能用）
//
// 用法：
//   node scripts/install-toolchain/check.mjs           # 人类可读
//   node scripts/install-toolchain/check.mjs --json    # 结构化
//   node scripts/install-toolchain/check.mjs --strict  # 任一缺失即 exit 1
//
// npm run env:check  ← 上面这行的便捷别名

import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir, platform, tmpdir } from "node:os";
import { join, resolve } from "node:path";

const args = new Set(process.argv.slice(2));
const JSON_OUT = args.has("--json");
const STRICT = args.has("--strict");

const REPO_ROOT = resolve(new URL("../..", import.meta.url).pathname);
const OS = platform(); // 'win32' | 'darwin' | 'linux'

/** Run a command, capture stdout. Returns {ok, stdout, stderr} or {ok:false, error}. */
function probe(cmd, args = []) {
  try {
    const stdout = execFileSync(cmd, args, {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
      timeout: 8000,
    });
    return { ok: true, stdout: stdout.trim() };
  } catch (e) {
    return { ok: false, error: (e.stderr || e.message || String(e)).toString().trim() };
  }
}

function checkNode() {
  const localWin = join(REPO_ROOT, ".tools", "node-v20.19.5-win-x64", "node.exe");
  if (OS === "win32" && existsSync(localWin)) {
    const r = probe(localWin, ["--version"]);
    return r.ok
      ? { tool: "node", status: "installed", detail: `local ${r.stdout} at .tools/` }
      : { tool: "node", status: "broken", detail: r.error };
  }
  const r = probe("node", ["--version"]);
  return r.ok
    ? { tool: "node", status: "installed", detail: r.stdout }
    : { tool: "node", status: "missing", detail: "node not on PATH" };
}

function checkRust() {
  // rustup-init 默认装到 ~/.cargo/bin/，新 shell 才加入 PATH。
  // 探测时也检查这个位置，避免 "PATH 里没 cargo" 误报。
  const candidates = OS === "win32"
    ? [join(homedir(), ".cargo", "bin", "rustc.exe")]
    : [join(homedir(), ".cargo", "bin", "rustc")];
  for (const c of candidates) {
    if (existsSync(c)) {
      const r = probe(c, ["--version"]);
      if (r.ok) {
        const a = probe(c.replace(/rustc(\.exe)?$/, "rustup" + (OS === "win32" ? ".exe" : "")), [
          "show",
          "active-toolchain",
        ]);
        const active = a.ok ? a.stdout.replace(/\s+/g, " ") : "";
        return { tool: "rustc", status: "installed", detail: `${r.stdout}${active ? "  (" + active + ")" : ""}` };
      }
    }
  }
  const r = probe("rustc", ["--version"]);
  if (r.ok) return { tool: "rustc", status: "installed", detail: r.stdout };
  return { tool: "rustc", status: "missing", detail: "rustc not on PATH and not at ~/.cargo/bin/" };
}

function checkGcc() {
  if (OS !== "win32") return { tool: "gcc", status: "skipped", detail: "non-Windows uses system cc" };
  const r = probe("gcc", ["--version"]);
  if (!r.ok) return { tool: "gcc", status: "missing", detail: "needed for GNU toolchain" };
  return { tool: "gcc", status: "installed", detail: r.stdout.split("\n")[0] };
}

function checkMsvc() {
  if (OS !== "win32") return { tool: "msvc-link", status: "skipped", detail: "non-Windows" };
  // 探测 cl.exe 极贵（要拉 vswhere 之类）；只看是否装过 VC build tools 的痕迹。
  const programFiles = process.env["ProgramFiles(x86)"] || process.env.ProgramFiles || "C:/Program Files";
  const vswhere = join(programFiles, "Microsoft Visual Studio", "Installer", "vswhere.exe");
  if (!existsSync(vswhere)) {
    return { tool: "msvc-link", status: "missing", detail: "Visual Studio Build Tools not detected" };
  }
  const r = probe(vswhere, ["-latest", "-property", "installationPath"]);
  return r.ok
    ? { tool: "msvc-link", status: "installed", detail: r.stdout }
    : { tool: "msvc-link", status: "missing", detail: "vswhere did not find an installation" };
}

function checkWebkit() {
  if (OS !== "linux") return { tool: "webkit2gtk-4.1", status: "skipped", detail: "non-Linux" };
  const r = probe("pkg-config", ["--modversion", "webkit2gtk-4.1"]);
  return r.ok
    ? { tool: "webkit2gtk-4.1", status: "installed", detail: r.stdout }
    : { tool: "webkit2gtk-4.1", status: "missing", detail: "tauri linux build will fail without it" };
}

function checkNsis() {
  if (OS !== "win32") return { tool: "nsis", status: "skipped", detail: "only needed for NSIS bundling" };
  const nsisDir = join(process.env.LOCALAPPDATA || join(homedir(), "AppData", "Local"), "tauri", "NSIS");
  const makensis = join(nsisDir, "makensis.exe");
  return existsSync(makensis)
    ? { tool: "nsis", status: "installed", detail: nsisDir }
    : { tool: "nsis", status: "missing", detail: `expected at ${nsisDir} (needed for tauri build)` };
}

function checkXcodeClt() {
  if (OS !== "darwin") return { tool: "xcode-clt", status: "skipped", detail: "macOS only" };
  const r = probe("xcode-select", ["-p"]);
  return r.ok
    ? { tool: "xcode-clt", status: "installed", detail: r.stdout }
    : { tool: "xcode-clt", status: "missing", detail: "needed for codesign / notarytool" };
}

const results = [
  checkNode(),
  checkRust(),
  checkGcc(),
  checkMsvc(),
  checkNsis(),
  checkXcodeClt(),
  checkWebkit(),
];

if (JSON_OUT) {
  const summary = {
    os: OS,
    repo: REPO_ROOT,
    tmpdir: tmpdir(),
    tools: results,
    ok: results.every((r) => r.status === "installed" || r.status === "skipped"),
  };
  process.stdout.write(JSON.stringify(summary, null, 2) + "\n");
  if (STRICT && !summary.ok) process.exit(1);
} else {
  console.log(`\n=== zcode-switcher 工具链状态 (${OS}) ===`);
  for (const r of results) {
    const tag =
      r.status === "installed" ? "[OK]" :
      r.status === "skipped"   ? "[--]" :
      r.status === "broken"    ? "[!!]" :
                                 "[??]";
    const color =
      r.status === "installed" ? "\x1b[32m" :
      r.status === "skipped"   ? "\x1b[90m" :
      r.status === "broken"    ? "\x1b[31m" :
                                 "\x1b[33m";
    console.log(`${color}${tag}\x1b[0m ${r.tool.padEnd(15)} ${r.status.padEnd(10)} ${r.detail}`);
  }
  const missing = results.filter((r) => r.status === "missing" || r.status === "broken");
  if (missing.length === 0) {
    console.log("\n全部就绪。");
  } else {
    console.log(`\n缺失：${missing.map((r) => r.tool).join(", ")}`);
    console.log("运行对应平台 install 脚本补齐：");
    if (OS === "win32") {
      console.log("  powershell -File scripts/install-toolchain/windows.ps1");
    } else if (OS === "darwin") {
      console.log("  bash scripts/install-toolchain/macos.sh");
    } else {
      console.log("  bash scripts/install-toolchain/linux.sh");
    }
    if (STRICT) process.exit(1);
  }
}
