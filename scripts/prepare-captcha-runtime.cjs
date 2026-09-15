#!/usr/bin/env node
// scripts/prepare-captcha-runtime.cjs
//
// 为 Tauri 主程序内的验证码求解器准备独立的 Node 运行时。
//
// 设计依据：src-tauri/src/captcha.rs 的 solver_candidates() 会按以下顺序查找
// captcha-solver.cjs：
//   1) $ZCODE_CAPTCHA_SOLVER
//   2) <resource_dir>/captcha-runtime/captcha-solver.cjs
//   3) <cwd>/scripts/captcha-runtime/captcha-solver.cjs
//   4) <cwd>/scripts/captcha-solver.cjs
//   5) <exe_dir>/captcha-runtime/captcha-solver.cjs
//   6) <exe_dir>/resources/captcha-runtime/captcha-solver.cjs
//
// 并且 solver_is_usable() 要求脚本所在目录必须有 node_modules/jsdom，
// 否则运行时会 MODULE_NOT_FOUND。因此本脚本：
//   1) 把 captcha-solver.cjs 拷到 scripts/captcha-runtime/（若不存在则给出错误）
//   2) 在该目录单独 npm install jsdom，避免污染项目根 node_modules

const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");

const ROOT = path.resolve(__dirname, "..");
const RUNTIME_DIR = path.join(__dirname, "captcha-runtime");
const SOLVER_DEST = path.join(RUNTIME_DIR, "captcha-solver.cjs");

const SOURCE_CANDIDATES = [
  path.join(ROOT, "scripts", "captcha-solver.cjs"),
  path.join(ROOT, "captcha-solver.cjs"),
  path.join(ROOT, "src-tauri", "captcha-solver.cjs"),
];

function findSource() {
  for (const candidate of SOURCE_CANDIDATES) {
    if (fs.existsSync(candidate)) return candidate;
  }
  return null;
}

function main() {
  fs.mkdirSync(RUNTIME_DIR, { recursive: true });

  const source = findSource();
  if (!source) {
    console.error(
      "[prepare-captcha-runtime] 找不到 captcha-solver.cjs 源文件。\n" +
        "  期望位置之一：\n" +
        SOURCE_CANDIDATES.map((p) => "    - " + p).join("\n") +
        "\n  请把求解器脚本放到上述任一位置后重试。"
    );
    process.exit(1);
  }

  fs.copyFileSync(source, SOLVER_DEST);
  console.log(
    `[prepare-captcha-runtime] 已复制 ${path.relative(ROOT, source)} -> ${path.relative(
      ROOT,
      SOLVER_DEST
    )}`
  );

  // 在 runtime 目录里独立装 jsdom，避免主项目 package.json 锁住 jsdom 版本
  const pkgPath = path.join(RUNTIME_DIR, "package.json");
  if (!fs.existsSync(pkgPath)) {
    fs.writeFileSync(
      pkgPath,
      JSON.stringify({ name: "zcode-captcha-runtime", private: true, version: "0.0.0" }, null, 2)
    );
  }

  const hasJsdom = fs.existsSync(path.join(RUNTIME_DIR, "node_modules", "jsdom"));
  if (hasJsdom) {
    console.log("[prepare-captcha-runtime] node_modules/jsdom 已存在，跳过 install");
    return;
  }

  console.log("[prepare-captcha-runtime] 安装 jsdom (npm install --no-save jsdom)...");
  const result = spawnSync("npm", ["install", "--no-save", "--no-audit", "--no-fund", "jsdom"], {
    cwd: RUNTIME_DIR,
    stdio: "inherit",
    shell: process.platform === "win32",
  });
  if (result.status !== 0) {
    console.error("[prepare-captcha-runtime] npm install jsdom 失败");
    process.exit(result.status || 1);
  }
  console.log("[prepare-captcha-runtime] 完成。运行时：" + path.relative(ROOT, RUNTIME_DIR));
}

main();
