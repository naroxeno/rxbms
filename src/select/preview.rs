//! 选曲界面的音频预览。
//!
//! 机制：选中铺面后（防抖 [`PREVIEW_DEBOUNCE`]）在后台线程用
//! [`LoadedChart::load`] 解析谱面，把 BGM / 键音事件整理成按时间排序的时间轴；
//! 随后在选曲界面里以独立时钟推进，按时间触发 `AudioManager`，
//! 最多播 [`PREVIEW_MAX_SECS`] 秒（或谱面结束）。
//!
//! 与游玩互斥：进入游玩/切走时停止预览并释放音频租约。

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        mpsc::{Receiver, TryRecvError, channel},
    },
    thread,
    time::{Duration, Instant},
};

use bevy::prelude::*;

use bms_rs::chart::prelude::ChartEvent;

use crate::{
    audio::{AudioLease, AudioManager},
    gameplay::chart::LoadedChart,
};

use super::SelectState;

/// 预览最长播放时长（秒）：谱面再长也只播开头这么久。
pub const PREVIEW_MAX_SECS: f64 = 30.0;
/// 选中变化后的防抖时长（对齐 beatoraja 的 `previewDuration = 400ms`）：
/// 快速滚动时不逐个解析/逐个起播。
pub const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(400);

/// 时间轴上的一个音频触发点。
#[derive(Clone, Debug)]
struct PreviewEvent {
    /// 触发时刻（谱面时间，秒）。
    time: f64,
    /// `#WAV` 槽位 id（键音并发按槽位计数，多槽位叠音要用）。
    slot: u32,
    /// 音频文件路径。
    path: PathBuf,
    /// `true` 走 BGM 语义（长音去重/一次性采样），`false` 走键音语义。
    bgm: bool,
}

/// 一个铺面的预览时间轴（事件按时间升序）。
#[derive(Default)]
pub struct PreviewTimeline {
    events: Vec<PreviewEvent>,
    /// 谱面总时长（秒）。
    total_sec: f64,
}

impl PreviewTimeline {
    /// 事件条数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// 是否没有任何可播事件。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

/// 预览状态（界面展示用）。
#[derive(Clone, Debug, Default, PartialEq)]
pub enum PreviewStatus {
    /// 没有可预览的铺面。
    #[default]
    Idle,
    /// 正在解析谱面。
    Loading,
    /// 正在播放（按谱面时间轴）。
    Playing {
        /// 已播放时长（秒）。
        elapsed: f64,
        /// 预览总时长（秒）。
        total: f64,
    },
    /// 正在循环播放谱面自带的 `#PREVIEW` 音源。
    PlayingLoop,
    /// 解析或播放失败。
    Failed(String),
}

impl PreviewStatus {
    /// 状态文案。
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Idle => String::from("试听：空格开始"),
            Self::Loading => String::from("试听：解析谱面…"),
            Self::Playing { elapsed, total } => {
                format!("试听中 {elapsed:.1}s / {total:.1}s（空格重播）")
            }
            Self::PlayingLoop => String::from("试听中（#PREVIEW 循环，空格重播）"),
            Self::Failed(error) => format!("试听失败：{error}"),
        }
    }

}

/// 后台解析结果：`(铺面路径, 时间轴或错误)`。
type PreviewOutcome = (PathBuf, Result<PreviewTimeline, String>);
/// 后台解析结果通道。
type PreviewReceiver = Mutex<Receiver<PreviewOutcome>>;

/// 一次进行中的预览播放。
struct Playing {
    started: Instant,
    /// 已触发到时间轴的哪个下标。
    idx: usize,
    /// 本次播放的音频租约（停止时释放）。
    lease: AudioLease,
    /// 自动停止时刻（秒，谱面时间）。
    limit_sec: f64,
}

