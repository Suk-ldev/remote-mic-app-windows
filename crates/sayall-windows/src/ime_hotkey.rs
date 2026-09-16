//! 读取输入法自己配置的语音热键。
//!
//! 为什么要读：本应用和输入法的语音热键必须完全一致，否则"按了没反应"。
//! 让用户两边手动对齐是最常见的一类配置错误。搜狗和豆包都把热键写在自己的
//! 用户配置文件里，可以直接读出来填。
//!
//! 数据来源：参考项目 ZSTDJan/windows-remote-mic-app 的
//! `voice_hotkey_sync_windows.py`——配置路径与字段结构照它的实现对齐。
//!
//! 边界：
//! - 只读，不写。改别人的配置文件属于越界，用户在输入法里改完再点一次读取即可。
//! - 微信输入法没有可读的配置文件（它的热键只在设置界面里可见，状态栏的隐藏
//!   提示会保留过期值，不可信），所以不在此列，仍用预设 + 手动核对。
//! - 读不到一律如实返回原因，绝不猜一个默认值填进去。

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::send_input::{KeyChord, KeyCode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImeTool {
    Sogou,
    Doubao,
}

impl ImeTool {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Sogou => "搜狗语音输入",
            Self::Doubao => "豆包输入法",
        }
    }

    /// 相对 %APPDATA% 的配置文件路径。
    pub fn config_relative_path(self) -> PathBuf {
        match self {
            Self::Sogou => PathBuf::from("sogou_voice_assistant_pc").join("config.json"),
            Self::Doubao => PathBuf::from("DoubaoIme").join("conf").join("config.json"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DetectError {
    #[error("没有找到{0}的配置文件，可能没装或还没设置过语音热键")]
    NotFound(&'static str),
    #[error("读取{tool}的配置失败：{reason}")]
    ReadFailed { tool: &'static str, reason: String },
}

pub fn detect(tool: ImeTool, appdata: &Path) -> Result<KeyChord, DetectError> {
    let path = appdata.join(tool.config_relative_path());
    if !path.is_file() {
        return Err(DetectError::NotFound(tool.display_name()));
    }
    let contents = std::fs::read_to_string(&path).map_err(|error| DetectError::ReadFailed {
        tool: tool.display_name(),
        reason: error.to_string(),
    })?;
    let parsed = match tool {
        ImeTool::Sogou => parse_sogou(&contents),
        ImeTool::Doubao => parse_doubao(&contents),
    };
    parsed.map_err(|reason| DetectError::ReadFailed {
        tool: tool.display_name(),
        reason,
    })
}

/// `%APPDATA%`。取不到返回 None，由调用方如实上报而不是猜路径。
pub fn appdata_root() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(PathBuf::from)
}

/// 搜狗把按住型热键写成 `setting.shortcutKeysPress` 的词元数组，
/// 例如 `["RightCtrl"]`。
pub fn parse_sogou(contents: &str) -> Result<KeyChord, String> {
    let document: serde_json::Value = serde_json::from_str(contents.trim_start_matches('\u{feff}'))
        .map_err(|error| format!("配置不是合法 JSON：{error}"))?;
    let tokens = document
        .get("setting")
        .and_then(|setting| setting.get("shortcutKeysPress"))
        .and_then(|value| value.as_array())
        .ok_or_else(|| "配置里没有 setting.shortcutKeysPress".to_owned())?;
    if tokens.is_empty() {
        return Err("搜狗当前没有配置按住型语音热键".to_owned());
    }
    let mut keys = Vec::with_capacity(tokens.len());
    for token in tokens {
        let text = token
            .as_str()
            .ok_or_else(|| "热键词元不是字符串".to_owned())?;
        keys.push(sogou_token_to_key(text).ok_or_else(|| format!("暂不识别的按键：{text}"))?);
    }
    KeyChord { keys }
        .validated()
        .map_err(|error| error.to_string())
}

fn sogou_token_to_key(token: &str) -> Option<KeyCode> {
    Some(match token {
        "LeftCtrl" => KeyCode::LeftControl,
        "RightCtrl" => KeyCode::RightControl,
        "LeftShift" => KeyCode::LeftShift,
        "RightShift" => KeyCode::RightShift,
        "LeftAlt" => KeyCode::LeftAlt,
        "RightAlt" => KeyCode::RightAlt,
        "LeftMeta" => KeyCode::LeftWindows,
        "RightMeta" => KeyCode::RightWindows,
        "Up" => KeyCode::Up,
        "Down" => KeyCode::Down,
        "Left" => KeyCode::Left,
        "Right" => KeyCode::Right,
        "Space" => KeyCode::Space,
        "Enter" => KeyCode::Enter,
        "Tab" => KeyCode::Tab,
        "Escape" => KeyCode::Escape,
        "Backspace" => KeyCode::Backspace,
        "Delete" => KeyCode::Delete,
        "Insert" => KeyCode::Insert,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        other => return named_key(other),
    })
}

/// 单个字母、数字或功能键的词元（"A" / "7" / "F8"）。
fn named_key(token: &str) -> Option<KeyCode> {
    let lower = token.to_ascii_lowercase();
    ALL_NAMED
        .iter()
        .copied()
        .find_map(|(name, key)| if name == lower { Some(key) } else { None })
}

const ALL_NAMED: &[(&str, KeyCode)] = &[
    ("a", KeyCode::A),
    ("b", KeyCode::B),
    ("c", KeyCode::C),
    ("d", KeyCode::D),
    ("e", KeyCode::E),
    ("f", KeyCode::F),
    ("g", KeyCode::G),
    ("h", KeyCode::H),
    ("i", KeyCode::I),
    ("j", KeyCode::J),
    ("k", KeyCode::K),
    ("l", KeyCode::L),
    ("m", KeyCode::M),
    ("n", KeyCode::N),
    ("o", KeyCode::O),
    ("p", KeyCode::P),
    ("q", KeyCode::Q),
    ("r", KeyCode::R),
    ("s", KeyCode::S),
    ("t", KeyCode::T),
    ("u", KeyCode::U),
    ("v", KeyCode::V),
    ("w", KeyCode::W),
    ("x", KeyCode::X),
    ("y", KeyCode::Y),
    ("z", KeyCode::Z),
    ("0", KeyCode::Digit0),
    ("1", KeyCode::Digit1),
    ("2", KeyCode::Digit2),
    ("3", KeyCode::Digit3),
    ("4", KeyCode::Digit4),
    ("5", KeyCode::Digit5),
    ("6", KeyCode::Digit6),
    ("7", KeyCode::Digit7),
    ("8", KeyCode::Digit8),
    ("9", KeyCode::Digit9),
    ("f1", KeyCode::F1),
    ("f2", KeyCode::F2),
    ("f3", KeyCode::F3),
    ("f4", KeyCode::F4),
    ("f5", KeyCode::F5),
    ("f6", KeyCode::F6),
    ("f7", KeyCode::F7),
    ("f8", KeyCode::F8),
    ("f9", KeyCode::F9),
    ("f10", KeyCode::F10),
    ("f11", KeyCode::F11),
    ("f12", KeyCode::F12),
];

/// 豆包把按住型热键写成 `voice.voiceLongPressShortcut`：
/// `{ "modifierFlags": <位标记>, "keyCode": <Windows 虚拟键码> }`。
/// 位标记分"族"（Ctrl/Shift/Alt/Win，不分左右）与"分边"两组，分边位存在时
/// 以分边为准。
pub fn parse_doubao(contents: &str) -> Result<KeyChord, String> {
    const SIDED: &[(u32, KeyCode)] = &[
        (0x0100, KeyCode::LeftControl),
        (0x0200, KeyCode::RightControl),
        (0x1000, KeyCode::LeftShift),
        (0x2000, KeyCode::RightShift),
        (0x0400, KeyCode::LeftAlt),
        (0x0800, KeyCode::RightAlt),
        (0x4000, KeyCode::LeftWindows),
        (0x8000, KeyCode::RightWindows),
    ];
    const FAMILY: &[(u32, KeyCode)] = &[
        (0x0002, KeyCode::Control),
        (0x0004, KeyCode::Shift),
        (0x0001, KeyCode::Alt),
        (0x0008, KeyCode::LeftWindows),
    ];
    const SIDED_MASK: u32 = 0xFF00;
    const FAMILY_MASK: u32 = 0x000F;

    let document: serde_json::Value = serde_json::from_str(contents.trim_start_matches('\u{feff}'))
        .map_err(|error| format!("配置不是合法 JSON：{error}"))?;
    let shortcut = document
        .get("voice")
        .and_then(|voice| voice.get("voiceLongPressShortcut"))
        .ok_or_else(|| "配置里没有 voice.voiceLongPressShortcut".to_owned())?;
    let flags = shortcut
        .get("modifierFlags")
        .and_then(|value| value.as_u64())
        .unwrap_or(0) as u32;
    if flags & !(SIDED_MASK | FAMILY_MASK) != 0 {
        return Err(format!("暂不识别的修饰键标记：{flags}"));
    }
    let mut keys: Vec<KeyCode> = Vec::new();
    if flags & SIDED_MASK != 0 {
        // 分边位存在时以分边为准，忽略同族的粗粒度位。
        for (bit, key) in SIDED {
            if flags & bit != 0 {
                keys.push(*key);
            }
        }
    } else {
        for (bit, key) in FAMILY {
            if flags & bit != 0 {
                keys.push(*key);
            }
        }
    }
    let key_code = shortcut
        .get("keyCode")
        .and_then(|value| value.as_u64())
        .unwrap_or(0) as u16;
    if key_code != 0 {
        let key = KeyCode::from_virtual_key(key_code)
            .ok_or_else(|| format!("暂不识别的虚拟键码：{key_code}"))?;
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    if keys.is_empty() {
        return Err("豆包当前没有配置按住型语音热键".to_owned());
    }
    KeyChord { keys }
        .validated()
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_sogou_hold_to_talk_shortcut() {
        let chord = parse_sogou(r#"{"setting":{"shortcutKeysPress":["RightCtrl"]}}"#).unwrap();
        assert_eq!(chord.keys, vec![KeyCode::RightControl]);

        let chord =
            parse_sogou(r#"{"setting":{"shortcutKeysPress":["LeftCtrl","LeftMeta"]}}"#).unwrap();
        assert_eq!(chord.keys, vec![KeyCode::LeftControl, KeyCode::LeftWindows]);

        // 字母与功能键词元。
        assert_eq!(
            parse_sogou(r#"{"setting":{"shortcutKeysPress":["LeftAlt","F8"]}}"#)
                .unwrap()
                .keys,
            vec![KeyCode::LeftAlt, KeyCode::F8]
        );
    }

    #[test]
    fn sogou_read_failures_say_why_instead_of_guessing() {
        assert!(parse_sogou("not json").unwrap_err().contains("JSON"));
        assert!(parse_sogou(r#"{"setting":{}}"#)
            .unwrap_err()
            .contains("shortcutKeysPress"));
        assert!(parse_sogou(r#"{"setting":{"shortcutKeysPress":[]}}"#)
            .unwrap_err()
            .contains("没有配置"));
        assert!(
            parse_sogou(r#"{"setting":{"shortcutKeysPress":["NumpadEnter"]}}"#)
                .unwrap_err()
                .contains("NumpadEnter")
        );
    }

    #[test]
    fn reads_the_doubao_hold_to_talk_shortcut() {
        // 分边位：右 Alt。
        let chord = parse_doubao(
            r#"{"voice":{"voiceLongPressShortcut":{"modifierFlags":2048,"keyCode":0}}}"#,
        )
        .unwrap();
        assert_eq!(chord.keys, vec![KeyCode::RightAlt]);

        // 族位 + 主键：Ctrl + Q。
        let chord = parse_doubao(
            r#"{"voice":{"voiceLongPressShortcut":{"modifierFlags":2,"keyCode":81}}}"#,
        )
        .unwrap();
        assert_eq!(chord.keys, vec![KeyCode::Control, KeyCode::Q]);

        // 分边位存在时忽略同族的粗粒度位（0x0800 | 0x0001 仍只取右 Alt）。
        let chord = parse_doubao(
            r#"{"voice":{"voiceLongPressShortcut":{"modifierFlags":2049,"keyCode":0}}}"#,
        )
        .unwrap();
        assert_eq!(chord.keys, vec![KeyCode::RightAlt]);
    }

    #[test]
    fn doubao_read_failures_say_why_instead_of_guessing() {
        assert!(parse_doubao(r#"{}"#)
            .unwrap_err()
            .contains("voiceLongPressShortcut"));
        assert!(parse_doubao(
            r#"{"voice":{"voiceLongPressShortcut":{"modifierFlags":0,"keyCode":0}}}"#
        )
        .unwrap_err()
        .contains("没有配置"));
        assert!(parse_doubao(
            r#"{"voice":{"voiceLongPressShortcut":{"modifierFlags":65536,"keyCode":0}}}"#
        )
        .unwrap_err()
        .contains("修饰键标记"));
    }

    #[test]
    fn missing_config_file_is_reported_as_not_found() {
        let missing = std::env::temp_dir().join("sayall-ime-hotkey-test-missing");
        assert_eq!(
            detect(ImeTool::Sogou, &missing),
            Err(DetectError::NotFound("搜狗语音输入"))
        );
    }

    #[test]
    fn config_paths_match_the_documented_locations() {
        assert_eq!(
            ImeTool::Sogou.config_relative_path(),
            PathBuf::from("sogou_voice_assistant_pc").join("config.json")
        );
        assert_eq!(
            ImeTool::Doubao.config_relative_path(),
            PathBuf::from("DoubaoIme").join("conf").join("config.json")
        );
    }

    #[test]
    fn detect_reads_a_real_file_from_the_given_appdata_root() {
        let root = std::env::temp_dir().join("sayall-ime-hotkey-test-root");
        let dir = root.join("sogou_voice_assistant_pc");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.json"),
            r#"{"setting":{"shortcutKeysPress":["RightCtrl"]}}"#,
        )
        .unwrap();
        assert_eq!(
            detect(ImeTool::Sogou, &root).unwrap().keys,
            vec![KeyCode::RightControl]
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
