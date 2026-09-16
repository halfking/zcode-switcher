mod captcha;
mod crypto;
mod custom_provider;
mod jwt;
mod oauth;
mod oauth_cli;
mod profile;
mod proxy;
mod proxy_pool;
mod quota;
mod restart;
mod tray;
mod zcode_cdp;
mod zcode_launcher;

use reqwest::header::{
    HeaderMap, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE,
};
use reqwest::StatusCode;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::Manager;
use tauri_plugin_deep_link::DeepLinkExt;

#[derive(Serialize)]
struct EnableRemoteDebugResult {
    modified: usize,
    already: usize,
    total: usize,
}

#[derive(Serialize)]
struct DownloadToDirectoryResult {
    path: String,
    filename: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadInfo {
    filename: String,
    content_type: Option<String>,
    content_length: Option<u64>,
}

#[tauri::command]
fn zcode_launcher_scan() -> Result<Vec<zcode_launcher::ShortcutInfo>, String> {
    zcode_launcher::scan_zcode_shortcuts().map_err(|e| e.to_string())
}

#[tauri::command]
fn zcode_launcher_enable() -> Result<EnableRemoteDebugResult, String> {
    zcode_launcher::enable_remote_debug()
        .map(|(modified, already, total)| EnableRemoteDebugResult {
            modified,
            already,
            total,
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn zcode_launcher_disable() -> Result<usize, String> {
    zcode_launcher::disable_remote_debug().map_err(|e| e.to_string())
}

#[tauri::command]
async fn download_url_to_file(url: String, path: String) -> Result<(), String> {
    let parsed = parse_download_url(&url)?;
    let response = reqwest::get(parsed)
        .await
        .map_err(|e| format!("下载失败：{}", e))?;
    if !response.status().is_success() {
        return Err(format!("下载失败：HTTP {}", response.status()));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("读取下载内容失败：{}", e))?;
    std::fs::write(path, bytes).map_err(|e| format!("保存文件失败：{}", e))
}

#[tauri::command]
async fn inspect_download_url(url: String) -> Result<DownloadInfo, String> {
    let parsed = parse_download_url(&url)?;
    let client = reqwest::Client::new();
    let response = match client.head(parsed.clone()).send().await {
        Ok(response) if response.status().is_success() => response,
        _ => client
            .get(parsed.clone())
            .header(RANGE, "bytes=0-0")
            .send()
            .await
            .map_err(|e| format!("预读下载信息失败：{}", e))?,
    };
    if !(response.status().is_success() || response.status() == StatusCode::PARTIAL_CONTENT) {
        return Err(format!("预读下载信息失败：HTTP {}", response.status()));
    }

    let headers = response.headers();
    Ok(DownloadInfo {
        filename: suggested_download_filename(&parsed, headers, &[]),
        content_type: content_type_from_headers(headers),
        content_length: content_length_from_headers(headers),
    })
}

#[tauri::command]
async fn download_url_to_directory(
    url: String,
    directory: String,
) -> Result<DownloadToDirectoryResult, String> {
    let parsed = parse_download_url(&url)?;
    let metadata = std::fs::metadata(&directory).map_err(|e| format!("下载目录不可用：{}", e))?;
    if !metadata.is_dir() {
        return Err("下载目录不可用：请选择文件夹".into());
    }

    let response = reqwest::get(parsed.clone())
        .await
        .map_err(|e| format!("下载失败：{}", e))?;
    if !response.status().is_success() {
        return Err(format!("下载失败：HTTP {}", response.status()));
    }

    let headers = response.headers().clone();
    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("读取下载内容失败：{}", e))?;
    let filename = suggested_download_filename(&parsed, &headers, &bytes);
    let mut path = PathBuf::from(directory);
    path.push(&filename);
    let path = unique_download_path(path);
    std::fs::write(&path, bytes).map_err(|e| format!("保存文件失败：{}", e))?;

    Ok(DownloadToDirectoryResult {
        path: path.to_string_lossy().to_string(),
        filename,
    })
}

fn parse_download_url(url: &str) -> Result<reqwest::Url, String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("下载链接无效：{}", e))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err("仅支持 http/https 下载链接".into());
    }
    Ok(parsed)
}

