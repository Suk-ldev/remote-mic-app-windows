//! 按应用自动切换映射方案。
//!
//! 绑定「前台进程 → 预设方案」，切到该应用时自动套用，离开时回到用户自己
//! 保存的配置。参考 vibe-flow 的 Smart Profiles 应用绑定。
//!
//! 关键取舍：**自动切换只改运行中的映射，不写用户的配置文件**。否则每切一次
//! 应用就把用户手改的键位覆盖掉。用户保存的那份配置始终是基线，方案只是临时
//! 覆盖层；没有任何绑定命中时回到基线。
//!
//! 本应用自己的进程不会被绑定（界面里不提供），所以用户在映射页编辑时覆盖层
//! 必然是关闭的——编辑与保存始终作用在基线上。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// 进程名 → 预设方案 id。进程名不含路径与扩展名，大小写不敏感。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppProfileBindings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub bindings: BTreeMap<String, String>,
}

/// 归一化后的进程标识：去掉路径与 .exe，转小写。
pub fn normalize_process(name: &str) -> String {
    let base = name
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(name)
        .trim()
        .to_ascii_lowercase();
    base.strip_suffix(".exe").unwrap_or(&base).to_owned()
}

impl AppProfileBindings {
    /// 去掉空条目并归一化进程名；同名条目后写的胜出。
    pub fn normalized(self) -> Self {
        let mut bindings = BTreeMap::new();
        for (process, preset) in self.bindings {
            let process = normalize_process(&process);
            let preset = preset.trim().to_owned();
            if process.is_empty() || preset.is_empty() {
                continue;
            }
            if crate::presets::build(&preset, true).is_none() {
                continue;
            }
            bindings.insert(process, preset);
        }
        Self {
            enabled: self.enabled,
            bindings,
        }
    }

    /// 该前台进程应该套用哪个方案。总开关关闭或没有绑定时返回 None。
    pub fn resolve(&self, foreground_process: &str) -> Option<&str> {
        if !self.enabled {
            return None;
        }
        self.bindings
            .get(&normalize_process(foreground_process))
            .map(String::as_str)
    }
}

/// 覆盖层状态机：记住当前生效的方案，只有目标变化时才下发。
///
/// 返回 Some(decision) 表示需要切换；None 表示维持现状（避免每次轮询都重建
/// 映射，那会不断重置手势状态）。
#[derive(Debug, Default)]
pub struct ProfileOverlay {
    active: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayChange {
    /// 套用该预设方案（临时覆盖，不写配置文件）。
    Apply(String),
    /// 回到用户保存的基线配置。
    Restore,
}

impl ProfileOverlay {
    pub fn active(&self) -> Option<&str> {
        self.active.as_deref()
    }

    pub fn evaluate(
        &mut self,
        bindings: &AppProfileBindings,
        foreground_process: &str,
    ) -> Option<OverlayChange> {
        let wanted = bindings.resolve(foreground_process).map(str::to_owned);
        if wanted == self.active {
            return None;
        }
        self.active = wanted.clone();
        Some(match wanted {
            Some(preset) => OverlayChange::Apply(preset),
            None => OverlayChange::Restore,
        })
    }

    /// 用户改了基线或绑定：强制下一次求值重新下发。
    pub fn invalidate(&mut self) {
        self.active = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bindings(pairs: &[(&str, &str)]) -> AppProfileBindings {
        AppProfileBindings {
            enabled: true,
            bindings: pairs
                .iter()
                .map(|(process, preset)| ((*process).to_owned(), (*preset).to_owned()))
                .collect(),
        }
        .normalized()
    }

    #[test]
    fn process_names_normalize_away_path_case_and_extension() {
        assert_eq!(
            normalize_process(r"C:\Program Files\Foo\Chrome.EXE"),
            "chrome"
        );
        assert_eq!(normalize_process("msedge"), "msedge");
        assert_eq!(normalize_process("  Notepad.exe  "), "notepad");
    }

    #[test]
    fn unknown_presets_are_dropped_instead_of_failing_later() {
        let table = bindings(&[("chrome", "reading"), ("notepad", "no-such-preset")]);
        assert_eq!(table.resolve("Chrome.exe"), Some("reading"));
        assert_eq!(table.resolve("notepad.exe"), None);
    }

    #[test]
    fn resolve_respects_the_master_switch() {
        let mut table = bindings(&[("chrome", "reading")]);
        assert_eq!(table.resolve("chrome.exe"), Some("reading"));
        table.enabled = false;
        assert_eq!(table.resolve("chrome.exe"), None);
    }

    #[test]
    fn overlay_only_emits_a_change_when_the_target_changes() {
        let table = bindings(&[("chrome", "reading"), ("vlc", "media")]);
        let mut overlay = ProfileOverlay::default();

        assert_eq!(
            overlay.evaluate(&table, "chrome.exe"),
            Some(OverlayChange::Apply("reading".to_owned()))
        );
        // 同一应用继续在前台：不重复下发，否则每次轮询都会重置手势状态。
        assert_eq!(overlay.evaluate(&table, "chrome.exe"), None);
        assert_eq!(overlay.active(), Some("reading"));

        assert_eq!(
            overlay.evaluate(&table, "vlc.exe"),
            Some(OverlayChange::Apply("media".to_owned()))
        );
        // 切到没有绑定的应用：回到用户保存的基线。
        assert_eq!(
            overlay.evaluate(&table, "explorer.exe"),
            Some(OverlayChange::Restore)
        );
        assert_eq!(overlay.evaluate(&table, "explorer.exe"), None);
        assert_eq!(overlay.active(), None);
    }

    #[test]
    fn invalidate_forces_the_next_evaluation_to_re_apply() {
        let table = bindings(&[("chrome", "reading")]);
        let mut overlay = ProfileOverlay::default();
        overlay.evaluate(&table, "chrome.exe");
        assert_eq!(overlay.evaluate(&table, "chrome.exe"), None);
        overlay.invalidate();
        assert_eq!(
            overlay.evaluate(&table, "chrome.exe"),
            Some(OverlayChange::Apply("reading".to_owned()))
        );
    }

    #[test]
    fn disabled_bindings_restore_the_baseline_once() {
        let mut table = bindings(&[("chrome", "reading")]);
        let mut overlay = ProfileOverlay::default();
        overlay.evaluate(&table, "chrome.exe");
        table.enabled = false;
        assert_eq!(
            overlay.evaluate(&table, "chrome.exe"),
            Some(OverlayChange::Restore)
        );
        assert_eq!(overlay.evaluate(&table, "chrome.exe"), None);
    }
}

/// 前台进程监视线程（Windows）。
///
/// 轮询而非事件钩子：`SetWinEventHook` 要一个消息泵线程，而这里只需要秒级
/// 精度——用户切窗口到按遥控器之间总有人的反应时间。轮询间隔 700ms，读不到
/// 前台进程时按"没有绑定"处理（回到基线），绝不保留上一次的覆盖层。
#[cfg(windows)]
pub mod watcher {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, RwLock};
    use std::thread::JoinHandle;
    use std::time::Duration;

