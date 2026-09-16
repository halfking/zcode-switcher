//! 通过 Chrome DevTools Protocol 远程触发 ZCode 内的
//! `modelProviderService.refreshCodingPlanApiKey('builtin:zai-start-plan')`，
//! 让切号瞬时生效，不依赖用户当前在哪个 UI 页面。
//!
//! 前置：ZCode 必须用带 `--remote-debugging-port=9229` 的快捷方式启动
//! （由 zcode_launcher 模块负责改写）。否则这里所有调用都会静默失败。
//!
//! 注入流程：
//!   1. HTTP GET 127.0.0.1:9229/json/list 找到 type=page, title=ZCode 的渲染进程
//!   2. WS 连过去，发 Runtime.evaluate 跑一段 JS：
//!      - 走 React Fiber 树找 modelProviderService（首次发现后缓存到 window）
//!      - 调 refreshCodingPlanApiKey
//!   3. 等响应或超时（2s 上限），关闭 WS

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

const CDP_PORT: u16 = 9229;
const HTTP_TIMEOUT: Duration = Duration::from_millis(1500);
const WS_TIMEOUT: Duration = Duration::from_millis(3000);

/// 已知的 builtin:zai-* plan id 列表。导入别人 JSON 的用户，原账号可能用 coding-plan，
/// 我们这只更新 start-plan 的 apiKey 就会"切了号还是原来的"。所以全量刷一遍。
const ZAI_PLAN_IDS: &[&str] = &["builtin:zai-start-plan", "builtin:zai-coding-plan"];

/// 自包含 JS：找 service（带缓存）+ 依次刷新已知的 builtin:zai-* plan。返回 {ok, cached, refreshed, err?}
const INJECT_SCRIPT: &str = r#"(async () => {
  function getAnyFiber() {
    const cands = [document.body, document.getElementById('root'), document.documentElement];
    for (const el of cands) {
      if (!el) continue;
      const key = Object.keys(el).find(k => k.startsWith('__reactFiber$') || k.startsWith('__reactContainer$'));
      if (key) return el[key];
    }
    const all = document.querySelectorAll('*');
    for (let i = 0; i < Math.min(200, all.length); i++) {
      const key = Object.keys(all[i]).find(k => k.startsWith('__reactFiber$'));
      if (key) return all[i][key];
    }
    return null;
  }
  function getRoot(fiber) {
    let cur = fiber;
    while (cur.return) cur = cur.return;
    return cur;
  }
  function findService() {
    const anyFiber = getAnyFiber();
    if (!anyFiber) return null;
    const root = getRoot(anyFiber);
    const seen = new WeakSet();
    function deepFind(obj, depth) {
      if (!obj || depth > 3 || typeof obj !== 'object' || seen.has(obj)) return null;
      seen.add(obj);
      if (typeof obj.refreshCodingPlanApiKey === 'function') return obj;
      for (const k of ['modelProviderService', 'services', 'value']) {
        try {
          const v = obj[k];
          if (v && typeof v === 'object') {
            if (typeof v.refreshCodingPlanApiKey === 'function') return v;
            if (k === 'services' && v.modelProviderService &&
                typeof v.modelProviderService.refreshCodingPlanApiKey === 'function') {
              return v.modelProviderService;
            }
          }
        } catch {}
      }
      for (const k in obj) {
        try {
          const v = obj[k];
          if (v && typeof v === 'object') {
            const got = deepFind(v, depth + 1);
            if (got) return got;
          }
        } catch {}
      }
      return null;
    }
    const stack = [root];
    let count = 0;
    // 硬上限：覆盖到 ZCode 当前 fiber 节点规模的 ~3x 余量。WS_TIMEOUT=3s，
    // 实测 30 万节点遍历在 100-300ms 完成，60 万仍在 1s 以内；提高上限只为
    // 兼容未来 ZCode 渲染树变深。命中后立即返回，不写缓存，下次注入重走。
    while (stack.length && count < 600000) {
      const node = stack.pop();
      count++;
      for (const slot of ['memoizedProps', 'memoizedState', 'pendingProps', 'stateNode']) {
        const v = node[slot];
        if (!v) continue;
        const svc = deepFind(v, 0);
        if (svc) return svc;
      }
      if (node.child) stack.push(node.child);
      if (node.sibling) stack.push(node.sibling);
    }
    return null;
  }
  let svc = window.__zcsModelProviderService;
  let cached = !!svc;
  if (!svc) {
    svc = findService();
    if (svc) window.__zcsModelProviderService = svc;
  }
  if (!svc) return { ok: false, cached: false, err: 'service-not-found' };
  // 只刷 start-plan：它是 plan 入口（zcode.z.ai/zcode-plan），ZCode 内部
  // refreshCodingPlanApiKey 会自动连带处理同一 family，无需重复调 coding-plan。
  const planIds = ['builtin:zai-start-plan', 'builtin:bigmodel-start-plan'];
  const refreshed = [];
  const errs = [];
  for (const id of planIds) {
    try {
      await svc.refreshCodingPlanApiKey(id);
      refreshed.push(id);
    } catch (e) {
      errs.push(id + ':' + String(e));
    }
  }
  return refreshed.length > 0
    ? { ok: true, cached, refreshed, errs }
    : { ok: false, cached, err: errs.join(';') };
})()"#;

