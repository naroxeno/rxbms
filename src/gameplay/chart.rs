//! 谱面加载：选中铺面的完整 `Bms` → bms-rs `Chart` 处理，提取播放所需数据。
//!
//! 流程：选曲界面点击 → [`SelectedChart`] → 进入 Gameplay 后 [`load_chart`]
//! 解析并处理为 [`LoadedChart`]（音符、轨道、BGM、音频路径、时长）。

use std::sync::Arc;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use bevy::prelude::*;
use bms_rs::{
    bms::command::{JudgeLevel, LnMode},
    bms::prelude::*,
    chart::prelude::*,
};

use crate::database::decode_bms;

/// 选曲界面选中的铺面（进入 Gameplay 的信号）。
#[derive(Resource, Clone, Debug)]
pub struct SelectedChart {
    pub path: PathBuf,
    pub title: String,
}

/// 单条轨道的定义（Key → 屏幕列）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneDef {
    pub key: Key,
}

/// 一个可玩音符（渲染 + 判定用）。
#[derive(Debug, Clone)]
pub struct NoteView {
    /// 事件 ID（对应触发事件的 `ChartEventId`，用于把事件映射回本结构）。
    pub event_id: usize,
    /// 轨道位置（Key）。
    pub key: Key,
    /// 音符类型（可见 / 隐形 / 长音 / 地雷）。
    pub kind: NoteKind,
    /// 键音资源（可能为空）。
    pub wav_id: Option<WavId>,
    /// 谱面逻辑 Y 坐标（渲染位置，含 STOP 效果）。
    pub position: YCoordinate,
    /// 触发时间（秒，Auto 判定用）。
    pub activate_time: f64,
    /// 轨道列索引（`lanes` 中的位置）。
    pub lane: usize,
    /// 长音长度（YCoordinate 单位，非长音时为 `None`）。
    pub length: Option<NonNegativeF64>,
}

/// 加载完成的谱面（Gameplay 生命周期内持有）。
pub struct LoadedChart {
    /// bms-rs Chart（`Arc` 共享；播放头经 `ChartPlayback` self_cell 借用，
    /// 随 GameplaySession 整体释放，无泄漏）。
    pub chart: Arc<Chart>,
    /// 标题（结算界面/数据表使用）。
    pub title: String,
    /// `#ARTIST`。
    pub artist: Option<String>,
    /// `#GENRE`。
    pub genre: Option<String>,
    /// `#PLAYLEVEL`。
    pub play_level: Option<u8>,
    /// 可玩音符（仅 1P 的 Key/Scratch，去掉了 FootPedal/FreeZone）。
    pub notes: Vec<NoteView>,
    /// 事件 ID → 音符索引（触发事件反查）。
    pub note_by_event: HashMap<usize, usize>,
    /// 轨道定义（顺序即列）。
    pub lanes: Vec<LaneDef>,
    /// `#RANK` 判定难度（LR2 判定窗口选择）。
    pub rank: JudgeLevel,
    /// `#LNTYPE` 长音种类（LN / CN / HCN）。
    pub ln_mode: LnMode,
    /// BGA 数据（Base 层事件 + 图片/视频映射）。
    pub bga: super::bga::BgaData,
    /// 谱面逻辑结束时间（秒）。
    pub total_sec: f64,
    /// #TOTAL 值（血量属性，默认 100）。
    pub total_value: f32,
    /// 用到的音频资源（WavId → 磁盘真实路径，已过滤不存在的）。
    pub wav_paths: HashMap<WavId, PathBuf>,
    /// BGM 事件数（统计/日志）。
    pub bgm_event_count: usize,
    /// 键音事件数（统计/日志）。
    pub keysound_event_count: usize,
}

impl LoadedChart {
    /// 解析并处理铺面文件。
    ///
    /// # Errors
    ///
    /// 文件读取、解析或 Chart 处理失败时返回错误。
    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes = fs::read(path).map_err(|e| format!("读取失败: {e}"))?;
        let source = decode_bms(&bytes);
        let output = parse_bms(&source, default_config());
        let bms = output.bms.map_err(|e| format!("解析失败: {e}"))?;
        let chart = Process::<KeyLayoutBeat>::process(&bms)
            .map_err(|e| format!("Chart 处理失败: {e}"))?;
        let chart = Arc::new(chart);

        let title = bms
            .music_info
            .title
            .clone()
            .unwrap_or_else(|| "untitled".into());

        // 提取全部事件（y 从 0 到无穷）
        let all = chart
            .events()
            .events_in_y_range(YCoordinate::ZERO..);

        let mut notes: Vec<NoteView> = Vec::new();
        let mut max_time = 0.0_f64;
        let mut bgm_event_count = 0usize;
        let mut keysound_event_count = 0usize;