    use super::{AppProfileBindings, OverlayChange, ProfileOverlay};
    use crate::button_mapping::ButtonMappingRuntime;
    use crate::send_input::ButtonMappings;

    const POLL_INTERVAL: Duration = Duration::from_millis(700);

    pub struct ProfileWatcher {
        stop: Arc<AtomicBool>,
        worker: Mutex<Option<JoinHandle<()>>>,
        active: Arc<Mutex<Option<String>>>,
    }

    impl ProfileWatcher {
        pub fn start(
            bindings: Arc<RwLock<AppProfileBindings>>,
            baseline: Arc<RwLock<ButtonMappings>>,
            engine: Arc<ButtonMappingRuntime>,
        ) -> Self {
            let stop = Arc::new(AtomicBool::new(false));
            let active: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
            let worker = {
                let stop = Arc::clone(&stop);
                let active = Arc::clone(&active);
                std::thread::Builder::new()
                    .name("sayall-app-profiles".to_owned())
                    .spawn(move || {
                        let mut overlay = ProfileOverlay::default();
                        while !stop.load(Ordering::Relaxed) {
                            let process = foreground_process_name().unwrap_or_default();
                            let table = bindings
                                .read()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .clone();
                            if let Some(change) = overlay.evaluate(&table, &process) {
                                let mappings = match &change {
                                    OverlayChange::Apply(preset) => {
                                        let enabled = baseline
                                            .read()
                                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                                            .enabled;
                                        crate::presets::build(preset, enabled)
                                    }
                                    OverlayChange::Restore => Some(
                                        baseline
                                            .read()
                                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                                            .clone(),
                                    ),
                                };
                                if let Some(mappings) = mappings {
                                    crate::ble::gatt_note(format!(
                                        "app_profile action=switch process={process} target={}",
                                        match &change {
                                            OverlayChange::Apply(preset) => preset.as_str(),
                                            OverlayChange::Restore => "baseline",
                                        }
                                    ));
                                    engine.set_mappings(mappings);
                                    *active.lock().unwrap_or_else(|p| p.into_inner()) = match change
                                    {
                                        OverlayChange::Apply(preset) => Some(preset),
                                        OverlayChange::Restore => None,
                                    };
                                }
                            }
                            std::thread::sleep(POLL_INTERVAL);
                        }
                    })
                    .ok()
            };
            Self {
                stop,
                worker: Mutex::new(worker),
                active,
            }
        }

        /// 当前生效的方案 id（界面据此提示"已被方案接管"）。
        pub fn active_profile(&self) -> Option<String> {
            self.active
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }
    }

    impl Drop for ProfileWatcher {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(worker) = self
                .worker
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
            {
                let _ = worker.join();
            }
        }
    }

    /// 前台窗口所属进程的可执行文件名。读不到返回 None。
    fn foreground_process_name() -> Option<String> {
        use windows::Win32::Foundation::{CloseHandle, HANDLE, MAX_PATH};
        use windows::Win32::System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
            PROCESS_QUERY_LIMITED_INFORMATION,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowThreadProcessId,
        };

        let window = unsafe { GetForegroundWindow() };
        if window.0.is_null() {
            return None;
        }
        let mut process_id = 0u32;
        unsafe { GetWindowThreadProcessId(window, Some(&mut process_id)) };
        if process_id == 0 {
            return None;
        }
        let handle: HANDLE =
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) }.ok()?;
        let mut buffer = [0u16; MAX_PATH as usize];
        let mut length = buffer.len() as u32;
        let queried = unsafe {
            QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_FORMAT(0),
                windows::core::PWSTR(buffer.as_mut_ptr()),
                &mut length,
            )
        };
        let _ = unsafe { CloseHandle(handle) };
        queried.ok()?;
        Some(String::from_utf16_lossy(&buffer[..length as usize]))
    }
}
