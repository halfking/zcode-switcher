use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{ShortcutInfo, R, REMOTE_DEBUGGING_FLAG};
use crate::profile::AppError;

#[derive(Debug, Default, Serialize, Deserialize)]
struct LaunchSettings {
    #[serde(default)]
    app_path: Option<String>,
    #[serde(default)]
    remote_debugging: bool,
}

static UPDATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn settings_file() -> R<PathBuf> {
    Ok(crate::profile::zcode_settings_dir()?.join("zcode-switcher-macos-launcher.json"))
}

fn read_settings(path: &Path) -> R<(LaunchSettings, Option<Vec<u8>>)> {
    match fs::read(path) {
        Ok(data) => Ok((serde_json::from_slice(&data)?, Some(data))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((LaunchSettings::default(), None)),
        Err(e) => Err(e.into()),
    }
}

fn write_settings(path: &Path, data: &[u8]) -> R<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let parent = path
        .parent()
        .ok_or_else(|| AppError::Msg("无效的启动设置路径".into()))?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".launcher-{}.tmp", rand::random::<u64>()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)?;
    let result = (|| -> R<()> {
        file.write_all(data)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

fn app_bundle(candidate: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    let candidate = fs::canonicalize(candidate).ok()?;
    let app = if candidate.extension().and_then(|s| s.to_str()) == Some("app") {
        candidate.clone()
    } else {
        // Only accept the main executable, never a nested Electron Helper bundle.
        if candidate.file_name()?.to_str()? != "ZCode"
            || candidate.parent()?.file_name()? != "MacOS"
        {
            return None;
        }
        let contents = candidate.parent()?.parent()?;
        if contents.file_name()? != "Contents" {
            return None;
        }
        contents.parent()?.to_path_buf()
    };
    if app.extension().and_then(|s| s.to_str()) != Some("app") {
        return None;
    }
    let executable = app.join("Contents/MacOS/ZCode");
    let metadata = fs::metadata(executable).ok()?;
    (metadata.is_file() && metadata.permissions().mode() & 0o111 != 0).then_some(app)
}

fn entry_for(candidates: &[PathBuf], settings: &LaunchSettings) -> Option<ShortcutInfo> {
    let app = candidates
        .iter()
        .cloned()
        .chain(settings.app_path.as_ref().map(PathBuf::from))
        .find_map(|path| app_bundle(&path))?;
    Some(ShortcutInfo {
        path: app.to_string_lossy().into_owned(),
        target: app.to_string_lossy().into_owned(),
        arguments: if settings.remote_debugging {
            REMOTE_DEBUGGING_FLAG.into()
        } else {
            String::new()
        },
        has_flag: settings.remote_debugging,
    })
}

fn candidates(settings_path: &Path) -> R<Vec<PathBuf>> {
    let mut paths = crate::restart::macos_path_candidates();
    let (settings, _) = read_settings(settings_path)?;
    paths.extend(settings.app_path.map(PathBuf::from));
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join("Applications/ZCode.app"));
    }
    paths.push(PathBuf::from("/Applications/ZCode.app"));
    Ok(paths)
}

pub(super) fn scan_zcode_shortcuts() -> R<Vec<ShortcutInfo>> {
    let path = settings_file()?;
    scan_with(&candidates(&path)?, &path)
}

pub(super) fn enable_remote_debug() -> R<(usize, usize, usize)> {
    let _guard = UPDATE_LOCK
        .lock()
        .map_err(|_| AppError::Msg("启动设置锁不可用".into()))?;
    let path = settings_file()?;
    enable_with(
        &candidates(&path)?,
        &path,
        crate::restart::restart_macos_entry,
    )
}

pub(super) fn disable_remote_debug() -> R<usize> {
    let _guard = UPDATE_LOCK
        .lock()
        .map_err(|_| AppError::Msg("启动设置锁不可用".into()))?;
    disable_with(&settings_file()?)
}

fn scan_with(candidates: &[PathBuf], settings_path: &Path) -> R<Vec<ShortcutInfo>> {
    let (settings, _) = read_settings(settings_path)?;
    Ok(entry_for(candidates, &settings).into_iter().collect())
}

