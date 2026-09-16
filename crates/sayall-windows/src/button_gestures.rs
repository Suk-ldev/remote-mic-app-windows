//! 普通按键手势识别（单击/双击/长按/连发），语义对齐 Mac 原版
//! `RemoteButtonGestureRecognizer` + `HIDRemoteScheduler`：
//!
//! - 双击窗口 300ms、长按阈值 550ms、连发起始 350ms（稳定释放闸门 600ms
//!   属 Mac 防抖细节，Windows v1 不引入）。
//! - **按配置动态启用**（Mac 官方文案："双击会等待约 0.3 秒确认单击；长按
//!   约 0.55 秒触发。未配置时保持原有即时响应。"）：
//!   - 未配置双击/长按 → 单击在按下沿立即触发（零延迟），并按住连发
//!     （返回 50ms、方向/音量 100ms，见 [`crate::raw_input::RemoteButton::repeat_interval`]）；
//!   - 配置了长按（未配置双击）→ 快速按下/释放触发单击（释放沿），按住 550ms 触发长按；
//!   - 配置了双击 → 释放沿等待 300ms 双击窗口，第二击释放立即触发双击，
//!     窗口超时补发单击；连发停用。
//! - 第二击按住同样可触发长按（Mac：press 时重启长按计时）。
//! - 语音键不进入本识别器（保持按下开始/释放结束的实时生命周期）。
//!
//! 纯状态机：不持锁、不触 IO，时间由调用方注入，便于单元测试。
//! 计时器不自行调度：引擎线程以 [`GestureRecognizer::next_deadline`] 作为
//! recv_timeout，超时后调用 [`GestureRecognizer::advance`] 处理到期定时器。

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::raw_input::RemoteButton;
use crate::send_input::{ButtonAction, ButtonMappings, ButtonTrigger, WHEEL_REPEAT_INTERVAL};

/// 双击判定窗口的下限与上限。
///
/// 为什么不像 Mac 版那样硬编码 300ms：蓝牙链路上的按键边沿抖动远大于有线
/// 键盘，硬编码窗口在两个方向都会失败——参考项目 vibe-flow 在真机 RC003 上
/// 实测，320ms 时一次自然双击的实测间隔是 378ms，被判成两次独立单击；而要求
/// 用户敲得更快，会快到遥控器根本不上报按下（日志里只有两个释放沿、没有按下
/// 沿，手势无从形成）。
///
/// 因此窗口取用户自己在 Windows 里调好的双击速度（`GetDoubleClickTime`），
/// 只夹到本项目可用的区间：下限不低于系统默认的 500ms，上限 900ms 以免
/// 单击的补发延迟长到像卡顿。
pub const DOUBLE_CLICK_WINDOW_FLOOR: Duration = Duration::from_millis(500);
pub const DOUBLE_CLICK_WINDOW_CEILING: Duration = Duration::from_millis(900);

/// 把系统双击速度夹到可用区间。非 Windows 主机取不到系统值，用下限。
pub fn clamp_double_click_window(system: Duration) -> Duration {
    system.clamp(DOUBLE_CLICK_WINDOW_FLOOR, DOUBLE_CLICK_WINDOW_CEILING)
}

/// 读一次系统双击速度（毫秒）。读不到返回 None，由调用方回落到下限。
#[cfg(windows)]
fn system_double_click_millis() -> Option<u64> {
    // GetDoubleClickTime 不会失败，但返回 0 表示状态异常，按读不到处理。
    let value = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() };
    (value > 0).then_some(u64::from(value))
}

#[cfg(not(windows))]
fn system_double_click_millis() -> Option<u64> {
    None
}

/// 进程内的双击判定窗口：首次使用时读一次系统值并缓存。用户改系统双击速度
/// 后需要重启应用才生效——这是刻意的，避免手势识别的判定标准在运行中漂移。
pub fn double_click_window() -> Duration {
    static WINDOW: std::sync::OnceLock<Duration> = std::sync::OnceLock::new();
    *WINDOW.get_or_init(|| match system_double_click_millis() {
        Some(millis) => clamp_double_click_window(Duration::from_millis(millis)),
        None => DOUBLE_CLICK_WINDOW_FLOOR,
    })
}
/// 长按阈值（按住多久触发长按），Mac 同款。
pub const LONG_PRESS_THRESHOLD: Duration = Duration::from_millis(550);
/// 连发起始延迟（按住多久开始重复单击），Mac 同款。
pub const REPEAT_START_DELAY: Duration = Duration::from_millis(350);

