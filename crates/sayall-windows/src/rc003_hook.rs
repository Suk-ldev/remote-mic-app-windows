//! RC003 返回/音量± 钩子管理器。
//!
//! RC003 的返回/音量± 报文在本机不进任何常规输入通道（见
//! `docs/investigations/2026-09-13-rc003-wudfhost-tap-confirmed.md`），唯一可达
//! 路径是向承载其 HID-over-GATT 的 WUDFHost 注入钩子 DLL。本模块把已验证的
//! C++ 注入器/流送器 `sayall-rc003-inject.exe` 作为子进程托管：
//!
//! - 生命周期由连接状态轮询驱动（方案 3，钩子只在遥控器在用时驻留）：型号为
//!   RC003 且连接就绪时注入；断连时关掉子进程 stdin（injector 收到 EOF → 卸钩）。
//! - 子进程 stdout 逐行 `"<button> <0|1>"`，解析为 [`ButtonEdge`] 后以
//!   [`EngineMessage::GateEdge`] 灌入按键映射引擎，与其它键走同一手势/映射管线。
//! - 注入需管理员；未提权时 injector 立即失败，本管理器按冷却退避重试，不刷屏。
//!
//! 仅按键 id + 上下越过进程边界，无原始 HID/设备身份。子进程崩溃不影响应用。
//!
//! ## 语音键报告层合成（2026-10-06）
//!
//! 同一条钩子还承载「按住说话快捷键」的第二条路径：注入器把用户配置的单键
//! （右/左 Alt）下发给钩子，钩子在报告层把语音键 usage 原地换成它。这样按键
//! 以**设备报告**而不是 SendInput 到达系统（`injected=0`），豆包、Chatterfly
//! 这类丢弃注入按键的工具才会响应——它们对 SendInput 的全部表达形式都无反应，
//! 这一点由上游真机实测闭环（见 `Bugs/2026-10-06-injected-key-voice-tools.md`）。
//!
//! 两条路径**互斥**：合成生效期间 BLE 层不得再注入同一个和弦（双写互扰）。
//! 判据是 [`voice_synth_active`]——只有注入器回执了与当前目标一致的 ack 才为真，
//! 子进程退出、卸钩、断连一律立即回落到 SendInput 路径（fail-open）。
#![cfg(windows)]

use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::ble::gatt_note;
use crate::button_mapping::EngineMessage;
use crate::raw_input::{ButtonEdge, RemoteButton};
use crate::send_input::{KeyCode, VoiceHotkeyMode, VoiceHotkeySettings};
use crate::{ConnectionPhase, ConnectionSnapshot, RemoteModel};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const INJECTOR_EXE: &str = "sayall-rc003-inject.exe";
/// 注入失败后的重试冷却：未提权/宿主未就绪时不每个轮询周期都重启子进程。
const FAILURE_COOLDOWN: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// 报告层合成是否真的在生效（注入器已回执当前目标）。
///
/// 写入方只有本模块：收到匹配 ack 置真；子进程退出/卸钩/改配置/断连置假。
/// 读取方是 `ble.rs` 的语音会话起点——为真就不再注入快捷键。宁可多注入一次
/// （回到既有行为）也不要两条路径同时写，所以所有不确定路径一律置假。
static VOICE_SYNTH_ACTIVE: AtomicBool = AtomicBool::new(false);

/// BLE 层的二选一门禁：报告层合成已确认生效时，SendInput 注入路径停用。
pub fn voice_synth_active() -> bool {
    VOICE_SYNTH_ACTIVE.load(Ordering::SeqCst)
}

/// 「按住说话快捷键」能否交给报告层合成。三个条件缺一不可：
///
/// * **按住说话**：合成是槽内替换，按键的按下时长就是语音键的按下时长；
///   单次触发（开始点按一次、排空结束再点按一次）无法用报告状态表达，
///   继续走 SendInput。
/// * **恰好单键**：一个报告槽一次只能呈现一个 usage，和弦表达不了。
/// * **usage 已实测**：替换 usage 会被 Windows HID 映射层重新翻成 VK，未实测
///   的 usage 可能产出意外键值或不产出事件。白名单与 `hook_protocol.h` 的
///   `HookSynthUsageAllowed` 同源（0x00E6 → 右 Alt、0x00E2 → 左 Alt），
///   钩子侧是终审，这里只做「不下发注定被拒的配置」的预过滤。
pub fn voice_synth_usage(hotkey: &VoiceHotkeySettings) -> Option<u16> {
    if hotkey.mode != VoiceHotkeyMode::Hold {
        return None;
    }
    let keys = &hotkey.chord.as_ref()?.keys;
    let [key] = keys[..] else {
        return None;
    };
    match key {
        KeyCode::RightAlt => Some(0x00E6),
        KeyCode::LeftAlt => Some(0x00E2),
        _ => None,
    }
}

