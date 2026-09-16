//! 订阅额度查询：用解密后的 zcodejwttoken 调 ZCode 的 billing 接口。
//!
//! - GET https://zcode.z.ai/api/v1/zcode-plan/billing/balance?app_version=... → 当前套餐 + 用量/余额

use rand::{rngs::OsRng, RngCore};
use serde::Deserialize;
use serde_json::Value;
use std::{fs, path::PathBuf, time::SystemTime};

use crate::crypto;

const BASE: &str = "https://zcode.z.ai";
const APP_VERSION_CANDIDATES: &[&str] = &["3.11.2", "3.2.5", crate::captcha::ZCODE_APP_VERSION];

/// 单个模型的用量条目（balance.data.balances[]）。
#[derive(Debug, Clone, serde::Serialize, Deserialize)]
pub struct BalanceItem {
    #[serde(default)]
    pub show_name: String,
    #[serde(default)]
    pub used_units: f64,
    #[serde(default)]
    pub total_units: f64,
    #[serde(default)]
    pub remaining_units: f64,
    #[serde(default)]
    pub unit_type: Option<String>,
    #[serde(default)]
    pub period: Option<String>,
    /// 条目所属套餐的 plan_id（billing 桶带原值；个人套餐积分桶为
    /// "personal:glm-coding"）。前端据此把条目归组到 plans[] 里的套餐。
    #[serde(default)]
    pub plan_id: Option<String>,
}

/// 账号名下的一个套餐摘要（plans[] 条目，前端按此分组展示余额）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct PlanSummary {
    /// billing 体系套餐的 plan_id；个人套餐积分组的合成 id 为
    /// "personal:glm-coding"。
    pub plan_id: Option<String>,
    pub name: String,
    pub status: Option<String>,
    /// 套餐到期时间（Unix 秒，None 表示无/未知）
    pub ends_at: Option<f64>,
    /// 套餐优先级（服务端排序依据，主展示套餐取最高者）
    #[serde(default)]
    pub priority: i64,
    /// 是否为 ZCode 当前选中的供应者对应的套餐（组头"使用中"标记）。
    pub is_current: bool,
}

/// 一个账号的订阅/额度汇总（传给前端）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct QuotaInfo {
    pub plan_name: Option<String>,
    pub plan_description: Option<String>,
    pub plan_status: Option<String>,
    /// 套餐到期时间（Unix 秒，0 表示无）
    pub plan_ends_at: Option<f64>,
    pub balances: Vec<BalanceItem>,
    /// 账号名下所有套餐（billing 体系 + 个人套餐积分组），供前端分组。
    #[serde(default)]
    pub plans: Vec<PlanSummary>,
    /// ZCode 当前选中的模型供应者（setting.json 的
    /// modelProviderFamilySelectedKeys.bigmodel，如
    /// "coding-plan:builtin:bigmodel-start-plan"）。前端据此标记
    /// "使用中" 的套餐条目；None 表示读不到（非当前登录账号等）。
    #[serde(default)]
    pub active_provider: Option<String>,
}

#[derive(Deserialize)]
struct ApiEnvelope<T> {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    data: Option<T>,
}

#[derive(Deserialize)]
struct PlanInfo {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    plan_id: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    priority: Option<i64>,
    #[serde(default)]
    ends_at: Option<f64>,
}

#[derive(Deserialize, Default)]
struct BillingBalanceData {
    #[serde(default)]
    plans: Vec<PlanInfo>,
    #[serde(default)]
    balances: Vec<Value>,
}