fn suggested_download_filename(url: &reqwest::Url, headers: &HeaderMap, bytes: &[u8]) -> String {
    let name = filename_from_content_disposition(headers)
        .or_else(|| filename_from_url(url))
        .unwrap_or_else(default_download_filename);
    let name = sanitize_download_filename(&name);
    if Path::new(&name).extension().is_some() {
        return name;
    }
    match inferred_extension(headers, bytes) {
        Some(ext) => format!("{}.{}", name, ext),
        None => name,
    }
}

fn filename_from_content_disposition(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(CONTENT_DISPOSITION)?.to_str().ok()?;
    let mut plain = None;
    for part in value.split(';').skip(1) {
        let Some((name, raw_value)) = part.trim().split_once('=') else {
            continue;
        };
        let raw_value = strip_header_quotes(raw_value.trim());
        if name.trim().eq_ignore_ascii_case("filename*") {
            return Some(percent_decode(filename_star_value(raw_value)));
        }
        if name.trim().eq_ignore_ascii_case("filename") {
            plain = Some(percent_decode(raw_value));
        }
    }
    plain
}

fn content_type_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string())
}

fn content_length_from_headers(headers: &HeaderMap) -> Option<u64> {
    if let Some(total) = headers
        .get(CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.rsplit_once('/').map(|(_, total)| total))
        .and_then(|total| total.parse::<u64>().ok())
    {
        return Some(total);
    }
    headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
}

fn filename_from_url(url: &reqwest::Url) -> Option<String> {
    url.path_segments()?
        .filter(|segment| !segment.is_empty())
        .last()
        .map(percent_decode)
}

fn strip_header_quotes(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
}

fn filename_star_value(value: &str) -> &str {
    let mut parts = value.splitn(3, '\'');
    if parts.next().is_some() && parts.next().is_some() {
        if let Some(encoded) = parts.next() {
            return encoded;
        }
    }
    value
        .find("''")
        .map(|index| &value[index + 2..])
        .unwrap_or(value)
}

fn sanitize_download_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
            {
                '_'
            } else {
                ch
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    if cleaned.is_empty() {
        default_download_filename()
    } else {
        cleaned
    }
}

fn inferred_extension(headers: &HeaderMap, bytes: &[u8]) -> Option<&'static str> {
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().to_ascii_lowercase());
    match content_type.as_deref() {
        Some("application/zip") | Some("application/x-zip-compressed") => Some("zip"),
        Some("application/json") => Some("json"),
        Some("application/pdf") => Some("pdf"),
        Some("application/x-msdownload")
        | Some("application/vnd.microsoft.portable-executable") => Some("exe"),
        Some("text/plain") => Some("txt"),
        _ if is_zip_bytes(bytes) => Some("zip"),
        _ => None,
    }
}

fn is_zip_bytes(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(b"PK\x05\x06")
        || bytes.starts_with(b"PK\x07\x08")
}

fn unique_download_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(default_download_filename);
    let extension = path
        .extension()
        .map(|value| format!(".{}", value.to_string_lossy()))
        .unwrap_or_default();
    for index in 1..1000 {
        let candidate = parent.join(format!("{} ({}){}", stem, index, extension));
        if !candidate.exists() {
            return candidate;
        }
    }
    path
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                output.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).to_string()
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn default_download_filename() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    format!("download-{}", timestamp)
}