/// 把 injector 的一行输出解析为一次按键边沿。仅接受返回/音量±；`ready`、
/// 空行、未知键返回 `None`。
fn parse_edge(line: &str) -> Option<ButtonEdge> {
    let mut parts = line.split_whitespace();
    let name = parts.next()?;
    let pressed = match parts.next()? {
        "1" => true,
        "0" => false,
        _ => return None,
    };
    let button = match name {
        "back" => RemoteButton::Back,
        "volume_up" => RemoteButton::VolumeUp,
        "volume_down" => RemoteButton::VolumeDown,
        _ => return None,
    };
    Some(ButtonEdge {
        button,
        is_pressed: pressed,
    })
}

fn injector_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join(INJECTOR_EXE))
}

/// 注入器回执的一行 `synth_ack ...`：`Some(Some(usage))` = 已武装该 usage，
/// `Some(None)` = 已关闭，`None` = 无法解析（按未武装处理）。
fn parse_synth_ack(rest: &str) -> Option<Option<u16>> {
    if rest == "off" {
        return Some(None);
    }
    let hex = rest.strip_prefix("to=")?;
    u16::from_str_radix(hex, 16).ok().map(Some)
}

/// 回执只有与**当前**目标一致才置门禁为真：滞后的 ack（用户刚改过快捷键）
/// 置真会让 BLE 层停用注入，而报告层换的是另一个键——两边都不出声。
fn apply_synth_ack(rest: &str, desired: &Mutex<Option<u16>>) {
    let acked = parse_synth_ack(rest);
    let want = *crate::lock(desired);
    let active = matches!((acked, want), (Some(Some(acked)), Some(want)) if acked == want);
    VOICE_SYNTH_ACTIVE.store(active, Ordering::SeqCst);
    gatt_note(format!(
        "rc003_hook synth result=ack armed={active} acked={} want={}",
        match acked {
            Some(Some(usage)) => format!("{usage:04X}"),
            Some(None) => "off".to_owned(),
            None => "unparsed".to_owned(),
        },
        want.map_or_else(|| "off".to_owned(), |usage| format!("{usage:04X}"))
    ));
}

/// 任何不确定路径都回落到 SendInput：子进程退出、卸钩、断连、下发失败。
fn disarm_synth(reason: &str) {
    if VOICE_SYNTH_ACTIVE.swap(false, Ordering::SeqCst) {
        gatt_note(format!("rc003_hook synth result=disarmed reason={reason}"));
    }
}

struct HookChild {
    child: Child,
    stdin: Option<ChildStdin>,
    readers: Vec<JoinHandle<()>>,
    /// 已经下发给这个子进程的合成目标；子进程换代即重新下发。
    sent: Option<Option<u16>>,
}

impl HookChild {
    /// 下发合成目标（仅在与已下发值不同时写一行）。写失败视为子进程已死：
    /// 调用方下一轮 `try_wait` 会回收它，这里先把门禁落回 SendInput。
    fn send_voice_synth(&mut self, target: Option<u16>) -> bool {
        if self.sent == Some(target) {
            return true;
        }
        let line = match target {
            Some(usage) => format!("synth {usage:04X}\n"),
            None => "synth off\n".to_owned(),
        };
        let Some(stdin) = self.stdin.as_mut() else {
            disarm_synth("stdin_closed");
            return false;
        };
        if stdin.write_all(line.as_bytes()).is_err() || stdin.flush().is_err() {
            disarm_synth("stdin_write_failed");
            gatt_note(
                "rc003_hook synth result=err error_domain=injector error_code=write_failed reason=child_stdin_unavailable retryable=true"
                    .to_owned(),
            );
            return false;
        }
        // 门禁只认回执：这里只记录"已下发"，置真由 apply_synth_ack 完成。
        if target.is_none() {
            disarm_synth("disarm_requested");
        }
        self.sent = Some(target);
        gatt_note(format!(
            "rc003_hook synth result=sent to={}",
            target.map_or_else(|| "off".to_owned(), |usage| format!("{usage:04X}"))
        ));
        true
    }

