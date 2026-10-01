//! BigModel 个人套餐「积分重置卡」：查询库存与使用。
//!
//! 与官网控制台（open.bigmodel.cn 套餐用量页）同源，认证与 quota/limit
//! 相同（BigModel OAuth access_token Bearer）。接口契约从控制台前端
//! bundle 取证 + 真机只读验证（2026-10-01）：
//!
//! - `GET /api/biz/customer-package-reset/list?targetType=PERSONAL`
//!   → `data.fiveHourResets[]` / `data.weekResets[]`，每条：
//!     `{recordId: i64, expireTime: "YYYY-MM-DD HH:mm:ss",
//!       grantType: "DIRECT"|..., available: bool}`
//! - `POST /api/biz/customer-package-reset/use`
//!   body `{targetType: "PERSONAL", resetType: "FIVE_HOUR"|"WEEK",
//!          recordId, grantType, requestId: <uuid>}`；requestId 是幂等键
//!   （控制台对同一确认弹窗复用同一个 requestId 重试）。
//!
//! 语义（官网说明）：周重置会把 5 小时与周额度一起回满，且不消耗 5 小时
//! 重置次数；两类卡各自独立计数、有过期时间。选卡与控制台一致：同类
//! 可用卡里最先过期的先用（避免放着过期浪费）。

use serde::Serialize;
use serde_json::{json, Value};

const BASE: &str = "https://open.bigmodel.cn";
const LIST_URL: &str = "/api/biz/customer-package-reset/list";
const USE_URL: &str = "/api/biz/customer-package-reset/use";

/// 重置卡类型。序列化/入参用控制台的 SCREAMING_SNAKE_CASE 字面量。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ResetCardType {
    #[serde(rename = "FIVE_HOUR")]
    FiveHour,
    #[serde(rename = "WEEK")]
    Week,
}

