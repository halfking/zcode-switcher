//! 订阅额度查询：用解密后的 zcodejwttoken 调 ZCode 的 billing 接口。
//!
//! - GET https://zcode.z.ai/api/v1/zcode-plan/billing/balance?app_version=... → 当前套餐 + 用量/余额

use serde::Deserialize;
use serde_json::Value;
use std::{fs, path::PathBuf, time::SystemTime};

use crate::crypto;

const BASE: &str = "https://zcode.z.ai";
const APP_VERSION_CANDIDATES: &[&str] = &["3.2.5", crate::captcha::ZCODE_APP_VERSION];

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

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("HTTP 客户端创建失败：{}", e))?;

    let balance = match fetch_billing_balance(&client, &token).await {
        Ok(balance) => balance,
        Err(e) => latest_logged_balance_for_current_token(&token).ok_or(e)?,
    };
    let (plan_name, plan_description, plan_status, plan_ends_at) = pick_best_plan(balance.plans);
    let balances: Vec<BalanceItem> = balance
        .balances
        .into_iter()
        .filter_map(parse_balance_item)
        .collect();
    if balances.is_empty() {
        return Err("额度接口未返回可显示的模型额度明细".into());
    }

    Ok(QuotaInfo {
        plan_name,
        plan_description,
        plan_status,
        plan_ends_at,
        balances,
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

fn parse_balance_item(value: Value) -> Option<BalanceItem> {
    let mut item = serde_json::from_value::<BalanceItem>(value.clone()).ok()?;
    if item.show_name.trim().is_empty() {
        item.show_name = string_field(&value, "name")
            .or_else(|| string_field(&value, "entitlement_id"))
            .unwrap_or_else(|| "额度".into());
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
                "show_name": "GLM-5.2",
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
            .filter_map(parse_balance_item)
            .collect();
        assert_eq!(name.as_deref(), Some("ZCode Start Plan"));
        assert_eq!(status.as_deref(), Some("active"));
        assert_eq!(ends_at, Some(1783785599.0));
        assert_eq!(balances.len(), 2);
        assert_eq!(balances[0].show_name, "GLM-5.2");
        assert_eq!(balances[0].remaining_units, 2_900_000.0);
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
            .filter_map(parse_balance_item)
            .collect();
        assert_eq!(name.as_deref(), Some("ZCode Coding Plan"));
        assert_eq!(balances.len(), 3);
        assert_eq!(balances[0].show_name, "每5小时使用额度");
        assert_eq!(balances[1].remaining_units, 80.0);
        assert_eq!(balances[2].period.as_deref(), Some("monthly"));
    }

    #[test]
    fn parse_logged_balance_payload() {
        let line = r#"[2026-07-07 08:52:01.399] [info] [usage-stats] billing/balance 请求完成 {"balanceCount":2,"payload":{"code":0,"msg":"","data":{"plans":[{"name":"ZCode Start Plan","priority":100,"status":"active"}],"balances":[{"show_name":"GLM-5.2","total_units":3000000,"used_units":0,"remaining_units":3000000},{"show_name":"GLM-5-Turbo","total_units":2000000,"used_units":0,"remaining_units":2000000}]}}}"#;
        let data = parse_logged_balance_line(line).unwrap();
        assert_eq!(data.plans.len(), 1);
        assert_eq!(data.balances.len(), 2);
    }
}