        // 第一遍：收集全部出现过的轨道键（去重）
        let mut key_set: Vec<Key> = Vec::new();
        for pe in &all {
            if let ChartEvent::Note {
                side: PlayerSide::Player1,
                key,
                ..
            } = &pe.event
            {
                if matches!(key, Key::FootPedal | Key::FreeZone) {
                    continue;
                }
                if !key_set.contains(key) {
                    key_set.push(*key);
                }
            }
        }
        // 轨道排序：scratch 最左，然后键位升序（IIDX 布局）
        key_set.sort_by_key(|k| Self::lane_order(*k));
        let lane_of: HashMap<Key, usize> = key_set
            .iter()
            .enumerate()
            .map(|(i, k)| (*k, i))
            .collect();

        // 第二遍：提取音符
        for pe in &all {
            let t = pe.activate_time.as_secs_f64();
            if t > max_time {
                max_time = t;
            }
            match &pe.event {
                ChartEvent::Note {
                    side: PlayerSide::Player1,
                    key,
                    kind,
                    wav_id,
                    length,
                    ..
                } => {
                    keysound_event_count += 1;
                    // 忽略踏板与自由区（暂不支持）
                    if matches!(key, Key::FootPedal | Key::FreeZone) {
                        continue;
                    }
                    // 隐形 note（通道 3x/4x）：不渲染、不参与判定，
                    // 键音由 ChartPlayer 事件实时触发（beatoraja 语义）。
                    if *kind == NoteKind::Invisible {
                        continue;
                    }
                    notes.push(NoteView {
                        event_id: pe.id.0,
                        key: *key,
                        kind: *kind,
                        wav_id: *wav_id,
                        position: pe.position,
                        activate_time: t,
                        lane: lane_of[key],
                        length: match kind {
                            NoteKind::Long => *length,
                            _ => None,
                        },
                    });
                }
                ChartEvent::Bgm { .. } => {
                    bgm_event_count += 1;
                }
                _ => {}
            }
        }
        notes.sort_by_key(|a| a.event_id);
        let note_by_event = notes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.event_id, i))
            .collect();

        // 音频路径：BMS 引用路径 → 磁盘真实路径（扩展名 fallback）
        let wav_paths = resolve_wav_paths(path, chart.as_ref());

        // BGA 数据：Base 层事件流 + 图片/视频映射
        let bga = extract_bga(path, chart.as_ref());

        Ok(Self {
            chart,
            title,
            artist: bms.music_info.artist.clone(),
            genre: bms.music_info.genre.clone(),
            play_level: bms.metadata.play_level,
            notes,
            note_by_event,
            lanes: key_set.into_iter().map(|key| LaneDef { key }).collect(),
            rank: bms.judge.rank.unwrap_or(JudgeLevel::Normal),
            ln_mode: bms.repr.ln_mode,
            total_sec: max_time,
            total_value: bms
                .judge
                .total
                .as_ref()
                .and_then(|v| v.raw().parse::<f32>().ok())
                .unwrap_or(100.0),
            wav_paths,
            bgm_event_count,
            keysound_event_count,
            bga,
        })
    }

    /// 轨道排序权重：scratch 最左（0），然后键位升序（1, n），其余靠后。
fn lane_order(key: Key) -> (u8, u8) {
    match key {
        Key::Scratch(_) => (0, 0),
        Key::Key(n) => (1, n),
        _ => (2, 0),
    }
}

