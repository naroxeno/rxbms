//! 自定义 Sound 实现（kira 第 6 章 "Creating Sound Implementations"）：节拍器。
//!
//! 在音频线程实时合成点击音（正弦 × 指数衰减包络），不占用任何采样文件；
//! `tempo`（拍速）与开关由 gameplay 线程经命令通道（ringbuf/triple_buffer）
//! 平滑控制——演示如何绕过 kira 内置 `StaticSoundData`/`StreamingSoundData`，
//! 为播放器接入任意自产声源。
//!
//! 生命周期：`MetronomeData::into_sound` 把 `MetronomeSound` 送入音频线程并
//! 返回 `MetronomeHandle`。本实现中节拍器是**常驻**声源（`finished()` 恒为
//! false），由 `AudioManager` 持有句柄跨谱面复用；`AudioManager` drop 时
//! kira 连同音频线程一起清理。

use kira::{
    command::{CommandReader, CommandWriter, command_writer_and_reader},
    info::Info,
    sound::{Sound, SoundData},
    Frame,
};

/// 节拍器参数（未播放状态）。
#[derive(Debug, Clone, Copy)]
pub struct MetronomeData {
    /// 每分钟拍数。
    pub tempo: f64,
    /// 是否发声（false 时静音空转，随时可开）。
    pub enabled: bool,
}

impl Default for MetronomeData {
    fn default() -> Self {
        Self {
            tempo: 120.0,
            enabled: false,
        }
    }
}

/// 控制句柄：向音频线程写入 tempo / 开关命令。
#[derive(Debug)]
pub struct MetronomeHandle {
    tempo: CommandWriter<f64>,
    enabled: CommandWriter<bool>,
}

impl MetronomeHandle {
    /// 设置拍速（限制在合理范围，防止除零/爆音）。
    #[allow(dead_code)] // 经 AudioManager::set_metronome 调用（预留接入）
    pub fn set_tempo(&mut self, tempo: f64) {
        self.tempo.write(tempo.clamp(30.0, 400.0));
    }

    /// 打开 / 关闭节拍器。
    #[allow(dead_code)] // 经 AudioManager::set_metronome 调用（预留接入）
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled.write(enabled);
    }
}

impl SoundData for MetronomeData {
    type Error = ();
    type Handle = MetronomeHandle;

    fn into_sound(self) -> Result<(Box<dyn Sound>, Self::Handle), Self::Error> {
        let (mut tempo_tx, tempo_rx) = command_writer_and_reader();
        let (mut enabled_tx, enabled_rx) = command_writer_and_reader();
        // 把初始参数经命令通道注入，避免 Sound 与 Handle 各自持有初值导致不一致
        tempo_tx.write(self.tempo);
        enabled_tx.write(self.enabled);
        let sound = MetronomeSound {
            tempo: self.tempo,
            enabled: self.enabled,
            time_since_beat: 0.0,
            click_phase: 0.0,
            tempo_rx,
            enabled_rx,
        };
        Ok((Box::new(sound), MetronomeHandle { tempo: tempo_tx, enabled: enabled_tx }))
    }
}

/// 点击音的时长（秒）与基频（Hz）。
const CLICK_SECS: f64 = 0.03;
const CLICK_FREQ: f64 = 1200.0;

/// 音频线程上的节拍器声源。
struct MetronomeSound {
    /// 每分钟拍数。
    tempo: f64,
    /// 是否发声。
    enabled: bool,
    /// 距上一拍经过的秒数。
    time_since_beat: f64,
    /// 当前点击音的进度（秒，超过 [`CLICK_SECS`] 视为静音）。
    click_phase: f64,
    tempo_rx: CommandReader<f64>,
    enabled_rx: CommandReader<bool>,
}