/// 用某份 credentials.json（JSON 文本）查询其额度。
pub async fn fetch_quota(creds_text: &str) -> Result<QuotaInfo, String> {
    let creds: Value =
        serde_json::from_str(creds_text).map_err(|e| format!("解析 credentials 失败：{}", e))?;
    let token =
        crypto::extract_jwt_token(&creds).ok_or_else(|| "无法解出 zcodejwttoken".to_string())?;

    // billing 接口要求 X-Device-Mid（缺失时服务端返回 HTTP 400 code=3001
    // "parameter error"）。ZCode 桌面端把设备标识持久化在 telemetry-state.json，
    // 这里优先读同一个文件、带同一个值；本机没跑过 ZCode 桌面端时（典型场景：
    // 干净虚机上只装 Switcher），退回 Switcher 自己生成并持久化的设备标识。
    let mut default_headers = reqwest::header::HeaderMap::new();
    if let Some(mid) = effective_device_mid() {
        if let Ok(value) = reqwest::header::HeaderValue::from_str(&mid) {
            default_headers.insert("X-Device-Mid", value);
        }
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .default_headers(default_headers)
        .build()
        .map_err(|e| format!("HTTP 客户端创建失败：{}", e))?;

    let mut billing_error: Option<String> = None;
    let balance = match fetch_billing_balance(&client, &token).await {
        Ok(balance) => Some(balance),
        Err(e) => {
            billing_error = Some(e);
            latest_logged_balance_for_current_token(&token)
        }
    };

    // Coding Plan（个人套餐，积分制）和 Start Plan（体验套餐）的额度
    // 不在 zcode-plan 体系里。数据源优先级：
    // 1. open.bigmodel.cn quota/limit + OAuth access_token —— 官网控制台同源，
    //    返回实时窗口明细（5 小时积分 / 周积分 / 月度额度）；
    // 2. 同接口 + 供应者 API Key（ZCode 已不再存储，通常不可用）；
    // 3. mcp/usage 汇总桶 —— 仅兜底：total_usage 是服务端滞后缓存
    //    （实测长时间停留在 used=0），不能反映 5 小时窗口的真实用量。
    let mut coding: Option<CodingPlanSnapshot> = None;
    // 记录积分链路的最后一次失败原因：积分是部分账号（billing 返回空）的
    // 唯一额度来源，全部来源失败时错误信息必须能指明原因（如 401 过期），
    // 否则只报泛化的"未返回额度明细"，无法排查。
    let mut coding_error: Option<String> = None;

    match oauth_access_token(&creds) {
        Ok(oauth_token) => match fetch_coding_plan_usage(&client, &oauth_token).await {
            Ok(snapshot) => coding = Some(snapshot),
            Err(e) => coding_error = Some(e),
        },
        Err(e) => coding_error = Some(e),
    }
    if coding.is_none() {
        let coding_key = read_bigmodel_provider_key("builtin:bigmodel-coding-plan");
        let start_key = read_bigmodel_provider_key("builtin:bigmodel-start-plan");
        if let Some(key) = coding_key {
            match fetch_coding_plan_usage(&client, &key).await {
                Ok(snapshot) => coding = Some(snapshot),
                Err(e) => coding_error = Some(e),
            }
        }
        if coding.is_none() {
            if let Some(key) = start_key {
                if let Ok(start) = fetch_coding_plan_usage(&client, &key).await {
                    coding = Some(start);
                }
            }
        }
    }
    if coding.is_none() {
        match fetch_mcp_usage(&client, &token, &creds).await {
            Ok(mcp) => coding = Some(mcp),
            Err(e) => coding_error = Some(e),
        }
    }

    let (plan_parts, mut balances, mut plan_summaries) = match balance {
        Some(balance) => {
            // plan_id → 套餐名映射：同名模型条目可能来自多个同时生效的套餐
            // （如 Global Build 与 Start Plan 各有一个 GLM-5.3-Flash 桶），
            // 用套餐短名前缀区分。
            let plan_names: std::collections::HashMap<String, String> = balance
                .plans
                .iter()
                .filter_map(|p| {
                    let id = p.plan_id.as_deref()?.to_string();
                    let name = p.name.clone().unwrap_or_default();
                    if name.is_empty() {
                        None
                    } else {
                        Some((id, name))
                    }
                })
                .collect();
            let balances: Vec<BalanceItem> = balance
                .balances
                .into_iter()
                .filter_map(|v| parse_balance_item(v, &plan_names))
                .collect();
            // 全部套餐摘要（不止 priority 最高的那个），供前端分组展示。
            let summaries: Vec<PlanSummary> = balance
                .plans
                .into_iter()
                .filter(|p| p.name.is_some() || p.plan_id.is_some())
                .map(|p| PlanSummary {
                    plan_id: p.plan_id,
                    name: p.name.unwrap_or_default(),
                    status: p.status,
                    ends_at: p.ends_at.filter(|v| *v > 0.0),
                    priority: p.priority.unwrap_or(0),
                    is_current: false,
                })
                .collect();
            (Some(pick_best_plan_parts(&summaries)), balances, summaries)
        }
        None => (None, Vec::new(), Vec::new()),
    };
    // 个人套餐积分组：合成 plan_id 并加入套餐列表，前端按组展示。
    if let Some(snapshot) = &mut coding {
        let personal_plan_id = "personal:glm-coding".to_string();
        for item in &mut snapshot.items {
            item.plan_id = Some(personal_plan_id.clone());
        }
        balances.extend(snapshot.items.iter().cloned());
        plan_summaries.push(PlanSummary {
            plan_id: Some(personal_plan_id),
            name: coding_plan_display_name(&snapshot.level),
            status: Some("active".into()),
            ends_at: None,
            priority: i64::MAX,
            is_current: false,
        });
    }
    if balances.is_empty() {
        return Err(match billing_error {
            Some(e) => format!("套餐额度刷新失败：{}", friendly_balance_error(Some(&e))),
            None => match coding_error {
                Some(e) => format!("积分额度获取失败：{}", e),
                None => "额度接口未返回可显示的模型额度明细".into(),
            },
        });
    }

    let active_provider = read_active_bigmodel_provider();
    // 标记当前使用中的套餐。selectedKey 形如
    // "coding-plan:builtin:bigmodel-start-plan"——前缀 "coding-plan:" 是
    // key 类型，真正的供应者在最后一段，必须用最后一段匹配，否则
    // start-plan 的 key 会被误判成 coding-plan。
    let selected_provider_id = active_provider
        .as_deref()
        .and_then(|key| key.rsplit(':').next())
        .unwrap_or("")
        .to_ascii_lowercase();
    for plan in &mut plan_summaries {
        plan.is_current = plan_is_current(plan, &selected_provider_id);
    }

    let (mut plan_name, plan_description, plan_status, plan_ends_at) =
        plan_parts.unwrap_or((None, None, None, None));
    // zcode-plan 体系没有套餐（个人套餐账号）时，用 Coding Plan 档位当套餐名。
    if plan_name.is_none() {
        if let Some(snapshot) = &coding {
            plan_name = Some(coding_plan_display_name(&snapshot.level));
        }
    }

    Ok(QuotaInfo {
        plan_name,
        plan_description,
        plan_status,
        plan_ends_at,
        balances,
        plans: plan_summaries,
        active_provider,
    })
}

/// 判断套餐是否为当前选中供应者对应的套餐。
/// selected_provider_id 是 selectedKey 的最后一段（如
/// "builtin:bigmodel-start-plan"）。
///
/// 用套餐名匹配而非 plan_id：Global Build 的 plan_id 也挂在 start-plan
/// 体系下（zcode-v3-start-plan-0914，仅日期后缀不同），按 id 匹配会把
/// Global Build 误标为使用中；名字（"ZCode Start Plan" vs
/// "ZCode Global Build"）才是可靠区分。
fn plan_is_current(plan: &PlanSummary, selected_provider_id: &str) -> bool {
    if selected_provider_id.is_empty() {
        return false;
    }
    let name = plan.name.to_ascii_lowercase();
    let plan_id = plan.plan_id.as_deref().unwrap_or("").to_ascii_lowercase();
    if selected_provider_id.contains("bigmodel-start-plan") {
        if !name.is_empty() {
            return name.contains("start") && !name.contains("global");
        }
        // 名字缺失时的 id 回退：Global Build 与 Start Plan 的 id 同挂
        // start-plan 体系（仅日期后缀不同），无法区分，保守不标记。
        return false;
    }
    if selected_provider_id.contains("bigmodel-coding-plan") {
        if !name.is_empty() {
            return name.contains("coding");
        }
        return plan_id.starts_with("personal:") || plan_id.contains("coding");
    }
    false
}

/// 从套餐摘要列表里选主展示套餐（priority 最高者，与原 pick_best_plan 一致）。
fn pick_best_plan_parts(
    summaries: &[PlanSummary],
) -> (Option<String>, Option<String>, Option<String>, Option<f64>) {
    let best = summaries.iter().max_by_key(|p| p.priority);
    match best {
        Some(p) => (Some(p.name.clone()), None, p.status.clone(), p.ends_at),
        None => (None, None, None, None),
    }
}

/// 读取 ZCode setting.json 里 bigmodel 家族当前选中的供应者
/// （modelProviderFamilySelectedKeys.bigmodel，如
/// "coding-plan:builtin:bigmodel-start-plan"）。
/// 这是"当前在用哪个套餐"的权威数据，与桌面端模型切换保持同源。
fn read_active_bigmodel_provider() -> Option<String> {
    let path = crate::profile::zcode_settings_dir().ok()?.join("setting.json");
    let text = fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let selected = value
        .get("modelProviderFamilySelectedKeys")?
        .get("bigmodel")?
        .as_str()?
        .trim()
        .to_string();
    if selected.is_empty() {
        None
    } else {
        Some(selected)
    }
}

/// 套餐短名：完整套餐名太长（"ZCode Global Build"），额度条目里用短前缀。
fn plan_short_name(plan_name: &str) -> String {
    let lower = plan_name.to_ascii_lowercase();
    if lower.contains("start") {
        "Start".into()
    } else if lower.contains("global") {
        "Global".into()
    } else if lower.contains("coding") {
        "Coding".into()
    } else if lower.contains("team") {
        "Team".into()
    } else {
        plan_name
            .split_whitespace()
            .next()
            .unwrap_or(plan_name)
            .to_string()
    }
}

/// Coding Plan（个人套餐，积分制）的额度快照：一组按窗口重置的积分桶。
///
/// 数据来自 open.bigmodel.cn /api/monitor/usage/quota/limit，与官网控制台
/// 显示的"积分 988/2,000、周积分 991/1万"同源。积分与 token 不同量纲，
/// 条目的 unit_type 标记为 "point"，前端据此区分展示：模型条目按 token 阈值、
/// 积分条目按积分阈值参与低额度自动切换判定。
#[derive(Debug, Clone)]
pub struct CodingPlanSnapshot {
    /// 套餐档位（小写，如 "lite" / "pro" / "max"）。
    pub level: String,
    /// 积分桶（5 小时窗口的"积分"、每周窗口的"周积分"等）。
    pub items: Vec<BalanceItem>,
}

/// 套餐显示名，如 "GLM Coding Lite"。
fn coding_plan_display_name(level: &str) -> String {
    let mut chars = level.chars();
    match chars.next() {
        Some(first) => format!(
            "GLM Coding {}{}",
            first.to_ascii_uppercase(),
            chars.as_str()
        ),
        None => "GLM Coding".into(),
    }
}

/// 从 ZCode 的 config.json 读取指定供应者的 BigModel API Key。
///
/// ZCode 在用户领取/订阅后会把 key 写入对应供应者的 options.apiKey 并定期刷新；
/// 官方客户端查额度用的就是这把 key（OAuth token 会被 401 拒绝）。
/// 注意：key 跟随本机 ZCode 登录，不区分 Switcher 档案。
fn read_bigmodel_provider_key(provider_id: &str) -> Option<String> {
    let path = crate::profile::zcode_settings_dir()
        .ok()?
        .join("config.json");
    let text = fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let key = value
        .get("provider")?
        .get(provider_id)?
        .get("options")?
        .get("apiKey")?
        .as_str()?
        .trim()
        .to_string();
    if key.is_empty() {
        None
    } else {
        Some(key)
    }
}

/// 窗口 → 周期标记（与官网窗口对齐；unit=3 & number=5 是 5 小时窗口，
/// unit=6 & number=1 是每周窗口，unit=5 & number=1 是每月窗口；
/// Lite 套餐只返回 5h+周两窗口，更高套餐可能有月窗口）。
fn window_period(unit: i64, number: i64) -> Option<String> {
    match (unit, number) {
        (3, 5) => Some("5h".into()),
        (6, 1) => Some("weekly".into()),
        (5, 1) => Some("monthly".into()),
        _ => None,
    }
}

/// 积分桶展示名。unit/number 描述重置窗口：unit=3 & number=5 是 5 小时窗口
/// （官网叫"积分"），unit=6 & number=1 是每周窗口（官网叫"周积分"），
/// unit=5 & number=1 是每月窗口（"月积分"）；
/// Lite 套餐只有 5h+周两窗口，更高套餐可能返回月窗口。
fn credit_window_label(unit: i64, number: i64) -> String {
    let unit_word = match unit {
        3 => "小时",
        5 => "月",
        6 => "周",
        _ => "周期",
    };
    match (unit, number) {
        (3, 5) => "5小时积分".into(),
        (3, 1) => "小时积分".into(),
        (6, 1) => "周积分".into(),
        (5, 1) => "月积分".into(),
        _ => {
            if number == 1 {
                format!("{unit_word}积分")
            } else {
                format!("{number}{unit_word}积分")
            }
        }
    }
}

/// 工具额度桶展示名（TIME_LIMIT：MCP 工具调用次数，与积分不同量纲）。
fn tool_window_label(unit: i64, number: i64) -> String {
    let unit_word = match unit {
        3 => "小时",
        5 => "月",
        6 => "周",
        _ => "周期",
    };
    match (unit, number) {
        (3, 5) => "5小时工具额度".into(),
        (6, 1) => "周工具额度".into(),
        (5, 1) => "月度工具额度".into(),
        _ => {
            if number == 1 {
                format!("{unit_word}工具额度")
            } else {
                format!("{number}{unit_word}工具额度")
            }
        }
    }
}

/// 解析 quota/limit 响应；未订阅或字段缺失时返回 None。
///
/// CREDIT_LIMIT 是积分桶（参与积分阈值判定）；TIME_LIMIT 是 MCP 工具
/// 调用额度（次数，仅展示）；TOKENS_LIMIT 只有百分比、无绝对值，不展示。
fn parse_coding_plan_usage(value: &Value) -> Option<CodingPlanSnapshot> {
    // 该接口的成功包络是 {"code":200,"success":true,...}（区别于 billing 的 code=0）。
    let code_ok = value.get("code").and_then(Value::as_i64) == Some(200);
    let success = value.get("success").and_then(Value::as_bool) == Some(true);
    if !code_ok && !success {
        return None;
    }
    let data = value.get("data")?;
    let level = data
        .get("level")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let limits = data.get("limits")?.as_array()?;
    let mut items = Vec::new();
    for limit in limits {
        let limit_type = limit.get("type").and_then(Value::as_str).unwrap_or("");
        if limit_type != "CREDIT_LIMIT" && limit_type != "TIME_LIMIT" {
            continue;
        }
        // 单条目字段缺失只跳过该条，不能让整个快照解析失败——否则会静默
        // 跌落到滞后的 mcp/usage 汇总兜底，界面显示反而退回假数据。
        let total = match limit.get("usage").and_then(Value::as_f64) {
            Some(v) if v.is_finite() && v > 0.0 => v,
            _ => continue,
        };
        let used = match limit.get("currentValue").and_then(Value::as_f64) {
            Some(v) if v.is_finite() => v,
            _ => continue,
        };
        let remaining = match limit.get("remaining").and_then(Value::as_f64) {
            Some(v) if v.is_finite() => v,
            _ => (total - used).max(0.0),
        };
        let (unit, number) = (
            limit.get("unit").and_then(Value::as_i64).unwrap_or(0),
            limit.get("number").and_then(Value::as_i64).unwrap_or(0),
        );
        let is_credit = limit_type == "CREDIT_LIMIT";
        items.push(BalanceItem {
            show_name: if is_credit {
                credit_window_label(unit, number)
            } else {
                tool_window_label(unit, number)
            },
            used_units: used,
            total_units: total,
            remaining_units: remaining,
            unit_type: Some(if is_credit { "point".into() } else { "tool".into() }),
            period: window_period(unit, number),
            plan_id: None,
        });
    }
    if items.is_empty() {
        return None;
    }
    Some(CodingPlanSnapshot { level, items })
}

/// 拉取 Coding Plan（个人套餐，积分制）额度。
///
/// 认证接受两种 Bearer：BigModel OAuth access_token（credentials 里的
/// oauth:bigmodel:access_token，官网控制台同源）或供应者 API Key。
/// ZCode 的 zcodejwttoken JWT 对该接口无效（401）。
/// 多档案顺序刷新会连续请求该接口，429/网络抖动时退避重试一次。
async fn fetch_coding_plan_usage(
    client: &reqwest::Client,
    api_key: &str,
) -> Result<CodingPlanSnapshot, String> {
    const BIGMODEL_QUOTA_URL: &str = "https://open.bigmodel.cn/api/monitor/usage/quota/limit";
    const MAX_ATTEMPTS: usize = 2;
    let mut last_err = None;
    for attempt in 0..MAX_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        }
        let resp = match client
            .get(BIGMODEL_QUOTA_URL)
            .header("Authorization", format!("Bearer {}", api_key))
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                last_err = Some(format!("请求失败：{}", e));
                continue;
            }
        };
        if !resp.status().is_success() {
            // 429 按 Retry-After 退避（缺省 2s，封顶 10s），其余状态码直接报错。
            if resp.status().as_u16() == 429 && attempt + 1 < MAX_ATTEMPTS {
                let wait = resp
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.trim().parse::<u64>().ok())
                    .unwrap_or(2)
                    .clamp(1, 10);
                last_err = Some("积分额度限流".into());
                tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                continue;
            }
            return Err(format!("状态码 {}", resp.status()));
        }
        let value: Value = match resp.json().await {
            Ok(value) => value,
            Err(e) => {
                last_err = Some(format!("解析失败：{}", e));
                continue;
            }
        };
        return parse_coding_plan_usage(&value)
            .ok_or_else(|| "Coding Plan 额度不可用（可能未订阅）".into());
    }
    Err(last_err.unwrap_or_else(|| "积分额度请求失败".into()))
}