/// 选曲界面的音频预览资源。
#[derive(Resource, Default)]
pub struct SongPreview {
    /// 当前选中的铺面（预览目标）。
    target: Option<PathBuf>,
    /// 目标最后一次变化的时刻（防抖用）。
    changed_at: Option<Instant>,
    /// 已派发解析请求的路径（避免重复派发）。
    requested: Option<PathBuf>,
    /// 后台解析结果通道（`Mutex` 仅为满足 `Resource: Sync`）。
    rx: Option<PreviewReceiver>,
    /// 当前可播放的时间轴。
    timeline: Option<(PathBuf, PreviewTimeline)>,
    /// 进行中的时间轴播放。
    playing: Option<Playing>,
    /// 正在循环播放的 `#PREVIEW` 音源（文件模式）。
    file_playing: Option<PathBuf>,
    /// 状态文案数据。
    status: PreviewStatus,
}

impl SongPreview {
    /// 当前预览状态。
    #[must_use]
    pub fn status(&self) -> &PreviewStatus {
        &self.status
    }

    /// 切换预览目标：立即停止当前播放，防抖后重新解析。
    ///
    /// 目标未变化时什么都不做（每帧调用是安全的）。
    pub fn request(&mut self, path: Option<PathBuf>, audio: &mut AudioManager) {
        if self.target == path {
            return;
        }
        self.stop(audio);
        self.target = path;
        self.changed_at = Some(Instant::now());
        self.requested = None;
        self.timeline = None;
        self.rx = None;
        self.file_playing = None;
        self.status = if self.target.is_some() {
            PreviewStatus::Loading
        } else {
            PreviewStatus::Idle
        };
    }

    /// 停止播放并释放音频资源（时间轴播放与 `#PREVIEW` 循环都停）。
    pub fn stop(&mut self, audio: &mut AudioManager) {
        let had_playback = self.playing.is_some() || self.file_playing.is_some();
        if let Some(playing) = self.playing.take() {
            audio.stop_all();
            audio.release(&playing.lease);
        }
        if self.file_playing.take().is_some() {
            audio.stop_menu_bgm();
        }
        if had_playback {
            self.status = PreviewStatus::Idle;
        }
    }

    /// 重播当前预览（`#PREVIEW` 循环音源或时间轴，取决于当前模式）。
    pub fn restart(&mut self, audio: &mut AudioManager) {
        if let Some(file) = self.file_playing.clone() {
            audio.stop_menu_bgm();
            if audio.play_preview_loop(&file) {
                self.status = PreviewStatus::PlayingLoop;
            } else {
                self.file_playing = None;
                self.status = PreviewStatus::Failed(format!("无法播放 {}", file.display()));
            }
            return;
        }
        self.stop(audio);
        self.play_timeline(audio);
    }

    /// 起播 `#PREVIEW` 循环音源；失败返回 `false`，由调用方回退到时间轴。
    fn play_file(&mut self, audio: &mut AudioManager, file: &Path) -> bool {
        if !audio.play_preview_loop(file) {
            return false;
        }
        self.file_playing = Some(file.to_owned());
        self.status = PreviewStatus::PlayingLoop;
        true
    }

    /// 从头起播时间轴（无时间轴/无事件时什么都不做）。
    fn play_timeline(&mut self, audio: &mut AudioManager) {
        let Some((_, timeline)) = self.timeline.as_ref() else {
            return;
        };
        if timeline.is_empty() {
            return;
        }

        // 预览期间复用游玩那套"整场铺面音轨"：先清干净上一轮残留
        audio.stop_all();
        let unique: Vec<PathBuf> = {
            let mut seen = std::collections::HashSet::new();
            timeline
                .events
                .iter()
                .filter(|event| seen.insert(event.path.clone()))
                .map(|event| event.path.clone())
                .collect()
        };
        let lease = audio.acquire(&unique);

        // 预加载时间轴：每个路径的首次触发时间（±10s 滚动窗口解码）
        let mut first: HashMap<&PathBuf, f64> = HashMap::new();
        for event in &timeline.events {
            first
                .entry(&event.path)
                .and_modify(|time| *time = time.min(event.time))
                .or_insert(event.time);
        }
        let mut preload: Vec<(f64, PathBuf)> =
            first.into_iter().map(|(p, t)| (t, p.clone())).collect();
        preload.sort_by(|a, b| a.0.total_cmp(&b.0));
        audio.set_preload_timeline(preload);
        audio.pump_preload(0.0);

        let limit_sec = timeline.total_sec.min(PREVIEW_MAX_SECS);
        self.playing = Some(Playing {
            started: Instant::now(),
            idx: 0,
            lease,
            limit_sec,
        });
        self.status = PreviewStatus::Playing {
            elapsed: 0.0,
            total: limit_sec,
        };
    }

