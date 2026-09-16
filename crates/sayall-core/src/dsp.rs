//! 语音增强链：高通（兼去直流）→ AGC → 软限幅。
//!
//! 默认关闭，关闭时输出与固定增益路径逐样本一致——增强会改变现有用户
//! 的识别结果，先作为可选项发一版。
//!
//! 为什么需要高通：遥控器麦克风的低频漂移与隆隆声不携带语音信息，却会
//! 顶住 AGC 盯的峰值，把人声频段（300–3400 Hz）的增益压低。先切掉低频，
//! AGC 才盯得住人声。一阶高通同时消掉直流分量，不再单独做去直流。

use serde::{Deserialize, Serialize};

/// 遥控器语音链路的采样率（ATVV 能力协商固定 16 kHz）。
const SAMPLE_RATE: f32 = 16_000.0;
/// 高通截止频率。
const HIGH_PASS_CUTOFF_HZ: f32 = 100.0;
/// AGC 目标峰值（留 3 dB 余量给软限幅）。
const AGC_TARGET_PEAK: f32 = 0.7;
const AGC_MIN_GAIN: f32 = 0.5;
const AGC_MAX_GAIN: f32 = 8.0;
/// 低于该电平视为静音，不调整增益（避免在静段把底噪拉起来）。
const AGC_SILENCE_FLOOR: f32 = 0.002;
/// 峰值跟踪衰减（约 300 ms 时间常数）与增益滑动（约 50 ms）。
const AGC_PEAK_DECAY: f32 = 0.999_79;
const AGC_GAIN_GLIDE: f32 = 0.001_25;
/// 软限幅拐点：拐点以下线性，以上渐近到满刻度。
const SOFT_CLIP_KNEE: f32 = 0.9;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct VoiceDspSettings {
    /// 固定增益（dB）。语音增强开启时由 AGC 接管电平，该值不再生效。
    pub gain_db: f32,
    /// 语音增强：高通 + AGC + 软限幅。
    pub enhance: bool,
}

impl Default for VoiceDspSettings {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            enhance: false,
        }
    }
}

impl VoiceDspSettings {
    pub fn normalized(mut self) -> Self {
        self.gain_db = if self.gain_db.is_finite() {
            self.gain_db.clamp(0.0, 24.0)
        } else {
            0.0
        };
        self
    }
}

/// 一阶高通：y[n] = a * (y[n-1] + x[n] - x[n-1])。
#[derive(Debug, Clone, Copy)]
struct HighPass {
    alpha: f32,
    prev_in: f32,
    prev_out: f32,
}

impl HighPass {
    fn new(cutoff_hz: f32, sample_rate: f32) -> Self {
        let rc = 1.0 / (2.0 * std::f32::consts::PI * cutoff_hz);
        let dt = 1.0 / sample_rate;
        Self {
            alpha: rc / (rc + dt),
            prev_in: 0.0,
            prev_out: 0.0,
        }
    }

    fn reset(&mut self) {
        self.prev_in = 0.0;
        self.prev_out = 0.0;
    }

    fn process(&mut self, sample: f32) -> f32 {
        let out = self.alpha * (self.prev_out + sample - self.prev_in);
        self.prev_in = sample;
        self.prev_out = out;
        out
    }
}

/// 峰值跟踪自动增益：向目标电平靠拢，带上下限与静音门限。
#[derive(Debug, Clone, Copy)]
struct Agc {
    gain: f32,
    peak: f32,
}

