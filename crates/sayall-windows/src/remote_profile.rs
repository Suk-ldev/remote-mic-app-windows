//! 遥控器机型档案：把"哪个设备、哪些按键、HID usage 怎么解"从散落的常量
//! 收成一张表，让本项目能同时支持不同厂商的遥控器。
//!
//! 已收录：
//! - 小米蓝牙遥控器 2 / 2 Pro（RC001 / RC003）：VID 0x2717 PID 0x32B8。
//!   报文用的是 HID **键盘页** usage（0x0028 Enter、0x0052 上 等），
//!   这张表由本项目真机采集，已验收。
//! - Google TV / Chromecast 语音遥控器：VID 0x18D1 PID 0x9450。报文用的是
//!   HID **消费者页** usage（0x0041 Menu Pick、0x0223 AC Home 等），数据来自
//!   参考项目 Tilkmilk/vibe-mote 的受控采集表。
//!
//! ⚠ Google 档案尚未在本项目真机验收（`verified: false`）：VID/PID、
//! usage 表与 Raw Input 报文形状都需要接上真机后核对，核对前不要把它当
//! 已支持机型对外宣传。两个厂商的 usage 分属不同 HID 页、数值会撞车
//! （0x0041 在键盘页是 F8、在消费者页是 Menu Pick），所以 usage 必须按
//! 连接中的机型档案解，不能用一张全局表。

use crate::raw_input::{normalize_device_path, RemoteButton};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteProfile {
    pub id: &'static str,
    pub name: &'static str,
    pub vendor_id: u16,
    pub product_id: u16,
    /// 是否已在真机验收。false = 表数据来自外部来源，未经本项目确认。
    pub verified: bool,
    /// 该机型实际存在的按键（UI 只显示这些）。
    pub buttons: &'static [RemoteButton],
    usages: &'static [(u16, RemoteButton)],
}

impl RemoteProfile {
    pub fn button_for_usage(&self, usage: u16) -> Option<RemoteButton> {
        self.usages
            .iter()
            .find(|(candidate, _)| *candidate == usage)
            .map(|(_, button)| *button)
    }

    pub fn has_button(&self, button: RemoteButton) -> bool {
        self.buttons.contains(&button)
    }

    /// Raw Input 设备路径归属判定。经典 HID 路径写作 `vid_xxxx&pid_xxxx`，
    /// BLE HID 路径写作 `dev_vid&00xxxx ... pid&xxxx`，两种都要认。
    pub fn device_path_matches(&self, path: &str) -> bool {
        let normalized = normalize_device_path(path);
        let vid = format!("{:04x}", self.vendor_id);
        let pid = format!("{:04x}", self.product_id);
        let classic = normalized.contains(&format!("vid_{vid}"))
            && normalized.contains(&format!("pid_{pid}"));
        let ble = (normalized.contains(&format!("dev_vid&00{vid}"))
            || normalized.contains(&format!("dev_vid&01{vid}")))
            && normalized.contains(&format!("pid&{pid}"));
        classic || ble
    }
}

pub const XIAOMI: RemoteProfile = RemoteProfile {
    id: "xiaomi",
    name: "小米蓝牙遥控器 2 / 2 Pro",
    vendor_id: 0x2717,
    product_id: 0x32B8,
    verified: true,
    buttons: &crate::raw_input::ALL_BUTTONS_XIAOMI,
    usages: &[
        (0x00F1, RemoteButton::Back),
        (0x0028, RemoteButton::Ok),
        (0x0035, RemoteButton::Tv),
        (0x004A, RemoteButton::Home),
        (0x004F, RemoteButton::Right),
        (0x0050, RemoteButton::Left),
        (0x0051, RemoteButton::Down),
        (0x0052, RemoteButton::Up),
        (0x0065, RemoteButton::Menu),
        (0x0066, RemoteButton::Power),
        (0x007F, RemoteButton::VolumeMute),
        (0x0080, RemoteButton::VolumeUp),
        (0x0081, RemoteButton::VolumeDown),
    ],
};