#[derive(Debug, Deserialize)]
struct CdpTarget {
    #[serde(rename = "type")]
    target_type: String,
    title: String,
    url: String,
    #[serde(rename = "webSocketDebuggerUrl")]
    ws_url: String,
}

async fn pick_zcode_page() -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .ok()?;
    let url = format!("http://127.0.0.1:{}/json/list", CDP_PORT);
    let resp = client.get(&url).send().await.ok()?;
    let targets: Vec<CdpTarget> = resp.json().await.ok()?;
    // 主判断用 URL：renderer/index.html 是 ZCode 自己的页面（devtools 等其它 target
    // 不会带这个路径）。title 只做兜底且用包含匹配——不同版本/本地化下窗口标题
    // 不一定恰好叫 "ZCode"，之前的 == 精确匹配会漏掉页面导致无感刷新静默失效。
    targets
        .iter()
        .find(|t| t.target_type == "page" && t.url.contains("renderer/index.html"))
        .or_else(|| {
            targets
                .iter()
                .find(|t| t.target_type == "page" && t.title.to_lowercase().contains("zcode"))
        })
        .map(|t| t.ws_url.clone())
}

async fn evaluate(ws_url: &str, expression: &str) -> Result<String, String> {
    let (mut ws, _) = tokio_tungstenite::connect_async(ws_url)
        .await
        .map_err(|e| format!("ws connect: {}", e))?;

    let req = serde_json::json!({
        "id": 1,
        "method": "Runtime.evaluate",
        "params": {
            "expression": expression,
            "returnByValue": true,
            "awaitPromise": true,
        }
    });
    ws.send(Message::Text(req.to_string().into()))
        .await
        .map_err(|e| format!("ws send: {}", e))?;

    let result = tokio::time::timeout(WS_TIMEOUT, async {
        while let Some(msg) = ws.next().await {
            let msg = msg.map_err(|e| format!("ws recv: {}", e))?;
            if let Message::Text(text) = msg {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                    if v.get("id").and_then(|v| v.as_u64()) == Some(1) {
                        return Ok::<_, String>(text.to_string());
                    }
                }
            }
        }
        Err("ws closed before response".to_string())
    })
    .await
    .map_err(|_| "ws timeout".to_string())?;

    let _ = ws.close(None).await;
    result
}

/// 试图通过 CDP 远程触发 refreshCodingPlanApiKey。
/// 端口不通 / ZCode 没开 / 注入失败 都会返回 false，不抛错。
pub async fn try_trigger_refresh() -> bool {
    let Some(ws_url) = pick_zcode_page().await else {
        return false;
    };
    matches!(evaluate(&ws_url, INJECT_SCRIPT).await, Ok(text) if text.contains("\"ok\":true"))
}

/// 切号后单次注入 refresh：切换开始 +880ms 触发一次。
///
/// 之前是 +0.5s / +3s / +5s 三次兜底，实测一次就够，多刷一遍只是无意义的 RPC 噪音。
pub fn schedule_post_switch_refresh() {
    tauri::async_runtime::spawn(async {
        tokio::time::sleep(Duration::from_millis(880)).await;
        let _ = try_trigger_refresh().await;
    });
}

