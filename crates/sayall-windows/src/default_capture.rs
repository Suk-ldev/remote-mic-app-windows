//! 语音期间临时把系统默认录音设备切到虚拟声卡，松开还原。
//!
//! 为什么要有：不开这个功能时，用户必须进输入法的语音设置把麦克风改成
//! `CABLE Output`，这是装机流程里最容易漏的一步。开启后按住语音键期间由本应用
//! 临时接管默认录音设备，松开立刻还原——参考项目 QL-4/RemoteMapper 的
//! "用完即还"。
//!
//! ⚠ **默认关闭，且是本项目里唯一走未公开 COM 接口的地方。**
//! `IPolicyConfig`（CLSID 870af99c-…，IID f8679f50-…）没有公开文档，
//! `SetDefaultEndpoint` 在虚拟表里的位置来自 RemoteMapper 的实现（IUnknown 三项
//! 之后第 11 个方法）。微软从未承诺该布局跨 Windows 版本稳定。
//!
//! 因此这里的护栏比别处更厚：
//! - 默认关闭，用户需要在界面上显式开启；
//! - 接口创建或调用失败一律如实返回错误，不静默重试；
//! - 切换前记住原设备，语音结束、断连、睡眠、退出四条路径都还原；
//! - 启动时自检：默认录音设备如果还停在虚拟声卡上，说明上次没还原干净
//!   （多半是崩溃），主动提示并帮用户还原。
//!
//! 真机验收前不要把该开关作为推荐配置对外发布。

#[cfg(windows)]
mod windows_impl {
    use std::ffi::c_void;
    use std::sync::Mutex;

    use wasapi::{DeviceEnumerator, Direction};
    use windows::core::{GUID, HRESULT, PCWSTR};
    use windows::Win32::Media::Audio::{eCommunications, eConsole, eMultimedia, ERole};
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};

    /// `CPolicyConfigClient`，未公开。
    const POLICY_CONFIG_CLSID: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
    /// `IPolicyConfig`，未公开。
    const POLICY_CONFIG_IID: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);

    /// `IPolicyConfig` 的虚拟表：IUnknown 三项 + 10 个本项目不使用的方法，
    /// `SetDefaultEndpoint` 排在其后。占位项声明成裸指针而不是函数指针，
    /// 避免不小心调用到。
    #[repr(C)]
    struct PolicyConfigVtbl {
        query_interface:
            unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
        add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
        _reserved: [*const c_void; 10],
        set_default_endpoint: unsafe extern "system" fn(*mut c_void, PCWSTR, ERole) -> HRESULT,
    }

    /// 持有一个 `IPolicyConfig` 实例，Drop 时 Release。
    struct PolicyConfig(*mut c_void);

    // COM 接口指针只在本模块的互斥区内使用，跨线程搬运是安全的。
    unsafe impl Send for PolicyConfig {}

    impl PolicyConfig {
        fn create() -> Result<Self, String> {
            let unknown: windows::core::IUnknown =
                unsafe { CoCreateInstance(&POLICY_CONFIG_CLSID, None, CLSCTX_ALL) }
                    .map_err(|error| format!("创建默认设备策略接口失败：{error}"))?;
            let mut raw: *mut c_void = std::ptr::null_mut();
            let hr =
                unsafe { windows::core::Interface::query(&unknown, &POLICY_CONFIG_IID, &mut raw) };
            if hr.is_err() || raw.is_null() {
                return Err(format!(
                    "当前 Windows 不支持默认设备策略接口（{hr:?}）；请关闭".to_owned()
                        + "「语音期间临时切换默认麦克风」"
                ));
            }
            Ok(Self(raw))
        }

        fn vtable(&self) -> &PolicyConfigVtbl {
            unsafe { &**(self.0 as *mut *mut PolicyConfigVtbl) }
        }

        fn set_default(&self, endpoint_id: &str) -> Result<(), String> {
            let wide: Vec<u16> = endpoint_id
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            // 三个角色都要设：只设 eConsole 时通讯类应用仍走旧设备。
            for role in [eConsole, eMultimedia, eCommunications] {
                let hr = unsafe {
                    (self.vtable().set_default_endpoint)(self.0, PCWSTR(wide.as_ptr()), role)
                };
                if hr.is_err() {
                    return Err(format!("设置默认录音设备失败（{hr:?}）"));
                }
            }
            Ok(())
        }
    }

    impl Drop for PolicyConfig {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { (self.vtable().release)(self.0) };
                self.0 = std::ptr::null_mut();
            }
        }
    }

    /// 当前默认录音设备的端点 id。读设备走 wasapi（与 audio.rs 同一套），
    /// 只有写默认设备不得不用未公开接口。
    pub fn current_default_capture() -> Result<String, String> {
        DeviceEnumerator::new()
            .map_err(|error| format!("枚举音频设备失败：{error}"))?
            .get_default_device(&Direction::Capture)
            .map_err(|error| format!("读取默认录音设备失败：{error}"))?
            .get_id()
            .map_err(|error| format!("读取默认录音设备标识失败：{error}"))
    }

    /// 默认录音设备的显示名，用于启动自检提示。
    pub fn current_default_capture_name() -> Option<String> {
        DeviceEnumerator::new()
            .ok()?
            .get_default_device(&Direction::Capture)
            .ok()?
            .get_friendlyname()
            .ok()
    }

    /// 虚拟声卡的**录音**端点 id（CABLE Output）。用户选的是播放端
    /// （CABLE Input），这里要的是它的另一半。
    pub fn find_virtual_cable_capture_id() -> Option<String> {
        let collection = wasapi::DeviceCollection::new(&Direction::Capture).ok()?;
        for device in &collection {
            let device = device.ok()?;
            let name = device.get_friendlyname().ok()?;
            if crate::is_virtual_cable_output_name(&name) {
                return device.get_id().ok();
            }
        }
        None
    }

    static SAVED_DEFAULT: Mutex<Option<String>> = Mutex::new(None);

    /// 切到目标端点并记住原设备。已经处于借用状态时不重复记录。
    pub fn borrow(target_endpoint_id: &str) -> Result<(), String> {
        let mut saved = SAVED_DEFAULT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if saved.is_some() {
            return Ok(());
        }
        let previous = current_default_capture()?;
        if previous == target_endpoint_id {
            // 已经是目标设备：不记录，也就不会在还原时乱改。
            return Ok(());
        }
        PolicyConfig::create()?.set_default(target_endpoint_id)?;
        *saved = Some(previous);
        Ok(())
    }

    /// 还原到借用前的设备。没有借用记录时是空操作。
    pub fn restore() -> Result<(), String> {
        let mut saved = SAVED_DEFAULT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(previous) = saved.take() else {
            return Ok(());
        };
        PolicyConfig::create()?.set_default(&previous)
    }

    pub fn borrowed() -> bool {
        SAVED_DEFAULT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
    }
}

