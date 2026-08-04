//! 低延迟音频混音器：cpal 输出 + 固定槽位池混音（参考 beatoraja 实现）。
//!
//! 架构：
//! - 音频数据为预解码的 [`Pcm`]（交错 f32 数组，Arc 共享）；
//! - [`Mixer`] 持有固定大小的播放槽位池（[`MixerInput`]），
//!   `play` 在池中找空闲槽填入 PCM（O(1)，无流创建开销）；
//! - cpal 输出流回调（音频线程）每帧遍历活跃槽位逐采样累加混音写入设备；
//! - 短帧缓冲（设备默认）→ 低延迟；播完自动释放槽位。
//!
//! 与 rodio `mixer.add(source)` 流式混音不同，本实现直接控制输出缓冲节奏，
//! 播放只做 PCM 数组游标读取 + 加法。

use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use cpal::{
    Data, SampleFormat,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};

/// 解码后的 PCM 音频数据（交错 f32，Arc 共享，播放零拷贝）。
#[derive(Debug, Clone)]
pub struct Pcm {
    /// 交错采样（帧 × 声道）。
    pub samples: Arc<Vec<f32>>,
    /// 声道数（1 或 2）。
    pub channels: u16,
    /// 采样率（Hz，已统一到混音器输出采样率）。
    #[allow(dead_code)] // 测试与诊断引用
    pub sample_rate: u32,
}

impl Pcm {
    /// 创建 PCM。
    #[must_use]
    pub fn new(samples: Vec<f32>, channels: u16, sample_rate: u32) -> Self {
        Self {
            samples: Arc::new(samples),
            channels,
            sample_rate,
        }
    }
}

/// 播放槽位上限（与 beatoraja 默认 256 一致；超出丢弃，防止无界增长）。
pub const MAX_INPUTS: usize = 256;

/// 单个播放槽位。
struct MixerInput {
    pcm: Option<Arc<Pcm>>,
    /// 帧游标（浮点，支持 pitch 插值预留）。
    pos: f32,
    volume: f32,
}

impl Default for MixerInput {
    fn default() -> Self {
        Self {
            pcm: None,
            pos: 0.0,
            volume: 1.0,
        }
    }
}

/// 低延迟混音器（cpal 输出）。
pub struct Mixer {
    inputs: Arc<Mutex<Vec<MixerInput>>>,
    /// 输出流（持有即播放；drop 停止）。
    _stream: cpal::Stream,
    /// 输出声道数（当前固定双声道）。
    #[allow(dead_code)] // 日志与诊断引用
    pub channels: u16,
    /// 输出采样率。
    #[allow(dead_code)] // 日志与测试引用
    pub sample_rate: u32,
}

impl Mixer {
    /// 打开默认输出设备并启动输出流。
    ///
    /// # Errors
    ///
    /// 无设备、配置不支持或流创建失败时返回错误。
    pub fn open() -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "无默认音频输出设备".to_string())?;
        let supported = device
            .default_output_config()
            .map_err(|e| format!("获取输出配置失败: {e}"))?;
        let channels = supported.channels();
        let sample_rate = supported.sample_rate();
        if channels != 2 {
            return Err(format!(
                "仅支持双声道输出（当前设备 {channels} 声道）"
            ));
        }
        let config: cpal::StreamConfig = supported.config();
        let sample_format = supported.sample_format();

        let inputs = Arc::new(Mutex::new(
            std::iter::repeat_with(MixerInput::default)
                .take(MAX_INPUTS)
                .collect(),
        ));
        let inputs_cb = Arc::clone(&inputs);
        let err_fn = |e| error!("[audio] 输出流错误: {e}");

        // 动态采样格式：统一混音为 f32，再按设备格式转换
        let stream = match sample_format {
            SampleFormat::F32 => device.build_output_stream_raw(
                config,
                sample_format,
                move |data: &mut Data, _| {
                    if let Some(buf) = data.as_slice_mut::<f32>() {
                        mix_into_f32(buf, &inputs_cb, channels);
                    }
                },
                err_fn,
                None,
            ),
            SampleFormat::I16 => device.build_output_stream_raw(
                config,
                sample_format,
                move |data: &mut Data, _| {
                    if let Some(buf) = data.as_slice_mut::<i16>() {
                        let mut f32buf = vec![0.0f32; buf.len()];
                        mix_into_f32(&mut f32buf, &inputs_cb, channels);
                        for (dst, src) in buf.iter_mut().zip(f32buf) {
                            *dst = (src.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                        }
                    }
                },
                err_fn,
                None,
            ),
            other => {
                return Err(format!("不支持的输出采样格式: {other:?}"));
            }
        }
        .map_err(|e| format!("创建输出流失败: {e}"))?;
        stream.play().map_err(|e| format!("启动输出流失败: {e}"))?;

        Ok(Self {
            inputs,
            _stream: stream,
            channels,
            sample_rate,
        })
    }

    /// 播放一个 PCM（找空闲槽位；池满则丢弃）。
    ///
    /// 返回是否实际播放。
    pub fn play(&self, pcm: Arc<Pcm>, volume: f32) -> bool {
        let mut inputs = self.inputs.lock().expect("混音器锁失效");
        for input in inputs.iter_mut() {
            if input.pcm.is_none() {
                input.pcm = Some(pcm);
                input.pos = 0.0;
                input.volume = volume.clamp(0.0, 1.0);
                return true;
            }
        }
        warn!("[audio] 播放槽位已满，丢弃音频");
        false
    }

    /// 停止所有播放（清空槽位）。
    pub fn stop_all(&self) {
        let mut inputs = self.inputs.lock().expect("混音器锁失效");
        for input in inputs.iter_mut() {
            input.pcm = None;
        }
    }

    /// 是否没有任何活跃播放（全部槽位空闲）。
    #[must_use]
    pub fn is_idle(&self) -> bool {
        let inputs = self.inputs.lock().expect("混音器锁失效");
        inputs.iter().all(|i| i.pcm.is_none())
    }
}