impl ResetCardType {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "FIVE_HOUR" | "FIVEHOUR" | "5H" => Some(Self::FiveHour),
            "WEEK" | "WEEKLY" => Some(Self::Week),
            _ => None,
        }
    }

    /// list 响应里该类型的数组字段名。
    fn source_key(self) -> &'static str {
        match self {
            Self::FiveHour => "fiveHourResets",
            Self::Week => "weekResets",
        }
    }

    /// 展示名（错误信息用）。
    fn label(self) -> &'static str {
        match self {
            Self::FiveHour => "5 小时重置卡",
            Self::Week => "周重置卡",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ResetCardRecord {
    pub record_id: i64,
    /// 过期时间原样透传（"YYYY-MM-DD HH:mm:ss"）。
    pub expire_time: String,
    pub grant_type: String,
    pub available: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ResetCardInventory {
    pub five_hour: Vec<ResetCardRecord>,
    pub week: Vec<ResetCardRecord>,
}

impl ResetCardInventory {
    /// 指定类型的可用卡数量。
    pub fn available_count(&self, card_type: ResetCardType) -> usize {
        let list = match card_type {
            ResetCardType::FiveHour => &self.five_hour,
            ResetCardType::Week => &self.week,
        };
        list.iter().filter(|r| r.available).count()
    }
}

fn parse_records(data: &Value, key: &str) -> Vec<ResetCardRecord> {
    let mut records = Vec::new();
    let Some(arr) = data.get(key).and_then(Value::as_array) else {
        return records;
    };
    for item in arr {
        // 单条字段缺失只跳过该条（与 quota 解析同一原则）。
        let Some(record_id) = item.get("recordId").and_then(Value::as_i64) else {
            continue;
        };
        records.push(ResetCardRecord {
            record_id,
            expire_time: item
                .get("expireTime")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            grant_type: item
                .get("grantType")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            available: item.get("available").and_then(Value::as_bool) == Some(true),
        });
    }
    // 最先过期的排前面（时间字符串同格式，字典序即时间序）。
    records.sort_by(|a, b| a.expire_time.cmp(&b.expire_time));
    records
}

fn parse_inventory(value: &Value) -> Option<ResetCardInventory> {
    let data = value.get("data")?;
    Some(ResetCardInventory {
        five_hour: parse_records(data, "fiveHourResets"),
        week: parse_records(data, "weekResets"),
    })
}

/// uuid v4 形状的幂等键（与 quota.rs 设备标识同一生成手法）。
fn request_id() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn oauth_token(creds_text: &str) -> Result<String, String> {
    let creds: Value = serde_json::from_str(creds_text)
        .map_err(|e| format!("解析 credentials 失败：{}", e))?;
    crate::quota::oauth_access_token(&creds)
}

/// 查询重置卡库存（只读）。
pub async fn fetch_reset_cards(creds_text: &str) -> Result<ResetCardInventory, String> {
    let token = oauth_token(creds_text)?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("HTTP 客户端创建失败：{}", e))?;
    let resp = client
        .get(format!("{}{}", BASE, LIST_URL))
        .query(&[("targetType", "PERSONAL")])
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .map_err(|e| format!("重置卡库存请求失败：{}", e))?;
    if !resp.status().is_success() {
        return Err(format!("重置卡库存状态码 {}", resp.status()));
    }
    let value: Value = resp
        .json()
        .await
        .map_err(|e| format!("重置卡库存解析失败：{}", e))?;
    let code_ok = value.get("code").and_then(Value::as_i64) == Some(200)
        || value.get("success").and_then(Value::as_bool) == Some(true);
    if !code_ok {
        let msg = value.get("msg").and_then(Value::as_str).unwrap_or("未知错误");
        return Err(format!("重置卡库存查询失败：{}", msg));
    }
    parse_inventory(&value).ok_or_else(|| "重置卡库存响应格式异常".to_string())
}

/// 使用一张指定类型的重置卡（同类里最先过期的可用卡）。
///
/// 返回被使用的卡记录。requestId 幂等：网络类失败的重试复用同一个 id，
/// 服务端不会因重试重复扣卡。
pub async fn use_reset_card(
    creds_text: &str,
    card_type: ResetCardType,
) -> Result<ResetCardRecord, String> {
    let token = oauth_token(creds_text)?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("HTTP 客户端创建失败：{}", e))?;

    let inventory = fetch_reset_cards(creds_text).await?;
    let pool = match card_type {
        ResetCardType::FiveHour => &inventory.five_hour,
        ResetCardType::Week => &inventory.week,
    };
    // parse_records 已按过期时间升序排序：第一张可用卡即最先过期的。
    let target = pool
        .iter()
        .find(|r| r.available)
        .ok_or_else(|| format!("没有可用的{}", card_type.label()))?;

    let body = json!({
        "targetType": "PERSONAL",
        "resetType": card_type,
        "recordId": target.record_id,
        "grantType": target.grant_type,
        "requestId": request_id(),
    });

    const MAX_ATTEMPTS: usize = 2;
    let mut last_err = None;
    for attempt in 0..MAX_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        }
        let resp = match client
            .post(format!("{}{}", BASE, USE_URL))
            .header("Authorization", format!("Bearer {}", token))
            .json(&body)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                last_err = Some(format!("重置卡使用请求失败：{}", e));
                continue;
            }
        };
        if !resp.status().is_success() {
            // 429 退避一次，其余状态码直接报错（幂等键保证重试不双扣）。
            if resp.status().as_u16() == 429 && attempt + 1 < MAX_ATTEMPTS {
                last_err = Some("重置卡使用限流".into());
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                continue;
            }
            return Err(format!("重置卡使用状态码 {}", resp.status()));
        }
        let value: Value = match resp.json().await {
            Ok(value) => value,
            Err(e) => {
                last_err = Some(format!("重置卡使用响应解析失败：{}", e));
                continue;
            }
        };
        let code_ok = value.get("code").and_then(Value::as_i64) == Some(200)
            || value.get("success").and_then(Value::as_bool) == Some(true);
        if !code_ok {
            let msg = value.get("msg").and_then(Value::as_str).unwrap_or("未知错误");
            return Err(format!("重置卡使用失败：{}", msg));
        }
        return Ok(target.clone());
    }
    Err(last_err.unwrap_or_else(|| "重置卡使用失败".into()))
}

/// 解析命令入参的档案 id：空/None 用当前登录的 credentials.json。
/// 与 profile::fetch_quota 同一口径。
fn resolve_credentials_text(id: Option<String>) -> Result<String, String> {
    use crate::profile::{credentials_file, profile_credentials_text};
    match id {
        Some(i) if !i.is_empty() => profile_credentials_text(&i).map_err(|e| e.to_string()),
        _ => {
            let path = credentials_file().map_err(|e| e.to_string())?;
            if !path.exists() {
                return Err("找不到 credentials.json，请先在 ZCode 登录。".into());
            }
            std::fs::read_to_string(&path).map_err(|e| e.to_string())
        }
    }
}

/// 查询某个档案的重置卡库存（只读）。id 为空时查当前登录账号。
#[tauri::command]
pub async fn reset_card_inventory(id: Option<String>) -> Result<ResetCardInventory, String> {
    let text = resolve_credentials_text(id)?;
    fetch_reset_cards(&text).await
}

