//! 账号档案库：读取/写入 credentials.json 的快照。
//!
//! 路径与数据格式与 Python 版完全一致，已保存的档案可被任一版本接管：
//!   - 凭据文件:  ~/.zcode/v2/credentials.json
//!   - 身份指纹:  ~/.zcode/v2/config.json 里 coding-plan 的 JWT apiKey
//!   - 档案库:    ~/.zcode/v2/account-profiles/profiles.json (+ credentials.{id}.json)
//!   - 备份:      ~/.zcode/v2/account-backups/credentials.switch.{ts}.json

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::jwt;

/// 命令错误类型：Tauri 2 要求返回的错误实现 Serialize + Into<InvokeError>。
/// 这里把错误序列化成字符串返回给前端。
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Msg(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.to_string().as_ref())
    }
}

/// 把任何可显示的错误转成 AppError::Msg
fn err<E: std::fmt::Display>(e: E) -> AppError {
    AppError::Msg(e.to_string())
}

type R<T> = std::result::Result<T, AppError>;

// --------------------------------------------------------------------------- //
//  路径
//
//  ZCode 把"设置目录"和"数据目录"拆成两套（见 out/main/index.js）：
//    - setting.json        → getSettingsDir() = (HOME‖USERPROFILE)/.zcode/v2
//                            **永远 home 基址，不受 dataBaseDir 影响**
//    - credentials.json    → getCredentialsFile = J() = getDataBaseDir()/.zcode/v2
//      config.json         → J() 同上
//
//  getDataBaseDir() = dataBaseDir(读 setting.json) ‖ ZCODE_DATA_BASE_DIR 环境变量
//                     ‖ HOME 环境变量 ‖ os.homedir()
//
//  **为什么必须这样**：用户可以在 ZCode 设置里把数据目录改到别的盘（如
//  `dataBaseDir: "D:\\AppGallery\\Zcode"`），于是 ZCode 把 credentials/config 读写到
//  `D:\AppGallery\Zcode\.zcode\v2`，但 setting.json 仍在 `C:\Users\xxx\.zcode\v2`。
//  之前 switcher 把所有文件写死到 home，导致 credentials/config 落到 ZCode 看不到的盘 ——
//  表现为"任何账号都切不了"（自己的、导入的、所有模式都一样）。
//
//  注意：加密密钥派生用的是 os.homedir()（= USERPROFILE），所以 crypto.rs 仍用
//  dirs::home_dir()，**不要**改成这里的函数。
// --------------------------------------------------------------------------- //

/// home 基址：HOME 环境变量 → USERPROFILE 环境变量 → dirs::home_dir()
/// （对应 ZCode 的 resolveUserHomeDir：`process.env.HOME || process.env.USERPROFILE || homedir`）
fn home_base_dir() -> R<PathBuf> {
    for key in ["HOME", "USERPROFILE"] {
        if let Ok(v) = std::env::var(key) {
            let t = v.trim();
            if !t.is_empty() && looks_like_native_path(t) {
                return Ok(PathBuf::from(t));
            }
        }
    }
    dirs::home_dir().ok_or_else(|| AppError::Msg("找不到用户主目录".into()))
}

/// 设置目录：跟 ZCode getSettingsDir 一致 = home/.zcode/v2。setting.json 永远在这里。
pub fn zcode_settings_dir() -> R<PathBuf> {
    Ok(home_base_dir()?.join(".zcode").join("v2"))
}