/// 单个按键的手势配置（由按键映射推导）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GestureConfig {
    /// 单击动作已配置（未配置时单击触发为空操作）。
    pub single_configured: bool,
    /// 双击列已配置 → 单击等待双击窗口。
    pub double_enabled: bool,
    /// 长按列已配置 → 按住 550ms 触发长按。
    pub long_enabled: bool,
    /// 原始单击路径（单击+连发在按下沿立即触发）：
    /// 仅当只配置了单击且该键支持连发时成立。
    pub repeat: Option<Duration>,
}

impl GestureConfig {
    /// 从按键映射推导某键的手势配置。未配置任何动作 → None（识别器忽略该键）。
    pub fn for_button(mappings: &ButtonMappings, button: RemoteButton) -> Option<Self> {
        let actions = mappings.actions(button);
        if !mappings.enabled || !actions.any_configured() {
            return None;
        }
        // 按住快捷键走按下/松开边沿（button_mapping 的 hold 路径），不参与
        // 手势识别；归一化保证它只单独占据单击列且无双击/长按。
        if matches!(
            actions.single.as_slice(),
            [ButtonAction::HoldShortcut { .. }]
        ) {
            return None;
        }
        let single_configured = !actions.single.is_empty();
        let double_enabled = !actions.double.is_empty();
        let long_enabled = !actions.long.is_empty();
        let repeat = if single_configured && !double_enabled && !long_enabled {
            // 滚轮的连滚由动作决定而非按键：挂在哪个键上都按住连滚。
            // 多步序列不连发——连发一串带等待的动作没有可用语义。
            match actions.single.as_slice() {
                [ButtonAction::Mouse { kind }] if kind.is_wheel() => Some(WHEEL_REPEAT_INTERVAL),
                [_] => button.repeat_interval(),
                _ => None,
            }
        } else {
            None
        };
        Some(Self {
            single_configured,
            double_enabled,
            long_enabled,
            repeat,
        })
    }

