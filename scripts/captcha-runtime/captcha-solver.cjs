#!/usr/bin/env node
// scripts/captcha-solver.cjs  (PLACEHOLDER ONLY)
//
// ！！！此文件是占位实现，仅保证 scripts/prepare-captcha-runtime.cjs 能跑通、
//    Tauri captcha.rs 的求解器调度链路不会因 MODULE_NOT_FOUND 崩溃。
//
// 真实实现通常需要：
//   - 使用 jsdom 加载目标验证码页面
//   - 调用第三方打码平台 API（例如 recaptcha / hcaptcha / 自研滑动验证）
//   - 解析 token 后按 captcha.rs 约定的格式输出：
//       VERIFY_PARAM=<value>     （必须输出到 stdout）
//
// 占位实现仅打印演示 token，不会通过任何真实验证。
// 接入真实打码服务前请用真实求解器覆盖本文件。

const [sceneId, region, prefix] = process.argv.slice(2);

if (!sceneId || !region || !prefix) {
  console.error(
    "[captcha-solver] 用法: node captcha-solver.cjs <sceneId> <region> <prefix>"
  );
  process.exit(2);
}

// 故意加载 jsdom 以验证 prepare-captcha-runtime.cjs 是否成功安装了依赖。
// 真实求解器在解析验证码 DOM 时也会用到它。
let jsdomStatus = "jsdom not loaded";
try {
  const { JSDOM } = require("jsdom");
  const dom = new JSDOM("<!doctype html><html><body>captcha</body></html>");
  jsdomStatus = "jsdom " + (dom && dom.window ? "ok" : "missing");
} catch (e) {
  console.error("[captcha-solver] 无法加载 jsdom:", e && e.message);
  process.exit(3);
}

console.error(
  `[captcha-solver] PLACEHOLDER scene=${sceneId} region=${region} prefix=${prefix} (${jsdomStatus})`
);
console.log("VERIFY_PARAM=placeholder-not-a-real-token");