/// 自包含 JS：定位渲染层的 settings 服务并更新
/// `modelProviderFamilySelectedKeys[family]`，返回切换前后值。
///
/// 实测（ZCode 3.11.x，CDP Runtime.evaluate）：ZCode 不监听 setting.json 的
/// 外部修改，直接改文件对运行中的实例无效；只有走渲染层的
/// `settingService.update({...})`（与 UI 点击同一条路径）才能同时更新内存
/// 选择并落盘，让请求立即路由到新的套餐入口。
///
/// 定位方式：从 React 根 fiber 向下找携带 `settingService`（同时具备
/// `get`/`update` 方法）的 props/state/context 对象，命中后缓存到
/// `window.__zcsSettingsRegistry`。找不到返回 {ok:false, err:"service-not-found"}。
const SET_SELECTED_KEY_SCRIPT: &str = r#"(async (targetKey) => {
  function findRegistry() {
    let cached = window.__zcsSettingsRegistry;
    if (cached && typeof cached.settingService?.update === 'function') return cached;
    function getAnyFiber() {
      const cands = [document.body, document.getElementById('root'), document.documentElement];
      for (const el of cands) {
        if (!el) continue;
        const key = Object.keys(el).find(k => k.startsWith('__reactFiber$') || k.startsWith('__reactContainer$'));
        if (key) return el[key];
      }
      return null;
    }
    const fiber = getAnyFiber();
    if (!fiber) return null;
    let cur = fiber;
    while (cur.return) cur = cur.return;
    const root = cur;
    const seen = new WeakSet();
    const stack = [root];
    let count = 0;
    const isSvc = (o) => o && typeof o === 'object' &&
      o.settingService && typeof o.settingService.update === 'function' &&
      typeof o.settingService.get === 'function';
    while (stack.length && count < 300000) {
      const node = stack.pop();
      count++;
      for (const slot of ['memoizedProps', 'memoizedState', 'pendingProps', 'stateNode', 'context']) {
        const v = node[slot];
        if (!v) continue;
        if (isSvc(v)) { window.__zcsSettingsRegistry = v; return v; }
      }
      if (node.child) stack.push(node.child);
      if (node.sibling) stack.push(node.sibling);
    }
    return null;
  }
  const reg = findRegistry();
  if (!reg) return { ok: false, err: 'service-not-found' };
  const svc = reg.settingService;
  // targetKey 形如 "coding-plan:builtin:bigmodel-start-plan"，family 取
  // builtin: 与末段 plan 名之间的段。
  const m = /builtin:([a-z0-9]+)-(start-plan|coding-plan)$/.exec(targetKey || '');
  if (!m) return { ok: false, err: 'bad-target-key' };
  const family = m[1];
  const before = (await svc.get()).modelProviderFamilySelectedKeys ?? {};
  const next = { ...before, [family]: targetKey };
  if (before[family] === targetKey) return { ok: true, unchanged: true, before: before[family], after: before[family] };
  await svc.update({ modelProviderFamilySelectedKeys: next });
  const after = ((await svc.get()).modelProviderFamilySelectedKeys ?? {})[family];
  return after === targetKey
    ? { ok: true, before: before[family], after }
    : { ok: false, err: 'update-not-applied', before: before[family], after };
})"#;

/// 通过 CDP 把 ZCode 当前 family 的套餐入口切换为 `selected_key`
/// （形如 "coding-plan:builtin:bigmodel-start-plan"）。
///
/// 成功返回 Ok(new_key)（内存已生效并落盘）；ZCode 未运行 / 无调试端口 /
/// 找不到服务时返回 Err，调用方应退回“直接写 setting.json”的路径。
pub async fn try_update_selected_provider(selected_key: &str) -> Result<String, String> {
    let Some(ws_url) = pick_zcode_page().await else {
        return Err("ZCode 调试端口不可用".into());
    };
    // SET_SELECTED_KEY_SCRIPT 本身就是箭头函数表达式，直接传参调用。
    let expression = format!(
        "({})({})",
        SET_SELECTED_KEY_SCRIPT,
        serde_json::to_string(selected_key).map_err(|e| e.to_string())?
    );
    let text = evaluate(&ws_url, &expression).await?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("解析 CDP 响应失败：{}", e))?;
    // Runtime.evaluate 的返回结构是：
    // { "result": { "result": { "type": "object", "value": {...} } } }
    // 同时兼容少数实现直接把 value 放在第一层 result 下的情况。
    let outer = value
        .get("result")
        .ok_or_else(|| "CDP 响应缺少 result".to_string())?;
    let result = outer
        .get("result")
        .and_then(|r| r.get("value"))
        .or_else(|| outer.get("value"))
        .cloned()
        .ok_or_else(|| "CDP 响应缺少 result.result.value".to_string())?;
    if result.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err(format!(
            "settingService 更新未生效：{}",
            result
                .get("err")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown")
        ));
    }
    result
        .get("after")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "CDP 响应缺少 after".to_string())
}

#[allow(dead_code)]
pub fn known_plan_ids() -> &'static [&'static str] {
    ZAI_PLAN_IDS
}
