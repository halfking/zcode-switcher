#!/usr/bin/env node
// scripts/preview-notice.mjs
//
// 读取 public/notice.json 并按 src/lib/notices.ts 的解析规则校验/预览。
//
// 用法：
//   node scripts/preview-notice.mjs           打印所有启用的通知
//   node scripts/preview-notice.mjs --check   CI 模式：发现问题时 exit 1

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, "..");
const NOTICE_PATH = path.join(ROOT, "public", "notice.json");

const LANGS = ["zh", "en", "ru"];
const KINDS = new Set(["system", "temporary"]);
const LEVELS = new Set(["info", "warn", "error"]);

function readNoticeFile() {
  if (!fs.existsSync(NOTICE_PATH)) {
    throw new Error(`找不到 ${path.relative(ROOT, NOTICE_PATH)}`);
  }
  const raw = fs.readFileSync(NOTICE_PATH, "utf8");
  return JSON.parse(raw);
}

function validate(payload) {
  const errors = [];
  if (!payload || typeof payload !== "object") {
    return { errors: ["notice.json 顶层不是对象"], notices: [] };
  }
  if (payload.enabled === false) {
    return { errors: [], notices: [], globallyDisabled: true };
  }
  if (!Array.isArray(payload.notices)) {
    return { errors: ["notice.json.notices 必须是数组"], notices: [] };
  }

  const notices = [];
  payload.notices.forEach((item, index) => {
    const where = `notices[${index}]`;
    if (!item || typeof item !== "object") {
      errors.push(`${where}: 不是对象`);
      return;
    }
    if (item.enabled === false) return; // 禁用的不算错误
    if (typeof item.id !== "string" || !item.id.trim()) {
      errors.push(`${where}: id 缺失或为空`);
    }
    if (!KINDS.has(item.kind)) {
      errors.push(`${where}: kind 必须是 system|temporary，实际是 ${JSON.stringify(item.kind)}`);
    }
    if (item.level !== undefined && !LEVELS.has(item.level)) {
      errors.push(`${where}: level 必须是 info|warn|error，实际是 ${JSON.stringify(item.level)}`);
    }
    const titleOk = checkLocalized(item.title, `${where}.title`, errors);
    const bodyOk = checkLocalized(item.body, `${where}.body`, errors);
    if (titleOk && bodyOk) notices.push({ index, id: item.id, item });
  });

  return { errors, notices };
}

function checkLocalized(value, label, errors) {
  if (value == null) {
    errors.push(`${label}: 缺失`);
    return false;
  }
  if (typeof value === "string") return value.trim().length > 0;
  if (typeof value !== "object") {
    errors.push(`${label}: 必须是 string 或 {zh,en,ru}`);
    return false;
  }
  const hasAny = LANGS.some((l) => typeof value[l] === "string" && value[l].trim());
  if (!hasAny) errors.push(`${label}: 至少需要 zh/en/ru 之一`);
  return hasAny;
}

function render(notices) {
  if (notices.length === 0) {
    console.log("(没有启用的通知)");
    return;
  }
  for (const { id, item } of notices) {
    const title = pickText(item.title);
    const body = pickText(item.body);
    console.log(`\n— [${item.kind}${item.level ? "/" + item.level : ""}] ${id}  (${item.date || "无日期"})`);
    console.log("  标题: " + title);
    console.log("  正文: " + body);
    if (item.showOnce === false) console.log("  每次启动都显示");
    if (item.showOnStartup === false) console.log("  不在启动时显示");
  }
}

function pickText(value) {
  if (typeof value === "string") return value;
  return value.zh ?? value.en ?? value.ru ?? "";
}

const isCheck = process.argv.includes("--check");
let payload;
try {
  payload = readNoticeFile();
} catch (e) {
  if (isCheck) {
    console.error("[notice:check] FAIL -", e.message);
    process.exit(1);
  }
  console.error(e.message);
  process.exit(1);
}

const { errors, notices, globallyDisabled } = validate(payload);

if (isCheck) {
  if (errors.length > 0) {
    console.error("[notice:check] FAIL");
    for (const err of errors) console.error("  - " + err);
    process.exit(1);
  }
  if (globallyDisabled) {
    console.log("[notice:check] OK - 全局禁用 (enabled=false)");
  } else {
    console.log(`[notice:check] OK - ${notices.length} 条启用通知`);
  }
} else {
  console.log(`来源: ${path.relative(ROOT, NOTICE_PATH)}`);
  if (globallyDisabled) {
    console.log("状态: 全局禁用 (enabled=false)");
  } else {
    console.log(`状态: ${notices.length} 条启用通知`);
    if (errors.length > 0) {
      console.log("\n警告:");
      for (const err of errors) console.log("  - " + err);
    }
    render(notices);
  }
}
