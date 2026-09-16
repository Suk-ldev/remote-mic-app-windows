use crate::UsageStatistics;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VoiceTriggerMode {
    #[default]
    Hold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub schema_version: u32,
    pub selected_remote_id: Option<String>,
    pub audio_endpoint_id: Option<String>,
    pub audio_endpoint_name: Option<String>,
    pub gain_db: f32,
    /// 语音增强（高通 + AGC + 软限幅）。默认关闭，开启后由 AGC 接管电平。
    pub voice_enhance: bool,
    /// 按键映射注入的保持时长（毫秒）：DOWN 与 UP 之间的间隔。
    /// 轮询键盘状态的程序会丢掉零间隔的点按，目标应用漏识别时调高。
    pub injection_hold_ms: u32,
    /// 语音期间临时把系统默认录音设备切到虚拟声卡，松开还原。默认关闭：
    /// 走的是未公开 COM 接口，且会影响同时在录音的其它程序。
    pub borrow_default_capture: bool,
    pub voice_trigger_mode: VoiceTriggerMode,
    pub launch_at_login: bool,
    pub open_window_at_launch: bool,
    pub check_prerelease_updates: bool,
    pub theme_preference: ThemePreference,
    pub usage_statistics: UsageStatistics,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: 4,
            selected_remote_id: None,
            audio_endpoint_id: None,
            audio_endpoint_name: None,
            gain_db: 0.0,
            voice_enhance: false,
            injection_hold_ms: 30,
            borrow_default_capture: false,
            voice_trigger_mode: VoiceTriggerMode::Hold,
            launch_at_login: false,
            open_window_at_launch: true,
            check_prerelease_updates: false,
            theme_preference: ThemePreference::System,
            usage_statistics: UsageStatistics::default(),
        }
    }
}

impl AppSettings {
    pub fn voice_dsp(&self) -> crate::VoiceDspSettings {
        crate::VoiceDspSettings {
            gain_db: self.gain_db,
            enhance: self.voice_enhance,
        }
        .normalized()
    }

    pub fn normalized(mut self) -> Self {
        self.schema_version = Self::default().schema_version;
        self.gain_db = if self.gain_db.is_finite() {
            self.gain_db.clamp(0.0, 24.0)
        } else {
            0.0
        };
        self.injection_hold_ms = self.injection_hold_ms.min(1_000);
        self.usage_statistics = self.usage_statistics.normalized();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_gain_and_keeps_hold_as_only_voice_mode() {
        let settings = AppSettings {
            gain_db: 30.0,
            ..AppSettings::default()
        }
        .normalized();
        assert_eq!(settings.gain_db, 24.0);
        assert_eq!(settings.voice_trigger_mode, VoiceTriggerMode::Hold);
    }

    #[test]
    fn older_settings_without_endpoint_name_remain_compatible() {
        let settings: AppSettings = serde_json::from_str(
            r#"{"schema_version":1,"audio_endpoint_id":"endpoint-1","gain_db":0.0,"voice_trigger_mode":"hold","launch_at_login":false,"open_window_at_launch":true}"#,
        )
        .unwrap();
        let settings = settings.normalized();

        assert_eq!(settings.audio_endpoint_id.as_deref(), Some("endpoint-1"));
        assert_eq!(settings.audio_endpoint_name, None);
        assert_eq!(settings.schema_version, 4);
        assert!(!settings.check_prerelease_updates);
        assert_eq!(settings.theme_preference, ThemePreference::System);
        assert_eq!(settings.usage_statistics, UsageStatistics::default());
    }

    #[test]
    fn theme_preferences_round_trip() {
        for preference in [
            ThemePreference::System,
            ThemePreference::Light,
            ThemePreference::Dark,
        ] {
            let settings = AppSettings {
                theme_preference: preference,
                ..AppSettings::default()
            };
            let encoded = serde_json::to_string(&settings).unwrap();
            let decoded: AppSettings = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded.normalized().theme_preference, preference);
        }
    }
}
