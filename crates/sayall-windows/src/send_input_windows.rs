use crate::send_input::{
    plan_key_down, plan_key_tap, plan_key_up, send_key_edges_spaced_with, send_key_tap_held_with,
    KeyChord, MouseAction, PlannedKeyEvent, SendInputError, SendInputSnapshot,
    DEFAULT_INJECTION_HOLD, HOLD_CHORD_EVENT_GAP, MAX_INJECTION_HOLD, WHEEL_NOTCHES,
};
use crate::PlatformError;
use std::mem::size_of;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
use windows::Win32::System::Shutdown::LockWorkStation;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, KEYEVENTF_UNICODE,
    MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
    MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL,
    MOUSEINPUT, MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
};

/// 一格滚轮的 mouseData 单位（Win32 WHEEL_DELTA）。
const WHEEL_DELTA: i32 = 120;

#[derive(Debug)]
pub struct SendInputRuntime {
    snapshot: Mutex<SendInputSnapshot>,
    /// 映射注入的按键保持时长（毫秒）。运行中可改，下一次注入即生效。
    hold_millis: AtomicU64,
}

impl SendInputRuntime {
    pub fn new() -> Self {
        Self {
            snapshot: Mutex::new(SendInputSnapshot {
                available: true,
                ..SendInputSnapshot::default()
            }),
            hold_millis: AtomicU64::new(DEFAULT_INJECTION_HOLD.as_millis() as u64),
        }
    }

    pub fn injection_hold(&self) -> Duration {
        Duration::from_millis(self.hold_millis.load(Ordering::Relaxed))
    }

    pub fn set_injection_hold(&self, hold: Duration) {
        let clamped = hold.min(MAX_INJECTION_HOLD);
        self.hold_millis
            .store(clamped.as_millis() as u64, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> SendInputSnapshot {
        lock(&self.snapshot).clone()
    }

    pub fn tap(&self, chord: KeyChord) -> Result<SendInputSnapshot, PlatformError> {
        if chord.is_lock_workstation() {
            let started = Instant::now();
            crate::ble::gatt_note(
                "shortcut_execute action=lock_workstation phase=requested method=win32_api"
                    .to_owned(),
            );
            let result = unsafe { LockWorkStation() }
                .map(|_| 0_usize)
                .map_err(|error| SendInputError::Backend(error.to_string()));
            crate::ble::gatt_note(match &result {
                Ok(_) => format!(
                    "shortcut_execute action=lock_workstation phase=completed terminal_result=passed api_request_started=true elapsed_ms={}",
                    started.elapsed().as_millis()
                ),
                Err(_) => format!(
                    "shortcut_execute action=lock_workstation phase=completed terminal_result=failed error_domain=win32 error_code=lock_workstation_failed reason=api_rejected retryable=true elapsed_ms={}",
                    started.elapsed().as_millis()
                ),
            });
            return self.record(result, "LockWorkStation");
        }
        // DOWN 与 UP 之间保持一段时间：零间隔的点按会被轮询键盘状态的程序
        // 整个丢掉（见 send_input::DEFAULT_INJECTION_HOLD）。
        let result = send_key_tap_held_with(&chord, self.injection_hold(), real_send_input_batch);
        self.record(result, "SendInput")
    }

    /// Submit the key-down edges of a chord (voice-key hold-to-talk press).
    /// One SendInput call per edge with HOLD_CHORD_EVENT_GAP spacing: WeType
    /// rejects zero-gap batched Ctrl+Win chords (evidence/p, 2026-09-04).
    pub fn press(&self, chord: &KeyChord) -> Result<SendInputSnapshot, PlatformError> {
        let events =
            plan_key_down(chord).map_err(|error| PlatformError::SendInput(error.to_string()))?;
        let result =
            send_key_edges_spaced_with(&events, HOLD_CHORD_EVENT_GAP, real_send_input_batch);
        self.record(result, "SendInput key-down")
    }

    /// Submit the key-up edges of a chord in reverse order (voice-key release),
    /// with the same per-event spacing as `press` for symmetric edge timing.
    pub fn release(&self, chord: &KeyChord) -> Result<SendInputSnapshot, PlatformError> {
        let events =
            plan_key_up(chord).map_err(|error| PlatformError::SendInput(error.to_string()))?;
        let result =
            send_key_edges_spaced_with(&events, HOLD_CHORD_EVENT_GAP, real_send_input_batch);
        self.record(result, "SendInput key-up")
    }

    /// 点按整个和弦（DOWN 全部 → 反序 UP 全部），逐事件提交、事件间
    /// HOLD_CHORD_EVENT_GAP 间隔——单次触发型语音工具（Typeless 等）的
    /// 开始/结束点按。
    ///
    /// 与按键映射的 [`Self::tap`]（单批零间隔）刻意不同：语音工具的热键
    /// 与微信输入法同属"注入和弦"路径，单批零间隔被实证拒绝（evidence/p，
    /// 2026-09-04），且间隔同时给出了非零的按下时长——瞬时 DOWN+UP 可能
    /// 被目标应用当作抖动丢弃。
    pub fn tap_spaced(&self, chord: &KeyChord) -> Result<SendInputSnapshot, PlatformError> {
        let events =
            plan_key_tap(chord).map_err(|error| PlatformError::SendInput(error.to_string()))?;
        let result =
            send_key_edges_spaced_with(&events, HOLD_CHORD_EVENT_GAP, real_send_input_batch);
        self.record(result, "SendInput key-tap")
    }

    /// 注入一次鼠标动作。滚轮为单个事件；按键为 DOWN+UP 一批提交。
    pub fn mouse(&self, kind: MouseAction) -> Result<SendInputSnapshot, PlatformError> {
        let inputs = build_mouse_inputs(kind);
        let result = submit_batch(&inputs);
        self.record(result, "SendInput mouse")
    }

    /// 按 Unicode 逐字符注入文本（KEYEVENTF_UNICODE，不依赖当前键盘布局）。
    /// 代理对按 UTF-16 码元逐个提交，emoji 等增补平面字符同样可发。
    pub fn text(&self, value: &str) -> Result<SendInputSnapshot, PlatformError> {
        let inputs = build_text_inputs(value);
        if inputs.is_empty() {
            return Err(PlatformError::SendInput(
                SendInputError::EmptyChord.to_string(),
            ));
        }
        let result = submit_batch(&inputs);
        self.record(result, "SendInput text")
    }

    /// 注入单个 F5 释放沿，清理可能粘在 OS 键态的 F5（2026-09-05 21:08
    /// 实证链路：断连重连场景首个遥控器 F5 D 在武装前泄漏进 OS，若其
    /// UP 沿丢失，OS 认为 F5 持续按下，后续语音和弦全部被微信输入法按
    /// "三键同按"拒绝）。配对规则保证该 UP 仅在确有泄漏时放行到 OS：
    /// 干净场景（本应用抑制器已吞下全部 F5 DOWN）下它同样被吞，无副作用。
    pub fn release_stuck_f5(&self) {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
            KEYEVENTF_KEYUP, VIRTUAL_KEY,
        };
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0x74),
                    wScan: 0,
                    dwFlags: KEYBD_EVENT_FLAGS(KEYEVENTF_KEYUP.0),
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        unsafe {
            let _ = SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
        }
    }