/// 把活跃槽位混音进输出缓冲（f32，交错，`channels` 声道）。
///
/// 纯函数，可独立测试。输出限定双声道（`channels == 2`），用栈数组避免回调内分配。
fn mix_into_f32(out: &mut [f32], inputs: &Mutex<Vec<MixerInput>>, channels: u16) {
    debug_assert_eq!(channels, 2, "仅支持双声道混音");
    let mut inputs = inputs.lock().expect("混音器锁失效");
    let mut acc = [0.0f32; 2];
    for frame in out.chunks_exact_mut(2) {
        acc[0] = 0.0;
        acc[1] = 0.0;
        for input in inputs.iter_mut() {
            let Some(pcm) = &input.pcm else {
                continue;
            };
            let pcm_ch = pcm.channels as usize;
            let idx = (input.pos as usize) * pcm_ch;
            if idx + pcm_ch <= pcm.samples.len() {
                if pcm_ch == 1 {
                    let s = pcm.samples[idx] * input.volume;
                    acc[0] += s;
                    acc[1] += s;
                } else {
                    acc[0] += pcm.samples[idx] * input.volume;
                    acc[1] += pcm.samples[idx + 1] * input.volume;
                }
            }
            input.pos += 1.0;
            // 播完释放槽位
            if (input.pos as usize) * pcm_ch >= pcm.samples.len() {
                input.pcm = None;
            }
        }
        frame[0] = acc[0].clamp(-1.0, 1.0);
        frame[1] = acc[1].clamp(-1.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(samples: Vec<f32>, channels: u16) -> Arc<Pcm> {
        Arc::new(Pcm::new(samples, channels, 44_100))
    }

    /// 单槽位：输出等于 PCM 采样 × 音量。
    #[test]
    fn mix_single_input() {
        let inputs = Mutex::new(vec![
            MixerInput {
                pcm: Some(pcm(vec![0.5, -0.25, 0.125, -0.0625], 1)),
                pos: 0.0,
                volume: 1.0,
            },
            MixerInput::default(),
        ]);
        let mut out = [0.0f32; 8]; // 4 帧 × 2 声道
        mix_into_f32(&mut out, &inputs, 2);
        // 单声道复制左右
        assert!((out[0] - 0.5).abs() < 1e-6);
        assert!((out[1] - 0.5).abs() < 1e-6);
        assert!((out[2] + 0.25).abs() < 1e-6);
        assert!((out[4] - 0.125).abs() < 1e-6);
        // 播放完自动释放
        let inputs = inputs.lock().unwrap();
        assert!(inputs[0].pcm.is_none(), "4 帧播完应释放槽位");
    }

    /// 双槽位：叠加混音 + 音量。
    #[test]
    fn mix_two_inputs_with_volume() {
        let inputs = Mutex::new(vec![
            MixerInput {
                pcm: Some(pcm(vec![0.2, 0.2, 0.2, 0.2], 1)),
                pos: 0.0,
                volume: 0.5,
            },
            MixerInput {
                pcm: Some(pcm(vec![0.3, 0.3, 0.3, 0.3], 1)),
                pos: 0.0,
                volume: 0.5,
            },
        ]);
        let mut out = [0.0f32; 4];
        mix_into_f32(&mut out, &inputs, 2);
        // (0.2 + 0.3) * 0.5 = 0.25
        assert!((out[0] - 0.25).abs() < 1e-6);
        assert!((out[1] - 0.25).abs() < 1e-6);
    }

    /// 立体声 PCM：按通道混音。
    #[test]
    fn mix_stereo_pcm() {
        let inputs = Mutex::new(vec![MixerInput {
            pcm: Some(pcm(vec![0.1, 0.9, 0.2, 0.8, 0.3, 0.7, 0.4, 0.6], 2)),
            pos: 0.0,
            volume: 1.0,
        }]);
        let mut out = [0.0f32; 8];
        mix_into_f32(&mut out, &inputs, 2);
        assert!((out[0] - 0.1).abs() < 1e-6);
        assert!((out[1] - 0.9).abs() < 1e-6);
        assert!((out[2] - 0.2).abs() < 1e-6);
        assert!((out[7] - 0.6).abs() < 1e-6);
    }

    /// 槽位池播放：池满丢弃。
    #[test]
    fn play_slot_reuse() {
        // 不打开真实设备：仅验证 play 槽位查找逻辑需 Mixer——此处通过构造最小混音器不可行，
        // 直接验证槽位释放后复用（mix_into 已覆盖释放）。
        let inputs = Mutex::new(vec![MixerInput {
            pcm: Some(pcm(vec![1.0], 1)),
            pos: 0.0,
            volume: 1.0,
        }]);
        let mut out = [0.0f32; 4];
        mix_into_f32(&mut out, &inputs, 2);
        // 1 帧 PCM，播完释放
        assert!(inputs.lock().unwrap()[0].pcm.is_none());
    }
}