/// 音符总数（标题栏显示）。
    #[must_use]
    pub fn note_count(&self) -> usize {
        self.notes.len()
    }

    /// 首批音频路径：`deadline_sec` 秒之前出现的**所有事件**（BGM + 键音，去重）。
    ///
    /// 首批就绪即可开玩；其余音频游玩中后台渐进加载。
    #[must_use]
    pub fn priority_audio_paths(&self, deadline_sec: f64) -> std::collections::HashSet<PathBuf> {
        let mut set = std::collections::HashSet::new();
        let all = self.chart.events().events_in_y_range(YCoordinate::ZERO..);
        for pe in &all {
            if pe.activate_time.as_secs_f64() > deadline_sec {
                continue;
            }
            match &pe.event {
                ChartEvent::Bgm { wav_id: Some(id) } | ChartEvent::Note { wav_id: Some(id), .. } => {
                    if let Some(p) = self.wav_paths.get(id) {
                        set.insert(p.clone());
                    }
                }
                _ => {}
            }
        }
        set
    }

    /// BGM 事件引用统计：wav 路径 → 事件次数。
    ///
    /// 事件次数用于判断 BGM 通道文件的播放形态：**密集引用**（同一文件被大量
    /// BGM 事件触发，如循环采样）必须走静态缓存多路并发，否则每次事件停旧流+
    /// 重开流会把背景音掐断；**稀疏引用**（1-2 次）才是真正的分段长 BGM，
    /// 适合流式解码。
    #[must_use]
    pub fn bgm_audio_stats(&self) -> std::collections::HashMap<PathBuf, usize> {
        let mut stats = std::collections::HashMap::new();
        let all = self.chart.events().events_in_y_range(YCoordinate::ZERO..);
        for pe in &all {
            if let ChartEvent::Bgm { wav_id: Some(id) } = &pe.event
                && let Some(p) = self.wav_paths.get(id)
            {
                *stats.entry(p.clone()).or_insert(0) += 1;
            }
        }
        stats
    }

    /// 游玩模式：按谱面使用的最大键号判断（≤5 → 5K，否则 7K）。
    #[must_use]
    pub fn play_mode(&self) -> crate::core::keybind::PlayMode {
        let max_key = self
            .lanes
            .iter()
            .filter_map(|l| match l.key {
                Key::Key(n) => Some(n),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        if max_key <= 5 {
            crate::core::keybind::PlayMode::FiveKey
        } else {
            crate::core::keybind::PlayMode::SevenKey
        }
    }
}

/// 提取 BGA 数据：Base 层事件流（秒）+ 图片/视频文件映射。
fn extract_bga(
    bms_path: &Path,
    chart: &Chart,
) -> crate::gameplay::bga::BgaData {
    use crate::gameplay::bga::{BgaData, BgaEvent};

    let dir = bms_path.parent().unwrap_or_else(|| Path::new("."));

    // Base 层事件（带秒时间）
    let events: Vec<BgaEvent> = chart
        .events()
        .events_in_y_range(YCoordinate::ZERO..)
        .iter()
        .filter_map(|pe| match &pe.event {
            ChartEvent::BgaChange {
                layer: BgaLayer::Base,
                bmp_id: Some(id),
            } => Some(BgaEvent {
                time_sec: pe.activate_time.as_secs_f64(),
                bmp_id: id.0,
            }),
            _ => None,
        })
        .collect();

    // 图片/视频分类（按扩展名）
    let mut images = std::collections::HashMap::new();
    let mut videos = std::collections::HashMap::new();
    for (id, rel) in chart.bmp_files() {
        let real = dir.join(rel);
        if !real.is_file() {
            continue;
        }
        if is_video_file(&real) {
            videos.insert(id.0, real);
        } else {
            images.insert(id.0, real);
        }
    }

    BgaData {
        events,
        images,
        videos,
    }
}

/// BGA 视频文件扩展名判断（wmv 等）。
fn is_video_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "wmv" | "avi" | "mp4" | "mkv" | "flv" | "mov" | "mpg" | "mpeg" | "webm"
            )
        })
}

/// 解析 BMS 引用的音频路径，返回磁盘上真实存在的文件映射。
///
/// BMS 的 `#WAVxx` 路径相对铺面文件目录，且常见"引 .wav 实为 .ogg"的情况，
/// 因此做存在性检查 + 扩展名替换 fallback。
fn resolve_wav_paths(
    bms_path: &Path,
    chart: &Chart,
) -> HashMap<WavId, PathBuf> {
    let dir = bms_path.parent().unwrap_or_else(|| Path::new("."));
    let mut out = HashMap::new();
    for (wav_id, rel) in chart.audio_files() {
        if let Some(real) = resolve_audio_path(dir, rel) {
            out.insert(*wav_id, real);
        }
    }
    out
}

/// 单条音频路径的磁盘解析：原路径 → 常见扩展名替换。
fn resolve_audio_path(dir: &Path, rel: &Path) -> Option<PathBuf> {
    let candidates = [
        rel.to_path_buf(),
        rel.with_extension("ogg"),
        rel.with_extension("wav"),
        rel.with_extension("mp3"),
    ];
    candidates
        .into_iter()
        .map(|p| dir.join(p))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 迷你 BMS：2 个 1P 音符（通道 11/12）+ 1 个 BGM（通道 01）。
    const MINI_BMS: &str = "\
#PLAYER 1
#TITLE Mini
#BPM 120
#WAV01 bgm.wav
#WAV02 key1.wav
#00111:02
#00112:02
#00101:01
";

    fn temp_chart(name: &str, content: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rxbms_gameplay_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("chart.bms");
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn load_mini_chart() {
        let path = temp_chart("mini", MINI_BMS);
        let loaded = LoadedChart::load(&path).expect("加载迷你铺面");
        // 2 个可玩音符（11/12 通道）
        assert_eq!(loaded.note_count(), 2, "应提取 2 个可玩音符");
        assert_eq!(loaded.lanes.len(), 2, "应 2 条轨道");
        // 轨道按键位排序：通道 11 → Key::Key(1)，通道 12 → Key::Key(2)
        assert_eq!(loaded.lanes[0].key, Key::Key(1));
        assert_eq!(loaded.lanes[1].key, Key::Key(2));
        // 音符按时间排序
        assert!(loaded.notes[0].activate_time <= loaded.notes[1].activate_time);
        // 总时长 > 0
        assert!(loaded.total_sec > 0.0);
    }

}