    fn record(
        &self,
        result: Result<usize, crate::send_input::SendInputError>,
        operation: &'static str,
    ) -> Result<SendInputSnapshot, PlatformError> {
        let mut snapshot = lock(&self.snapshot);
        match result {
            Ok(events) => {
                snapshot.submitted_batches += 1;
                snapshot.submitted_events += events as u64;
                snapshot.last_error = None;
                Ok(snapshot.clone())
            }
            Err(error) => {
                snapshot.last_error = Some(format!("{operation}：{error}"));
                Err(PlatformError::SendInput(error.to_string()))
            }
        }
    }
}

impl Default for SendInputRuntime {
    fn default() -> Self {
        Self::new()
    }
}

fn submit_batch(inputs: &[INPUT]) -> Result<usize, SendInputError> {
    let sent = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) } as usize;
    if sent == inputs.len() {
        Ok(sent)
    } else {
        Err(SendInputError::PartialDelivery {
            sent,
            expected: inputs.len(),
        })
    }
}

fn mouse_input(flags: MOUSE_EVENT_FLAGS, mouse_data: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: mouse_data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn build_mouse_inputs(kind: MouseAction) -> Vec<INPUT> {
    let delta = WHEEL_DELTA * WHEEL_NOTCHES;
    match kind {
        MouseAction::WheelUp => vec![mouse_input(MOUSEEVENTF_WHEEL, delta)],
        MouseAction::WheelDown => vec![mouse_input(MOUSEEVENTF_WHEEL, -delta)],
        // 水平滚轮正方向为向右（Win32 约定与垂直滚轮相反的直觉，别改）。
        MouseAction::WheelRight => vec![mouse_input(MOUSEEVENTF_HWHEEL, delta)],
        MouseAction::WheelLeft => vec![mouse_input(MOUSEEVENTF_HWHEEL, -delta)],
        MouseAction::LeftClick => vec![
            mouse_input(MOUSEEVENTF_LEFTDOWN, 0),
            mouse_input(MOUSEEVENTF_LEFTUP, 0),
        ],
        MouseAction::RightClick => vec![
            mouse_input(MOUSEEVENTF_RIGHTDOWN, 0),
            mouse_input(MOUSEEVENTF_RIGHTUP, 0),
        ],
        MouseAction::MiddleClick => vec![
            mouse_input(MOUSEEVENTF_MIDDLEDOWN, 0),
            mouse_input(MOUSEEVENTF_MIDDLEUP, 0),
        ],
    }
}

fn unicode_input(unit: u16, is_key_up: bool) -> INPUT {
    let mut flags = KEYEVENTF_UNICODE;
    if is_key_up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: unit,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn build_text_inputs(value: &str) -> Vec<INPUT> {
    value
        .encode_utf16()
        .flat_map(|unit| [unicode_input(unit, false), unicode_input(unit, true)])
        .collect()
}

fn real_send_input_batch(events: &[PlannedKeyEvent]) -> Result<usize, String> {
    let inputs: Vec<_> = events.iter().copied().map(build_input).collect();
    let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) } as usize;
    Ok(sent)
}