    /// 每帧推进：防抖派发解析、收割结果、按时间轴触发音频、超时停止。
    fn step(&mut self, audio: &mut AudioManager, state: Option<&SelectState>) {
        // 1) 跟随选曲界面当前选中项
        if let Some(state) = state {
            self.request(state.selected_path(), audio);
        }

        // 2) 防抖后自动起播：优先谱面自带的 `#PREVIEW` 音源（beatoraja 行为），
        //    没有则回退到"解析谱面 + 按事件时间轴试听"。
        let due = self
            .changed_at
            .is_some_and(|at| at.elapsed() >= PREVIEW_DEBOUNCE);
        let idle = self.rx.is_none()
            && self.timeline.is_none()
            && self.playing.is_none()
            && self.file_playing.is_none()
            && self.requested != self.target;
        if due
            && idle
            && let Some(target) = self.target.clone()
        {
            self.requested = Some(target.clone());
            let preview_file = state.and_then(SelectState::selected_preview);
            match preview_file {
                Some(file) => {
                    info!("[preview] 自动试听 #PREVIEW: {}", file.display());
                    if !self.play_file(audio, &file) {
                        // 音源坏了：回退到时间轴
                        self.spawn_load(target);
                    }
                }
                None => self.spawn_load(target),
            }
        }

        // 3) 收割解析结果（只接受当前目标的）
        let received = self
            .rx
            .as_ref()
            .and_then(|rx| rx.lock().ok())
            .and_then(|rx| match rx.try_recv() {
                Ok(value) => Some(value),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => None,
            });
        if let Some((path, result)) = received {
            self.rx = None;
            self.requested = None;
            if Some(&path) == self.target.as_ref() {
                match result {
                    Ok(timeline) => {
                        info!(
                            "[preview] 时间轴就绪：{} 个事件 / {:.1}s（{}）",
                            timeline.len(),
                            timeline.total_sec,
                            path.display()
                        );
                        self.timeline = Some((path, timeline));
                        // 选中后自动试听（用户约定：移到铺面就预览）
                        self.play_timeline(audio);
                    }
                    Err(error) => {
                        warn!("[preview] 解析失败 {}: {error}", path.display());
                        self.status = PreviewStatus::Failed(error);
                    }
                }
            }
        }

        // 4) 播放推进
        let Some(playing) = self.playing.as_ref() else {
            return;
        };
        let elapsed = playing.started.elapsed().as_secs_f64();
        let limit = playing.limit_sec;
        if elapsed >= limit {
            // 循环试听：播完从头再来
            self.stop(audio);
            self.play_timeline(audio);
            return;
        }

        // 按谱面时间预加载 ±10s 窗口，然后触发到期事件。
        // `timeline` / `playing` 是结构体的不同字段，借用互不冲突，
        // 因此无需每帧克隆整个事件表。
        audio.pump_preload(elapsed);
        let Some((_, timeline)) = self.timeline.as_ref() else {
            return;
        };
        let Some(playing) = self.playing.as_mut() else {
            return;
        };
        let events = &timeline.events;
        while let Some(event) = events.get(playing.idx) {
            if event.time > elapsed {
                break;
            }
            if event.bgm {
                audio.play_bgm_event(&event.path);
            } else {
                audio.play_synced(event.slot, &event.path);
            }
            playing.idx += 1;
        }
        self.status = PreviewStatus::Playing {
            elapsed: elapsed.min(limit),
            total: limit,
        };
    }