    /// 关掉 stdin（injector 收到 EOF → 卸钩），等待其干净退出，超时则杀。
    fn teardown(mut self) {
        disarm_synth("hook_teardown");
        drop(self.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                _ if Instant::now() >= deadline => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
                _ => thread::sleep(Duration::from_millis(50)),
            }
        }
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

fn spawn_injector(
    engine: &Sender<EngineMessage>,
    desired: &Arc<Mutex<Option<u16>>>,
) -> std::io::Result<HookChild> {
    let path = injector_path()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "injector path"))?;
    let mut child = Command::new(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()?;
    let stdin = child.stdin.take();
    let mut readers = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        let engine = engine.clone();
        let desired = Arc::clone(desired);
        readers.push(thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line == "ready" {
                    gatt_note("rc003_hook stream=ready".to_owned());
                    continue;
                }
                if let Some(rest) = line.strip_prefix("synth_ack ") {
                    apply_synth_ack(rest, &desired);
                    continue;
                }
                if let Some(rest) = line.strip_prefix("synth_nak ") {
                    disarm_synth("injector_rejected");
                    gatt_note(format!(
                        "rc003_hook synth result=nak {rest} error_domain=injector error_code=command_rejected retryable=false"
                    ));
                    continue;
                }
                if let Some(edge) = parse_edge(&line) {
                    let _ = engine.send(EngineMessage::GateEdge(edge));
                }
            }
        }));
    }
    if let Some(stderr) = child.stderr.take() {
        // injector 诊断（stage=... result=... win32=...）落进统一诊断日志。
        readers.push(thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if !line.is_empty() {
                    gatt_note(format!("rc003_hook inject {line}"));
                }
            }
        }));
    }
    Ok(HookChild {
        child,
        stdin,
        readers,
        sent: None,
    })
}