/// 从 credentials.json 读出并解密 OAuth access_token
/// （oauth:bigmodel:access_token，quota/limit 与 mcp/usage 都用它认证）。
fn oauth_access_token(creds: &Value) -> Result<String, String> {
    let oauth_token = creds
        .get("oauth:bigmodel:access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "credentials 缺少 oauth:bigmodel:access_token".to_string())?;
    crypto::decrypt(oauth_token).map_err(|e| format!("解密 OAuth token 失败：{}", e))
}

/// 拉取个人套餐积分额度（从 ZCode mcp/usage 接口）。
///
/// 用三头认证：JWT token + OAuth access_token + Bigmodel-Target-Type: PERSONAL。
/// 返回格式与 fetch_coding_plan_usage 一致（CodingPlanSnapshot），便于合并。
async fn fetch_mcp_usage(
    client: &reqwest::Client,
    jwt_token: &str,
    creds: &Value,
) -> Result<CodingPlanSnapshot, String> {
    const MCP_USAGE_URL: &str = "https://zcode.z.ai/api/v1/mcp/usage";

    let decrypted_oauth = oauth_access_token(&creds)?;
    
    let resp = client
        .get(MCP_USAGE_URL)
        .header("Authorization", format!("Bearer {}", jwt_token))
        .header("X-Bigmodel-Authorization", format!("Bearer {}", decrypted_oauth))
        .header("Bigmodel-Target-Type", "PERSONAL")
        .send()
        .await
        .map_err(|e| format!("mcp/usage 请求失败：{}", e))?;
    
    if !resp.status().is_success() {
        return Err(format!("mcp/usage 状态码 {}", resp.status()));
    }
    
    let value: Value = resp.json().await
        .map_err(|e| format!("mcp/usage 解析失败：{}", e))?;
    
    parse_mcp_usage(&value)
        .ok_or_else(|| "mcp/usage 响应格式异常".to_string())
}

/// 解析 mcp/usage 响应，提取积分桶。
fn parse_mcp_usage(value: &Value) -> Option<CodingPlanSnapshot> {
    // mcp/usage 返回信封格式 {code, data: {level, total_usage}}
    let data = value.get("data").unwrap_or(value);
    let level = data.get("level")?.as_str()?;
    let usage = data.get("total_usage")?;
    let used = usage.get("used")?.as_f64()?;
    let limit = usage.get("limit")?.as_f64()?;
    let remaining = usage.get("remaining")?.as_f64()?;
    
    let plan_name = match level {
        "lite" => "积分汇总 (Lite)",
        "pro" => "积分汇总 (Pro)",
        "max" => "积分汇总 (Max)",
        _ => "积分汇总",
    };
    
    Some(CodingPlanSnapshot {
        level: level.to_string(),
        items: vec![BalanceItem {
            show_name: plan_name.into(),
            used_units: used,
            total_units: limit,
            remaining_units: remaining,
            unit_type: Some("point".into()),
            period: None,
            plan_id: None,
        }],
    })
}

fn pick_best_plan(
    plans: Vec<PlanInfo>,
) -> (Option<String>, Option<String>, Option<String>, Option<f64>) {
    let best = plans.into_iter().max_by_key(|p| p.priority.unwrap_or(0));
    match best {
        Some(p) => (
            p.name,
            p.description,
            p.status,
            p.ends_at.filter(|v| *v > 0.0),
        ),
        None => (None, None, None, None),
    }
}

fn number_field(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64)
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn parse_balance_item(
    value: Value,
    plan_names: &std::collections::HashMap<String, String>,
) -> Option<BalanceItem> {
    let mut item = serde_json::from_value::<BalanceItem>(value.clone()).ok()?;
    if item.show_name.trim().is_empty() {
        item.show_name = string_field(&value, "name")
            .or_else(|| string_field(&value, "entitlement_id"))
            .unwrap_or_else(|| "额度".into());
    }
    // 多套餐同时生效时同名模型会出现多个桶（如 Global·GLM-5.3-Flash 与
    // Start·GLM-5.3-Flash），用套餐短名前缀区分；个人套餐积分条目
    // （mcp/usage 来的）没有 plan_id，保持原样。
    if let Some(plan_id) = value.get("plan_id").and_then(Value::as_str) {
        item.plan_id = Some(plan_id.to_string());
        if let Some(plan_name) = plan_names.get(plan_id) {
            let short = plan_short_name(plan_name);
            if !item.show_name.starts_with(&format!("{short}·")) {
                item.show_name = format!("{}·{}", short, item.show_name);
            }
        }
    }
    if number_field(&value, "remaining_units").is_none() {
        item.remaining_units = number_field(&value, "available_units")
            .unwrap_or_else(|| (item.total_units - item.used_units).max(0.0));
    }
    if item.period.is_none() {
        item.period = string_field(&value, "period");
    }
    Some(item)
}

fn latest_logged_balance_for_current_token(token: &str) -> Option<BillingBalanceData> {
    if !current_credentials_match(token) {
        return None;
    }
    latest_logged_billing_balance()
}

fn current_credentials_match(token: &str) -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    let path = home.join(".zcode").join("v2").join("credentials.json");
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(creds) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    crypto::extract_jwt_token(&creds).as_deref() == Some(token)
}

