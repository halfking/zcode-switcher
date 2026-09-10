//! 自动重启 ZCode：枚举进程 → kill → 重启。
//!
//! ZCode 是基于 Electron 的桌面应用（Windows 上进程名为 `ZCode.exe`，
//! 多进程模型：1 个主进程 + 多个辅助/GPU 渲染进程）。重启时先把所有同名
//! 进程结束掉，再用之前记录的 exe 路径重新拉起。

use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, Signal, System, UpdateKind};

use crate::profile::AppError;

/// 目标进程名（跨平台）。macOS 查找主程序路径时只认 `ZCode`，避免误把 Helper
/// 的可执行文件路径保存成下次启动目标；结束进程时仍覆盖所有 Helper。
#[cfg(target_os = "windows")]
const PROC_NAMES: &[&str] = &["ZCode.exe"];
#[cfg(target_os = "macos")]
const PROC_NAMES: &[&str] = &[
    "ZCode",
    "ZCode Helper",
    "ZCode Helper (GPU)",
    "ZCode Helper (Plugin)",
    "ZCode Helper (Renderer)",
];
#[cfg(target_os = "linux")]
const PROC_NAMES: &[&str] = &["zcode", "ZCode"];

#[cfg(target_os = "macos")]
const MAIN_PROC_NAMES: &[&str] = &["ZCode"];
#[cfg(not(target_os = "macos"))]
const MAIN_PROC_NAMES: &[&str] = PROC_NAMES;

type R<T> = std::result::Result<T, AppError>;