impl Agc {
    fn new() -> Self {
        Self {
            gain: 1.0,
            peak: 0.0,
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    fn process(&mut self, sample: f32) -> f32 {
        let magnitude = sample.abs();
        self.peak = if magnitude > self.peak {
            magnitude
        } else {
            self.peak * AGC_PEAK_DECAY
        };
        if self.peak > AGC_SILENCE_FLOOR {
            let want = (AGC_TARGET_PEAK / self.peak).clamp(AGC_MIN_GAIN, AGC_MAX_GAIN);
            self.gain += (want - self.gain) * AGC_GAIN_GLIDE;
        }
        sample * self.gain
    }
}

fn soft_clip(sample: f32) -> f32 {
    let magnitude = sample.abs();
    if magnitude <= SOFT_CLIP_KNEE {
        return sample;
    }
    let over = magnitude - SOFT_CLIP_KNEE;
    let headroom = 1.0 - SOFT_CLIP_KNEE;
    sample.signum() * (SOFT_CLIP_KNEE + headroom * (over / (over + headroom)))
}

/// 三点平滑 [1 2 1]/4：ADPCM 解码后的量化噪声抑制，增强开关无关，
/// 始终生效（与 [`crate::process_pcm`] 一致）。
fn smooth(input: &[i16]) -> Vec<i32> {
    let mut filtered: Vec<i32> = input.iter().map(|sample| i32::from(*sample)).collect();
    if input.len() >= 3 {
        for index in 1..(input.len() - 1) {
            filtered[index] = (i32::from(input[index - 1])
                + 2 * i32::from(input[index])
                + i32::from(input[index + 1]))
                >> 2;
        }
    }
    filtered
}

/// 有状态的语音处理链。滤波器状态跨帧保留，每次语音会话开始调用
/// [`VoiceDsp::reset`]：不重置会把上一段话的尾巴带进新会话的首帧。
#[derive(Debug, Clone)]
pub struct VoiceDsp {
    settings: VoiceDspSettings,
    high_pass: HighPass,
    agc: Agc,
}

impl Default for VoiceDsp {
    fn default() -> Self {
        Self::new(VoiceDspSettings::default())
    }
}

impl VoiceDsp {
    pub fn new(settings: VoiceDspSettings) -> Self {
        Self {
            settings: settings.normalized(),
            high_pass: HighPass::new(HIGH_PASS_CUTOFF_HZ, SAMPLE_RATE),
            agc: Agc::new(),
        }
    }

    pub fn settings(&self) -> VoiceDspSettings {
        self.settings
    }

    /// 改配置即重置滤波器状态：增强开关切换时链路结构变了，旧状态无意义。
    pub fn set_settings(&mut self, settings: VoiceDspSettings) {
        self.settings = settings.normalized();
        self.reset();
    }

    pub fn reset(&mut self) {
        self.high_pass.reset();
        self.agc.reset();
    }

    pub fn process(&mut self, input: &[i16]) -> Vec<i16> {
        if input.is_empty() {
            return Vec::new();
        }
        let filtered = smooth(input);
        if !self.settings.enhance {
            let gain = 10_f32.powf(self.settings.gain_db / 20.0);
            return filtered
                .into_iter()
                .map(|sample| clamp_to_i16((sample as f32 * gain).round() as i32))
                .collect();
        }
        filtered
            .into_iter()
            .map(|sample| {
                let normalized = sample as f32 / 32768.0;
                let processed = soft_clip(self.agc.process(self.high_pass.process(normalized)));
                clamp_to_i16((processed * 32768.0).round() as i32)
            })
            .collect()
    }
}

fn clamp_to_i16(sample: i32) -> i16 {
    sample.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_enhancement_matches_the_fixed_gain_path() {
        let mut dsp = VoiceDsp::default();
        let input: Vec<i16> = (0..64).map(|index| (index * 400 - 12000) as i16).collect();
        assert_eq!(dsp.process(&input), crate::process_pcm(&input, 0.0));

        let mut dsp = VoiceDsp::new(VoiceDspSettings {
            gain_db: 6.0,
            enhance: false,
        });
        assert_eq!(dsp.process(&input), crate::process_pcm(&input, 6.0));
    }

    #[test]
    fn enhancement_removes_a_constant_offset() {
        let mut dsp = VoiceDsp::new(VoiceDspSettings {
            gain_db: 0.0,
            enhance: true,
        });
        // 纯直流输入：高通稳态输出应趋近于零。
        let input = vec![8000_i16; 16_000];
        let output = dsp.process(&input);
        let tail_peak = output[output.len() - 1000..]
            .iter()
            .map(|sample| sample.unsigned_abs())
            .max()
            .unwrap();
        assert!(
            tail_peak < 200,
            "直流应被高通滤掉，实测尾部峰值 {tail_peak}"
        );
    }

    #[test]
    fn agc_converges_to_the_target_level() {
        // 1 秒 400 Hz 正弦，峰值为满刻度的 15%：目标增益 4.7x，在上限内。
        let output = process_sine(0.15);
        let tail_peak = peak(&output[output.len() - 1_000..]);
        assert!(
            (tail_peak - AGC_TARGET_PEAK).abs() < 0.1,
            "稳态应收敛到目标电平 {AGC_TARGET_PEAK}，实测 {tail_peak:.3}"
        );
    }

    #[test]
    fn agc_gain_is_capped_so_very_quiet_input_is_not_over_amplified() {
        // 3% 峰值：目标增益 23x 超过上限，输出应停在 上限 × 输入 附近。
        let output = process_sine(0.03);
        let tail_peak = peak(&output[output.len() - 1_000..]);
        assert!(
            tail_peak < 0.03 * AGC_MAX_GAIN + 0.05,
            "增益上限未生效，实测 {tail_peak:.3}"
        );
        assert!(
            tail_peak > 0.03 * 2.0,
            "弱信号仍应被抬起，实测 {tail_peak:.3}"
        );
    }

    fn process_sine(amplitude: f32) -> Vec<i16> {
        let mut dsp = VoiceDsp::new(VoiceDspSettings {
            gain_db: 0.0,
            enhance: true,
        });
        let input: Vec<i16> = (0..16_000)
            .map(|index| {
                let phase = 2.0 * std::f32::consts::PI * 400.0 * index as f32 / SAMPLE_RATE;
                (phase.sin() * amplitude * 32767.0) as i16
            })
            .collect();
        dsp.process(&input)
    }

    #[test]
    fn soft_clip_never_exceeds_full_scale_and_is_continuous_at_the_knee() {
        assert_eq!(soft_clip(0.5), 0.5);
        assert!((soft_clip(SOFT_CLIP_KNEE) - SOFT_CLIP_KNEE).abs() < 1e-6);
        for value in [1.0_f32, 2.0, 12.0, -1.0, -8.0] {
            assert!(soft_clip(value).abs() < 1.0, "{value} 被限幅后仍超幅");
            assert_eq!(soft_clip(value).signum(), value.signum());
        }
    }

    #[test]
    fn loud_input_stays_within_full_scale() {
        let mut dsp = VoiceDsp::new(VoiceDspSettings {
            gain_db: 0.0,
            enhance: true,
        });
        let input: Vec<i16> = (0..16_000)
            .map(|index| if index % 2 == 0 { i16::MAX } else { i16::MIN })
            .collect();
        assert!(dsp.process(&input).iter().all(|sample| *sample > i16::MIN));
    }

    #[test]
    fn reset_clears_filter_state_between_sessions() {
        let mut dsp = VoiceDsp::new(VoiceDspSettings {
            gain_db: 0.0,
            enhance: true,
        });
        let input = vec![6000_i16; 4_000];
        let first = dsp.process(&input);
        dsp.reset();
        let second = dsp.process(&input);
        assert_eq!(first, second, "重置后同一段输入应得到同样的输出");
    }

    #[test]
    fn settings_are_clamped_on_construction() {
        let dsp = VoiceDsp::new(VoiceDspSettings {
            gain_db: 99.0,
            enhance: false,
        });
        assert_eq!(dsp.settings().gain_db, 24.0);
        let dsp = VoiceDsp::new(VoiceDspSettings {
            gain_db: f32::NAN,
            enhance: false,
        });
        assert_eq!(dsp.settings().gain_db, 0.0);
    }

    fn peak(samples: &[i16]) -> f32 {
        samples
            .iter()
            .map(|sample| (*sample as f32 / 32768.0).abs())
            .fold(0.0_f32, f32::max)
    }
}