#[cfg(windows)]
pub use windows_impl::{
    borrow, borrowed, current_default_capture, current_default_capture_name,
    find_virtual_cable_capture_id, restore,
};

#[cfg(not(windows))]
pub fn find_virtual_cable_capture_id() -> Option<String> {
    None
}

use std::sync::atomic::{AtomicBool, Ordering};

/// 功能开关。默认关闭——这是本项目唯一走未公开 COM 接口的地方，
/// 真机验收前不要作为推荐配置。
static ENABLED: AtomicBool = AtomicBool::new(false);

pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
    if !enabled {
        // 关掉时立刻还原，不留借用状态。
        let _ = restore();
    }
}

pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// 语音会话开始时调用：开关关闭、找不到虚拟声卡录音端点或切换失败都
/// 静默跳过——语音本身照常工作，只是用户得自己在输入法里选麦克风。
pub fn borrow_for_voice() -> Option<String> {
    if !is_enabled() {
        return None;
    }
    let target = find_virtual_cable_capture_id()?;
    borrow(&target).err()
}

#[cfg(not(windows))]
pub fn borrow(_target_endpoint_id: &str) -> Result<(), String> {
    Err("仅 Windows 支持切换默认录音设备".to_owned())
}

#[cfg(not(windows))]
pub fn restore() -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn borrowed() -> bool {
    false
}

#[cfg(not(windows))]
pub fn current_default_capture() -> Result<String, String> {
    Err("仅 Windows 支持读取默认录音设备".to_owned())
}

#[cfg(not(windows))]
pub fn current_default_capture_name() -> Option<String> {
    None
}

/// 自检：默认录音设备名看起来像虚拟声卡时，说明上次没还原干净。
/// 名称匹配复用音频端点那套判定，与"推荐端点"的口径一致。
pub fn looks_like_stale_borrow(default_capture_name: &str) -> bool {
    crate::is_virtual_cable_output_name(default_capture_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_detection_matches_the_virtual_cable_naming_rule() {
        assert!(looks_like_stale_borrow(
            "CABLE Output (VB-Audio Virtual Cable)"
        ));
        assert!(!looks_like_stale_borrow("麦克风 (Realtek Audio)"));
    }
}