/// Google TV / Chromecast 语音遥控器。usage 表来源：vibe-mote `keys.py` 的
/// `USAGE_TO_KEY`（该项目注明"受控采集，14/14 全命中"）。
/// 输入源键映射到本项目的 TV 位（两者都是"切信号源"语义）。
pub const GOOGLE_TV: RemoteProfile = RemoteProfile {
    id: "google_tv",
    name: "Google TV / Chromecast 语音遥控器",
    vendor_id: 0x18D1,
    product_id: 0x9450,
    verified: false,
    buttons: &[
        RemoteButton::Back,
        RemoteButton::Ok,
        RemoteButton::Tv,
        RemoteButton::Home,
        RemoteButton::Right,
        RemoteButton::Left,
        RemoteButton::Down,
        RemoteButton::Up,
        RemoteButton::Power,
        RemoteButton::VolumeMute,
        RemoteButton::VolumeUp,
        RemoteButton::VolumeDown,
        RemoteButton::Youtube,
        RemoteButton::Netflix,
    ],
    usages: &[
        (0x019E, RemoteButton::Power),
        (0x0189, RemoteButton::Tv),
        (0x0042, RemoteButton::Up),
        (0x0043, RemoteButton::Down),
        (0x0044, RemoteButton::Left),
        (0x0045, RemoteButton::Right),
        (0x0041, RemoteButton::Ok),
        (0x0224, RemoteButton::Back),
        (0x0223, RemoteButton::Home),
        (0x00E9, RemoteButton::VolumeUp),
        (0x00EA, RemoteButton::VolumeDown),
        (0x00E2, RemoteButton::VolumeMute),
        (0x0077, RemoteButton::Youtube),
        (0x0078, RemoteButton::Netflix),
    ],
};

pub const PROFILES: &[&RemoteProfile] = &[&XIAOMI, &GOOGLE_TV];

/// 默认档案：未识别出机型时沿用小米（本项目的既有行为）。
pub const DEFAULT_PROFILE: &RemoteProfile = &XIAOMI;

pub fn profile_by_id(id: &str) -> Option<&'static RemoteProfile> {
    PROFILES.iter().copied().find(|profile| profile.id == id)
}

/// 按 Raw Input 设备路径判定机型。
pub fn profile_for_device_path(path: &str) -> Option<&'static RemoteProfile> {
    PROFILES
        .iter()
        .copied()
        .find(|profile| profile.device_path_matches(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xiaomi_usage_table_matches_the_shipped_decoder() {
        // 重构护栏：档案表必须与原有的全局解码表逐条一致。
        for usage in 0u16..=0x00FF {
            assert_eq!(
                XIAOMI.button_for_usage(usage),
                crate::raw_input::button_for_usage(usage),
                "usage {usage:#06x} 的解码结果与既有实现不一致"
            );
        }
    }

    #[test]
    fn profiles_have_unique_ids_and_no_duplicate_usages() {
        for profile in PROFILES {
            let mut seen: Vec<u16> = profile.usages.iter().map(|(usage, _)| *usage).collect();
            let count = seen.len();
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(count, seen.len(), "{} 的 usage 表有重复", profile.id);
            for (_, button) in profile.usages {
                assert!(
                    profile.has_button(*button),
                    "{} 的 usage 表映射到了不属于该机型的按键 {button:?}",
                    profile.id
                );
            }
        }
        assert_eq!(profile_by_id("google_tv"), Some(&GOOGLE_TV));
        assert_eq!(profile_by_id("nope"), None);
    }

    #[test]
    fn device_paths_resolve_to_the_right_profile() {
        assert_eq!(
            profile_for_device_path(r"\\?\HID#VID_2717&PID_32B8&MI_01#7&x#{cls}"),
            Some(&XIAOMI)
        );
        assert_eq!(
            profile_for_device_path(
                r"\\?\HID#{00001812-0000-1000-8000-00805f9b34fb}_DEV_VID&0018d1_PID&9450_REV&0001"
            ),
            Some(&GOOGLE_TV)
        );
        assert_eq!(profile_for_device_path(r"\\?\HID#VID_046D&PID_C52B"), None);
    }

    #[test]
    fn the_same_usage_decodes_differently_per_profile() {
        // 0x0041：键盘页是 F8（小米表里没有），消费者页是 Menu Pick。
        assert_eq!(XIAOMI.button_for_usage(0x0041), None);
        assert_eq!(GOOGLE_TV.button_for_usage(0x0041), Some(RemoteButton::Ok));
        // 0x0052：键盘页是方向上，消费者页不在 Google 表里。
        assert_eq!(XIAOMI.button_for_usage(0x0052), Some(RemoteButton::Up));
        assert_eq!(GOOGLE_TV.button_for_usage(0x0052), None);
    }

    #[test]
    fn google_profile_is_marked_unverified_until_hardware_confirms_it() {
        assert!(XIAOMI.verified);
        assert!(
            !GOOGLE_TV.verified,
            "Google 档案未经真机验收前必须保持 verified=false"
        );
    }
}