/// 真机端到端验证额度刷新（`zcode-switcher.exe --quota-probe [credentials 路径]`）。
/// 只打印套餐与额度明细，不打印任何 token / 密钥。返回进程退出码。
fn quota_probe_cli() -> i32 {
    let path = std::env::args()
        .nth(2)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            let home = dirs::home_dir().expect("无法确定用户目录");
            home.join(".zcode").join("v2").join("credentials.json")
        });
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("读取 {} 失败：{}", path.display(), e);
            return 1;
        }
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("tokio runtime 创建失败：{}", e);
            return 1;
        }
    };
    match runtime.block_on(quota::fetch_quota(&text)) {
        Ok(info) => {
            println!("刷新结果: 成功");
            println!("套餐: {:?}", info.plan_name);
            println!("状态: {:?}", info.plan_status);
            println!(
                "当前供应者: {}",
                info.active_provider.as_deref().unwrap_or("(未知)")
            );
            if !info.plans.is_empty() {
                println!("套餐列表:");
                for plan in &info.plans {
                    println!(
                        "  - {}{} 状态: {}",
                        plan.name,
                        if plan.is_current { " [使用中]" } else { "" },
                        plan.status.as_deref().unwrap_or("-")
                    );
                }
            }
            println!("额度条目 ({}):", info.balances.len());
            for item in &info.balances {
                let tag = match (item.period.as_deref(), item.unit_type.as_deref()) {
                    (Some(period), _) => period.to_string(),
                    (None, Some("point")) => "积分".to_string(),
                    _ => "-".to_string(),
                };
                println!(
                    "  - {:<22} 剩余 {:>10.0} / {:>10.0} ({})",
                    item.show_name,
                    item.remaining_units,
                    item.total_units,
                    tag
                );
            }
            if info.balances.is_empty() {
                eprintln!("!! balances 为空");
                return 2;
            }
            0
        }
        Err(e) => {
            eprintln!("刷新结果: 失败");
            eprintln!("错误: {}", e);
            1
        }
    }
}

/// 额度守护真机自测（`zcode-switcher.exe --guard-selftest [端口]`）。
/// 起一个独立网关实例，验证暂停标志生效链路：注入暂停 → 非流式请求被 429 拦截
/// → 流式请求以 SSE error 事件返回 → 解除暂停后恢复放行。全程不打上游、
/// 不写 ZCode 配置。返回进程退出码。
fn guard_selftest_cli() -> i32 {
    let port: u16 = std::env::args()
        .nth(2)
        .and_then(|v| v.parse().ok())
        .unwrap_or(17899);
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("tokio runtime 创建失败：{}", e);
            return 1;
        }
    };
    runtime.block_on(async {
        let mut failures = 0usize;

        // 1. 起一个独立网关（不写 ZCode 配置）
        let (port, shutdown) = match proxy::serve_on(port, "guard-selftest-key".into()).await {
            Ok(result) => result,
            Err(e) => {
                eprintln!("[1] 启动自测网关失败: {}", e);
                return 1;
            }
        };
        println!("[1] 自测网关已启动: 127.0.0.1:{}", port);
        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{}/v1/messages", port);

        // 2. 注入暂停 → 非流式请求应被 429 拒绝
        let status = proxy::quota_guard_set_for_test(true, "自测：余额低于阈值");
        println!("[2] 守护状态注入: paused={} reason={:?}", status.paused, status.reason);

        let body = serde_json::json!({
            "model": "glm-5.3",
            "messages": [{"role": "user", "content": "hi"}],
            "stream": false
        });
        match client
            .post(&url)
            .header("x-api-key", "guard-selftest-key")
            .json(&body)
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if status.as_u16() == 429
                    && text.contains("rate_limit_error")
                    && text.contains("额度守护")
                {
                    println!("[2] 暂停时非流式拦截: PASS (HTTP 429)");
                } else {
                    println!(
                        "[2] 暂停时非流式拦截: FAIL (status={} body={})",
                        status,
                        text.chars().take(200).collect::<String>()
                    );
                    failures += 1;
                }
            }
            Err(e) => {
                println!("[2] 暂停时非流式拦截: FAIL (请求异常 {})", e);
                failures += 1;
            }
        }

        // 3. 暂停时流式请求应以 SSE error 事件返回
        let body_stream = serde_json::json!({
            "model": "glm-5.3",
            "messages": [{"role": "user", "content": "hi"}],
            "stream": true
        });
        match client
            .post(&url)
            .header("x-api-key", "guard-selftest-key")
            .json(&body_stream)
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if status.as_u16() == 200
                    && text.contains("event: error")
                    && text.contains("额度守护")
                {
                    println!("[3] 暂停时流式拦截: PASS (SSE error)");
                } else {
                    println!(
                        "[3] 暂停时流式拦截: FAIL (status={} body={})",
                        status,
                        text.chars().take(200).collect::<String>()
                    );
                    failures += 1;
                }
            }
            Err(e) => {
                println!("[3] 暂停时流式拦截: FAIL (请求异常 {})", e);
                failures += 1;
            }
        }

        // 4. 解除暂停 → 请求不再被守护拦截（无上游时表现为 502/400 而非 429）
        let status = proxy::quota_guard_set_for_test(false, "");
        if !status.paused {
            println!("[4] 守护解除: PASS (paused=false)");
        } else {
            println!("[4] 守护解除: FAIL");
            failures += 1;
        }
        match client
            .post(&url)
            .header("x-api-key", "guard-selftest-key")
            .json(&body)
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status();
                if status.as_u16() != 429 {
                    println!("[5] 解除后放行: PASS (status={}，非守护拦截)", status);
                } else {
                    println!("[5] 解除后放行: FAIL (仍被 429 拦截)");
                    failures += 1;
                }
            }
            Err(e) => {
                println!("[5] 解除后放行: FAIL (请求异常 {})", e);
                failures += 1;
            }
        }

        let _ = shutdown.send(());
        println!(
            "== 守护自测完成: {} ==",
            if failures == 0 { "全部通过" } else { "存在失败" }
        );
        if failures > 0 { 1 } else { 0 }
    })
}