#[derive(Debug, Serialize)]
pub struct RefreshZcodeAppServerReport {
    pub killed: usize,
    pub recovered: bool,
    pub restarted: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RestartSettings {
    zcode_exe_path: Option<String>,
}

/// 检测 ZCode 是否正在运行，返回主 exe 路径（若有）。
#[tauri::command]
pub fn zcode_running() -> R<Option<String>> {
    let path = find_main_path();
    if let Some(ref p) = path {
        let _ = save_known_path(p);
    }
    Ok(path)
}

/// 切号前先杀 ZCode：autoRestart 路径下用它来避免"还没杀完 ZCode 已经把内存里的旧
/// credentials/config 反写回磁盘盖掉我们的"。
///
/// 行为：先记录当前正在运行的 exe 路径（kill 之后就枚举不到了），kill 全部 ZCode 进程，
/// 等 800ms。后面再调 `restart_zcode` 时它会因为枚举不到运行进程而走 `load_known_path`，
/// 加上原本就已经检测过的快捷方式，重新拉起。
#[tauri::command]
pub fn kill_zcode_for_switch() -> R<()> {
    let path = find_main_path();
    if let Some(ref p) = path {
        let _ = save_known_path(p);
    }
    kill_all_zcode();
    thread::sleep(Duration::from_millis(800));
    Ok(())
}

/// 重启 ZCode：kill 全部同名进程 → 等待 800ms → 优先按用户的快捷方式重启，回落到 exe 直拉。
/// - 找不到运行中的 ZCode：直接尝试启动（若有已知路径）。
/// - 找不到 exe 路径：返回错误。
///
/// 为什么"快捷方式优先"：用户可能通过我们的"无感切换增强"在快捷方式上加了
/// `--remote-debugging-port=9229`，直接拉 exe 会丢失这些参数（CDP 端口不开 → 下次切号回落
/// 到 ZCode 自身 ~30s 轮询）。所以重启时优先用同一份快捷方式的 target + arguments 拉起。
#[tauri::command]
pub fn restart_zcode() -> R<()> {
    #[cfg(target_os = "macos")]
    {
        let entry = crate::zcode_launcher::scan_zcode_shortcuts()?
            .into_iter()
            .next()
            .ok_or_else(|| {
                AppError::Msg("找不到 ZCode.app，请先安装或手动打开一次 ZCode 后再试。".into())
            })?;
        restart_macos_entry(&entry)
    }
    #[cfg(not(target_os = "macos"))]
    {
        // 1. 记录 exe 路径（先于 kill，否则后续枚举不到）。
        let running_path = find_main_path();
        if let Some(ref p) = running_path {
            let _ = save_known_path(p);
        }
        // 2. 先查增强启动入口。macOS 未运行时也可由已记录的 ZCode.app 路径启动。
        let preferred = crate::zcode_launcher::find_preferred_shortcut();
        let exe_path = running_path.or_else(load_known_path);
        let exe_path = exe_path.ok_or_else(|| {
            AppError::Msg(
                "找不到 ZCode 进程路径，也没有已保存的安装路径。请手动打开一次 ZCode 后再试。"
                    .into(),
            )
        })?;

        // 3. kill 全部同名进程
        kill_all_zcode();
        thread::sleep(Duration::from_millis(800));

        // 4. 重启：有快捷方式就用它的 target + args；否则回落到 exe 直拉。
        if let Some(sc) = preferred {
            let target = if sc.target.trim().is_empty() {
                exe_path.clone()
            } else {
                sc.target.clone()
            };
            return spawn_zcode_with_args(&target, &sc.arguments);
        }
        spawn_zcode(&exe_path)
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn macos_path_candidates() -> Vec<PathBuf> {
    find_main_path()
        .into_iter()
        .chain(load_known_path())
        .map(PathBuf::from)
        .collect()
}

#[cfg(target_os = "macos")]
pub(crate) fn restart_macos_entry(entry: &crate::zcode_launcher::ShortcutInfo) -> R<()> {
    let executable = PathBuf::from(&entry.target).join("Contents/MacOS/ZCode");
    if !executable.is_file() {
        return Err(AppError::Msg("ZCode 主程序不存在，未执行重启".into()));
    }
    kill_all_zcode();
    // A fixed sleep can relaunch into the old Electron singleton and silently lose the flag.
    let start = Instant::now();
    while find_main_path().is_some() {
        if start.elapsed() >= Duration::from_secs(5) {
            return Err(AppError::Msg(
                "ZCode 未能退出，请手动关闭后重新开启无感切换".into(),
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
    spawn_zcode_with_args(&entry.target, &entry.arguments)
}

/// 更激进的热刷新：只结束 ZCode 的 `app-server --stdio` 子进程。
/// 如果后台服务没有自动恢复，只返回状态给前端，不自动重启 ZCode。
#[tauri::command]
pub fn refresh_zcode_app_server() -> R<RefreshZcodeAppServerReport> {
    let killed = kill_app_server_processes();
    if killed == 0 {
        return Ok(RefreshZcodeAppServerReport {
            killed,
            recovered: false,
            restarted: false,
        });
    }

    let recovered = wait_for_app_server(Duration::from_secs(5));
    if recovered {
        return Ok(RefreshZcodeAppServerReport {
            killed,
            recovered,
            restarted: false,
        });
    }

    Ok(RefreshZcodeAppServerReport {
        killed,
        recovered: false,
        restarted: false,
    })
}

fn settings_file() -> R<PathBuf> {
    // switcher 内部设置放稳定的设置目录（home 基址），不随 ZCode dataBaseDir 变动
    Ok(crate::profile::zcode_settings_dir()?.join("zcode-switcher-settings.json"))
}

fn load_settings() -> RestartSettings {
    let Ok(path) = settings_file() else {
        return RestartSettings::default();
    };
    let Ok(text) = fs::read_to_string(path) else {
        return RestartSettings::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn save_settings(settings: &RestartSettings) -> R<()> {
    let path = settings_file()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_vec_pretty(settings)?;
    fs::write(path, data)?;
    Ok(())
}

fn save_known_path(exe_path: &str) -> R<()> {
    let mut settings = load_settings();
    settings.zcode_exe_path = Some(exe_path.to_string());
    save_settings(&settings)
}

fn load_known_path() -> Option<String> {
    let path = load_settings().zcode_exe_path?;
    if path.trim().is_empty() {
        return None;
    }
    if !std::path::Path::new(&path).exists() {
        return None;
    }
    Some(path)
}

/// 查找运行中的 ZCode 主进程 exe 路径（最先枚举到的那条，通常是主进程）。
fn find_main_path() -> Option<String> {
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::new().with_exe(UpdateKind::Always),
    );

    for (_pid, proc_) in sys.processes() {
        let name = proc_.name().to_string_lossy().to_string();
        if MAIN_PROC_NAMES
            .iter()
            .any(|n| n.eq_ignore_ascii_case(&name))
        {
            if let Some(path) = proc_.exe().and_then(|p| p.to_str()) {
                if !path.is_empty() {
                    return Some(path.to_string());
                }
            }
        }
    }
    None
}

fn is_zcode_process_name(name: &str) -> bool {
    PROC_NAMES.iter().any(|n| n.eq_ignore_ascii_case(name))
}

fn is_app_server_process(proc_: &sysinfo::Process) -> bool {
    let name = proc_.name().to_string_lossy().to_string();
    if !is_zcode_process_name(&name) {
        return false;
    }
    let cmd = proc_
        .cmd()
        .iter()
        .map(|s| s.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    cmd.iter().any(|arg| arg == "app-server")
        && cmd.iter().any(|arg| arg == "--stdio")
        && cmd.iter().any(|arg| arg.contains("zcode.cjs"))
}

fn kill_process(proc_: &sysinfo::Process) {
    // 先尝试温和终止，失败则强制 kill（kill_with 返回 None 表示信号不支持）
    if proc_.kill_with(Signal::Term).is_none() {
        let _ = proc_.kill();
    }
}

fn kill_app_server_processes() -> usize {
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::new().with_cmd(UpdateKind::Always),
    );

    let pids: Vec<_> = sys
        .processes()
        .iter()
        .filter(|(_, p)| is_app_server_process(p))
        .map(|(pid, _)| *pid)
        .collect();

    let killed = pids.len();
    for pid in pids {
        if let Some(proc_) = sys.process(pid) {
            kill_process(proc_);
        }
    }
    killed
}

fn count_app_server_processes() -> usize {
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::new().with_cmd(UpdateKind::Always),
    );

    sys.processes()
        .iter()
        .filter(|(_, p)| is_app_server_process(p))
        .count()
}

fn wait_for_app_server(timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        thread::sleep(Duration::from_millis(500));
        if count_app_server_processes() > 0 {
            return true;
        }
    }
    false
}

/// kill 所有 ZCode 相关进程。
fn kill_all_zcode() {
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::new());

    let pids: Vec<_> = sys
        .processes()
        .iter()
        .filter(|(_, p)| {
            let name = p.name().to_string_lossy().to_string();
            is_zcode_process_name(&name)
        })
        .map(|(pid, _)| *pid)
        .collect();

    for pid in pids {
        if let Some(proc_) = sys.process(pid) {
            kill_process(proc_);
        }
    }
}

/// 用给定路径拉起 ZCode（独立进程，不阻塞）。
#[cfg(not(target_os = "macos"))]
fn spawn_zcode(exe_path: &str) -> R<()> {
    spawn_zcode_with_args(exe_path, "")
}

/// 用 target + 命令行字符串拉起 ZCode（保留 --remote-debugging-port=9229 等参数）。
/// 简单按空格切分参数；ZCode 自己的参数都是 `--key=value` 风格，无引号转义需求。
fn spawn_zcode_with_args(exe_path: &str, args: &str) -> R<()> {
    #[cfg(target_os = "macos")]
    {
        let output = macos_launch_command(exe_path, args)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| AppError::Msg(format!("重启 ZCode 失败：{}", e)))?;
        check_macos_launch_output(&output)
    }

    #[cfg(not(target_os = "macos"))]
    {
        std::process::Command::new(exe_path)
            .args(args.split_whitespace())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| AppError::Msg(format!("重启 ZCode 失败：{}", e)))?;
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn macos_launch_command(exe_path: &str, args: &str) -> std::process::Command {
    let app = macos_app_bundle(exe_path).unwrap_or_else(|| PathBuf::from(exe_path));
    let mut command = std::process::Command::new("/usr/bin/open");
    command.arg("-n").arg(app);
    if !args.trim().is_empty() {
        command.arg("--args").args(args.split_whitespace());
    }
    command
}

#[cfg(target_os = "macos")]
fn check_macos_launch_output(output: &std::process::Output) -> R<()> {
    if !output.status.success() {
        return Err(AppError::Msg(format!(
            "重启 ZCode 失败（{}）：{}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn macos_app_bundle(path: &str) -> Option<PathBuf> {
    std::path::Path::new(path)
        .ancestors()
        .find(|part| part.extension().and_then(|ext| ext.to_str()) == Some("app"))
        .map(std::path::Path::to_path_buf)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    #[test]
    fn macos_launch_preserves_app_path_and_debugging_flag() {
        let command = macos_launch_command(
            "/Users/test/Custom Apps/ZCode.app/Contents/MacOS/ZCode",
            crate::zcode_launcher::REMOTE_DEBUGGING_FLAG,
        );
        assert_eq!(command.get_program(), "/usr/bin/open");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![
                "-n",
                "/Users/test/Custom Apps/ZCode.app",
                "--args",
                "--remote-debugging-port=9229",
            ]
        );
    }

    #[test]
    fn disabled_launch_has_no_debugging_arguments() {
        let command = macos_launch_command("/Applications/ZCode.app", "");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec!["-n", "/Applications/ZCode.app"]
        );
    }

    #[test]
    fn open_failure_is_reported_instead_of_spawn_success() {
        let mut output = std::process::Output {
            status: std::process::ExitStatus::from_raw(256),
            stdout: vec![],
            stderr: b"application cannot be opened".to_vec(),
        };
        assert!(check_macos_launch_output(&output)
            .unwrap_err()
            .to_string()
            .contains("application cannot be opened"));
        output.status = std::process::ExitStatus::from_raw(0);
        assert!(check_macos_launch_output(&output).is_ok());
    }
}