/// 读取 ZCode 持久化的设备标识（~/.zcode/v2/telemetry-state.json 的 deviceMid）。
fn read_device_mid() -> Option<String> {
    let path = dirs::home_dir()?
        .join(".zcode")
        .join("v2")
        .join("telemetry-state.json");
    let text = fs::read_to_string(path).ok()?;
    parse_device_mid(&text)
}

/// billing 请求实际使用的设备标识：
/// 1. ZCode 桌面端持久化的 telemetry-state.json（保持与其自身请求同源）；
/// 2. 缺失时（干净虚机上没装过 ZCode 桌面端）用 Switcher 生成并持久化在
///    `zcode-switcher-device.json` 的兜底设备标识。持久化是必须的：每次请求
///    换一个设备 ID 会显得异常，也浪费服务端风控的信任。
fn effective_device_mid() -> Option<String> {
    read_device_mid().or_else(persistent_fallback_device_mid)
}

fn fallback_device_mid_path() -> Option<PathBuf> {
    crate::profile::zcode_settings_dir()
        .ok()
        .map(|dir| dir.join("zcode-switcher-device.json"))
}

fn persistent_fallback_device_mid() -> Option<String> {
    let path = fallback_device_mid_path()?;
    if let Ok(text) = fs::read_to_string(&path) {
        if let Some(mid) = parse_device_mid(&text) {
            return Some(mid);
        }
    }
    let mid = new_device_mid();
    if write_fallback_device_mid(&path, &mid).is_err() {
        return None;
    }
    Some(mid)
}