    /// 原始单击路径：未配置双击/长按时单击在按下沿立即触发（零延迟，
    /// Mac 同款），按住时按 repeat 间隔连发（无连发能力的按键不重复）。
    fn raw_path(&self) -> bool {
        self.single_configured && !self.double_enabled && !self.long_enabled
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct ButtonGestureState {
    pressed: bool,
    is_second_press: bool,
    waiting_for_second: bool,
    long_fired: bool,
    long_deadline: Option<Instant>,
    double_deadline: Option<Instant>,
    repeat_deadline: Option<Instant>,
}

/// 手势识别器：每键独立状态机。所有方法都不会 panic，未配置的按键被忽略。
#[derive(Debug)]
pub struct GestureRecognizer {
    buttons: BTreeMap<RemoteButton, (GestureConfig, ButtonGestureState)>,
    double_click_window: Duration,
}

impl Default for GestureRecognizer {
    fn default() -> Self {
        Self::new()
    }
}

impl GestureRecognizer {
    pub fn new() -> Self {
        Self {
            buttons: BTreeMap::new(),
            double_click_window: double_click_window(),
        }
    }

    /// 指定双击窗口构造（测试用；生产走 [`GestureRecognizer::new`]）。
    pub fn with_double_click_window(window: Duration) -> Self {
        Self {
            buttons: BTreeMap::new(),
            double_click_window: clamp_double_click_window(window),
        }
    }

    pub fn double_click_window(&self) -> Duration {
        self.double_click_window
    }

    /// 按当前映射重建配置。配置变化会重置全部手势状态（进行中的双击窗口、
    /// 长按计时与连发一并取消，不触发任何动作）。
    pub fn configure(&mut self, mappings: &ButtonMappings) {
        self.buttons.clear();
        for button in crate::raw_input::ALL_BUTTONS {
            if let Some(config) = GestureConfig::for_button(mappings, button) {
                self.buttons
                    .insert(button, (config, ButtonGestureState::default()));
            }
        }
    }

    /// 按下沿：返回立即触发的手势（原始单击路径）。
    pub fn press(&mut self, button: RemoteButton, now: Instant) -> Vec<ButtonTrigger> {
        let Some((config, state)) = self.buttons.get_mut(&button) else {
            return Vec::new();
        };
        if state.waiting_for_second {
            // 第二击：取消双击窗口，标记第二击并重启长按计时。
            state.waiting_for_second = false;
            state.is_second_press = true;
            state.double_deadline = None;
        }
        state.pressed = true;
        if config.long_enabled {
            state.long_deadline = Some(now + LONG_PRESS_THRESHOLD);
        }
        if config.raw_path() {
            // 原始单击路径：按下沿立即触发单击；有连发能力的按键 350ms 后连发。
            if config.repeat.is_some() {
                state.repeat_deadline = Some(now + REPEAT_START_DELAY);
            }
            return vec![ButtonTrigger::Single];
        }
        Vec::new()
    }

    /// 释放沿：返回立即触发的手势（双击/单击）。
    pub fn release(&mut self, button: RemoteButton, now: Instant) -> Vec<ButtonTrigger> {
        let Some((config, state)) = self.buttons.get_mut(&button) else {
            return Vec::new();
        };
        state.pressed = false;
        state.long_deadline = None;
        state.repeat_deadline = None;
        if state.long_fired {
            // 长按已触发：静默收尾。
            state.long_fired = false;
            state.is_second_press = false;
            return Vec::new();
        }
        if state.is_second_press {
            state.is_second_press = false;
            return vec![ButtonTrigger::Double];
        }
        if config.double_enabled {
            state.waiting_for_second = true;
            state.double_deadline = Some(now + self.double_click_window);
            return Vec::new();
        }
        if config.raw_path() {
            // 原始路径已在按下沿触发，释放只取消连发。
            return Vec::new();
        }
        // 手势路径且未配置双击：立即触发单击。
        vec![ButtonTrigger::Single]
    }

    /// 处理到期定时器（双击窗口超时/长按/连发），返回触发的手势。
    pub fn advance(&mut self, now: Instant) -> Vec<(RemoteButton, ButtonTrigger)> {
        let mut fired = Vec::new();
        for (button, (config, state)) in &mut self.buttons {
            if state
                .double_deadline
                .is_some_and(|deadline| deadline <= now)
            {
                state.double_deadline = None;
                state.waiting_for_second = false;
                fired.push((*button, ButtonTrigger::Single));
            }
            if state.long_deadline.is_some_and(|deadline| deadline <= now) {
                state.long_deadline = None;
                if state.pressed {
                    state.long_fired = true;
                    fired.push((*button, ButtonTrigger::Long));
                }
            }
            if let Some(interval) = config.repeat {
                if state
                    .repeat_deadline
                    .is_some_and(|deadline| deadline <= now)
                {
                    if state.pressed {
                        state.repeat_deadline = Some(now + interval);
                        fired.push((*button, ButtonTrigger::Single));
                    } else {
                        state.repeat_deadline = None;
                    }
                }
            }
        }
        fired
    }

    /// 最近的定时器截止时间（引擎线程的 recv_timeout 依据）。
    pub fn next_deadline(&self) -> Option<Instant> {
        self.buttons
            .values()
            .flat_map(|(_, state)| {
                [
                    state.long_deadline,
                    state.double_deadline,
                    state.repeat_deadline,
                ]
            })
            .flatten()
            .min()
    }

    /// 取消全部手势状态（监听器停止/设备移除/配置变化时调用），不触发动作。
    pub fn release_all(&mut self) {
        for (_, state) in self.buttons.values_mut() {
            *state = ButtonGestureState::default();
        }
    }

    #[allow(dead_code)]
    pub fn is_pressed(&self, button: RemoteButton) -> bool {
        self.buttons
            .get(&button)
            .is_some_and(|(_, state)| state.pressed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::send_input::{ButtonAction, ButtonActions, KeyChord, KeyCode, MouseAction};

    /// 测试用的固定双击窗口：生产值跟随系统双击速度，断言不能依赖它。
    const TEST_DOUBLE_WINDOW: Duration = DOUBLE_CLICK_WINDOW_FLOOR;

    fn mappings_with(
        button: RemoteButton,
        single: Option<KeyCode>,
        double: Option<KeyCode>,
        long: Option<KeyCode>,
    ) -> ButtonMappings {
        let action = |keys: Option<KeyCode>| match keys {
            Some(key) => vec![ButtonAction::Shortcut {
                chord: KeyChord { keys: vec![key] },
            }],
            None => Vec::new(),
        };
        let mut mappings = ButtonMappings::default();
        mappings.actions.insert(
            button,
            ButtonActions {
                single: action(single),
                double: action(double),
                long: action(long),
            },
        );
        mappings
    }

    fn mappings_with_single(button: RemoteButton, single: ButtonAction) -> ButtonMappings {
        let mut mappings = ButtonMappings::default();
        mappings
            .actions
            .insert(button, ButtonActions::single(single));
        mappings
    }

    #[test]
    fn double_click_window_follows_the_system_setting_within_bounds() {
        // 低于下限的系统设置被抬到下限：蓝牙边沿抖动下 320ms 会把自然双击
        // （实测 378ms）判成两次单击。
        assert_eq!(
            clamp_double_click_window(Duration::from_millis(320)),
            DOUBLE_CLICK_WINDOW_FLOOR
        );
        // 区间内原样采用用户自己的设置。
        assert_eq!(
            clamp_double_click_window(Duration::from_millis(700)),
            Duration::from_millis(700)
        );
        // 高于上限被压到上限：单击的补发延迟不能长到像卡顿。
        assert_eq!(
            clamp_double_click_window(Duration::from_millis(5_000)),
            DOUBLE_CLICK_WINDOW_CEILING
        );
        // 进程内的实际取值始终落在区间内。
        let window = double_click_window();
        assert!(window >= DOUBLE_CLICK_WINDOW_FLOOR && window <= DOUBLE_CLICK_WINDOW_CEILING);
        assert_eq!(GestureRecognizer::new().double_click_window(), window);
    }

    /// 参考项目 vibe-flow 在真机 RC003 上实测的失败场景：两击相隔 378ms。
    #[test]
    fn a_natural_bluetooth_double_tap_at_378ms_is_recognized() {
        let t0 = Instant::now();
        let mut recognizer = GestureRecognizer::with_double_click_window(DOUBLE_CLICK_WINDOW_FLOOR);
        recognizer.configure(&mappings_with(
            RemoteButton::Ok,
            Some(KeyCode::Enter),
            Some(KeyCode::Escape),
            None,
        ));

        assert!(recognizer.press(RemoteButton::Ok, t0).is_empty());
        assert!(recognizer
            .release(RemoteButton::Ok, t0 + Duration::from_millis(40))
            .is_empty());
        // 第二击落在 378ms：旧的 300ms 窗口会先超时补发单击。
        let second = t0 + Duration::from_millis(378);
        assert!(
            recognizer.advance(second).is_empty(),
            "378ms 时双击窗口不应已超时"
        );
        assert!(recognizer.press(RemoteButton::Ok, second).is_empty());
        assert_eq!(
            recognizer.release(RemoteButton::Ok, second + Duration::from_millis(40)),
            vec![ButtonTrigger::Double]
        );
    }

    #[test]
    fn wheel_repeats_even_on_buttons_without_a_native_repeat_interval() {
        // 确定键本身不连发（repeat_interval = None），但滚轮动作应连滚。
        assert_eq!(RemoteButton::Ok.repeat_interval(), None);
        let config = GestureConfig::for_button(
            &mappings_with_single(
                RemoteButton::Ok,
                ButtonAction::Mouse {
                    kind: MouseAction::WheelDown,
                },
            ),
            RemoteButton::Ok,
        )
        .unwrap();
        assert_eq!(config.repeat, Some(WHEEL_REPEAT_INTERVAL));

        // 鼠标左键不是滚轮：沿用按键自身的连发能力（确定键为不连发）。
        let config = GestureConfig::for_button(
            &mappings_with_single(
                RemoteButton::Ok,
                ButtonAction::Mouse {
                    kind: MouseAction::LeftClick,
                },
            ),
            RemoteButton::Ok,
        )
        .unwrap();
        assert_eq!(config.repeat, None);
    }

    #[test]
    fn hold_shortcut_is_not_a_gesture() {
        let mappings = mappings_with_single(
            RemoteButton::Ok,
            ButtonAction::HoldShortcut {
                chord: KeyChord {
                    keys: vec![KeyCode::Space],
                },
            },
        );
        assert!(GestureConfig::for_button(&mappings, RemoteButton::Ok).is_none());

        let mut recognizer = GestureRecognizer::with_double_click_window(TEST_DOUBLE_WINDOW);
        recognizer.configure(&mappings);
        let now = Instant::now();
        assert!(recognizer.press(RemoteButton::Ok, now).is_empty());
        assert!(recognizer.release(RemoteButton::Ok, now).is_empty());
    }

    #[test]
    fn raw_single_path_fires_on_press_and_repeats_while_held() {
        let t0 = Instant::now();
        let mut recognizer = GestureRecognizer::with_double_click_window(TEST_DOUBLE_WINDOW);
        recognizer.configure(&mappings_with(
            RemoteButton::Up,
            Some(KeyCode::Up),
            None,
            None,
        ));

        // 按下沿立即触发单击。
        assert_eq!(
            recognizer.press(RemoteButton::Up, t0),
            vec![ButtonTrigger::Single]
        );
        // 350ms 内无连发。
        assert_eq!(recognizer.advance(t0 + Duration::from_millis(340)), vec![]);
        // 350ms 起始延迟后按 100ms 间隔连发。
        let t1 = t0 + REPEAT_START_DELAY;
        assert_eq!(
            recognizer.advance(t1),
            vec![(RemoteButton::Up, ButtonTrigger::Single)]
        );
        assert_eq!(recognizer.advance(t1 + Duration::from_millis(99)), vec![]);
        assert_eq!(
            recognizer.advance(t1 + Duration::from_millis(100)),
            vec![(RemoteButton::Up, ButtonTrigger::Single)]
        );
        // 释放取消连发。
        assert_eq!(
            recognizer.release(RemoteButton::Up, t1 + Duration::from_millis(200)),
            vec![]
        );
        assert_eq!(recognizer.advance(t1 + Duration::from_millis(400)), vec![]);
    }

    #[test]
    fn back_repeats_at_50ms_and_non_repeat_buttons_do_not_repeat() {
        let t0 = Instant::now();
        let mut mappings = ButtonMappings::default();
        for (button, key) in [
            (RemoteButton::Back, KeyCode::Escape),
            (RemoteButton::Ok, KeyCode::Enter),
        ] {
            mappings.actions.insert(
                button,
                ButtonActions::single(ButtonAction::Shortcut {
                    chord: KeyChord { keys: vec![key] },
                }),
            );
        }
        let mut recognizer = GestureRecognizer::with_double_click_window(TEST_DOUBLE_WINDOW);
        recognizer.configure(&mappings);

        assert_eq!(
            recognizer.press(RemoteButton::Back, t0),
            vec![ButtonTrigger::Single]
        );
        assert_eq!(
            recognizer.press(RemoteButton::Ok, t0),
            vec![ButtonTrigger::Single]
        );
        // OK（Enter）不连发：原始路径 repeat=None → 按下沿触发一次后无重复。
        let t1 = t0 + REPEAT_START_DELAY + Duration::from_millis(500);
        let fired = recognizer.advance(t1);
        assert_eq!(fired, vec![(RemoteButton::Back, ButtonTrigger::Single)]);
    }

    #[test]
    fn long_press_only_config_fires_single_on_release_and_long_at_threshold() {
        let t0 = Instant::now();
        let mut recognizer = GestureRecognizer::with_double_click_window(TEST_DOUBLE_WINDOW);
        recognizer.configure(&mappings_with(
            RemoteButton::Menu,
            Some(KeyCode::Enter),
            None,
            Some(KeyCode::Escape),
        ));

        // 快速按下/释放 → 释放沿立即单击。
        assert_eq!(recognizer.press(RemoteButton::Menu, t0), vec![]);
        assert_eq!(
            recognizer.release(RemoteButton::Menu, t0 + Duration::from_millis(120)),
            vec![ButtonTrigger::Single]
        );

        // 按住 550ms → 长按触发；随后释放静默收尾。
        assert_eq!(
            recognizer.press(RemoteButton::Menu, t0 + Duration::from_secs(1)),
            vec![]
        );
        assert_eq!(
            recognizer.advance(t0 + Duration::from_secs(1) + LONG_PRESS_THRESHOLD),
            vec![(RemoteButton::Menu, ButtonTrigger::Long)]
        );
        assert_eq!(
            recognizer.release(
                RemoteButton::Menu,
                t0 + Duration::from_secs(1) + Duration::from_millis(700)
            ),
            vec![]
        );
    }

    #[test]
    fn double_click_window_defers_single_and_second_release_fires_double() {
        let t0 = Instant::now();
        let mut recognizer = GestureRecognizer::with_double_click_window(TEST_DOUBLE_WINDOW);
        recognizer.configure(&mappings_with(
            RemoteButton::Ok,
            Some(KeyCode::Enter),
            Some(KeyCode::Space),
            None,
        ));

        // 第一击：释放后进入 300ms 双击窗口，不立即单击。
        recognizer.press(RemoteButton::Ok, t0);
        assert_eq!(
            recognizer.release(RemoteButton::Ok, t0 + Duration::from_millis(80)),
            vec![]
        );
        // 窗口内第二击：按下沿无触发（双击未配置长按）……
        let t1 = t0 + Duration::from_millis(80) + Duration::from_millis(120);
        assert_eq!(recognizer.press(RemoteButton::Ok, t1), vec![]);
        // 第二击释放 → 立即双击。
        assert_eq!(
            recognizer.release(RemoteButton::Ok, t1 + Duration::from_millis(60)),
            vec![ButtonTrigger::Double]
        );

        // 另一轮：只有一击 → 窗口超时后补发单击。
        let t2 = t0 + Duration::from_secs(2);
        recognizer.press(RemoteButton::Ok, t2);
        assert_eq!(
            recognizer.release(RemoteButton::Ok, t2 + Duration::from_millis(50)),
            vec![]
        );
        assert_eq!(
            recognizer.advance(t2 + Duration::from_millis(50) + TEST_DOUBLE_WINDOW),
            vec![(RemoteButton::Ok, ButtonTrigger::Single)]
        );
    }

    #[test]
    fn second_press_hold_can_still_trigger_long_press() {
        let t0 = Instant::now();
        let mut recognizer = GestureRecognizer::with_double_click_window(TEST_DOUBLE_WINDOW);
        recognizer.configure(&mappings_with(
            RemoteButton::Home,
            Some(KeyCode::Enter),
            Some(KeyCode::Space),
            Some(KeyCode::Escape),
        ));

        recognizer.press(RemoteButton::Home, t0);
        recognizer.release(RemoteButton::Home, t0 + Duration::from_millis(60));
        let t1 = t0 + Duration::from_millis(200);
        recognizer.press(RemoteButton::Home, t1);
        // 第二击按住 550ms → 长按（Mac：第二击重启长按计时）。
        assert_eq!(
            recognizer.advance(t1 + LONG_PRESS_THRESHOLD),
            vec![(RemoteButton::Home, ButtonTrigger::Long)]
        );
        assert_eq!(
            recognizer.release(
                RemoteButton::Home,
                t1 + LONG_PRESS_THRESHOLD + Duration::from_millis(100)
            ),
            vec![]
        );
    }

    #[test]
    fn configuring_double_or_long_disables_repeat() {
        let t0 = Instant::now();
        let mut recognizer = GestureRecognizer::with_double_click_window(TEST_DOUBLE_WINDOW);
        recognizer.configure(&mappings_with(
            RemoteButton::Up,
            Some(KeyCode::Up),
            Some(KeyCode::Space),
            None,
        ));
        // 配置了双击 → 按下沿不再立即单击（等待窗口语义），也无连发。
        assert_eq!(recognizer.press(RemoteButton::Up, t0), vec![]);
        assert_eq!(
            recognizer.release(RemoteButton::Up, t0 + Duration::from_millis(50)),
            vec![]
        );
        assert_eq!(
            recognizer.advance(t0 + Duration::from_millis(50) + TEST_DOUBLE_WINDOW),
            vec![(RemoteButton::Up, ButtonTrigger::Single)]
        );
        assert_eq!(
            recognizer.advance(t0 + Duration::from_secs(2)),
            vec![],
            "双击窗口结束后不应再有连发"
        );
    }

    #[test]
    fn release_all_cancels_pending_windows_and_repeat_without_firing() {
        let t0 = Instant::now();
        let mut recognizer = GestureRecognizer::with_double_click_window(TEST_DOUBLE_WINDOW);
        recognizer.configure(&mappings_with(
            RemoteButton::Back,
            Some(KeyCode::Escape),
            Some(KeyCode::Space),
            None,
        ));
        recognizer.press(RemoteButton::Back, t0);
        recognizer.release(RemoteButton::Back, t0 + Duration::from_millis(40));
        recognizer.release_all();
        assert_eq!(
            recognizer.advance(t0 + Duration::from_secs(2)),
            vec![],
            "释放全部状态后双击窗口不应再触发单击"
        );
    }

    #[test]
    fn unconfigured_and_disabled_buttons_are_ignored() {
        let t0 = Instant::now();
        let mut recognizer = GestureRecognizer::with_double_click_window(TEST_DOUBLE_WINDOW);
        recognizer.configure(&ButtonMappings::default());
        assert_eq!(recognizer.press(RemoteButton::Ok, t0), vec![]);
        assert_eq!(recognizer.release(RemoteButton::Ok, t0), vec![]);
        assert_eq!(recognizer.advance(t0 + Duration::from_secs(5)), vec![]);
        assert_eq!(recognizer.next_deadline(), None);

        // 只有禁用动作的按键同样被忽略。
        let mut mappings = ButtonMappings::default();
        mappings
            .actions
            .insert(RemoteButton::Ok, ButtonActions::default());
        recognizer.configure(&mappings);
        assert_eq!(recognizer.press(RemoteButton::Ok, t0), vec![]);
    }

    #[test]
    fn enabled_toggle_off_disables_everything() {
        let t0 = Instant::now();
        let mut mappings = mappings_with(RemoteButton::Up, Some(KeyCode::Up), None, None);
        let mut recognizer = GestureRecognizer::with_double_click_window(TEST_DOUBLE_WINDOW);
        recognizer.configure(&mappings);
        mappings.enabled = false;
        recognizer.configure(&mappings);
        assert_eq!(recognizer.press(RemoteButton::Up, t0), vec![]);
        assert_eq!(recognizer.release(RemoteButton::Up, t0), vec![]);
    }

    #[test]
    fn next_deadline_is_the_earliest_timer() {
        let t0 = Instant::now();
        let mut mappings = mappings_with(RemoteButton::Up, Some(KeyCode::Up), None, None);
        mappings.actions.insert(
            RemoteButton::Down,
            ButtonActions {
                single: vec![ButtonAction::Shortcut {
                    chord: KeyChord {
                        keys: vec![KeyCode::Down],
                    },
                }],
                double: vec![ButtonAction::Shortcut {
                    chord: KeyChord {
                        keys: vec![KeyCode::Space],
                    },
                }],
                long: Vec::new(),
            },
        );
        let mut recognizer = GestureRecognizer::with_double_click_window(TEST_DOUBLE_WINDOW);
        recognizer.configure(&mappings);
        recognizer.press(RemoteButton::Up, t0);
        recognizer.press(RemoteButton::Down, t0);
        recognizer.release(RemoteButton::Down, t0 + Duration::from_millis(30));
        // Up 连发起始 = t0 + REPEAT_START_DELAY；Down 双击窗口 = t0+30ms + 窗口。
        // 断言取两者较早的一个——双击窗口跟随系统设置，不能写死数值。
        let up_repeat = t0 + REPEAT_START_DELAY;
        let down_double = t0 + Duration::from_millis(30) + TEST_DOUBLE_WINDOW;
        assert_eq!(recognizer.next_deadline(), Some(up_repeat.min(down_double)));
    }
}
