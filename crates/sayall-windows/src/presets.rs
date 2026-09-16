//! 按键映射预设方案：一次套用整套键位，替代逐键点 13 次。
//!
//! 参考 vibe-mote 的"目标场景 → 一整套映射"机制；具体键位按 Windows 上
//! 普遍成立的约定挑选，每条的依据写在对应行的注释里。三套方案的取舍：
//! 只用目标场景里跨程序都成立的键，不绑定单个应用的私有快捷键。
//!
//! 套用是整体替换（未列出的按键回到未配置），与"恢复默认"的区别在于
//! 后者把所有按键清空、前者给出一套可直接用的配置。

use std::collections::BTreeMap;

use serde::Serialize;

use crate::raw_input::RemoteButton;
use crate::send_input::{
    ButtonAction, ButtonActions, ButtonMappings, KeyChord, KeyCode, MouseAction,
};

/// 预设方案目录项（UI 选择器用）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MappingPresetInfo {
    pub id: String,
    pub name: String,
    pub note: String,
}

/// 推荐方案：恢复推荐配置与首次引导都用它。
pub const RECOMMENDED_PRESET: &str = "generic";

fn key(code: KeyCode) -> ButtonAction {
    ButtonAction::Shortcut {
        chord: KeyChord { keys: vec![code] },
    }
}

fn combo(keys: &[KeyCode]) -> ButtonAction {
    ButtonAction::Shortcut {
        chord: KeyChord {
            keys: keys.to_vec(),
        },
    }
}

fn wheel(kind: MouseAction) -> ButtonAction {
    ButtonAction::Mouse { kind }
}

fn preset_actions(id: &str) -> Option<Vec<(RemoteButton, ButtonAction)>> {
    let actions = match id {
        // 通用：只用各程序都常见的键，不依赖具体 App。
        "generic" => vec![
            (RemoteButton::Ok, key(KeyCode::Enter)),
            (RemoteButton::Back, key(KeyCode::Escape)),
            (RemoteButton::Up, wheel(MouseAction::WheelUp)),
            (RemoteButton::Down, wheel(MouseAction::WheelDown)),
            // 上/下一个标签页：浏览器、编辑器、终端普遍成立。
            (
                RemoteButton::Left,
                combo(&[KeyCode::LeftControl, KeyCode::PageUp]),
            ),
            (
                RemoteButton::Right,
                combo(&[KeyCode::LeftControl, KeyCode::PageDown]),
            ),
            (RemoteButton::Home, ButtonAction::Native),
            (RemoteButton::Menu, ButtonAction::Native),
            (RemoteButton::Power, key(KeyCode::F11)),
            (RemoteButton::VolumeMute, ButtonAction::Native),
            (RemoteButton::VolumeUp, ButtonAction::Native),
            (RemoteButton::VolumeDown, ButtonAction::Native),
        ],
        // 阅读：滚动与翻页为主，前进/后退沿用 Windows 通用的 Alt+方向键。
        "reading" => vec![
            (RemoteButton::Ok, key(KeyCode::Space)),
            (RemoteButton::Back, key(KeyCode::Escape)),
            (RemoteButton::Up, wheel(MouseAction::WheelUp)),
            (RemoteButton::Down, wheel(MouseAction::WheelDown)),
            (
                RemoteButton::Left,
                combo(&[KeyCode::LeftAlt, KeyCode::Left]),
            ),
            (
                RemoteButton::Right,
                combo(&[KeyCode::LeftAlt, KeyCode::Right]),
            ),
            (
                RemoteButton::Home,
                combo(&[KeyCode::LeftControl, KeyCode::Home]),
            ),
            (RemoteButton::Menu, ButtonAction::Native),
            (RemoteButton::Power, key(KeyCode::F11)),
            (RemoteButton::VolumeMute, ButtonAction::Native),
            (RemoteButton::VolumeUp, ButtonAction::Native),
            (RemoteButton::VolumeDown, ButtonAction::Native),
        ],
        // 影音：空格播放/暂停、方向键快退快进走原生；F 全屏、M 静音是
        // YouTube / 哔哩哔哩等网页播放器的通用键位。
        "media" => vec![
            (RemoteButton::Ok, key(KeyCode::Space)),
            (RemoteButton::Back, key(KeyCode::Escape)),
            (RemoteButton::Up, ButtonAction::Native),
            (RemoteButton::Down, ButtonAction::Native),
            (RemoteButton::Left, ButtonAction::Native),
            (RemoteButton::Right, ButtonAction::Native),
            (RemoteButton::Home, key(KeyCode::F)),
            (RemoteButton::Menu, ButtonAction::Native),
            (RemoteButton::Power, key(KeyCode::M)),
            (RemoteButton::VolumeMute, ButtonAction::Native),
            (RemoteButton::VolumeUp, ButtonAction::Native),
            (RemoteButton::VolumeDown, ButtonAction::Native),
        ],
        _ => return None,
    };
    Some(actions)
}

pub fn catalog() -> Vec<MappingPresetInfo> {
    [
        (
            "generic",
            "通用（浏览器 / 任意程序）",
            "上下滚轮、左右切标签页、确定回车、返回 Esc、电源全屏",
        ),
        (
            "reading",
            "阅读（网页 / 文档）",
            "上下滚轮、确定空格翻页、左右前进后退、主页回到顶部",
        ),
        (
            "media",
            "影音（播放器 / 视频网站）",
            "确定播放暂停、方向键快退快进、主页全屏、电源静音",
        ),
    ]
    .into_iter()
    .map(|(id, name, note)| MappingPresetInfo {
        id: id.to_owned(),
        name: name.to_owned(),
        note: note.to_owned(),
    })
    .collect()
}

/// 按预设 id 生成整套映射。`enabled` 沿用当前配置的总开关，套用预设
/// 不会顺手把用户关掉的自定义按键功能打开。
pub fn build(id: &str, enabled: bool) -> Option<ButtonMappings> {
    let actions: BTreeMap<RemoteButton, ButtonActions> = preset_actions(id)?
        .into_iter()
        .map(|(button, action)| (button, ButtonActions::single(action)))
        .collect();
    Some(ButtonMappings { enabled, actions })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalog_entry_builds_and_normalizes() {
        for info in catalog() {
            let mappings = build(&info.id, true).expect("目录里的方案必须可构建");
            let normalized = mappings
                .clone()
                .normalized()
                .expect("预设必须通过和弦校验与归一化");
            assert_eq!(
                normalized, mappings,
                "{} 的预设不该被归一化改写（说明键位配置不合法）",
                info.id
            );
        }
    }

    #[test]
    fn presets_never_assign_native_to_buttons_without_one() {
        // 返回/电源/TV 没有原生键，配 Native 会在归一化时被降级为未配置。
        for info in catalog() {
            for (button, action) in preset_actions(&info.id).unwrap() {
                if action == ButtonAction::Native {
                    assert!(
                        crate::send_input::native_key(button).is_some(),
                        "{}：{:?} 没有原生键，不能配原生透传",
                        info.id,
                        button
                    );
                }
            }
        }
    }

    #[test]
    fn unknown_preset_id_is_rejected() {
        assert!(build("nope", true).is_none());
        assert!(catalog().iter().any(|info| info.id == RECOMMENDED_PRESET));
    }

    #[test]
    fn build_keeps_the_enabled_flag() {
        assert!(!build(RECOMMENDED_PRESET, false).unwrap().enabled);
        assert!(build(RECOMMENDED_PRESET, true).unwrap().enabled);
    }
}