    /// 在后台线程解析谱面，把结果送回。
    fn spawn_load(&mut self, path: PathBuf) {
        let (tx, rx) = channel();
        self.rx = Some(Mutex::new(rx));
        let spawn = thread::Builder::new()
            .name(String::from("rxbms-preview"))
            .spawn(move || {
                let result = build_timeline(&path);
                // 接收端可能已被替换（快速切换选中项）：发送失败忽略
                let _ = tx.send((path, result));
            });
        if let Err(error) = spawn {
            error!("[preview] 启动解析线程失败: {error}");
            self.status = PreviewStatus::Failed(format!("启动解析线程失败: {error}"));
        }
    }
}

/// 解析铺面并整理成预览时间轴（在后台线程执行）。
pub fn build_timeline(path: &Path) -> Result<PreviewTimeline, String> {
    let chart = LoadedChart::load(path)?;
    let mut events: Vec<PreviewEvent> = Vec::with_capacity(chart.bgm_event_count + 256);
    for pe in chart.chart.events().as_events() {
        let time = pe.activate_time.as_secs_f64();
        match &pe.event {
            ChartEvent::Bgm { wav_id: Some(id) } => {
                if let Some(wav) = chart.wav_paths.get(id) {
                    events.push(PreviewEvent {
                        time,
                        slot: id.value() as u32,
                        path: wav.clone(),
                        bgm: true,
                    });
                }
            }
            ChartEvent::Note {
                wav_id: Some(id), ..
            } => {
                if let Some(wav) = chart.wav_paths.get(id) {
                    events.push(PreviewEvent {
                        time,
                        slot: id.value() as u32,
                        path: wav.clone(),
                        bgm: false,
                    });
                }
            }
            _ => {}
        }
    }
    events.sort_by(|a, b| a.time.total_cmp(&b.time));
    Ok(PreviewTimeline {
        events,
        total_sec: chart.total_sec,
    })
}

/// 每帧驱动预览（只在选曲界面运行）。
pub fn preview_tick(
    mut preview: ResMut<SongPreview>,
    mut audio: ResMut<AudioManager>,
    state: Option<Res<SelectState>>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    preview.step(&mut audio, state.as_deref());
    if keys.just_pressed(KeyCode::Space) {
        preview.restart(&mut audio);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实铺面 → 时间轴：事件升序、覆盖 BGM 与键音、总时长非零。
    #[test]
    fn timeline_from_real_chart_is_sorted() {
        let root = std::env::temp_dir().join(format!("rxbms_preview_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // 占位 wav：路径解析只做存在性检查
        std::fs::write(root.join("bgm.wav"), b"").unwrap();
        std::fs::write(root.join("key.wav"), b"").unwrap();
        let chart = root.join("a.bms");
        std::fs::write(
            &chart,
            "#TITLE Preview\n#BPM 120\n#WAV01 bgm.wav\n#WAV02 key.wav\n\
             #00101:01\n#00111:02\n#00211:02\n",
        )
        .unwrap();

        let timeline = build_timeline(&chart).expect("应解析成功");
        assert!(timeline.len() >= 3, "应至少有 3 个事件: {}", timeline.len());
        assert!(timeline.total_sec > 0.0);
        assert!(
            timeline.events.windows(2).all(|w| w[0].time <= w[1].time),
            "事件必须按时间升序"
        );
        assert!(
            timeline.events.iter().any(|event| event.bgm),
            "应包含 BGM 事件"
        );
        assert!(
            timeline.events.iter().any(|event| !event.bgm),
            "应包含键音事件"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 损坏文件：返回错误而不是 panic（后台线程里不能把界面拖崩）。
    #[test]
    fn broken_chart_reports_error() {
        let root = std::env::temp_dir().join(format!("rxbms_preview_bad_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let chart = root.join("bad.bms");
        std::fs::write(&chart, b"\xFF\xFE\x00not-a-chart").unwrap();
        assert!(build_timeline(&chart).is_err(), "二进制垃圾应报错");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 状态文案覆盖四种状态。
    #[test]
    fn status_labels() {
        assert!(PreviewStatus::Idle.label().contains("空格开始"));
        assert!(PreviewStatus::Loading.label().contains("解析"));
        assert!(
            PreviewStatus::Playing {
                elapsed: 1.5,
                total: 30.0
            }
            .label()
            .contains("1.5")
        );
        assert!(PreviewStatus::Failed("x".into()).label().contains('x'));
    }
}