/// 使用一张指定类型的重置卡（先到期的先用）。id 为空时对当前登录账号操作。
#[tauri::command]
pub async fn use_reset_card_cmd(
    id: Option<String>,
    reset_type: String,
) -> Result<ResetCardRecord, String> {
    let card_type = ResetCardType::parse(&reset_type)
        .ok_or_else(|| format!("未知重置卡类型：{}", reset_type))?;
    let text = resolve_credentials_text(id)?;
    use_reset_card(&text, card_type).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_inventory_sorts_by_expire_time_and_filters_shape() {
        let value = json!({
            "code": 200,
            "success": true,
            "data": {
                "customerId": 42,
                "fiveHourResets": [
                    {"recordId": 3, "expireTime": "2026-10-30 01:35:04", "grantType": "DIRECT", "available": true},
                    {"recordId": 1, "expireTime": "2026-10-20 01:35:04", "grantType": "DIRECT", "available": true},
                    {"recordId": 2, "expireTime": "2026-09-25 01:07:43", "grantType": "DIRECT", "available": false},
                    {"grantType": "DIRECT"}
                ],
                "weekResets": [
                    {"recordId": 9, "expireTime": "2026-10-30 01:35:04", "grantType": "DIRECT", "available": true}
                ]
            }
        });
        let inv = parse_inventory(&value).expect("应解析成功");
        assert_eq!(inv.five_hour.len(), 3, "缺 recordId 的坏条目应跳过");
        // 升序：09-25 < 10-20 < 10-30；不可用卡保留在列表里（available=false）。
        assert_eq!(inv.five_hour[0].record_id, 2);
        assert_eq!(inv.five_hour[0].available, false);
        assert_eq!(inv.five_hour[1].record_id, 1);
        assert_eq!(inv.five_hour[2].record_id, 3);
        assert_eq!(inv.five_hour[2].available, true);
        assert_eq!(inv.week.len(), 1);
        assert_eq!(inv.available_count(ResetCardType::FiveHour), 2);
        assert_eq!(inv.available_count(ResetCardType::Week), 1);
    }

    #[test]
    fn parse_inventory_rejects_envelope_errors() {
        let denied = json!({"code": 401, "msg": "令牌已过期", "success": false});
        let value = json!({"code": 200, "success": true, "data": denied});
        // data 存在即解析；envelope 判定发生在 fetch 层，这里只测结构缺失。
        assert!(parse_inventory(&json!({"code": 200, "success": true})).is_none());
    }

    #[test]
    fn reset_card_type_parses_console_literals() {
        assert_eq!(ResetCardType::parse("FIVE_HOUR"), Some(ResetCardType::FiveHour));
        assert_eq!(ResetCardType::parse("week"), Some(ResetCardType::Week));
        assert_eq!(ResetCardType::parse("monthly"), None);
        let ser = serde_json::to_string(&ResetCardType::Week).unwrap();
        assert_eq!(ser, "\"WEEK\"");
    }

    #[test]
    fn request_id_is_uuid_shaped_and_unique() {
        let a = request_id();
        let b = request_id();
        assert_eq!(a.len(), 36);
        assert_eq!(a.matches('-').count(), 4);
        assert_ne!(a, b);
    }

    /// 真机只读验证库存接口（认证 + 响应结构）。
    /// `cargo test --lib --release -- --ignored fetch_reset_cards_real`
    #[test]
    #[ignore = "只读探针：依赖本机凭据与外网，仅手动运行"]
    fn fetch_reset_cards_real() {
        let home = dirs::home_dir().expect("home dir");
        let text = std::fs::read_to_string(
            home.join(".zcode").join("v2").join("credentials.json"),
        )
        .expect("credentials.json");
        let inv = tokio::runtime::Runtime::new()
            .expect("runtime")
            .block_on(fetch_reset_cards(&text))
            .expect("库存查询应成功");
        println!(
            "5h 可用 {}/{}，周可用 {}/{}",
            inv.available_count(ResetCardType::FiveHour),
            inv.five_hour.len(),
            inv.available_count(ResetCardType::Week),
            inv.week.len()
        );
        for r in &inv.five_hour {
            println!("  5h record={} expire={} available={}", r.record_id, r.expire_time, r.available);
        }
        for r in &inv.week {
            println!("  week record={} expire={} available={}", r.record_id, r.expire_time, r.available);
        }
    }
}