/// 从 setting.json 读 `dataBaseDir`（ZCode 设置里的"数据目录"覆盖）。没有/读不到/空 → None。
fn data_base_dir_from_setting() -> Option<PathBuf> {
    let path = zcode_settings_dir().ok()?.join("setting.json");
    let text = fs::read_to_string(&path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let d = v.get("dataBaseDir")?.as_str()?.trim();
    if d.is_empty() || !looks_like_native_path(d) {
        None
    } else {
        Some(PathBuf::from(d))
    }
}

/// 数据基址：跟 ZCode getDataBaseDir 一致
///   dataBaseDir(setting.json) → ZCODE_DATA_BASE_DIR 环境变量 → HOME → USERPROFILE → homedir
pub fn zcode_data_base_dir() -> R<PathBuf> {
    if let Some(d) = data_base_dir_from_setting() {
        return Ok(d);
    }
    if let Ok(v) = std::env::var("ZCODE_DATA_BASE_DIR") {
        let t = v.trim();
        if !t.is_empty() && looks_like_native_path(t) {
            return Ok(PathBuf::from(t));
        }
    }
    home_base_dir()
}

/// 是否是原生路径（Windows 下要求带盘符 `C:\` 或 UNC `\\`，避免把 git-bash 的
/// MSYS 风格 `/c/Users/xxx` 误当成可用路径——ZCode（GUI 启动）也只会看到原生路径）。
/// 非 Windows 平台一律放行（绝对路径即可）。
fn looks_like_native_path(p: &str) -> bool {
    #[cfg(windows)]
    {
        let bytes = p.as_bytes();
        let drive = bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/');
        let unc = p.starts_with("\\\\");
        drive || unc
    }
    #[cfg(not(windows))]
    {
        p.starts_with('/')
    }
}

/// 数据目录：credentials.json / config.json / 我们的档案库 / 备份都在这里。
pub fn zcode_v2_dir() -> R<PathBuf> {
    Ok(zcode_data_base_dir()?.join(".zcode").join("v2"))
}

/// setting.json 的完整路径（设置目录，home 基址）。
pub fn setting_file() -> R<PathBuf> {
    Ok(zcode_settings_dir()?.join("setting.json"))
}

pub fn credentials_file() -> R<PathBuf> {
    Ok(zcode_v2_dir()?.join("credentials.json"))
}

fn config_file() -> R<PathBuf> {
    Ok(zcode_v2_dir()?.join("config.json"))
}

/// 档案库放在**设置目录（home，稳定）**而非数据目录：
/// 这样用户即便改了 ZCode 的 dataBaseDir，已保存的账号档案也不会丢；
/// 且老版本本就把档案写在 home，升级后能继续看到原有档案。
fn profiles_dir() -> R<PathBuf> {
    Ok(zcode_settings_dir()?.join("account-profiles"))
}

fn profiles_index() -> R<PathBuf> {
    Ok(profiles_dir()?.join("profiles.json"))
}

fn backup_dir() -> R<PathBuf> {
    Ok(zcode_settings_dir()?.join("account-backups"))
}

// --------------------------------------------------------------------------- //
//  数据结构
// --------------------------------------------------------------------------- //
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub phone: String,
    #[serde(default)]
    pub avatar: String,
    pub cred_hash: String,
    #[serde(default)]
    pub cred_file: String,
    pub created_at: f64,
    pub updated_at: f64,

    /// `"zai"` | `"bigmodel"` —— 这个账号在 ZCode 里属于哪条 provider family。
    /// 用于切号时把 `setting.json.providerFamilyDomain` 锁回对应值。
    /// 老档案没这字段，按"zai"兜底。
    #[serde(default)]
    pub family: String,

    /// `"oauth"` | `"apikey"` —— 这个账号的登录方式。
    /// - `oauth`：credentials.json 里有 zcodejwttoken，apiKey 由 ZCode 自己从凭据派生
    /// - `apikey`：用户自带 Z.ai / BigModel API Key，apiKey 直接存在 config.json 里
    /// 老档案没这字段，按"oauth"兜底。
    #[serde(default)]
    pub mode: String,

    /// 捕获时刻 `config.json.provider.<id>.options.apiKey` 的快照，覆盖 ZCode 6 条内置
    /// provider。切号时按存的值原样写回。仅 apikey 模式真正会用到这里的值；oauth 模式
    /// 切号仍以 credentials.json 解出的 JWT 为准（不依赖这个快照），但这里也会存以便回溯。
    #[serde(default)]
    pub provider_api_keys: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileView {
    #[serde(flatten)]
    pub profile: Profile,
    pub active: bool,
    /// 显示用的短 id
    pub short_id: String,
}

/// IPC 边界脱敏：前端（ProfileView TS 类型）从不消费 provider_api_keys 的值，
/// 返回给 webview 的 Profile 一律清空 key 值，缩小注入/自动化读取明文密钥的面。
/// 磁盘索引（save_index）仍保存完整值，切换等内部逻辑读的是磁盘，不受影响。
fn redact_provider_keys(mut p: Profile) -> Profile {
    for value in p.provider_api_keys.values_mut() {
        value.clear();
    }
    p
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PortableAccount {
    pub schema: String,
    pub exported_at: f64,
    pub profile: PortableProfile,
    pub credentials: Value,
    /// `"zai"` | `"bigmodel"` —— 老导出 JSON 没这字段，导入时按 credentials 现场推断。
    #[serde(default)]
    pub family: String,
    /// `"oauth"` | `"apikey"` —— 同上。
    #[serde(default)]
    pub mode: String,
    /// 老导出 JSON 没这字段，导入时空 map；apikey 账号靠这里才能还原 apiKey。
    #[serde(default)]
    pub provider_api_keys: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PortableAccountBundle {
    pub schema: String,
    pub exported_at: f64,
    pub accounts: Vec<PortableAccount>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PortableProfile {
    pub name: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub phone: String,
    #[serde(default)]
    pub avatar: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BatchImportReport {
    pub imported: usize,
    pub skipped: usize,
    pub failed: usize,
    pub messages: Vec<String>,
}

// --------------------------------------------------------------------------- //
//  工具函数
// --------------------------------------------------------------------------- //
fn now_ts() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn sha256_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

fn safe_export_file_name(name: &str, fallback: &str, index: usize) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else if ch.is_whitespace() {
            out.push('_');
        }
    }
    let base = if out.trim_matches('_').is_empty() {
        fallback.to_string()
    } else {
        out.trim_matches('_').chars().take(36).collect()
    };
    format!("{:03}-{}-{}.json", index + 1, base, fallback)
}

fn short_id(uid: &str) -> String {
    if uid.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = uid.chars().collect();
    if chars.len() <= 10 {
        uid.to_string()
    } else {
        let head: String = chars[..6].iter().collect();
        let tail: String = chars[chars.len() - 4..].iter().collect();
        format!("{}…{}", head, tail)
    }
}

/// 从 config.json 提取 user_id（coding-plan 的 JWT apiKey payload）。
fn extract_user_id_from_config() -> String {
    let path = match config_file() {
        Ok(p) => p,
        Err(_) => return String::new(),
    };
    let Ok(text) = fs::read_to_string(&path) else {
        return String::new();
    };
    let Ok(cfg) = serde_json::from_str::<Value>(&text) else {
        return String::new();
    };
    let Some(providers) = cfg.get("provider").and_then(|v| v.as_object()) else {
        return String::new();
    };
    for (_key, prov) in providers {
        let Some(opts) = prov.get("options").and_then(|v| v.as_object()) else {
            continue;
        };
        let api_key = opts.get("apiKey").and_then(|v| v.as_str()).unwrap_or("");
        if api_key.starts_with("eyJ") {
            if let Some(uid) = jwt::extract_user_id(api_key) {
                return uid;
            }
        }
    }
    String::new()
}

#[derive(Debug, Clone, Default)]
struct AccountIdentity {
    name: String,
    email: String,
    phone: String,
    avatar: String,
    user_id: String,
}

impl AccountIdentity {
    fn key(&self) -> Option<String> {
        identity_key(&self.email, &self.phone, &self.user_id)
    }
}

/// ZCode 6 条内置 provider，按 family 分组。
/// 每个 family 三条：无后缀（自带 API Key 入口）/ -start-plan / -coding-plan。
const BUILTIN_PROVIDERS_BY_FAMILY: &[(&str, [&str; 3])] = &[
    (
        "zai",
        [
            "builtin:zai",
            "builtin:zai-start-plan",
            "builtin:zai-coding-plan",
        ],
    ),
    (
        "bigmodel",
        [
            "builtin:bigmodel",
            "builtin:bigmodel-start-plan",
            "builtin:bigmodel-coding-plan",
        ],
    ),
];

fn all_builtin_provider_ids() -> impl Iterator<Item = &'static str> {
    BUILTIN_PROVIDERS_BY_FAMILY
        .iter()
        .flat_map(|(_, ids)| ids.iter().copied())
}

/// 从 credentials.json 字节里读出 `oauth:active_provider`，得到当前账号属于哪条 family。
/// 返回 `"zai"`/`"bigmodel"`，无法识别返回 None。
fn extract_active_provider_family(cred_bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(cred_bytes).ok()?;
    let creds: Value = serde_json::from_str(text).ok()?;
    let raw = creds
        .get("oauth:active_provider")
        .and_then(|v| v.as_str())?;
    let plain = crate::crypto::decrypt(raw).ok()?;
    let trimmed = plain.trim();
    if trimmed == "zai" || trimmed == "bigmodel" {
        Some(trimmed.to_string())
    } else {
        None
    }
}

/// 读当前 `~/.zcode/v2/config.json`，把 6 条 builtin provider 的 `options.apiKey`
/// 拍成 `provider_id -> apiKey` 的 map（空字符串也照存）。读不到返回空 map。
fn read_current_provider_api_keys() -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    let Ok(path) = config_file() else {
        return out;
    };
    let Ok(text) = fs::read_to_string(&path) else {
        return out;
    };
    let Ok(cfg) = serde_json::from_str::<Value>(&text) else {
        return out;
    };
    let Some(providers) = cfg.get("provider").and_then(|v| v.as_object()) else {
        return out;
    };
    for id in all_builtin_provider_ids() {
        let Some(prov) = providers.get(id) else {
            continue;
        };
        let api_key = prov
            .get("options")
            .and_then(|v| v.get("apiKey"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        out.insert(id.to_string(), api_key);
    }
    out
}

/// 账号元数据：决定切号时怎么写 config.json 和 setting.json。
#[derive(Debug, Clone)]
struct AccountMetadata {
    /// `"zai"` | `"bigmodel"` —— 没识别出来就是空串（兜底当 zai）。
    family: String,
    /// `"oauth"` | `"apikey"` —— 没识别出来就是空串（兜底当 oauth）。
    mode: String,
    /// 当时 config.json 里 6 条 builtin provider 的 apiKey 快照（空字符串也存）。
    provider_api_keys: std::collections::BTreeMap<String, String>,
}

/// 综合 credentials.json + 当前 config.json 判断这个账号的 family / mode / 各 provider apiKey 快照。
///
/// 优先级：
/// 1. credentials 有可解密 JWT → mode=oauth；family 取 `oauth:active_provider`（兜底 zai）
/// 2. 没 JWT，但 `builtin:zai.options.apiKey` 非空 → mode=apikey, family=zai
/// 3. 没 JWT，但 `builtin:bigmodel.options.apiKey` 非空 → mode=apikey, family=bigmodel
/// 4. 都没有 → 空（按 oauth/zai 兜底，跟老版本行为一致）
fn detect_account_metadata(cred_bytes: &[u8]) -> AccountMetadata {
    let provider_api_keys = read_current_provider_api_keys();
    let has_jwt = matches!(zcode_jwt_from_credentials(cred_bytes), Ok(Some(_)));

    let (family, mode) = if has_jwt {
        let fam = extract_active_provider_family(cred_bytes).unwrap_or_else(|| "zai".to_string());
        (fam, "oauth".to_string())
    } else if provider_api_keys
        .get("builtin:zai")
        .map(|s| !s.is_empty())
        .unwrap_or(false)
    {
        ("zai".to_string(), "apikey".to_string())
    } else if provider_api_keys
        .get("builtin:bigmodel")
        .map(|s| !s.is_empty())
        .unwrap_or(false)
    {
        ("bigmodel".to_string(), "apikey".to_string())
    } else {
        (String::new(), String::new())
    };

    AccountMetadata {
        family,
        mode,
        provider_api_keys,
    }
}

/// 从 Profile 字段（含老档案兜底）反推切号要用的 family/mode。
fn effective_family_mode(profile: &Profile) -> (String, String) {
    let family = if profile.family.is_empty() {
        "zai".to_string()
    } else {
        profile.family.clone()
    };
    let mode = if profile.mode.is_empty() {
        "oauth".to_string()
    } else {
        profile.mode.clone()
    };
    (family, mode)
}

/// 从 credentials.json 字节里解密出账号身份。
fn extract_identity_from_credentials(cred_bytes: &[u8]) -> AccountIdentity {
    let Ok(text) = std::str::from_utf8(cred_bytes) else {
        return AccountIdentity::default();
    };
    let Ok(creds) = serde_json::from_str::<Value>(text) else {
        return AccountIdentity::default();
    };
    let mut identity = match crate::crypto::extract_user_info(&creds) {
        Some(info) => {
            let phone = info
                .phone
                .or(info.phone_number)
                .or(info.mobile)
                .or(info.mobile_phone)
                .unwrap_or_default();
            AccountIdentity {
                name: info.name.unwrap_or_default(),
                email: info.email.unwrap_or_default(),
                phone,
                avatar: info.avatar.unwrap_or_default(),
                user_id: info.user_id.unwrap_or_default(),
            }
        }
        None => AccountIdentity::default(),
    };
    if identity.user_id.is_empty() {
        if let Some(token) = crate::crypto::extract_jwt_token(&creds) {
            if let Some(uid) = jwt::extract_user_id(&token) {
                identity.user_id = uid;
            }
        }
    }
    identity
}

fn decrypt_portable_credentials(value: Value) -> R<Value> {
    let Value::Object(map) = value else {
        return Err(AppError::Msg("credentials 必须是 JSON 对象".into()));
    };
    let mut out = Map::with_capacity(map.len());
    for (key, value) in map {
        let converted = match value {
            Value::String(s) if crate::crypto::is_encrypted(&s) => {
                Value::String(crate::crypto::decrypt(&s).map_err(AppError::Msg)?)
            }
            other => other,
        };
        out.insert(key, converted);
    }
    Ok(Value::Object(out))
}

fn encrypt_portable_credentials(value: Value) -> R<Vec<u8>> {
    let Value::Object(map) = value else {
        return Err(AppError::Msg("credentials 必须是 JSON 对象".into()));
    };
    let mut out = Map::with_capacity(map.len());
    for (key, value) in map {
        let converted = match value {
            Value::String(s) if crate::crypto::is_encrypted(&s) => {
                return Err(AppError::Msg(
                    "portable credentials 禁止 enc:v1: 双重加密输入".into(),
                ));
            }
            Value::String(s) => Value::String(crate::crypto::encrypt(&s).map_err(AppError::Msg)?),
            other => other,
        };
        out.insert(key, converted);
    }
    serde_json::to_vec_pretty(&Value::Object(out)).map_err(AppError::from)
}

/// 默认账号名：优先用户名(name)，其次邮箱名（@ 前部分），再其次手机号，最后短 user_id。
fn default_name(name: &str, email: &str, phone: &str, user_id: &str) -> String {
    let n = name.trim();
    if !n.is_empty() {
        return n.to_string();
    }
    if let Some(at) = email.find('@') {
        return email[..at].to_string();
    }
    if !email.is_empty() {
        return email.to_string();
    }
    if !phone.is_empty() {
        return format!("账号 {}", phone);
    }
    if !user_id.is_empty() {
        return format!("账号 {}", short_id(user_id));
    }
    format!("账号 {}", chrono::Local::now().format("%m%d-%H%M"))
}

fn normalize_email(email: &str) -> String {
    email.trim().to_ascii_lowercase()
}

fn normalize_phone(phone: &str) -> String {
    phone.chars().filter(|ch| ch.is_ascii_digit()).collect()
}

fn identity_key(email: &str, phone: &str, user_id: &str) -> Option<String> {
    let email = normalize_email(email);
    if !email.is_empty() {
        return Some(format!("email:{}", email));
    }
    let phone = normalize_phone(phone);
    if !phone.is_empty() {
        return Some(format!("phone:{}", phone));
    }
    let user_id = user_id.trim();
    if !user_id.is_empty() {
        return Some(format!("uid:{}", user_id));
    }
    None
}

fn profile_identity_key(profile: &Profile) -> Option<String> {
    identity_key(&profile.email, &profile.phone, &profile.user_id)
}

/// 原子写入：先写 .tmp 再 rename，避免写一半导致文件损坏。
/// 失败时清理 .tmp，避免残留半截（凭据类文件残留即明文泄漏面）。
fn atomic_write(path: &Path, data: &[u8]) -> R<()> {
    let mut tmp = path.to_path_buf();
    tmp.set_extension("tmp");
    let result = (|| -> R<()> {
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(data)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// 原地 truncate + 写入新内容，**不**用 tmp+rename。
///
/// 用途：让 ZCode 的 fs.watch（chokidar 类）识别成"同一个文件 modify"，
/// 等同于用户在 notepad 里 Ctrl+S 的事件。atomic rename 会换 inode，
/// 部分 watcher 实现会因此漏掉变化。
///
/// 代价：写到一半崩溃可能让 config.json 残缺。对于切号场景，这点损失换无感切换值得；
/// 真坏了再次启动 ZCode 会失败，用户重切一次就好。
fn write_in_place(path: &Path, data: &[u8]) -> R<()> {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)?;
    f.write_all(data)?;
    f.sync_all()?;
    Ok(())
}

// --------------------------------------------------------------------------- //
//  仓库操作
// --------------------------------------------------------------------------- //

/// 读取索引；文件不存在或损坏则返回空列表。
fn load_index() -> Vec<Profile> {
    let Ok(path) = profiles_index() else {
        return vec![];
    };
    let Ok(text) = fs::read_to_string(&path) else {
        return vec![];
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn save_index(profiles: &[Profile]) -> R<()> {
    let dir = profiles_dir()?;
    fs::create_dir_all(&dir)?;
    let data = serde_json::to_string_pretty(profiles)?;
    atomic_write(&profiles_index()?, data.as_bytes())
}

fn current_identity() -> Option<AccountIdentity> {
    let path = credentials_file().ok()?;
    if !path.exists() {
        return None;
    }
    let cred_bytes = fs::read(path).ok()?;
    Some(extract_identity_from_credentials(&cred_bytes))
}

fn find_profile_by_identity<'a>(
    profiles: &'a [Profile],
    identity: &AccountIdentity,
) -> Option<&'a Profile> {
    let key = identity.key()?;
    profiles
        .iter()
        .find(|p| profile_identity_key(p).as_deref() == Some(key.as_str()))
}

/// 当前 credentials.json 内容的 SHA256，用来判定"当前正在使用的账号快照"。
/// 未登录 / 文件缺失 / 读取失败返回 None。
fn current_cred_hash() -> Option<String> {
    let path = credentials_file().ok()?;
    let bytes = fs::read(path).ok()?;
    Some(sha256_bytes(&bytes))
}

/// 写盘前按身份键去重（macOS/Windows/Linux 共用同一份逻辑）。
///
/// 同一身份（email→phone→user_id）的重复档案只保留一条，优先级：
///   1. `cred_hash` 与当前 credentials.json 一致（即"当前正在使用"的快照）
///   2. `updated_at` 最大（最近使用）
///   3. `created_at` 最小（先建立的）
///
/// 背景：新写入路径本就按身份去重（capture_current / import），但磁盘上仍可能
/// 残留重复记录：旧版本迁移（去重逻辑加入之前）、手工编辑 profiles.json、
/// 同一账号一行存了邮箱另一行只有 user_id（identity_key 取值不同导致漏判）。
/// 每次写盘前统一清扫一遍，保证档案库收敛到无重复。
///
/// 注：同组重复档案的身份键必然相同，"是否匹配 live identity key"在组内要么
/// 全真要么全假，无法排序；所以"当前在用"用 cred_hash 与 live credentials.json
/// 的哈希比对来判定（切号/捕获刚写完盘时，被操作的档案必然命中第 1 优先级）。
fn dedup_before_save(profiles: &mut Vec<Profile>, live_cred_hash: Option<&str>) {
    let mut groups: std::collections::BTreeMap<String, Vec<usize>> =
        std::collections::BTreeMap::new();
    for (idx, p) in profiles.iter().enumerate() {
        if let Some(k) = profile_identity_key(p) {
            groups.entry(k).or_default().push(idx);
        }
    }

    let mut drop: Vec<usize> = groups
        .into_values()
        .filter(|group| group.len() > 1)
        .flat_map(|group| {
            let mut ranked = group;
            ranked.sort_by(|&a, &b| {
                let live_a = live_cred_hash == Some(profiles[a].cred_hash.as_str());
                let live_b = live_cred_hash == Some(profiles[b].cred_hash.as_str());
                live_b
                    .cmp(&live_a)
                    .then_with(|| {
                        profiles[b]
                            .updated_at
                            .partial_cmp(&profiles[a].updated_at)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .then_with(|| {
                        profiles[a]
                            .created_at
                            .partial_cmp(&profiles[b].created_at)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
            });
            ranked.into_iter().skip(1)
        })
        .collect();
    drop.sort_unstable();

    for idx in drop.into_iter().rev() {
        let removed = profiles.remove(idx);
        // 同身份重复档案的 id 是身份哈希，可能共享同一个 cred_file；
        // 只有当没有任何幸存档案还引用它时才删文件，避免把在用的凭据副本删掉。
        let still_referenced = profiles
            .iter()
            .any(|p| !p.cred_file.is_empty() && p.cred_file == removed.cred_file);
        if !removed.cred_file.is_empty() && !still_referenced {
            if let Ok(dir) = profiles_dir() {
                let _ = fs::remove_file(dir.join(&removed.cred_file));
            }
        }
    }
}

fn update_profile_from_capture(
    profile: &mut Profile,
    final_name: String,
    user_id: String,
    email: String,
    phone: String,
    avatar: String,
    cred_hash: String,
    cred_bytes: &[u8],
    metadata: &AccountMetadata,
) -> R<Profile> {
    if !profile.cred_file.is_empty() {
        atomic_write(&profiles_dir()?.join(&profile.cred_file), cred_bytes)?;
    }
    profile.name = final_name;
    profile.user_id = user_id;
    profile.email = email;
    profile.phone = phone;
    if !avatar.is_empty() {
        profile.avatar = avatar;
    }
    profile.cred_hash = cred_hash;
    profile.updated_at = now_ts();
    // 重新捕获也刷新登录方式 / 各 provider apiKey 快照（覆盖旧值，因为这次才是最新状态）
    profile.family = metadata.family.clone();
    profile.mode = metadata.mode.clone();
    profile.provider_api_keys = metadata.provider_api_keys.clone();
    Ok(profile.clone())
}

/// 切换前把当前 credentials.json 备份到 account-backups/。
fn backup_current(reason: &str) -> R<Option<PathBuf>> {
    let cur = credentials_file()?;
    if !cur.exists() {
        return Ok(None);
    }
    let dir = backup_dir()?;
    fs::create_dir_all(&dir)?;
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let dst = dir.join(format!("credentials.{}.{}.json", reason, ts));
    fs::copy(&cur, &dst)?;
    Ok(Some(dst))
}

/// 切换前把当前 config.json 备份到 account-backups/。
fn backup_current_config(reason: &str) -> R<Option<PathBuf>> {
    let cur = config_file()?;
    if !cur.exists() {
        return Ok(None);
    }
    let dir = backup_dir()?;
    fs::create_dir_all(&dir)?;
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let dst = dir.join(format!("config.{}.{}.json", reason, ts));
    fs::copy(&cur, &dst)?;
    Ok(Some(dst))
}

fn zcode_jwt_from_credentials(cred_bytes: &[u8]) -> R<Option<String>> {
    let creds: Value = serde_json::from_slice(cred_bytes)?;
    Ok(crate::crypto::extract_jwt_token(&creds)
        .map(|token| token.trim().to_string())
        .filter(|token| token.starts_with("eyJ")))
}

/// 切号时把 config.json 里目标 family 的 builtin provider apiKey 改成对的值。
///
/// **endpoint 关键区别**（jq 实测两条 provider 名字都叫 "Z.ai - Coding Plan"，靠 baseURL 区分）：
///   - `builtin:{family}-start-plan` → `zcode.z.ai/.../zcode-plan/anthropic`：**plan 入口**，
///     用 zcodejwttoken。无论买的是 Start 还是 Coding Plan 都走这条，等级由服务端按 token 判定。
///   - `builtin:{family}-coding-plan` → `api.z.ai/api/anthropic`：**自带 API Key 入口**，
///     塞 zcodejwttoken 不工作，还会冒出一个重复的 "Coding Plan" 条目。
///   - `builtin:{family}`（无后缀）→ 同为自带 API Key 入口。
///
/// - `mode == "oauth"`：JWT **只写 start-plan**。同时清空 coding-plan / 无后缀入口的
///   apiKey。ZCode 3.1.4 在 oauth 模式下仍会优先选择带非空 apiKey 的 coding-plan，
///   如果这里保留旧 API Key，就会切到新账号后仍走旧账号/旧额度状态。
/// - `mode == "apikey"`：从 profile 存的 `stored_keys` 里取 `builtin:{family}` 的值写回。
///
/// 其它 family 的 provider 一律不动。
fn prepare_config_provider_keys_update(
    cred_bytes: &[u8],
    family: &str,
    mode: &str,
    stored_keys: &std::collections::BTreeMap<String, String>,
) -> R<Option<(PathBuf, Vec<u8>)>> {
    let path = config_file()?;
    if !path.exists() {
        return Ok(None);
    }

    // (provider_id, 要写入的 apiKey)；另有切到 oauth 时必须清空的 API Key 入口。
    let mut target_writes: Vec<(String, String)> = Vec::new();
    let mut clear_api_key_targets: Vec<String> = Vec::new();
    let plan_start = format!("builtin:{}-start-plan", family);
    let plan_coding = format!("builtin:{}-coding-plan", family);
    let plain_key = format!("builtin:{}", family);

    if mode == "oauth" {
        let Some(jwt) = zcode_jwt_from_credentials(cred_bytes)? else {
            return Ok(None);
        };
        target_writes.push((plan_start, jwt));
        // coding-plan / 无后缀 都是 api.z.ai 入口；oauth 切号时不能保留旧 API Key，
        // 否则 ZCode 会优先走 coding-plan 而不是 start-plan。
        clear_api_key_targets.push(plan_coding);
        clear_api_key_targets.push(plain_key);
    } else if mode == "apikey" {
        let Some(apikey) = stored_keys.get(&plain_key) else {
            return Ok(None);
        };
        if apikey.is_empty() {
            return Ok(None);
        }
        target_writes.push((plain_key, apikey.clone()));
        // 同 family 的 start-plan / coding-plan 入口按捕获时的快照原样写回（空也写）：
        // start-plan 里可能残留上一个 oauth 账号的 JWT，不清会让 ZCode 继续用旧账号的
        // plan 入口，表现为"切到 apikey 账号后额度/身份还是旧的"。
        for id in [plan_start, plan_coding] {
            let Some(value) = stored_keys.get(&id) else {
                continue;
            };
            target_writes.push((id, value.clone()));
        }
    } else {
        return Ok(None);
    }

    let text = fs::read_to_string(&path)?;
    let mut cfg: Value = serde_json::from_str(&text)?;
    let Some(providers) = cfg.get_mut("provider").and_then(Value::as_object_mut) else {
        return Ok(None);
    };

    let mut changed = false;
    for (target_id, target_val) in &target_writes {
        let Some(prov) = providers.get_mut(target_id) else {
            continue;
        };
        let Some(options) = prov.get_mut("options").and_then(Value::as_object_mut) else {
            continue;
        };
        if options.get("apiKey").and_then(Value::as_str) == Some(target_val.as_str()) {
            continue;
        }
        options.insert("apiKey".to_string(), Value::String(target_val.clone()));
        changed = true;
    }
    for target_id in &clear_api_key_targets {
        let Some(prov) = providers.get_mut(target_id) else {
            continue;
        };
        let Some(options) = prov.get_mut("options").and_then(Value::as_object_mut) else {
            continue;
        };
        let has_api_key = options
            .get("apiKey")
            .and_then(Value::as_str)
            .map(|k| !k.is_empty())
            .unwrap_or(false);
        if has_api_key {
            options.insert("apiKey".to_string(), Value::String(String::new()));
            changed = true;
        }
    }

    if !changed {
        return Ok(None);
    }
    let data = serde_json::to_vec_pretty(&cfg)?;
    Ok(Some((path, data)))
}

/// oauth 切号后 `modelProviderFamilySelectedKeys[family]` 的重定向决策。
///
/// ZCode 渲染层会优先按这个值选择实际使用的 provider 入口。oauth 切号清空了
/// coding-plan / 无后缀入口的 apiKey 后，如果这里还指向它们，ZCode 就一直指着
/// 一个空 provider（实测 Win11：用户被迫手动重新粘 API Key 才能"切换成功"）。
///
/// - 缺省/空 → 指向 start-plan（显式锁定 plan 入口）
/// - 已经是 start-plan 引用 → 不动（None）
/// - `team-plan:` 前缀（BigModel 团队版特殊路由）→ 不动
/// - 同 family 的 coding-plan / 无后缀引用 → 重定向到 start-plan
/// - 其它值（其它 family / 未知格式）→ 不动
fn redirected_selected_key(current: Option<&str>, family: &str) -> Option<String> {
    let target = format!("coding-plan:builtin:{}-start-plan", family);
    let cur = current.map(str::trim).unwrap_or("");
    if cur.is_empty() {
        return Some(target);
    }
    if cur == target || cur.starts_with("team-plan:") {
        return None;
    }
    if cur == format!("coding-plan:builtin:{}", family)
        || cur == format!("coding-plan:builtin:{}-coding-plan", family)
    {
        return Some(target);
    }
    None
}

/// 按账号的 family/mode patch `setting.json`：
/// - `providerFamilyDomain = family`：让 ZCode 不禁用本 family 的 provider
/// - `modelProviderFamilyModes[family]`：ZCode 的 `m5` filter 用这个值决定走 plan 还是
///   apikey provider —— `"oauth"` 走 `builtin:<family>-start-plan`；**任何其它值或缺失**
///   都走 `builtin:<family>`（apikey 入口）
///   - oauth 模式 → 强制写入 `"oauth"`
///   - apikey 模式 → 主动**删掉**该 key（包括之前 oauth 切号时写入的"oauth"），让 ZCode
///     退回 apikey 路由
/// - `modelProviderFamilySelectedKeys[family]`：ZCode 选中入口（渲染层优先按它选 provider）
///   - oauth 模式 → 重定向到 start-plan（原指向的 coding-plan/无后缀入口刚被清空 apiKey）
///   - apikey 模式 → 删掉，交还 ZCode 默认 apikey 路由
///
/// 已经一致就跳过。setting.json 不存在 / 损坏 / 不是对象都返回 None。
fn prepare_setting_route_update(family: &str, mode: &str) -> R<Option<(PathBuf, Vec<u8>)>> {
    if family.is_empty() {
        return Ok(None);
    }
    // setting.json 在设置目录（home 基址），跟数据目录可能不同盘。
    let path = setting_file()?;
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path)?;
    let mut s: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    let Some(obj) = s.as_object_mut() else {
        return Ok(None);
    };

    let mut changed = false;

    // providerFamilyDomain = family
    if obj.get("providerFamilyDomain").and_then(Value::as_str) != Some(family) {
        obj.insert(
            "providerFamilyDomain".to_string(),
            Value::String(family.to_string()),
        );
        changed = true;
    }

    // modelProviderFamilyModes[family]
    match mode {
        "oauth" => {
            let modes = obj
                .entry("modelProviderFamilyModes".to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            if let Some(modes_obj) = modes.as_object_mut() {
                if modes_obj.get(family).and_then(Value::as_str) != Some("oauth") {
                    modes_obj.insert(family.to_string(), Value::String("oauth".into()));
                    changed = true;
                }
            }
            // oauth 切号后把选中入口重定向回 start-plan（见 redirected_selected_key 注释）
            let current = obj
                .get("modelProviderFamilySelectedKeys")
                .and_then(|v| v.get(family))
                .and_then(Value::as_str);
            if let Some(next) = redirected_selected_key(current, family) {
                let selected = obj
                    .entry("modelProviderFamilySelectedKeys".to_string())
                    .or_insert_with(|| Value::Object(Map::new()));
                if let Some(selected_obj) = selected.as_object_mut() {
                    selected_obj.insert(family.to_string(), Value::String(next));
                    changed = true;
                }
            }
        }
        "apikey" => {
            // 主动删 family 那两项，让 ZCode 退回 apikey 路由
            if let Some(modes_obj) = obj
                .get_mut("modelProviderFamilyModes")
                .and_then(Value::as_object_mut)
            {
                if modes_obj.remove(family).is_some() {
                    changed = true;
                }
            }
            if let Some(selected_obj) = obj
                .get_mut("modelProviderFamilySelectedKeys")
                .and_then(Value::as_object_mut)
            {
                if selected_obj.remove(family).is_some() {
                    changed = true;
                }
            }
        }
        _ => {}
    }

    if !changed {
        return Ok(None);
    }
    let data = serde_json::to_vec_pretty(&s)?;
    Ok(Some((path, data)))
}

// --------------------------------------------------------------------------- //
//  对外命令
// --------------------------------------------------------------------------- //

/// 列出所有档案，并标记当前活跃的那个。
#[tauri::command]
pub fn list_profiles() -> R<Vec<ProfileView>> {
    let profiles = load_index();
    let active_key = current_identity().and_then(|identity| identity.key());
    let views = profiles
        .into_iter()
        .map(|p| {
            let active = active_key
                .as_ref()
                .map(|key| profile_identity_key(&p).as_deref() == Some(key.as_str()))
                .unwrap_or(false);
            let short_id = short_id(&p.user_id);
            ProfileView {
                profile: redact_provider_keys(p),
                active,
                short_id,
            }
        })
        .collect();
    Ok(views)
}

/// 返回当前登录状态：是否已登录、当前账号名（若已识别）。
#[derive(Serialize)]
pub struct CurrentStatus {
    pub logged_in: bool,
    pub active_profile_id: Option<String>,
    pub active_profile_name: Option<String>,
    pub current_username: String,
    pub current_email: String,
    pub current_phone: String,
}

#[tauri::command]
pub fn current_status() -> R<CurrentStatus> {
    let cur = credentials_file()?;
    if !cur.exists() {
        return Ok(CurrentStatus {
            logged_in: false,
            active_profile_id: None,
            active_profile_name: None,
            current_username: String::new(),
            current_email: String::new(),
            current_phone: String::new(),
        });
    }
    let cred_bytes = fs::read(&cur).unwrap_or_default();
    let identity = extract_identity_from_credentials(&cred_bytes);
    let profiles = load_index();
    let matched = find_profile_by_identity(&profiles, &identity);
    Ok(CurrentStatus {
        logged_in: true,
        active_profile_id: matched.map(|p| p.id.clone()),
        active_profile_name: matched.map(|p| p.name.clone()),
        current_username: identity.name,
        current_email: identity.email,
        current_phone: identity.phone,
    })
}

/// 把当前 credentials.json 保存为账号档案。
/// 邮箱优先、手机号兜底作为账号唯一标识；再次保存同账号时更新已有档案。
#[tauri::command]
pub fn capture_current(name: String) -> R<Profile> {
    let cur = credentials_file()?;
    if !cur.exists() {
        return Err(AppError::Msg(
            "找不到 credentials.json，请先在 ZCode 登录。".into(),
        ));
    }
    let cred_bytes = fs::read(&cur)?;
    let cred_hash = sha256_bytes(&cred_bytes);
    let metadata = detect_account_metadata(&cred_bytes);

    let identity = extract_identity_from_credentials(&cred_bytes);
    let user_id = if !identity.user_id.is_empty() {
        identity.user_id.clone()
    } else {
        extract_user_id_from_config()
    };
    let Some(key) = identity.key() else {
        return Err(AppError::Msg(
            "未识别到当前账号邮箱、手机号或用户 ID，无法确认唯一账号。请先在 ZCode 重新登录后再保存。"
                .into(),
        ));
    };

    // 默认名字：若用户没自定义，优先用用户名(name)，其次邮箱名或手机号。
    let final_name = if name.trim().is_empty() {
        default_name(&identity.name, &identity.email, &identity.phone, &user_id)
    } else {
        name
    };

    let mut profiles = load_index();

    // 邮箱优先、手机号其次、用户 ID 兜底；再次保存同账号时更新凭据副本而不是新增档案。
    if let Some(p) = profiles
        .iter_mut()
        .find(|p| profile_identity_key(p).as_deref() == Some(key.as_str()))
    {
        let saved = update_profile_from_capture(
            p,
            final_name,
            user_id.clone(),
            identity.email.clone(),
            identity.phone.clone(),
            identity.avatar.clone(),
            cred_hash,
            &cred_bytes,
            &metadata,
        )?;
        // 刚捕获的档案 cred_hash 与 live credentials.json 一致，去重时必然保留。
        let live_hash = current_cred_hash();
        dedup_before_save(&mut profiles, live_hash.as_deref());
        save_index(&profiles)?;
        return Ok(redact_provider_keys(saved));
    }

    // 新建：内部 id 使用唯一身份 hash，避免依赖不稳定身份字段。
    let id = sha256_bytes(key.as_bytes())[..12].to_string();
    let cred_file_name = format!("credentials.{}.json", id);
    let dir = profiles_dir()?;
    fs::create_dir_all(&dir)?;
    atomic_write(&dir.join(&cred_file_name), &cred_bytes)?;

    let profile = Profile {
        id: id.clone(),
        name: final_name,
        user_id,
        email: identity.email,
        phone: identity.phone,
        avatar: identity.avatar,
        cred_hash,
        cred_file: cred_file_name,
        created_at: now_ts(),
        updated_at: now_ts(),
        family: metadata.family.clone(),
        mode: metadata.mode.clone(),
        provider_api_keys: metadata.provider_api_keys.clone(),
    };
    profiles.push(profile.clone());
    let live_hash = current_cred_hash();
    dedup_before_save(&mut profiles, live_hash.as_deref());
    save_index(&profiles)?;
    Ok(redact_provider_keys(profile))
}

/// 切换到指定档案：备份当前 → 写入目标凭据（原子写）。
#[tauri::command]
pub fn switch_to(id: String) -> R<Profile> {
    let mut profiles = load_index();
    let idx = profiles
        .iter()
        .position(|p| p.id == id)
        .ok_or_else(|| AppError::Msg("档案不存在".into()))?;
    let profile = profiles[idx].clone();

    // 找凭据副本
    let dir = profiles_dir()?;
    let cred_path = dir.join(&profile.cred_file);
    if !cred_path.exists() {
        return Err(AppError::Msg(
            "该档案缺少凭据副本，无法切换。请重新捕获。".into(),
        ));
    }
    let cred_bytes = fs::read(&cred_path)?;
    let (family, mode) = effective_family_mode(&profile);
    let config_update = prepare_config_provider_keys_update(
        &cred_bytes,
        &family,
        &mode,
        &profile.provider_api_keys,
    )?;
    let should_refresh_zcode_balance = config_update.is_some();
    let setting_update = prepare_setting_route_update(&family, &mode).ok().flatten();

    // 备份当前
    let credential_backup = backup_current("switch")?;
    if config_update.is_some() {
        let _ = backup_current_config("switch")?;
    }
    if setting_update.is_some() {
        // 备份 setting.json（设置目录，home 基址）：用不同前缀避免覆盖 config 备份
        if let Ok(setting_path) = setting_file() {
            if setting_path.exists() {
                if let Ok(backup_dir_path) = backup_dir() {
                    let _ = fs::create_dir_all(&backup_dir_path);
                    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
                    let dst = backup_dir_path.join(format!("setting.switch.{}.json", ts));
                    let _ = fs::copy(&setting_path, &dst);
                }
            }
        }
    }

    // 原子写入 credentials.json
    let active_credentials = credentials_file()?;
    atomic_write(&active_credentials, &cred_bytes)?;
    if let Some((config_path, config_bytes)) = config_update {
        // 不用 atomic rename：直接原地 truncate + write，让 ZCode 的 fs.watch 把它识别成
        // "同一个文件 modify"（等同于 notepad Ctrl+S），从而重读 config.json 并重建聊天 session。
        // atomic rename 会换 inode，部分 watcher 会漏掉变化。
        if let Err(e) = write_in_place(&config_path, &config_bytes) {
            if let Some(backup) = credential_backup {
                let _ = fs::copy(backup, &active_credentials);
            }
            return Err(e);
        }
    }
    // 锁定 modelProviderFamilyModes.zai = "oauth" 与 providerFamilyDomain = "zai"，
    // 否则跨用户导入后 ZCode 可能路由到 builtin:zai（自带 API Key 入口）报 missing API key。
    if let Some((setting_path, setting_bytes)) = setting_update {
        let _ = write_in_place(&setting_path, &setting_bytes);
    }

    // 删掉 coding-plan 权益缓存：它记录的是上一个账号买的是 Start Plan 还是 Coding Plan，
    // 切号后若不清，ZCode 会按旧账号的 entitlement 显示/启用 provider（典型：切到 Start Plan
    // 账号却仍显示 Coding Plan）。删除后 ZCode 会按新账号重新拉权益、显示正确的 plan。
    if mode == "oauth" {
        if let Ok(cache) = zcode_v2_dir().map(|d| d.join("coding-plan-cache.json")) {
            let _ = fs::remove_file(&cache);
        }
    }

    if should_refresh_zcode_balance {
        // CDP 无感切号：配置、路由与旧权益缓存都处理完后，再通过 Chrome DevTools Protocol
        // 触发 ZCode 内部 provider/权益刷新，避免今日余额面板继续读切换前的快照。
        crate::zcode_cdp::schedule_post_switch_refresh();
    }

    profiles[idx].updated_at = now_ts();
    // 切号刚把该档案的凭据原子写进 credentials.json，其 cred_hash 与 live 一致，
    // 去重时必然保留；顺带清扫同身份的历史重复。
    let live_hash = current_cred_hash();
    dedup_before_save(&mut profiles, live_hash.as_deref());
    save_index(&profiles)?;
    Ok(redact_provider_keys(profile))
}

// --------------------------------------------------------------------------- //
//  帐号内套餐切换（Start Plan 入口 ↔ GLM Coding Plan 入口）
// --------------------------------------------------------------------------- //

/// 套餐入口切换结果。
#[derive(Debug, Serialize)]
pub struct PlanSwitchOutcome {
    /// 切换后选中的完整 selected key。
    pub selected_key: String,
    /// true = 已通过 ZCode 渲染层 settingService 实时生效（运行中的 ZCode
    /// 立即路由到新入口）；false = 只写了 setting.json，需重启 ZCode 生效。
    pub applied_live: bool,
}

/// 校验目标入口并构造 selected key。target 只允许两个套餐入口。
pub fn plan_selected_key(target: &str) -> Result<(String, String), String> {
    plan_selected_key_for_family(&current_provider_family(), target)
}

/// 纯函数版：指定 family 构造 selected key（便于单测）。
fn plan_selected_key_for_family(family: &str, target: &str) -> Result<(String, String), String> {
    let normalized = target.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "start-plan" | "coding-plan" => Ok((
            format!("coding-plan:builtin:{}-{}", family, normalized),
            family.to_string(),
        )),
        other => Err(format!(
            "未知套餐入口：{}（可选 start-plan / coding-plan）",
            other
        )),
    }
}

/// 当前登录帐号所属的 provider family（"zai" / "bigmodel"）。
/// credentials 不可读或未识别时兜底 "bigmodel"（主流程与工具生态都在这条线）。
fn current_provider_family() -> String {
    let cred_bytes = credentials_file().ok().and_then(|path| fs::read(path).ok());
    cred_bytes
        .and_then(|bytes| extract_active_provider_family(&bytes))
        .unwrap_or_else(|| "bigmodel".to_string())
}

/// 把 selected key 合并进 setting.json 文本，返回更新后的完整 JSON 字节。
/// 纯函数（不碰文件系统），便于单测。返回 None 表示无需改动。
fn merge_selected_provider_key(
    setting_text: &str,
    family: &str,
    selected_key: &str,
) -> Option<Vec<u8>> {
    let mut value: Value = match serde_json::from_str(setting_text) {
        Ok(v) => v,
        Err(_) => return None,
    };
    let obj = value.as_object_mut()?;
    let selected = obj
        .entry("modelProviderFamilySelectedKeys".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let selected_obj = selected.as_object_mut()?;
    if selected_obj.get(family).and_then(Value::as_str) == Some(selected_key) {
        return None;
    }
    selected_obj.insert(family.to_string(), Value::String(selected_key.to_string()));
    serde_json::to_vec_pretty(&value).ok()
}

/// 读 config.json 里指定 provider 的 apiKey（空/缺失返回空串）。
fn read_provider_api_key(provider_id: &str) -> String {
    let Ok(path) = config_file() else {
        return String::new();
    };
    let Ok(text) = fs::read_to_string(path) else {
        return String::new();
    };
    let Ok(cfg) = serde_json::from_str::<Value>(&text) else {
        return String::new();
    };
    cfg.get("provider")
        .and_then(|p| p.get(provider_id))
        .and_then(|p| p.get("options"))
        .and_then(|o| o.get("apiKey"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

/// oauth 帐号切到 start-plan 入口时，若该入口没有 apiKey，把当前 credentials
/// 里的 zcodejwttoken 写进去（与切号流程 prepare_config_provider_keys_update 同语义）。
fn ensure_start_plan_entry_credentials(family: &str) -> R<()> {
    let entry_id = format!("builtin:{}-start-plan", family);
    if !read_provider_api_key(&entry_id).is_empty() {
        return Ok(());
    }
    let cred_bytes = fs::read(credentials_file()?)?;
    let Some(jwt) = zcode_jwt_from_credentials(&cred_bytes)? else {
        return Err(AppError::Msg(
            "start-plan 入口缺少凭据，且 credentials.json 里没有可用的 zcodejwttoken".into(),
        ));
    };
    let path = config_file()?;
    let text = fs::read_to_string(&path)?;
    let mut cfg: Value = serde_json::from_str(&text)
        .map_err(|e| AppError::Msg(format!("config.json 解析失败：{}", e)))?;
    let Some(api_key) = cfg
        .get_mut("provider")
        .and_then(|p| p.get_mut(&entry_id))
        .and_then(|p| p.get_mut("options"))
        .and_then(|o| o.get_mut("apiKey"))
    else {
        return Err(AppError::Msg(format!(
            "config.json 里没有 {} 供应者入口",
            entry_id
        )));
    };
    if let Some(slot) = api_key.as_str() {
        if slot.trim().is_empty() {
            *api_key = Value::String(jwt);
            let bytes = serde_json::to_vec_pretty(&cfg)?;
            // 与切号一致：原地写让 ZCode 的文件监听识别为同文件修改。
            write_in_place(&path, &bytes)?;
        }
    }
    Ok(())
}

/// 帐号内套餐切换：把 ZCode 的 `modelProviderFamilySelectedKeys[family]`
/// 指向另一个套餐入口（start-plan / coding-plan）。
///
/// 优先走 CDP（渲染层 settingService.update，运行中的 ZCode 立即生效——
/// 实测 ZCode 不监听 setting.json 外部修改，直接改文件只对下次启动有效）；
/// CDP 不可用时退回直接写 setting.json，返回 applied_live=false。
pub async fn switch_plan_internal(target: &str) -> Result<PlanSwitchOutcome, String> {
    let (selected_key, family) = plan_selected_key(target)?;

    if target == "coding-plan" {
        // coding-plan 入口走 open.bigmodel.cn 的 API Key 认证；没有 key 时
        // 切过去只会让所有请求 401，必须拒绝。
        let entry_id = format!("builtin:{}-coding-plan", family);
        if read_provider_api_key(&entry_id).is_empty() {
            return Err(format!(
                "{} 入口没有 API Key（请先在 ZCode 中登录/领取 Coding Plan 后重试）",
                entry_id
            ));
        }
    } else {
        ensure_start_plan_entry_credentials(&family).map_err(|e| e.to_string())?;
    }

    // 1. 实时路径：CDP settingService.update（内存 + 磁盘同时更新）。
    match crate::zcode_cdp::try_update_selected_provider(&selected_key).await {
        Ok(new_key) => {
            // 让 ZCode 顺带刷新 Coding Plan 入口的 key 缓存与权益状态。
            crate::zcode_cdp::schedule_post_switch_refresh();
            return Ok(PlanSwitchOutcome {
                selected_key: new_key,
                applied_live: true,
            });
        }
        Err(e) => {
            eprintln!("[plan-switch] CDP 实时切换不可用，退回文件写入：{}", e);
        }
    }

    // 2. 兜底路径：直接 patch setting.json（下次启动 ZCode 生效）。
    let setting_path = setting_file().map_err(|e| e.to_string())?;
    let text = if setting_path.exists() {
        fs::read_to_string(&setting_path).map_err(|e| e.to_string())?
    } else {
        "{}".to_string()
    };
    let bytes = merge_selected_provider_key(&text, &family, &selected_key)
        .ok_or_else(|| format!("selected key 已是 {}", selected_key))?;
    if setting_path.exists() {
        // 与切号流程一致：先留一份带时间戳的备份。
        if let Ok(backup_dir_path) = backup_dir() {
            let _ = fs::create_dir_all(&backup_dir_path);
            let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
            let dst = backup_dir_path.join(format!("setting.plan-switch.{}.json", ts));
            let _ = fs::copy(&setting_path, &dst);
        }
    } else if let Some(parent) = setting_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    write_in_place(&setting_path, &bytes).map_err(|e| e.to_string())?;
    Ok(PlanSwitchOutcome {
        selected_key,
        applied_live: false,
    })
}

/// 切换帐号内的套餐入口（前端在低额度自动切换与手动操作时调用）。
#[tauri::command]
pub async fn switch_plan(target: String) -> R<PlanSwitchOutcome> {
    switch_plan_internal(&target).await.map_err(AppError::Msg)
}

/// 重命名档案。
#[tauri::command]
pub fn rename_profile(id: String, name: String) -> R<bool> {
    let mut profiles = load_index();
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(AppError::Msg("名称不能为空".into()));
    }
    let mut found = false;
    for p in profiles.iter_mut() {
        if p.id == id {
            p.name = trimmed.to_string();
            p.updated_at = now_ts();
            found = true;
            break;
        }
    }
    if !found {
        return Ok(false);
    }
    let live_hash = current_cred_hash();
    dedup_before_save(&mut profiles, live_hash.as_deref());
    save_index(&profiles)?;
    Ok(true)
}

/// 删除档案（禁止删除当前活跃的）。
#[tauri::command]
pub fn delete_profile(id: String) -> R<bool> {
    let mut profiles = load_index();

    // 不允许删除当前正在使用的账号
    if let Some(identity) = current_identity() {
        let current_key = identity.key();
        let is_active = profiles
            .iter()
            .any(|p| p.id == id && current_key.as_deref() == profile_identity_key(p).as_deref());
        if is_active {
            return Err(AppError::Msg("不能删除当前正在使用的账号".into()));
        }
    }

    let before = profiles.len();
    // 先删凭据副本（取出文件名再 remove，避免借用冲突）
    let cred_file = profiles
        .iter()
        .find(|p| p.id == id)
        .map(|p| p.cred_file.clone());
    if let Some(cf) = cred_file {
        if !cf.is_empty() {
            let fp = profiles_dir()?.join(&cf);
            let _ = fs::remove_file(fp);
        }
    }
    profiles.retain(|p| p.id != id);
    let changed = profiles.len() != before;
    save_index(&profiles)?;
    Ok(changed)
}

#[tauri::command]
pub fn export_profile_json(id: String) -> R<String> {
    let profiles = load_index();
    let profile = profiles
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| AppError::Msg("档案不存在".into()))?;
    let portable = portable_account_from_profile(profile)?;
    serde_json::to_string_pretty(&portable).map_err(AppError::from)
}

fn portable_account_from_profile(profile: Profile) -> R<PortableAccount> {
    if profile.cred_file.is_empty() {
        return Err(AppError::Msg("该档案缺少凭据副本".into()));
    }
    let cred_path = profiles_dir()?.join(&profile.cred_file);
    if !cred_path.exists() {
        return Err(AppError::Msg("该档案缺少凭据副本，无法导出。".into()));
    }
    let cred_value: Value = serde_json::from_slice(&fs::read(&cred_path)?)?;
    Ok(PortableAccount {
        schema: "zcode-switcher-account/v1".to_string(),
        exported_at: now_ts(),
        profile: PortableProfile {
            name: profile.name,
            user_id: profile.user_id,
            email: profile.email,
            phone: profile.phone,
            avatar: profile.avatar,
        },
        credentials: decrypt_portable_credentials(cred_value)?,
        family: profile.family,
        mode: profile.mode,
        provider_api_keys: profile.provider_api_keys,
    })
}

#[tauri::command]
pub fn import_profile_json(json_text: String) -> R<Profile> {
    let portable: PortableAccount = serde_json::from_str(&json_text)?;
    if portable.schema != "zcode-switcher-account/v1" {
        return Err(AppError::Msg("不支持的账号 JSON 格式".into()));
    }
    Ok(redact_provider_keys(import_portable_account(portable)?))
}

fn import_portable_account(portable: PortableAccount) -> R<Profile> {
    let portable_family = portable.family.clone();
    let portable_mode = portable.mode.clone();
    let portable_keys = portable.provider_api_keys.clone();

    let cred_bytes = encrypt_portable_credentials(portable.credentials)?;
    let identity_check = extract_identity_from_credentials(&cred_bytes);
    let has_jwt = matches!(zcode_jwt_from_credentials(&cred_bytes), Ok(Some(_)));
    if identity_check.key().is_none() && !has_jwt {
        return Err(AppError::Msg(
            "credentials 缺少必需的账号身份字段，未导入".into(),
        ));
    }
    let cred_hash = sha256_bytes(&cred_bytes);
    let identity_from_creds = extract_identity_from_credentials(&cred_bytes);

    let email = if !identity_from_creds.email.trim().is_empty() {
        identity_from_creds.email
    } else {
        portable.profile.email.trim().to_string()
    };
    let phone = if !identity_from_creds.phone.trim().is_empty() {
        identity_from_creds.phone
    } else {
        portable.profile.phone.trim().to_string()
    };
    let user_id = if !identity_from_creds.user_id.trim().is_empty() {
        identity_from_creds.user_id.trim().to_string()
    } else {
        portable.profile.user_id.trim().to_string()
    };
    let Some(key) = identity_key(&email, &phone, &user_id) else {
        return Err(AppError::Msg(
            "未识别到账号邮箱、手机号或用户 ID，无法确认唯一账号，未导入。".into(),
        ));
    };
    let avatar = if !portable.profile.avatar.trim().is_empty() {
        portable.profile.avatar.trim().to_string()
    } else {
        identity_from_creds.avatar
    };
    let final_name = if portable.profile.name.trim().is_empty() {
        default_name(&identity_from_creds.name, &email, &phone, &user_id)
    } else {
        portable.profile.name.trim().to_string()
    };

    // family/mode：优先用导出 JSON 自带的；缺则按 credentials 现场推断。
    // 老导出 JSON 没这些字段 → 退回基于 credentials 的推断。
    let (family, mode) = if !portable_family.is_empty() && !portable_mode.is_empty() {
        (portable_family, portable_mode)
    } else {
        let active = extract_active_provider_family(&cred_bytes).unwrap_or_default();
        let has_jwt = matches!(zcode_jwt_from_credentials(&cred_bytes), Ok(Some(_)));
        let f = if !active.is_empty() {
            active
        } else if has_jwt {
            "zai".to_string()
        } else {
            String::new()
        };
        let m = if has_jwt {
            "oauth".to_string()
        } else if !portable_keys
            .get("builtin:zai")
            .map(|s| s.is_empty())
            .unwrap_or(true)
            || !portable_keys
                .get("builtin:bigmodel")
                .map(|s| s.is_empty())
                .unwrap_or(true)
        {
            "apikey".to_string()
        } else {
            String::new()
        };
        (f, m)
    };

    let mut profiles = load_index();
    // 先清扫磁盘上已有的重复，保证合并目标唯一（去重保留当前在用/最新的档案）。
    let live_hash = current_cred_hash();
    dedup_before_save(&mut profiles, live_hash.as_deref());

    let merged: Option<Profile> = {
        let mut merged_out = None;
        if let Some(existing) = profiles
            .iter_mut()
            .find(|p| profile_identity_key(p).as_deref() == Some(key.as_str()))
        {
            // 同身份档案已存在 → 就地合并而非拒绝导入：导入方是最新快照，
            // 凭据/family/mode/apiKey 快照整体覆盖；档案 id 和现有名字保留
            //（用户可能已重命名，且 id 是身份哈希，改名会破坏凭据文件对应关系）。
            if existing.cred_file.is_empty() {
                let cred_file_name = format!("credentials.{}.json", existing.id);
                let dir = profiles_dir()?;
                fs::create_dir_all(&dir)?;
                atomic_write(&dir.join(&cred_file_name), &cred_bytes)?;
                existing.cred_file = cred_file_name;
            } else {
                atomic_write(&profiles_dir()?.join(&existing.cred_file), &cred_bytes)?;
            }
            existing.cred_hash = cred_hash.clone();
            if existing.email.is_empty() {
                existing.email = email.clone();
            }
            if existing.phone.is_empty() {
                existing.phone = phone.clone();
            }
            if existing.user_id.is_empty() {
                existing.user_id = user_id.clone();
            }
            if !avatar.is_empty() {
                existing.avatar = avatar.clone();
            }
            if !family.is_empty() {
                existing.family = family.clone();
            }
            if !mode.is_empty() {
                existing.mode = mode.clone();
            }
            existing.provider_api_keys = portable_keys.clone();
            existing.updated_at = now_ts();
            merged_out = Some(existing.clone());
        }
        merged_out
    };
    if let Some(merged) = merged {
        save_index(&profiles)?;
        return Ok(merged);
    }

    let id = sha256_bytes(key.as_bytes())[..12].to_string();
    let cred_file_name = format!("credentials.{}.json", id);
    let dir = profiles_dir()?;
    fs::create_dir_all(&dir)?;
    atomic_write(&dir.join(&cred_file_name), &cred_bytes)?;

    let profile = Profile {
        id: id.clone(),
        name: final_name,
        user_id,
        email,
        phone,
        avatar,
        cred_hash,
        cred_file: cred_file_name,
        created_at: now_ts(),
        updated_at: now_ts(),
        family,
        mode,
        provider_api_keys: portable_keys,
    };
    profiles.push(profile.clone());
    let live_hash = current_cred_hash();
    dedup_before_save(&mut profiles, live_hash.as_deref());
    save_index(&profiles)?;
    Ok(redact_provider_keys(profile))
}

#[tauri::command]
pub fn export_profiles_bundle_to_file(path: String) -> R<()> {
    let profiles = load_index();
    if profiles.is_empty() {
        return Err(AppError::Msg("没有可导出的账号".into()));
    }
    write_profiles_zip(profiles, path)
}

/// 按给定 id 列表导出账号压缩包（支持选择）。
#[tauri::command]
pub fn export_profiles_to_file(ids: Vec<String>, path: String) -> R<()> {
    if ids.is_empty() {
        return Err(AppError::Msg("未选择任何账号".into()));
    }
    let all = load_index();
    let id_set: std::collections::HashSet<&str> = ids.iter().map(String::as_str).collect();
    let selected: Vec<Profile> = all
        .into_iter()
        .filter(|p| id_set.contains(p.id.as_str()))
        .collect();
    if selected.is_empty() {
        return Err(AppError::Msg("未找到匹配的账号".into()));
    }
    write_profiles_zip(selected, path)
}

/// 把账号列表写成 zip（每个账号一个 JSON 文件）。
fn write_profiles_zip(profiles: Vec<Profile>, path: String) -> R<()> {
    // 导出内容是明文凭据：先写同目录 .tmp，成功后 rename 覆盖目标；
    // 中途失败删除 .tmp，避免目标路径残留截断/部分明文的包。
    let dest = PathBuf::from(&path);
    let mut tmp = dest.clone();
    tmp.as_mut_os_string().push(".tmp");
    let result = (|| -> R<()> {
        let file = fs::File::create(&tmp)?;
        let mut zip = ZipWriter::new(file);
        let options = FileOptions::default().compression_method(CompressionMethod::Deflated);
        for (index, profile) in profiles.into_iter().enumerate() {
            let fallback = if !profile.user_id.is_empty() {
                profile.user_id.chars().take(12).collect::<String>()
            } else if !profile.id.is_empty() {
                profile.id.chars().take(12).collect::<String>()
            } else {
                profile.cred_hash.chars().take(12).collect::<String>()
            };
            let file_name = safe_export_file_name(&profile.name, &fallback, index);
            let account = portable_account_from_profile(profile)?;
            let json = serde_json::to_string_pretty(&account)?;
            zip.start_file(file_name, options).map_err(err)?;
            zip.write_all(json.as_bytes())?;
        }
        zip.finish().map_err(err)?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    fs::rename(&tmp, dest)?;
    Ok(())
}

fn import_json_into_report(json: &str, source: &str, report: &mut BatchImportReport) {
    let Ok(value) = serde_json::from_str::<Value>(json) else {
        report.failed += 1;
        report.messages.push(format!("{}：JSON 格式无效", source));
        return;
    };
    let schema = value
        .get("schema")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let accounts = if schema == "zcode-switcher-accounts/v1" {
        match serde_json::from_value::<PortableAccountBundle>(value) {
            Ok(bundle) => bundle.accounts,
            Err(e) => {
                report.failed += 1;
                report
                    .messages
                    .push(format!("{}：账号包格式无效：{}", source, e));
                return;
            }
        }
    } else if schema == "zcode-switcher-account/v1" {
        match serde_json::from_value::<PortableAccount>(value) {
            Ok(account) => vec![account],
            Err(e) => {
                report.failed += 1;
                report
                    .messages
                    .push(format!("{}：账号格式无效：{}", source, e));
                return;
            }
        }
    } else {
        report.failed += 1;
        report
            .messages
            .push(format!("{}：不支持的账号 JSON 格式", source));
        return;
    };

    for account in accounts {
        let name = account.profile.name.clone();
        if account.schema != "zcode-switcher-account/v1" {
            report.failed += 1;
            let label = if name.trim().is_empty() {
                source
            } else {
                name.as_str()
            };
            report
                .messages
                .push(format!("{}：不支持的账号 JSON 格式", label));
            continue;
        }
        match import_portable_account(account) {
            Ok(_) => report.imported += 1,
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("已存在") || msg.contains("重复导入") {
                    report.skipped += 1;
                } else {
                    report.failed += 1;
                }
                let label = if name.trim().is_empty() {
                    source
                } else {
                    name.as_str()
                };
                report.messages.push(format!("{}：{}", label, msg));
            }
        }
    }
}

#[tauri::command]
pub fn import_profiles_from_files(paths: Vec<String>) -> R<BatchImportReport> {
    if paths.is_empty() {
        return Err(AppError::Msg("请选择要导入的 JSON 文件".into()));
    }
    let mut report = BatchImportReport {
        imported: 0,
        skipped: 0,
        failed: 0,
        messages: Vec::new(),
    };
    for path in paths {
        if path.to_lowercase().ends_with(".zip") {
            match fs::File::open(&path)
                .map_err(AppError::from)
                .and_then(|file| ZipArchive::new(file).map_err(err))
            {
                Ok(mut archive) => {
                    for i in 0..archive.len() {
                        match archive.by_index(i).map_err(err) {
                            Ok(mut file) => {
                                let name = file.name().to_string();
                                if !name.to_lowercase().ends_with(".json") {
                                    continue;
                                }
                                let mut json = String::new();
                                match file.read_to_string(&mut json) {
                                    Ok(_) => {
                                        let source = format!("{}:{}", path, name);
                                        import_json_into_report(&json, &source, &mut report);
                                    }
                                    Err(e) => {
                                        report.failed += 1;
                                        report.messages.push(format!("{}:{}：{}", path, name, e));
                                    }
                                }
                            }
                            Err(e) => {
                                report.failed += 1;
                                report.messages.push(format!("{}：{}", path, e));
                            }
                        }
                    }
                }
                Err(e) => {
                    report.failed += 1;
                    report.messages.push(format!("{}：{}", path, e));
                }
            }
        } else {
            match fs::read_to_string(&path) {
                Ok(json) => import_json_into_report(&json, &path, &mut report),
                Err(e) => {
                    report.failed += 1;
                    report.messages.push(format!("{}：{}", path, e));
                }
            }
        }
    }
    Ok(report)
}

#[tauri::command]
pub fn export_profile_to_file(id: String, path: String) -> R<()> {
    let json = export_profile_json(id)?;
    // 导出文件内容是明文凭据：原子写避免中途失败在目标路径残留半截明文。
    atomic_write(Path::new(&path), json.as_bytes())?;
    Ok(())
}

#[tauri::command]
pub fn import_profile_from_file(path: String) -> R<Profile> {
    let json = fs::read_to_string(path)?;
    // import_profile_json 内部已完成 IPC 脱敏。
    import_profile_json(json)
}

pub fn profile_credentials_text(id: &str) -> R<String> {
    let profiles = load_index();
    let p = profiles
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| AppError::Msg("档案不存在".into()))?;
    if p.cred_file.is_empty() {
        return Err(AppError::Msg("该档案缺少凭据副本".into()));
    }
    let bytes = fs::read(profiles_dir()?.join(&p.cred_file))?;
    String::from_utf8(bytes).map_err(|e| AppError::Msg(format!("credentials 不是 UTF-8：{}", e)))
}

/// 在系统文件管理器里打开 ZCode 配置目录。
#[tauri::command]
pub fn open_config_dir() -> R<()> {
    let dir = zcode_v2_dir()?;
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        Command::new("explorer")
            .arg(dir.as_os_str())
            .spawn()
            .map_err(err)?;
    }
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        Command::new("open")
            .arg(dir.as_os_str())
            .spawn()
            .map_err(err)?;
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = dir;
    }
    Ok(())
}

/// 查询某个档案的订阅额度。
/// id 为空时查询"当前登录"的账号。
#[tauri::command]
pub async fn fetch_quota(id: Option<String>) -> R<crate::quota::QuotaInfo> {
    let cred_bytes = match id {
        Some(i) if !i.is_empty() => {
            // 从档案副本读取
            profile_credentials_text(&i)?.into_bytes()
        }
        _ => {
            // 当前 credentials.json
            let path = credentials_file()?;
            if !path.exists() {
                return Err(AppError::Msg(
                    "找不到 credentials.json，请先在 ZCode 登录。".into(),
                ));
            }
            fs::read(&path)?
        }
    };
    let text = std::str::from_utf8(&cred_bytes)
        .map_err(|e| AppError::Msg(format!("credentials 不是 UTF-8：{}", e)))?;
    crate::quota::fetch_quota(text).await.map_err(AppError::Msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_selected_key_builds_entry_and_validates_target() {
        assert_eq!(
            plan_selected_key_for_family("bigmodel", "start-plan")
                .unwrap()
                .0,
            "coding-plan:builtin:bigmodel-start-plan"
        );
        assert_eq!(
            plan_selected_key_for_family("zai", " Coding-Plan ")
                .unwrap()
                .0,
            "coding-plan:builtin:zai-coding-plan"
        );
        assert!(plan_selected_key_for_family("bigmodel", "global-build").is_err());
        assert!(plan_selected_key_for_family("bigmodel", "").is_err());
    }

    #[test]
    fn merge_selected_provider_key_merges_and_is_idempotent() {
        let setting = r#"{
  "providerFamilyDomain": "bigmodel",
  "modelProviderFamilyModes": { "bigmodel": "oauth" },
  "modelProviderFamilySelectedKeys": { "bigmodel": "coding-plan:builtin:bigmodel-coding-plan" }
}"#;
        let merged = merge_selected_provider_key(
            setting,
            "bigmodel",
            "coding-plan:builtin:bigmodel-start-plan",
        )
        .expect("应产生更新");
        let value: Value = serde_json::from_slice(&merged).expect("merged json");
        assert_eq!(
            value["modelProviderFamilySelectedKeys"]["bigmodel"],
            "coding-plan:builtin:bigmodel-start-plan"
        );
        // 其它键与其它 family 原样保留
        assert_eq!(value["providerFamilyDomain"], "bigmodel");
        assert_eq!(value["modelProviderFamilyModes"]["bigmodel"], "oauth");

        // 已是目标值 → 不产生更新
        assert!(merge_selected_provider_key(
            &String::from_utf8(merged).unwrap(),
            "bigmodel",
            "coding-plan:builtin:bigmodel-start-plan"
        )
        .is_none());

        // zai family 与 bigmodel 互不影响
        let merged_zai = merge_selected_provider_key(
            setting,
            "zai",
            "coding-plan:builtin:zai-start-plan",
        )
        .expect("zai 应产生更新");
        let value_zai: Value = serde_json::from_slice(&merged_zai).unwrap();
        assert_eq!(
            value_zai["modelProviderFamilySelectedKeys"]["bigmodel"],
            "coding-plan:builtin:bigmodel-coding-plan",
            "bigmodel 不应被改动"
        );
        assert_eq!(
            value_zai["modelProviderFamilySelectedKeys"]["zai"],
            "coding-plan:builtin:zai-start-plan"
        );

        // 损坏的 JSON → 不写入（宁可不切，也不能覆盖用户配置）
        assert!(merge_selected_provider_key("not json", "bigmodel", "x").is_none());
    }

    #[test]
    fn identity_key_prefers_email_then_phone_then_user_id() {
        assert_eq!(
            identity_key("Ada@Example.com", "13800138000", "u1").as_deref(),
            Some("email:ada@example.com")
        );
        assert_eq!(
            identity_key("", "+86 138-0013-8000", "u1").as_deref(),
            Some("phone:8613800138000")
        );
        assert_eq!(
            identity_key("", "", "bm-user-001").as_deref(),
            Some("uid:bm-user-001")
        );
        assert_eq!(identity_key("  ", "", "  "), None);
    }

    #[test]
    fn portable_credentials_roundtrip_reencrypts_only_strings() {
        let plain = serde_json::json!({
            "oauth:active_provider": "bigmodel",
            "tokens": "{\"access\":\"plain-secret\"}",
            "nested": { "child": "value" },
            "count": 3,
            "flag": true,
            "empty": ""
        });

        let encrypted = encrypt_portable_credentials(plain.clone()).expect("encrypt portable");
        let stored: Value = serde_json::from_slice(&encrypted).expect("encrypted json");

        for key in ["oauth:active_provider", "tokens"] {
            let value = stored[key].as_str().expect("string field");
            assert!(
                crate::crypto::is_encrypted(value),
                "{key} 应为 enc:v1: 密文"
            );
        }
        assert_eq!(stored["count"], serde_json::json!(3));
        assert_eq!(stored["flag"], serde_json::json!(true));
        assert_eq!(stored["nested"]["child"], serde_json::json!("value"));

        let decrypted = decrypt_portable_credentials(stored).expect("decrypt portable");
        assert_eq!(decrypted, plain, "往返后应还原为明文结构");
    }

    #[test]
    fn portable_encrypt_rejects_already_encrypted_input() {
        let cipher = crate::crypto::encrypt("plain-secret").expect("encrypt");
        let bad = serde_json::json!({ "tokens": cipher });
        let err = encrypt_portable_credentials(bad).expect_err("enc:v1: 输入必须被拒绝");
        assert!(format!("{err}").contains("enc:v1:"));
    }

    #[test]
    fn portable_credentials_reject_non_object_input() {
        assert!(encrypt_portable_credentials(serde_json::json!(["x"])).is_err());
        assert!(decrypt_portable_credentials(serde_json::json!("str")).is_err());
    }

    #[test]
    fn redact_provider_keys_clears_values_keeps_other_fields() {
        let profile = Profile {
            id: "abc123".into(),
            name: "tester".into(),
            user_id: "u-1".into(),
            email: "t@example.com".into(),
            phone: String::new(),
            avatar: String::new(),
            cred_hash: "hash".into(),
            cred_file: "credentials.abc123.json".into(),
            created_at: 1.0,
            updated_at: 2.0,
            family: "bigmodel".into(),
            mode: "oauth".into(),
            provider_api_keys: [
                ("builtin:bigmodel".to_string(), "sk-secret".to_string()),
                (
                    "builtin:bigmodel-start-plan".to_string(),
                    "jwt-secret".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        };
        let redacted = redact_provider_keys(profile.clone());
        assert!(
            redacted.provider_api_keys.values().all(|v| v.is_empty()),
            "IPC 返回的 key 值必须全为空"
        );
        assert_eq!(
            redacted.provider_api_keys.len(),
            profile.provider_api_keys.len(),
            "键名集合保留，前端/诊断仍能判断哪些入口有 key"
        );
        assert_eq!(redacted.name, profile.name);
        assert_eq!(redacted.cred_file, profile.cred_file);
        // 原对象不受影响（磁盘索引仍保存完整值）。
        assert!(profile.provider_api_keys.values().all(|v| !v.is_empty()));
    }

    #[test]
    fn atomic_write_roundtrip_replaces_and_leaves_no_tmp() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("zcs-atomic-test-{}.json", std::process::id()));
        let _ = fs::remove_file(&path);
        atomic_write(&path, b"first").expect("first write");
        assert_eq!(fs::read(&path).unwrap(), b"first");
        atomic_write(&path, b"second").expect("replace write");
        assert_eq!(fs::read(&path).unwrap(), b"second");

        let mut tmp = path.clone();
        tmp.set_extension("tmp");
        assert!(!path.with_extension("tmp").exists());
        assert!(!tmp.exists());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn atomic_write_cleans_tmp_when_target_dir_missing() {
        let dir = std::env::temp_dir().join(format!("zcs-atomic-missing-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("out.json");
        assert!(atomic_write(&path, b"data").is_err());
        assert!(!path.exists());
        assert!(!dir.exists(), "失败的写入不应在目标目录残留 .tmp");
    }

    #[test]
    fn identify_save_restore_local_zcode() {
        let status = current_status().expect("current_status");
        assert!(status.logged_in, "local ZCode credentials.json missing");
        assert!(
            !status.current_username.is_empty()
                || !status.current_email.is_empty()
                || !status.current_phone.is_empty(),
            "failed to identify logged-in ZCode user"
        );

        let saved = capture_current(String::new()).expect("capture_current");
        assert!(!saved.name.is_empty(), "saved profile should have a name");
        assert!(
            !saved.user_id.is_empty() || !saved.email.is_empty() || !saved.phone.is_empty(),
            "saved profile missing identity fields"
        );

        let after_save = current_status().expect("status after save");
        assert_eq!(
            after_save.active_profile_id.as_deref(),
            Some(saved.id.as_str())
        );

        let restored = switch_to(saved.id.clone()).expect("switch_to");
        assert_eq!(restored.id, saved.id);

        let after_restore = current_status().expect("status after restore");
        assert_eq!(
            after_restore.active_profile_id.as_deref(),
            Some(saved.id.as_str())
        );
        assert_eq!(after_restore.current_username, status.current_username);
    }
}

#[cfg(test)]
mod selected_key_tests {
    use super::redirected_selected_key;

    const START: &str = "coding-plan:builtin:bigmodel-start-plan";

    #[test]
    fn missing_or_empty_selects_start_plan() {
        assert_eq!(
            redirected_selected_key(None, "bigmodel").as_deref(),
            Some(START)
        );
        assert_eq!(
            redirected_selected_key(Some(""), "bigmodel").as_deref(),
            Some(START)
        );
        assert_eq!(
            redirected_selected_key(Some("  "), "bigmodel").as_deref(),
            Some(START)
        );
    }

    #[test]
    fn same_family_api_entries_are_redirected_to_start_plan() {
        assert_eq!(
            redirected_selected_key(Some("coding-plan:builtin:bigmodel-coding-plan"), "bigmodel")
                .as_deref(),
            Some(START)
        );
        assert_eq!(
            redirected_selected_key(Some("coding-plan:builtin:bigmodel"), "bigmodel").as_deref(),
            Some(START)
        );
    }

    #[test]
    fn start_plan_team_plan_and_other_families_are_untouched() {
        assert_eq!(redirected_selected_key(Some(START), "bigmodel"), None);
        assert_eq!(
            redirected_selected_key(
                Some("team-plan:builtin:bigmodel-coding-plan:xyz"),
                "bigmodel"
            ),
            None
        );
        assert_eq!(
            redirected_selected_key(Some("coding-plan:builtin:zai-coding-plan"), "bigmodel"),
            None
        );
    }
}