fn build_input(event: PlannedKeyEvent) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS::default();
    let (virtual_key, scan_code) =
        if let Some((scan_code, extended)) = event.key.physical_scan_code() {
            flags |= KEYEVENTF_SCANCODE;
            if extended {
                flags |= KEYEVENTF_EXTENDEDKEY;
            }
            (VIRTUAL_KEY(0), scan_code)
        } else {
            if event.key.is_extended() {
                flags |= KEYEVENTF_EXTENDEDKEY;
            }
            (VIRTUAL_KEY(event.key.virtual_key()), 0)
        };
    if event.is_key_up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: virtual_key,
                wScan: scan_code,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::send_input::KeyCode;

    #[test]
    fn wheel_actions_carry_signed_wheel_delta() {
        let up = build_mouse_inputs(MouseAction::WheelUp);
        assert_eq!(up.len(), 1);
        let mouse = unsafe { up[0].Anonymous.mi };
        assert_eq!(mouse.dwFlags, MOUSEEVENTF_WHEEL);
        assert_eq!(mouse.mouseData as i32, WHEEL_DELTA);

        let down = build_mouse_inputs(MouseAction::WheelDown);
        let mouse = unsafe { down[0].Anonymous.mi };
        assert_eq!(mouse.mouseData as i32, -WHEEL_DELTA);

        assert_eq!(build_mouse_inputs(MouseAction::LeftClick).len(), 2);
    }

    #[test]
    fn text_is_submitted_as_utf16_unicode_pairs() {
        // "a" 一个码元、emoji 两个码元（代理对），各自 DOWN+UP。
        assert_eq!(build_text_inputs("a").len(), 2);
        let inputs = build_text_inputs("🙂");
        assert_eq!(inputs.len(), 4);
        let first = unsafe { inputs[0].Anonymous.ki };
        assert_eq!(first.wVk.0, 0);
        assert!(first.dwFlags.contains(KEYEVENTF_UNICODE));
        assert!(!first.dwFlags.contains(KEYEVENTF_KEYUP));
        let second = unsafe { inputs[1].Anonymous.ki };
        assert!(second.dwFlags.contains(KEYEVENTF_KEYUP));
        assert_eq!(first.wScan, second.wScan);
    }

    #[test]
    fn right_control_uses_extended_physical_scan_code() {
        let input = build_input(PlannedKeyEvent {
            key: KeyCode::RightControl,
            is_key_up: true,
        });
        let keyboard = unsafe { input.Anonymous.ki };
        assert_eq!(keyboard.wVk.0, 0);
        assert_eq!(keyboard.wScan, 0x1D);
        assert!(keyboard.dwFlags.contains(KEYEVENTF_SCANCODE));
        assert!(keyboard.dwFlags.contains(KEYEVENTF_EXTENDEDKEY));
        assert!(keyboard.dwFlags.contains(KEYEVENTF_KEYUP));
    }

    #[test]
    fn right_alt_down_uses_extended_scan_code_without_key_up_flag() {
        let input = build_input(PlannedKeyEvent {
            key: KeyCode::RightAlt,
            is_key_up: false,
        });
        let keyboard = unsafe { input.Anonymous.ki };
        assert_eq!(keyboard.wVk.0, 0);
        assert_eq!(keyboard.wScan, 0x38);
        assert!(keyboard.dwFlags.contains(KEYEVENTF_SCANCODE));
        assert!(keyboard.dwFlags.contains(KEYEVENTF_EXTENDEDKEY));
        assert!(!keyboard.dwFlags.contains(KEYEVENTF_KEYUP));
    }
}