/// 生成 UUID v4 形态的设备标识（与 ZCode telemetry-state.json 中 deviceMid 同构）。
fn new_device_mid() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    format_device_mid(&bytes)
}

fn format_device_mid(bytes: &[u8; 16]) -> String {
    let mut out = String::with_capacity(36);
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            out.push('-');
        }
        let mut byte = *byte;
        // RFC 4122：第 7 字节高 4 位为版本 4，第 9 字节高 2 位为 10。
        if index == 6 {
            byte = (byte & 0x0f) | 0x40;
        }
        if index == 8 {
            byte = (byte & 0x3f) | 0x80;
        }
        out.push_str(&format!("{:02x}", byte));
    }
    out
}

fn write_fallback_device_mid(path: &std::path::Path, mid: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_vec_pretty(&serde_json::json!({ "deviceMid": mid }))
        .map_err(std::io::Error::other)?;
    fs::write(path, body)
}

fn parse_device_mid(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    value
        .get("deviceMid")?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn newest_log_files() -> Option<Vec<PathBuf>> {
    let logs_dir = dirs::home_dir()?.join(".zcode").join("v2").join("logs");
    let mut files: Vec<(SystemTime, PathBuf)> = fs::read_dir(logs_dir)
        .ok()?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("log") {
                return None;
            }
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, path))
        })
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    Some(files.into_iter().map(|(_, path)| path).take(5).collect())
}

fn latest_logged_billing_balance() -> Option<BillingBalanceData> {
    for path in newest_log_files()? {
        let text = fs::read_to_string(path).ok()?;
        for line in text.lines().rev() {
            if let Some(data) = parse_logged_balance_line(line) {
                return Some(data);
            }
        }
    }
    None
}

fn parse_logged_balance_line(line: &str) -> Option<BillingBalanceData> {
    if !line.contains("billing/balance 请求完成") {
        return None;
    }
    let (_, json_part) = line.split_once("请求完成 ")?;
    let value: Value = serde_json::from_str(json_part.trim()).ok()?;
    let env: ApiEnvelope<BillingBalanceData> =
        serde_json::from_value(value.get("payload")?.clone()).ok()?;
    if env.code != 0 {
        return None;
    }
    env.data.filter(|data| !data.balances.is_empty())
}

/// 发起 GET 请求。最多 2 次尝试：
/// - 429：按 Retry-After 头退避后重试一次；
/// - 请求超时 / 连接失败：立即重试一次；
/// - 其它非 2xx 错误：直接报错，不重试。
///
/// 最终若两次都超时，返回错误字符串里含"请求超时"，让前端可以识别成"超时"展示。
async fn get_with_retry(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    label: &str,
) -> Result<reqwest::Response, String> {
    const MAX_ATTEMPTS: usize = 2;
    let mut last_err: Option<String> = None;

    for attempt in 0..MAX_ATTEMPTS {
        let result = client
            .get(url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await;

        match result {
            Ok(resp) => {
                let status = resp.status();
                if status.as_u16() == 429 && attempt + 1 < MAX_ATTEMPTS {
                    // Retry-After 优先识别秒数，缺省 2s，封顶 10s，避免长时间挂起。
                    let wait_secs = resp
                        .headers()
                        .get(reqwest::header::RETRY_AFTER)
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.trim().parse::<u64>().ok())
                        .unwrap_or(2)
                        .clamp(1, 10);
                    drop(resp);
                    tokio::time::sleep(std::time::Duration::from_secs(wait_secs)).await;
                    last_err = Some(format!("{} 限流重试后仍未恢复", label));
                    continue;
                }
                if !status.is_success() {
                    return Err(format!("{} 状态码 {}", label, status));
                }
                return Ok(resp);
            }
            Err(e) => {
                let is_timeout_like = e.is_timeout() || e.is_connect();
                if is_timeout_like && attempt + 1 < MAX_ATTEMPTS {
                    last_err = Some("请求超时".to_string());
                    continue;
                }
                if is_timeout_like {
                    return Err("请求超时".to_string());
                }
                return Err(format!("请求 {} 失败：{}", label, e));
            }
        }
    }

    Err(last_err.unwrap_or_else(|| format!("{} 重试后仍未恢复", label)))
}

async fn fetch_billing_balance(
    client: &reqwest::Client,
    token: &str,
) -> Result<BillingBalanceData, String> {
    let mut last_error = None;
    let mut tried = Vec::new();

    for version in APP_VERSION_CANDIDATES {
        if tried.iter().any(|item| item == version) {
            continue;
        }
        tried.push(*version);
        let url = format!(
            "{}/api/v1/zcode-plan/billing/balance?app_version={}",
            BASE, version
        );
        let label = format!("billing/balance?app_version={}", version);
        let resp = match get_with_retry(client, &url, token, &label).await {
            Ok(resp) => resp,
            Err(e) => {
                last_error = Some(e);
                continue;
            }
        };
        let env: ApiEnvelope<BillingBalanceData> =
            resp.json().await.map_err(|e| format!("解析失败：{}", e))?;
        match env.data {
            Some(data) => return Ok(data),
            None => last_error = Some(format!("{} 返回 code={}", label, env.code)),
        }
    }

    Err(format!(
        "套餐额度刷新失败：{}",
        friendly_balance_error(last_error.as_deref())
    ))
}

