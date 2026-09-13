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
#![cfg(windows)]

use std::io::{BufRead, BufReader};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::ble::gatt_note;
use crate::button_mapping::EngineMessage;
use crate::raw_input::{ButtonEdge, RemoteButton};
use crate::{ConnectionPhase, ConnectionSnapshot, RemoteModel};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const INJECTOR_EXE: &str = "sayall-rc003-inject.exe";
/// 注入失败后的重试冷却：未提权/宿主未就绪时不每个轮询周期都重启子进程。
const FAILURE_COOLDOWN: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(500);

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

struct HookChild {
    child: Child,
    stdin: Option<ChildStdin>,
    readers: Vec<JoinHandle<()>>,
}

impl HookChild {
    /// 关掉 stdin（injector 收到 EOF → 卸钩），等待其干净退出，超时则杀。
    fn teardown(mut self) {
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

fn spawn_injector(engine: &Sender<EngineMessage>) -> std::io::Result<HookChild> {
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
        readers.push(thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line == "ready" {
                    gatt_note("rc003_hook stream=ready".to_owned());
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
    })
}

fn run(
    snapshot: impl Fn() -> ConnectionSnapshot,
    engine: Sender<EngineMessage>,
    stop: Arc<AtomicBool>,
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
            match spawn_injector(&engine) {
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
        // 以小步睡眠对 stop 保持响应。
        let wake = Instant::now() + POLL_INTERVAL;
        while Instant::now() < wake {
            if stop.load(Ordering::SeqCst) {
                break;
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
}

impl Rc003HookRuntime {
    pub fn start(
        snapshot: impl Fn() -> ConnectionSnapshot + Send + 'static,
        engine: Sender<EngineMessage>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let join = thread::Builder::new()
            .name("rc003-hook".to_owned())
            .spawn(move || run(snapshot, engine, worker_stop))
            .ok();
        Self { stop, join }
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
}