/// 帐号内套餐切换真机验证（`zcode-switcher.exe --plan-switch-probe <target>`）。
/// target 为 start-plan / coding-plan；执行后读回 setting.json 验证落盘值。
/// 注意：会真实切换当前帐号的套餐入口，验证后请再执行一次切回。
fn plan_switch_probe_cli() -> i32 {
    let Some(target) = std::env::args().nth(2) else {
        eprintln!("用法: zcode-switcher.exe --plan-switch-probe <start-plan|coding-plan>");
        return 2;
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("tokio runtime 创建失败：{}", e);
            return 1;
        }
    };
    match runtime.block_on(profile::switch_plan_internal(&target)) {
        Ok(outcome) => {
            println!(
                "切换结果: selected_key={} applied_live={}",
                outcome.selected_key, outcome.applied_live
            );
            // 读回 setting.json 验证磁盘值
            match profile::setting_file() {
                Ok(path) => match std::fs::read_to_string(path) {
                    Ok(text) => {
                        let family = if outcome.selected_key.contains(":builtin:zai-") {
                            "zai"
                        } else {
                            "bigmodel"
                        };
                        match serde_json::from_str::<serde_json::Value>(&text) {
                            Ok(v) => {
                                let on_disk = v
                                    .get("modelProviderFamilySelectedKeys")
                                    .and_then(|s| s.get(family))
                                    .and_then(|s| s.as_str())
                                    .unwrap_or("(缺失)");
                                if on_disk == outcome.selected_key {
                                    println!(
                                        "落盘验证: PASS (modelProviderFamilySelectedKeys.{} = {})",
                                        family, on_disk
                                    );
                                    0
                                } else {
                                    println!("落盘验证: FAIL (磁盘={} 期望={})", on_disk, outcome.selected_key);
                                    1
                                }
                            }
                            Err(e) => {
                                println!("落盘验证: FAIL (setting.json 解析失败 {})", e);
                                1
                            }
                        }
                    }
                    Err(e) => {
                        println!("落盘验证: FAIL (读取失败 {})", e);
                        1
                    }
                },
                Err(e) => {
                    println!("落盘验证: FAIL ({} )", e);
                    1
                }
            }
        }
        Err(e) => {
            eprintln!("切换失败: {}", e);
            1
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // --quota-probe / --guard-selftest / --plan-switch-probe：无窗口的真机
    // 验证模式（bin 目标自带 manifest，可在无 cargo test 环境下验证链路）。
    match std::env::args().nth(1).as_deref() {
        Some("--quota-probe") => std::process::exit(quota_probe_cli()),
        Some("--guard-selftest") => std::process::exit(guard_selftest_cli()),
        Some("--plan-switch-probe") => std::process::exit(plan_switch_probe_cli()),
        _ => {}
    }
    tauri::Builder::default()
        // Register single-instance before deep-link so callback URLs reuse the
        // existing window instead of opening a second process.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            for arg in args {
                oauth::handle_deep_link_url(&arg);
            }
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            captcha::init(app.handle().clone());
            let handle = app.handle().clone();
            handle.deep_link().on_open_url(move |event| {
                for url in event.urls() {
                    oauth::handle_deep_link_url(url.as_str());
                }
            });
            if let Err(error) = handle.deep_link().register_all() {
                eprintln!("无法注册 OAuth deep link: {error}");
            }
            if let Ok(Some(urls)) = handle.deep_link().get_current() {
                for url in urls {
                    oauth::handle_deep_link_url(url.as_str());
                }
            }
            // 点 X（或应用内关闭按钮、Cmd+Q/Alt+F4）不退出：隐藏到托盘驻留后台。
            // 托盘菜单「Show / Quit」由 tray::setup_tray 构建。
            tray::setup_tray(app.handle())?;
            if let Some(window) = app.get_webview_window("main") {
                let win = window.clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = win.hide();
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            profile::list_profiles,
            profile::current_status,
            profile::capture_current,
            profile::switch_to,
            profile::rename_profile,
            profile::delete_profile,
            profile::export_profile_json,
            profile::import_profile_json,
            profile::export_profile_to_file,
            profile::import_profile_from_file,
            profile::export_profiles_bundle_to_file,
            profile::export_profiles_to_file,
            profile::import_profiles_from_files,
            profile::open_config_dir,
            profile::fetch_quota,
            profile::switch_plan,
            custom_provider::list_custom_providers,
            custom_provider::add_custom_provider,
            custom_provider::update_custom_provider,
            custom_provider::delete_custom_provider,
            proxy::start_proxy,
            proxy::stop_proxy,
            proxy::proxy_status,
            proxy::set_quota_guard,
            proxy::quota_guard_status,
            proxy_pool::list_account_pool,
            proxy_pool::add_account_to_pool,
            proxy_pool::set_account_pool_enabled,
            proxy_pool::remove_account_from_pool,
            restart::zcode_running,
            restart::refresh_zcode_app_server,
            restart::restart_zcode,
            restart::kill_zcode_for_switch,
            zcode_launcher_scan,
            zcode_launcher_enable,
            zcode_launcher_disable,
            download_url_to_file,
            inspect_download_url,
            download_url_to_directory,
            oauth::oauth_init,
            oauth::oauth_acquire_and_import,
            oauth::oauth_cancel,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // macOS：窗口隐藏到托盘后，点 Dock 图标是用户最自然的重开手势。
            // 不处理 RunEvent::Reopen 的话，关闭窗口后就再也看不到窗口了。
            // Reopen 是 macOS 专属变体，其他平台编译不过，需条件编译。
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { has_visible_windows, .. } = event {
                eprintln!("[reopen] event fired, has_visible_windows={has_visible_windows}");
                tray::restore_main_window(app);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, &event);
        });
}