fn friendly_balance_error(error: Option<&str>) -> &'static str {
    let Some(error) = error else {
        return "请打开 ZCode 或切换账号后重试";
    };
    if error.contains("请求超时") || error.to_ascii_lowercase().contains("timeout") {
        return "请求超时";
    }
    if error.contains("429") || error.contains("限流") {
        return "请求过于频繁，请稍后重试";
    }
    "请打开 ZCode 或切换账号后重试"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::UNIX_EPOCH;

    #[test]
    fn parse_start_plan_from_balance_payload() {
        let data: BillingBalanceData = serde_json::from_value(json!({
            "plans": [{
                "name": "ZCode Start Plan",
                "description": "免费 GLM 旗舰模型体验",
                "priority": 100,
                "status": "active",
                "ends_at": 1783785599
            }],
            "balances": [{
                "show_name": "GLM-5.3",
                "total_units": 3000000,
                "used_units": 100000,
                "remaining_units": 2900000,
                "period": "daily"
            }, {
                "show_name": "GLM-5-Turbo",
                "total_units": 2000000,
                "used_units": 0,
                "remaining_units": 2000000,
                "period": "daily"
            }]
        }))
        .unwrap();

        let (name, _, status, ends_at) = pick_best_plan(data.plans);
        let balances: Vec<_> = data
            .balances
            .into_iter()
            .filter_map(|v| parse_balance_item(v, &std::collections::HashMap::new()))
            .collect();
        assert_eq!(name.as_deref(), Some("ZCode Start Plan"));
        assert_eq!(status.as_deref(), Some("active"));
        assert_eq!(ends_at, Some(1783785599.0));
        assert_eq!(balances.len(), 2);
        assert_eq!(balances[0].show_name, "GLM-5.3");
        assert_eq!(balances[0].remaining_units, 2_900_000.0);
    }

    #[test]
    fn balance_items_get_plan_prefix_from_plan_id() {
        // 多套餐同时生效：同名模型条目（GLM-5.3-Flash）各带套餐短名前缀区分。
        let data: BillingBalanceData = serde_json::from_value(json!({
            "plans": [
                {"plan_id": "zcode-v3-global-build", "name": "ZCode Global Build", "priority": 200},
                {"plan_id": "zcode-v3-start-plan-0817", "name": "ZCode Start Plan", "priority": 90}
            ],
            "balances": [
                {"plan_id": "zcode-v3-global-build", "show_name": "GLM-5.3-Flash",
                 "total_units": 100000000, "used_units": 0, "remaining_units": 100000000},
                {"plan_id": "zcode-v3-start-plan-0817", "show_name": "GLM-5.3-Flash",
                 "total_units": 5000000, "used_units": 5000000, "remaining_units": 0}
            ]
        }))
        .unwrap();
        let plan_names: std::collections::HashMap<String, String> = data
            .plans
            .iter()
            .filter_map(|p| {
                p.plan_id
                    .clone()
                    .zip(p.name.clone())
                    .map(|(id, name)| (id, name))
            })
            .collect();
        let balances: Vec<_> = data
            .balances
            .into_iter()
            .filter_map(|v| parse_balance_item(v, &plan_names))
            .collect();
        assert_eq!(balances.len(), 2);
        assert_eq!(balances[0].show_name, "Global·GLM-5.3-Flash");
        assert_eq!(balances[1].show_name, "Start·GLM-5.3-Flash");
        // 两个套餐各自的剩余量保留
        assert_eq!(balances[0].remaining_units, 100_000_000.0);
        assert_eq!(balances[1].remaining_units, 0.0);
    }

    #[test]
    fn plan_short_name_maps_known_plans() {
        assert_eq!(plan_short_name("ZCode Start Plan"), "Start");
        assert_eq!(plan_short_name("ZCode Global Build"), "Global");
        assert_eq!(plan_short_name("GLM Coding Plan"), "Coding");
        assert_eq!(plan_short_name("Team Enterprise"), "Team");
        // 未知套餐取第一个词
        assert_eq!(plan_short_name("Foo Bar Plan"), "Foo");
    }

    #[test]
    fn read_active_bigmodel_provider_parses_selection() {
        // 只验证解析逻辑走通：本机 setting.json 可能存在也可能不存在，
        // 存在时必须是合法字符串或 None，不允许 panic。
        let selected = read_active_bigmodel_provider();
        if let Some(value) = selected {
            assert!(!value.is_empty());
        }
    }

    #[test]
    fn plan_is_current_matches_provider_tail_not_key_prefix() {
        // selectedKey = "coding-plan:builtin:bigmodel-start-plan"：前缀
        // "coding-plan:" 是 key 类型，真正的供应者在最后一段。用整串
        // contains 判断会把 start-plan 误判成 coding-plan（回归防护）。
        // 另注意 Global Build 的 plan_id 也挂在 start-plan 体系下
        // （zcode-v3-start-plan-0914），必须按名字区分。
        let start_tail = "builtin:bigmodel-start-plan";
        let coding_tail = "builtin:bigmodel-coding-plan";
        let make = |name: &str, plan_id: &str| PlanSummary {
            plan_id: Some(plan_id.into()),
            name: name.into(),
            status: None,
            ends_at: None,
            priority: 0,
            is_current: false,
        };

        // Start Plan 供应者选中时：Start Plan 命中；Global Build 与
        // personal 积分组不命中（名字/id 均不含 start 或明确含 global）。
        assert!(plan_is_current(&make("ZCode Start Plan", "zcode-v3-start-plan-0817"), start_tail));
        assert!(!plan_is_current(
            &make("ZCode Global Build", "zcode-v3-start-plan-0914"),
            start_tail
        ));
        assert!(!plan_is_current(&make("GLM Coding Lite", "personal:glm-coding"), start_tail));

        // Coding Plan 供应者选中时：个人套餐组命中，start 不命中
        assert!(plan_is_current(&make("GLM Coding Lite", "personal:glm-coding"), coding_tail));
        assert!(!plan_is_current(&make("ZCode Start Plan", "zcode-v3-start-plan-0817"), coding_tail));

        // 名字缺失时 id 无法区分 Global/Start（同挂 start-plan 体系），不标记
        assert!(!plan_is_current(&make("", "zcode-v3-start-plan-0817"), start_tail));
        assert!(!plan_is_current(&make("", "zcode-v3-start-plan-0914"), start_tail));

        // 空 selectedKey 一律不命中
        assert!(!plan_is_current(&make("ZCode Start Plan", "zcode-v3-start-plan-0817"), ""));
    }

    #[test]
    fn personal_coding_group_joins_plans_with_current_flag() {
        // 端到端构造：billing 两个套餐 + 个人套餐积分组，选中 start-plan。
        // 注意 Global Build 的 plan_id 与 Start Plan 同挂 start-plan 体系
        // （真实数据：zcode-v3-start-plan-0914），靠名字区分。
        let data: BillingBalanceData = serde_json::from_value(json!({
            "plans": [
                {"plan_id": "zcode-v3-start-plan-0914", "name": "ZCode Global Build",
                 "priority": 200, "status": "active"},
                {"plan_id": "zcode-v3-start-plan-0817", "name": "ZCode Start Plan",
                 "priority": 90, "status": "active"}
            ],
            "balances": [
                {"plan_id": "zcode-v3-start-plan-0914", "show_name": "GLM-5.3-Flash",
                 "total_units": 100000000, "used_units": 0, "remaining_units": 100000000}
            ]
        }))
        .unwrap();
        let plan_names: std::collections::HashMap<String, String> = data
            .plans
            .iter()
            .filter_map(|p| {
                p.plan_id
                    .clone()
                    .zip(p.name.clone())
                    .map(|(id, name)| (id, name))
            })
            .collect();
        let mut summaries: Vec<PlanSummary> = data
            .plans
            .into_iter()
            .map(|p| PlanSummary {
                plan_id: p.plan_id,
                name: p.name.unwrap_or_default(),
                status: p.status,
                ends_at: p.ends_at.filter(|v| *v > 0.0),
                priority: p.priority.unwrap_or(0),
                is_current: false,
            })
            .collect();
        let _ = parse_balance_item(
            data.balances.into_iter().next().unwrap(),
            &plan_names,
        )
        .unwrap();
        summaries.push(PlanSummary {
            plan_id: Some("personal:glm-coding".into()),
            name: "GLM Coding Lite".into(),
            status: Some("active".into()),
            ends_at: None,
            priority: i64::MAX,
            is_current: false,
        });
        let selected = "builtin:bigmodel-start-plan";
        for plan in &mut summaries {
            plan.is_current = plan_is_current(plan, selected);
        }
        assert_eq!(summaries.len(), 3);
        assert_eq!(
            summaries
                .iter()
                .filter(|p| p.is_current)
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            vec!["ZCode Start Plan"]
        );
    }

    #[test]
    fn parse_coding_plan_keeps_returned_quota_items() {
        let data: BillingBalanceData = serde_json::from_value(json!({
            "plans": [{
                "name": "ZCode Coding Plan",
                "description": "Coding Plan 套餐",
                "priority": 200,
                "status": "active"
            }],
            "balances": [{
                "show_name": "每5小时使用额度",
                "total_units": 100,
                "used_units": 100,
                "available_units": 0,
                "period": "5h"
            }, {
                "show_name": "每周使用额度",
                "total_units": 100,
                "used_units": 20,
                "available_units": 80,
                "period": "weekly"
            }, {
                "show_name": "MCP 每月额度",
                "total_units": 100,
                "used_units": 0,
                "available_units": 100,
                "period": "monthly"
            }]
        }))
        .unwrap();

        let (name, _, _, _) = pick_best_plan(data.plans);
        let balances: Vec<_> = data
            .balances
            .into_iter()
            .filter_map(|v| parse_balance_item(v, &std::collections::HashMap::new()))
            .collect();
        assert_eq!(name.as_deref(), Some("ZCode Coding Plan"));
        assert_eq!(balances.len(), 3);
        assert_eq!(balances[0].show_name, "每5小时使用额度");
        assert_eq!(balances[1].remaining_units, 80.0);
        assert_eq!(balances[2].period.as_deref(), Some("monthly"));
    }

    #[test]
    fn parse_logged_balance_payload() {
        let line = r#"[2026-07-07 08:52:01.399] [info] [usage-stats] billing/balance 请求完成 {"balanceCount":2,"payload":{"code":0,"msg":"","data":{"plans":[{"name":"ZCode Start Plan","priority":100,"status":"active"}],"balances":[{"show_name":"GLM-5.3","total_units":3000000,"used_units":0,"remaining_units":3000000},{"show_name":"GLM-5-Turbo","total_units":2000000,"used_units":0,"remaining_units":2000000}]}}}"#;
        let data = parse_logged_balance_line(line).unwrap();
        assert_eq!(data.plans.len(), 1);
        assert_eq!(data.balances.len(), 2);
    }

    #[test]
    fn parse_device_mid_from_telemetry_state() {
        assert_eq!(
            parse_device_mid(r#"{"deviceMid":"957a0788-30eb-4aa9-9c71-1fcc3b92e8aa"}"#),
            Some("957a0788-30eb-4aa9-9c71-1fcc3b92e8aa".into())
        );
        assert_eq!(
            parse_device_mid(r#"{"deviceMid": "  padded  ","lastDailyActiveDate":"2026-09-15"}"#),
            Some("padded".into())
        );
        assert_eq!(parse_device_mid(r#"{}"#), None);
        assert_eq!(parse_device_mid(r#"{"deviceMid":""}"#), None);
        assert_eq!(parse_device_mid("not json"), None);
    }

    #[test]
    fn generated_device_mid_is_uuid_v4_shaped() {
        let mid = new_device_mid();
        assert_eq!(mid.len(), 36);
        let dashes: Vec<usize> = mid
            .char_indices()
            .filter(|(_, ch)| *ch == '-')
            .map(|(index, _)| index)
            .collect();
        assert_eq!(dashes, vec![8, 13, 18, 23]);
        let hex: String = mid.chars().filter(|ch| *ch != '-').collect();
        assert!(hex.chars().all(|ch| ch.is_ascii_hexdigit()));
        // 版本 4 + RFC 4122 变体位
        assert_eq!(hex.as_bytes()[12], b'4');
        assert!(matches!(
            hex.as_bytes()[16],
            b'8' | b'9' | b'a' | b'b'
        ));
        // 连续生成不重复
        assert_ne!(mid, new_device_mid());
    }

    #[test]
    fn fallback_device_mid_roundtrip_and_reuse() {
        let dir = std::env::temp_dir().join(format!(
            "zcs-quota-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let path = dir.join("zcode-switcher-device.json");
        let mid = new_device_mid();
        write_fallback_device_mid(&path, &mid).expect("write fallback device mid");
        // 落盘的文件能被 parse_device_mid 读回（与 telemetry-state.json 同构）。
        let text = fs::read_to_string(&path).expect("read back");
        assert_eq!(parse_device_mid(&text).as_deref(), Some(mid.as_str()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_coding_plan_usage_payload() {
        let value: Value = serde_json::from_str(
            r#"{"code":200,"msg":"操作成功","data":{"limits":[
                {"type":"CREDIT_LIMIT","unit":3,"number":5,"usage":28000,"currentValue":2566,"remaining":25434,"percentage":9,"nextResetTime":1789522664420},
                {"type":"CREDIT_LIMIT","unit":6,"number":1,"usage":140000,"currentValue":73547,"remaining":66453,"percentage":52,"nextResetTime":1789808998979},
                {"type":"CREDIT_LIMIT","unit":5,"number":1,"usage":6000,"currentValue":1200,"remaining":4800,"percentage":20,"nextResetTime":1790007765994},
                {"type":"TIME_LIMIT","unit":5,"number":1,"usage":4000,"currentValue":754,"remaining":3246,"percentage":18,"nextResetTime":1790256740998,"usageDetails":[{"modelCode":"search-prime","usage":676}]},
                {"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":0}
            ],"level":"max"},"success":true}"#,
        )
        .unwrap();
        let snapshot = parse_coding_plan_usage(&value).expect("应解析成功");
        assert_eq!(snapshot.level, "max");
        assert_eq!(snapshot.items.len(), 4);

        // 5 小时窗口的"5小时积分"桶（官网同源数据）
        let credits = &snapshot.items[0];
        assert_eq!(credits.show_name, "5小时积分");
        assert_eq!(credits.used_units, 2566.0);
        assert_eq!(credits.total_units, 28000.0);
        assert_eq!(credits.remaining_units, 25434.0);
        assert_eq!(credits.unit_type.as_deref(), Some("point"));
        assert_eq!(credits.period.as_deref(), Some("5h"));

        // 每周窗口的"周积分"桶
        let weekly = &snapshot.items[1];
        assert_eq!(weekly.show_name, "周积分");
        assert_eq!(weekly.total_units, 140000.0);
        assert_eq!(weekly.remaining_units, 66453.0);
        assert_eq!(weekly.period.as_deref(), Some("weekly"));

        // 每月窗口的"月积分"桶
        let monthly = &snapshot.items[2];
        assert_eq!(monthly.show_name, "月积分");
        assert_eq!(monthly.remaining_units, 4800.0);
        assert_eq!(monthly.period.as_deref(), Some("monthly"));

        // TIME_LIMIT 是 MCP 工具调用额度（次），仅展示、不参与积分阈值
        let tool = &snapshot.items[3];
        assert_eq!(tool.show_name, "月度工具额度");
        assert_eq!(tool.remaining_units, 3246.0);
        assert_eq!(tool.unit_type.as_deref(), Some("tool"));
        assert_eq!(tool.period.as_deref(), Some("monthly"));

        // TOKENS_LIMIT（仅百分比、无绝对值）不产出条目
        assert!(snapshot
            .items
            .iter()
            .all(|item| item.show_name != "5小时Token"));

        assert_eq!(coding_plan_display_name(&snapshot.level), "GLM Coding Max");
    }

    #[test]
    fn credit_window_threshold_uses_min_across_windows() {
        // 阈值判定语义：任一窗口积分剩余低于阈值即触发（取各积分桶最小值）。
        let value: Value = serde_json::from_str(
            r#"{"code":200,"success":true,"data":{"limits":[
                {"type":"CREDIT_LIMIT","unit":3,"number":5,"usage":28000,"currentValue":27900,"remaining":100},
                {"type":"CREDIT_LIMIT","unit":6,"number":1,"usage":140000,"currentValue":1000,"remaining":139000}
            ],"level":"max"}}"#,
        )
        .unwrap();
        let snapshot = parse_coding_plan_usage(&value).expect("应解析成功");
        let min_point = snapshot
            .items
            .iter()
            .filter(|item| item.unit_type.as_deref() == Some("point"))
            .map(|item| item.remaining_units)
            .fold(f64::INFINITY, f64::min);
        assert_eq!(min_point, 100.0);
    }

    #[test]
    fn parse_coding_plan_usage_skips_malformed_entries_not_whole_snapshot() {
        // 一条缺 usage/currentValue 的坏条目只应跳过自身；整快照被丢弃会
        // 静默跌落到滞后的 mcp/usage 汇总兜底（回归防护）。
        let value: Value = serde_json::from_str(
            r#"{"code":200,"success":true,"data":{"limits":[
                {"type":"CREDIT_LIMIT","unit":6,"number":1,"currentValue":100},
                {"type":"CREDIT_LIMIT","unit":3,"number":5,"usage":28000,"currentValue":2566,"remaining":25434},
                {"type":"TIME_LIMIT","unit":5,"number":1}
            ],"level":"max"}}"#,
        )
        .unwrap();
        let snapshot = parse_coding_plan_usage(&value).expect("好条目存在时应解析成功");
        assert_eq!(snapshot.items.len(), 1);
        assert_eq!(snapshot.items[0].show_name, "5小时积分");
        assert_eq!(snapshot.items[0].remaining_units, 25434.0);
    }

    #[test]
    fn parse_coding_plan_usage_rejects_errors_and_empty() {
        // 未订阅 / 无效凭据：code != 200
        let denied: Value =
            serde_json::from_str(r#"{"code":401,"msg":"令牌已过期或验证不正确","success":false}"#)
                .unwrap();
        assert!(parse_coding_plan_usage(&denied).is_none());
        // limits 为空 → 无可显示积分
        let empty: Value =
            serde_json::from_str(r#"{"code":200,"data":{"limits":[],"level":"lite"}}"#).unwrap();
        assert!(parse_coding_plan_usage(&empty).is_none());
        // 仅 TOKENS_LIMIT（无绝对值字段）→ 无可显示积分
        let tokens_only: Value = serde_json::from_str(
            r#"{"code":200,"success":true,"data":{"limits":[
                {"type":"TOKENS_LIMIT","unit":6,"number":1,"percentage":100}
            ],"level":"max"}}"#,
        )
        .unwrap();
        assert!(parse_coding_plan_usage(&tokens_only).is_none());
    }

    /// 真机端到端验证：用真实 credentials 走一遍完整刷新链路。
    /// 仅手动运行：`cargo test -p zcode-switcher --lib --release -- --ignored fetch_quota_real`
    #[test]
    #[ignore = "依赖本机 ~/.zcode/v2 凭据与外网，仅手动运行"]
    fn fetch_quota_real_credentials() {
        let home = dirs::home_dir().expect("home dir");
        let text = std::fs::read_to_string(
            home.join(".zcode").join("v2").join("credentials.json"),
        )
        .expect("credentials.json");
        let info = tokio::runtime::Runtime::new()
            .expect("runtime")
            .block_on(fetch_quota(&text))
            .expect("fetch_quota 应成功");
        println!(
            "plan={:?} status={:?} balances={}",
            info.plan_name,
            info.plan_status,
            info.balances.len()
        );
        assert!(!info.balances.is_empty(), "balances 不应为空");
    }
}