fn run(
    snapshot: impl Fn() -> ConnectionSnapshot,
    engine: Sender<EngineMessage>,
    stop: Arc<AtomicBool>,
    desired_synth: Arc<Mutex<Option<u16>>>,
) {
    let mut active: Option<HookChild> = None;
    let mut next_attempt = Instant::now();
    while !stop.load(Ordering::SeqCst) {
        // 子进程若已退出（注入失败或宿主消失），回收并进入冷却。
        if let Some(child) = active.as_mut() {
            if matches!(child.child.try_wait(), Ok(Some(_))) {
                if let Some(dead) = active.take() {
                    dead.teardown();
                }
                next_attempt = Instant::now() + FAILURE_COOLDOWN;
                gatt_note("rc003_hook child_exited backoff=15s".to_owned());
            }
        }
        let snap = snapshot();
        let want = snap.remote_model == RemoteModel::Rc003
            && matches!(
                snap.phase,
                ConnectionPhase::Ready | ConnectionPhase::Streaming
            );
        if want && active.is_none() && Instant::now() >= next_attempt {
            match spawn_injector(&engine, &desired_synth) {
                Ok(child) => {
                    gatt_note("rc003_hook start".to_owned());
                    active = Some(child);
                }
                Err(error) => {
                    next_attempt = Instant::now() + FAILURE_COOLDOWN;
                    gatt_note(format!("rc003_hook spawn_failed error={error} backoff=15s"));
                }
            }
        } else if !want {
            if let Some(child) = active.take() {
                gatt_note("rc003_hook stop reason=disconnected".to_owned());
                child.teardown();
            }
        }
        // 下发合成目标：新子进程、或用户改了快捷键时各一次。写失败不在这里
        // 回收子进程——下一轮 try_wait 统一处理，这里只保证门禁已落回。
        if let Some(child) = active.as_mut() {
            let target = *crate::lock(&desired_synth);
            child.send_voice_synth(target);
        }
        // 以小步睡眠对 stop 保持响应。
        let wake = Instant::now() + POLL_INTERVAL;
        while Instant::now() < wake {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            // 合成目标的变化在 50ms 内下发，而不是等满一个轮询周期：用户刚改完
            // 快捷键就按语音键时，钩子不应该还在按旧目标替换（旧键会进 OS）。
            if let Some(child) = active.as_mut() {
                let target = *crate::lock(&desired_synth);
                child.send_voice_synth(target);
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
    if let Some(child) = active.take() {
        child.teardown();
    }
}

/// 持有即运行的 RC003 钩子管理器；Drop 时停止轮询并卸钩。
pub struct Rc003HookRuntime {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    /// 期望的语音键合成目标（None = 关闭）。工作线程负责把它下发给子进程，
    /// 因此遥控器还没连上、子进程还没起来时设置也不会丢——注入器一起来就下发。
    desired_synth: Arc<Mutex<Option<u16>>>,
}

impl Rc003HookRuntime {
    pub fn start(
        snapshot: impl Fn() -> ConnectionSnapshot + Send + 'static,
        engine: Sender<EngineMessage>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let desired_synth = Arc::new(Mutex::new(None));
        let worker_synth = Arc::clone(&desired_synth);
        let join = thread::Builder::new()
            .name("rc003-hook".to_owned())
            .spawn(move || run(snapshot, engine, worker_stop, worker_synth))
            .ok();
        Self {
            stop,
            join,
            desired_synth,
        }
    }

    /// 设置语音键报告层合成目标（`None` = 关闭，回到 SendInput 注入路径）。
    /// 由 `WindowsPlatform::set_voice_hold_hotkey` 在快捷键变化时调用——配置的
    /// 单一事实源是用户的「按住说话快捷键」，这里不做第二份配置。
    pub fn set_voice_synth(&self, usage: Option<u16>) {
        let mut desired = crate::lock(&self.desired_synth);
        if *desired == usage {
            return;
        }
        *desired = usage;
        // 目标一变，旧回执立即失效：在新 ack 到达前一律走 SendInput。
        drop(desired);
        disarm_synth("target_changed");
    }
}

impl Drop for Rc003HookRuntime {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_target_button_edges() {
        assert_eq!(
            parse_edge("back 1"),
            Some(ButtonEdge {
                button: RemoteButton::Back,
                is_pressed: true
            })
        );
        assert_eq!(
            parse_edge("volume_up 0"),
            Some(ButtonEdge {
                button: RemoteButton::VolumeUp,
                is_pressed: false
            })
        );
        assert_eq!(
            parse_edge("volume_down 1"),
            Some(ButtonEdge {
                button: RemoteButton::VolumeDown,
                is_pressed: true
            })
        );
    }

    #[test]
    fn rejects_ready_unknown_and_malformed() {
        assert_eq!(parse_edge("ready"), None);
        assert_eq!(parse_edge(""), None);
        assert_eq!(parse_edge("ok 1"), None); // working key must not arrive here
        assert_eq!(parse_edge("back 2"), None);
        assert_eq!(parse_edge("back"), None);
    }

    fn hotkey(keys: &[KeyCode], mode: VoiceHotkeyMode) -> VoiceHotkeySettings {
        VoiceHotkeySettings {
            chord: Some(crate::send_input::KeyChord {
                keys: keys.to_vec(),
            }),
            mode,
            activate_wetype: false,
        }
    }

    /// 只有「按住说话 + 单键 + 已实测 usage」才交给报告层；其余一律留给
    /// SendInput，否则会出现"两条路径都不出声"或"合成出意外键值"。
    #[test]
    fn voice_synth_usage_accepts_only_measured_single_hold_keys() {
        assert_eq!(
            voice_synth_usage(&hotkey(&[KeyCode::RightAlt], VoiceHotkeyMode::Hold)),
            Some(0x00E6)
        );
        assert_eq!(
            voice_synth_usage(&hotkey(&[KeyCode::LeftAlt], VoiceHotkeyMode::Hold)),
            Some(0x00E2)
        );
        // 单次触发：开始/结束两次点按无法用报告状态表达。
        assert_eq!(
            voice_synth_usage(&hotkey(&[KeyCode::RightAlt], VoiceHotkeyMode::Toggle)),
            None
        );
        // 和弦：一个报告槽一次只能呈现一个 usage。
        assert_eq!(
            voice_synth_usage(&hotkey(
                &[KeyCode::LeftControl, KeyCode::LeftWindows],
                VoiceHotkeyMode::Hold
            )),
            None
        );
        // 未实测 usage：替换后产出什么 VK 不确定，不下发。
        assert_eq!(
            voice_synth_usage(&hotkey(&[KeyCode::RightControl], VoiceHotkeyMode::Hold)),
            None
        );
        assert_eq!(
            voice_synth_usage(&VoiceHotkeySettings::disabled()),
            None,
            "关闭快捷键时不得武装合成"
        );
    }

    /// 门禁只认与当前目标一致的回执：滞后 ack 置真会让 BLE 停用注入，而报告层
    /// 换的是另一个键——两边都不出声。
    #[test]
    fn synth_gate_follows_matching_ack_only() {
        let desired = Mutex::new(Some(0x00E6u16));

        apply_synth_ack("to=00E6", &desired);
        assert!(voice_synth_active(), "匹配回执应武装门禁");

        apply_synth_ack("to=00E2", &desired);
        assert!(!voice_synth_active(), "目标不一致的回执不得武装");

        apply_synth_ack("to=00E6", &desired);
        assert!(voice_synth_active());
        apply_synth_ack("off", &desired);
        assert!(!voice_synth_active(), "关闭回执必须落回 SendInput");

        apply_synth_ack("to=00E6", &desired);
        assert!(voice_synth_active());
        disarm_synth("test");
        assert!(!voice_synth_active());

        // 目标为"关闭"时，任何 to= 回执都不得武装。
        *crate::lock(&desired) = None;
        apply_synth_ack("to=00E6", &desired);
        assert!(!voice_synth_active());
    }

    #[test]
    fn parses_injector_synth_acks() {
        assert_eq!(parse_synth_ack("to=00E6"), Some(Some(0x00E6)));
        assert_eq!(parse_synth_ack("off"), Some(None));
        assert_eq!(parse_synth_ack("to=zz"), None);
        assert_eq!(parse_synth_ack("garbage"), None);
    }
}