impl Sound for MetronomeSound {
    fn on_start_processing(&mut self) {
        if let Some(tempo) = self.tempo_rx.read() {
            self.tempo = tempo.clamp(30.0, 400.0);
        }
        if let Some(enabled) = self.enabled_rx.read() {
            // 重新开启时重置拍点计时，避免残留的 time_since_beat 立即触发一拍
            if enabled && !self.enabled {
                self.time_since_beat = 0.0;
                self.click_phase = 0.0;
            }
            self.enabled = enabled;
        }
    }

    fn process(&mut self, out: &mut [Frame], dt: f64, _info: &Info) {
        if !self.enabled {
            out.fill(Frame::ZERO);
            return;
        }
        let beat_interval = 60.0 / self.tempo;
        for frame in out {
            // 拍点：触发一段短点击音（相位从 0 重新开始）
            self.time_since_beat += dt;
            if self.time_since_beat >= beat_interval {
                self.time_since_beat -= beat_interval; // 余量保留，避免累积漂移
                self.click_phase = 0.0;
            }
            let mut value = 0.0f32;
            if self.click_phase < CLICK_SECS {
                // 指数衰减包络 × 正弦振荡，模拟机械节拍器"嗒"声
                let envelope = (1.0 - self.click_phase / CLICK_SECS) as f32;
                let phase = self.click_phase as f32 * CLICK_FREQ as f32 * std::f32::consts::TAU;
                value = phase.sin() * envelope * 0.35;
                self.click_phase += dt;
            }
            *frame = Frame::from_mono(value);
        }
    }

    /// 常驻声源：不因播放结束而卸载，由句柄（`AudioManager`）控制生命周期。
    fn finished(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kira::info::MockInfoBuilder;

    fn make_sound() -> (Box<dyn Sound>, MetronomeHandle) {
        MetronomeData {
            tempo: 60.0,
            enabled: true,
        }
        .into_sound()
        .expect("into_sound 不应失败")
    }

    /// 1 Hz 拍速：44100 帧（1 秒）内应恰好触发一次点击，且输出非零。
    #[test]
    fn clicks_once_per_beat() {
        let (mut sound, _handle) = make_sound();
        let info = MockInfoBuilder::new().build();
        let dt = 1.0 / 44100.0;
        // 1 秒内应产生一次拍点 → 采样值非零
        let mut nonzero_frames = 0usize;
        for _ in 0..44100 {
            let f = sound.process_one(dt, &info);
            if f.left != 0.0 {
                nonzero_frames += 1;
            }
        }
        // 点击音持续约 0.03s ≈ 1323 帧
        assert!(
            (1323..1600).contains(&nonzero_frames),
            "nonzero_frames={nonzero_frames}"
        );
    }

    /// 关闭后静音；命令可即时生效。
    #[test]
    fn disabled_is_silent_and_can_be_enabled() {
        let (mut sound, mut handle) = make_sound();
        let info = MockInfoBuilder::new().build();
        let dt = 1.0 / 44100.0;
        handle.set_enabled(false);
        sound.on_start_processing(); // 模拟音频线程处理命令
        let f = sound.process_one(dt, &info);
        assert_eq!(f, Frame::ZERO);
        // 重新打开
        handle.set_enabled(true);
        sound.on_start_processing();
        handle.set_tempo(60.0);
        sound.on_start_processing();
        let mut heard = false;
        for _ in 0..44100 {
            if sound.process_one(dt, &info).left != 0.0 {
                heard = true;
                break;
            }
        }
        assert!(heard, "重新打开后应在 1 秒内听到拍点");
    }

    /// tempo 命令即时生效：120 → 半秒一拍。
    #[test]
    fn tempo_command_takes_effect() {
        let (mut sound, mut handle) = make_sound();
        let info = MockInfoBuilder::new().build();
        let dt = 1.0 / 44100.0;
        handle.set_tempo(120.0);
        sound.on_start_processing();
        // 半秒（22050 帧）内应听到第一声
        let mut heard = false;
        for _ in 0..22_050 {
            if sound.process_one(dt, &info).left != 0.0 {
                heard = true;
                break;
            }
        }
        assert!(heard, "120 BPM 应约 0.5s 触发拍点");
    }
}