fn enable_with(
    candidates: &[PathBuf],
    settings_path: &Path,
    restart: impl FnOnce(&ShortcutInfo) -> R<()>,
) -> R<(usize, usize, usize)> {
    let (mut settings, previous) = read_settings(settings_path)?;
    let Some(mut entry) = entry_for(candidates, &settings) else {
        return Ok((0, 0, 0));
    };
    let already = usize::from(settings.remote_debugging);
    settings.app_path = Some(entry.target.clone());
    settings.remote_debugging = true;
    // Persist before restarting: a failed write must never stop the user's running app.
    write_settings(settings_path, &serde_json::to_vec_pretty(&settings)?)?;
    entry.arguments = REMOTE_DEBUGGING_FLAG.into();
    entry.has_flag = true;
    if let Err(error) = restart(&entry) {
        let rollback = match previous {
            Some(data) => write_settings(settings_path, &data),
            None => fs::remove_file(settings_path).map_err(AppError::from),
        };
        if let Err(rollback_error) = rollback {
            return Err(AppError::Msg(format!(
                "{}；恢复启动设置失败：{}",
                error, rollback_error
            )));
        }
        return Err(error);
    }
    Ok((1 - already, already, 1))
}

fn disable_with(settings_path: &Path) -> R<usize> {
    let (mut settings, _) = read_settings(settings_path)?;
    if !settings.remote_debugging {
        return Ok(0);
    }
    settings.remote_debugging = false;
    write_settings(settings_path, &serde_json::to_vec_pretty(&settings)?)?;
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("zcs-launcher-test-{}", rand::random::<u64>()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn app(&self, name: &str) -> PathBuf {
            let app = self.0.join(name);
            let executable = app.join("Contents/MacOS/ZCode");
            fs::create_dir_all(executable.parent().unwrap()).unwrap();
            fs::write(&executable, b"fixture, never executed").unwrap();
            fs::set_permissions(executable, fs::Permissions::from_mode(0o755)).unwrap();
            app
        }

        fn settings(&self) -> PathBuf {
            self.0.join("settings/launcher.json")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn installed_application_is_found_without_modifying_it() {
        let f = Fixture::new();
        let app = f.app("Applications/ZCode.app");
        let before = fs::read(app.join("Contents/MacOS/ZCode")).unwrap();
        let entries = scan_with(&[app.clone()], &f.settings()).unwrap();
        assert_eq!(
            entries.len(),
            1,
            "an installed app must not be reported as missing"
        );
        assert!(!entries[0].has_flag);
        assert!(entries[0].arguments.is_empty());
        assert!(!f.settings().exists());
        assert_eq!(fs::read(app.join("Contents/MacOS/ZCode")).unwrap(), before);
    }

    #[test]
    fn enable_persists_flags_and_disable_preserves_discovery() {
        let f = Fixture::new();
        let app = f.app("User Applications/ZCode.app");
        let before = fs::read(app.join("Contents/MacOS/ZCode")).unwrap();
        let result = enable_with(&[app.clone()], &f.settings(), |entry| {
            assert!(entry.has_flag);
            assert_eq!(entry.arguments, REMOTE_DEBUGGING_FLAG);
            assert_eq!(
                PathBuf::from(&entry.target),
                fs::canonicalize(&app).unwrap()
            );
            assert!(scan_with(&[], &f.settings())?[0].has_flag);
            Ok(())
        })
        .unwrap();
        assert_eq!(result, (1, 0, 1));
        assert!(scan_with(&[], &f.settings()).unwrap()[0].has_flag);
        let mut restarted = false;
        assert_eq!(
            enable_with(&[], &f.settings(), |_| {
                restarted = true;
                Ok(())
            })
            .unwrap(),
            (0, 1, 1)
        );
        assert!(
            restarted,
            "re-enable must recover a manual launch without flags"
        );
        assert_eq!(disable_with(&f.settings()).unwrap(), 1);
        let entries = scan_with(&[], &f.settings()).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].has_flag);
        assert!(entries[0].arguments.is_empty());
        assert_eq!(disable_with(&f.settings()).unwrap(), 0);
        assert_eq!(fs::read(app.join("Contents/MacOS/ZCode")).unwrap(), before);
    }

    #[test]
    fn missing_or_invalid_apps_do_not_enable_or_restart() {
        let f = Fixture::new();
        let invalid = f.0.join("ZCode.app");
        fs::create_dir_all(&invalid).unwrap();
        for candidate in [invalid, f.0.join("missing.app")] {
            assert_eq!(
                enable_with(&[candidate], &f.settings(), |_| panic!("must not restart")).unwrap(),
                (0, 0, 0)
            );
            assert!(!f.settings().exists());
        }
    }

    #[test]
    fn running_executable_beats_saved_and_default_apps() {
        let f = Fixture::new();
        let default = f.app("Applications/ZCode.app");
        let running = f.app("Custom Location/ZCode Preview.app");
        enable_with(&[default.clone()], &f.settings(), |_| Ok(())).unwrap();
        let entries = scan_with(
            &[running.join("Contents/MacOS/ZCode"), default],
            &f.settings(),
        )
        .unwrap();
        assert_eq!(
            PathBuf::from(&entries[0].target),
            fs::canonicalize(running).unwrap()
        );
    }

    #[test]
    fn stale_saved_path_falls_back_to_valid_application() {
        let f = Fixture::new();
        let old = f.app("Old/ZCode.app");
        enable_with(&[old.clone()], &f.settings(), |_| Ok(())).unwrap();
        fs::remove_dir_all(old).unwrap();
        let new = f.app("New/ZCode.app");
        let entries = scan_with(&[new.clone()], &f.settings()).unwrap();
        assert_eq!(
            PathBuf::from(&entries[0].target),
            fs::canonicalize(new).unwrap()
        );
        assert!(entries[0].has_flag);
    }

    #[test]
    fn helper_and_non_executable_paths_are_rejected() {
        let f = Fixture::new();
        let app = f.app("ZCode.app");
        let helper = app.join("Contents/Frameworks/ZCode Helper.app/Contents/MacOS/ZCode Helper");
        assert!(scan_with(&[helper], &f.settings()).unwrap().is_empty());
        fs::set_permissions(
            app.join("Contents/MacOS/ZCode"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert!(scan_with(&[app], &f.settings()).unwrap().is_empty());
    }

    #[test]
    fn failed_restart_restores_previous_settings_exactly() {
        let f = Fixture::new();
        let app = f.app("ZCode.app");
        assert!(
            enable_with(&[app.clone()], &f.settings(), |_| Err(AppError::Msg(
                "launch failed".into()
            )))
            .is_err()
        );
        assert!(!f.settings().exists());
        fs::create_dir_all(f.settings().parent().unwrap()).unwrap();
        let previous = b"{\"remote_debugging\":false,\"unknown_field\":123}";
        fs::write(f.settings(), previous).unwrap();
        assert!(enable_with(&[app], &f.settings(), |_| Err(AppError::Msg(
            "launch failed".into()
        )))
        .is_err());
        assert_eq!(fs::read(f.settings()).unwrap(), previous);
    }

    #[test]
    fn corrupt_settings_and_write_errors_are_not_reported_as_success() {
        let f = Fixture::new();
        let app = f.app("ZCode.app");
        fs::create_dir_all(f.settings().parent().unwrap()).unwrap();
        fs::write(f.settings(), b"not json").unwrap();
        assert!(scan_with(&[app.clone()], &f.settings()).is_err());
        assert!(enable_with(&[app.clone()], &f.settings(), |_| panic!(
            "must not restart"
        ))
        .is_err());
        assert!(disable_with(&f.settings()).is_err());
        let blocker = f.0.join("not-a-directory");
        fs::write(&blocker, b"blocker").unwrap();
        assert!(
            enable_with(&[app], &blocker.join("launcher.json"), |_| panic!(
                "must not restart"
            ))
            .is_err()
        );
    }
}
