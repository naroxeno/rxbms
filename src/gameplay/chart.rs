//! 谱面加载：选中铺面的完整 `Bms` → bms-rs `Chart` 处理，提取播放所需数据。
//!
//! 流程：选曲界面点击 → [`SelectedChart`] → 进入 Gameplay 后 [`load_chart`]
//! 解析并处理为 [`LoadedChart`]（音符、轨道、BGM、音频路径、时长）。

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
    /// bms-rs Chart（`Box::leak` 以获得 `'static` 引用，供 `ChartPlayer` 借用；
    /// TODO: 每游玩一次泄漏一份谱面数据，需改为 owned 播放器。
    pub chart: &'static Chart,
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
        let chart: &'static Chart = Box::leak(Box::new(chart));

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
        let wav_paths = resolve_wav_paths(path, chart);

        // BGA 数据：Base 层事件流 + 图片/视频映射
        let bga = extract_bga(path, chart);

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

    /// 游玩模式检测：迷你铺面（键1-2）→ 5K；rainbowA（键1-7）→ 7K。
    #[test]
    fn play_mode_detection() {
        let path = temp_chart("mode5", MINI_BMS);
        let loaded = LoadedChart::load(&path).expect("加载迷你铺面");
        assert_eq!(
            loaded.play_mode(),
            crate::core::keybind::PlayMode::FiveKey
        );

        let home = std::env::var("HOME").unwrap_or_default();
        let rainbow = PathBuf::from(home).join(".local/share/lr2oraja/songs/rainbow_ogg/rainbowA.bms");
        if rainbow.exists() {
            let loaded = LoadedChart::load(&rainbow).expect("加载 rainbowA");
            assert_eq!(
                loaded.play_mode(),
                crate::core::keybind::PlayMode::SevenKey
            );
        }
    }

    /// 长音验证：tower_of_nirv 7HYPER（文件不存在时跳过）。
    #[test]
    fn load_real_ln_chart() {
        let home = std::env::var("HOME").unwrap_or_default();
        let path = PathBuf::from(home)
            .join(".local/share/lr2oraja/songs/[hangneil+atomicsphere]tower_of_nirv/22_tower_of_nirv_7HYPER.bme");
        if !path.exists() {
            eprintln!("跳过：找不到 LN 测试铺面 {}", path.display());
            return;
        }
        let loaded = LoadedChart::load(&path).expect("加载 LN 铺面");
        let ln_count = loaded.notes.iter().filter(|n| n.length.is_some()).count();
        eprintln!(
            "LN chart: {} notes, {} LN, {} lanes",
            loaded.note_count(),
            ln_count,
            loaded.lanes.len()
        );
        assert!(ln_count > 0, "应有长音音符");
        for n in loaded.notes.iter().filter(|n| n.length.is_some()) {
            assert!(n.length.unwrap().as_f64() > 0.0, "LN length 应为正");
        }
    }

    /// 真实 rainbowA：验证音符提取与音频路径 fallback（.wav 引用 → .ogg 实际）。
    #[test]
    fn load_real_rainbow() {
        let home = std::env::var("HOME").unwrap_or_default();
        let path =
            PathBuf::from(home).join(".local/share/lr2oraja/songs/rainbow_ogg/rainbowA.bms");
        if !path.exists() {
            eprintln!("跳过：找不到真实铺面 {}", path.display());
            return;
        }
        let loaded = LoadedChart::load(&path).expect("加载真实铺面");
        eprintln!(
            "rainbowA: {} notes, {} lanes, {:.1}s, {} wav files, BGM事件 {}, 键音事件 {}",
            loaded.note_count(),
            loaded.lanes.len(),
            loaded.total_sec,
            loaded.wav_paths.len(),
            loaded.bgm_event_count,
            loaded.keysound_event_count
        );
        eprintln!(
            "lane order: {:?}",
            loaded.lanes.iter().map(|l| l.key).collect::<Vec<_>>()
        );
        assert!(loaded.note_count() > 100, "应有大量音符");
        assert!(loaded.lanes.len() >= 5, "应为 5k 或 7k 轨道数");
        // 音频路径 fallback：rainbow 引用 .wav 但目录是 .ogg
        let has_ogg = loaded
            .wav_paths
            .values()
            .any(|p| p.extension().is_some_and(|e| e == "ogg"));
        assert!(has_ogg, "音频路径应能 fallback 到 .ogg: {:?}", loaded.wav_paths.values().take(5).collect::<Vec<_>>());
    }

    /// 判定基准一致性：逐帧（5ms）推进 `ChartPlayer`，验证**全部**音符事件最终都被
    /// 触发（manual 判定用 `elapsed_since(started_at)` 与 `activate_time` 求差，
    /// 若播放头到音符时刻 ≠ activate_time，音符将永远打不中 / 被提前 miss）。
    #[test]
    fn chart_player_events_align_with_activate_time() {
        use std::time::Duration;

        use gametime::{TimeSpan, TimeStamp};

        let home = std::env::var("HOME").unwrap_or_default();
        let path =
            PathBuf::from(home).join(".local/share/lr2oraja/songs/rainbow_ogg/rainbowA.bms");
        if !path.exists() {
            eprintln!("跳过：找不到真实铺面 {}", path.display());
            return;
        }
        let loaded = LoadedChart::load(&path).expect("加载真实铺面");

        let start = TimeStamp::from_elapsed(0).unwrap();
        let reaction = TimeSpan::from_duration(Duration::from_secs_f64(0.5));
        let range = VisibleRangePerBpm::new(loaded.chart.init_bpm(), reaction);
        let mut player = ChartPlayer::start(loaded.chart, range, start);

        // 5ms 步进推进播放头，收集全部被触发的事件 id
        let mut triggered: std::collections::HashSet<usize> = std::collections::HashSet::new();
        let step_ns: u64 = 5_000_000;
        let total_ns = (loaded.total_sec * 1e9) as u64 + 2_000_000_000;
        let mut t = 0u64;
        while t <= total_ns {
            let now = TimeStamp::from_elapsed(t).unwrap();
            for e in player.update(now) {
                triggered.insert(e.id.0);
            }
            t += step_ns;
        }

        let all_ids: std::collections::HashSet<usize> =
            loaded.notes.iter().map(|n| n.event_id).collect();
        let missing: Vec<usize> = all_ids.difference(&triggered).copied().collect();
        assert!(
            missing.is_empty(),
            "{} 个音符事件未被播放头触发（判定基准/事件生成问题）: 前 10 个 {:?}",
            missing.len(),
            missing.iter().take(10).collect::<Vec<_>>()
        );
    }

    /// Auto 模式端到端模拟：逐帧复刻 `tick_gameplay` 的 auto 分支
    /// （事件驱动 record(Pg) + consumed/start_ln）+ `hold_update` + `miss_detection`
    /// （auto 时应跳过），验证 Auto 下不应产生任何 POOR（Pr=0）、combo 单调累积。
    #[test]
    fn auto_mode_never_poors() {
        use crate::gameplay::judge::{JudgeDir, JudgeState, Judgement};
        use crate::gameplay::lane::{LnKind, LnState, start_ln, update_ln};
        use gametime::{TimeSpan, TimeStamp};
        use std::collections::HashMap;
        use std::time::Duration;

        let home = std::env::var("HOME").unwrap_or_default();
        let path =
            PathBuf::from(home).join(".local/share/lr2oraja/songs/rainbow_ogg/rainbowA.bms");
        if !path.exists() {
            eprintln!("跳过：找不到真实铺面 {}", path.display());
            return;
        }
        let loaded = LoadedChart::load(&path).expect("加载真实铺面");
        let start = TimeStamp::from_elapsed(0).unwrap();
        let reaction = TimeSpan::from_duration(Duration::from_secs_f64(0.5));
        let range = VisibleRangePerBpm::new(loaded.chart.init_bpm(), reaction);
        let mut player = ChartPlayer::start(loaded.chart, range, start);

        let mut judge_state = JudgeState::default();
        // 复刻 tick 的 auto 分支：LaneStates 简化（仅 LN processing 状态）
        let mut ln_processing: HashMap<usize, (LnState, f64, f64)> = HashMap::new(); // key → (state, head_y, len_y)
        let mut consumed = vec![false; loaded.notes.len()];
        let step = 0.005;
        let total = loaded.total_sec + 2.0;
        let mut t = 0.0;
        while t < total {
            let now = TimeStamp::from_elapsed((t * 1e9) as u64).unwrap();
            for e in player.update(now) {
                if let bms_rs::chart::prelude::ChartEvent::Note {
                    side: bms_rs::chart::prelude::PlayerSide::Player1,
                    key,
                    ..
                } = e.event()
                {
                    // auto：事件驱动判定 Pg（复刻 tick_gameplay auto 分支）
                    if let Some(&idx) = loaded.note_by_event.get(&e.id.0) {
                        let note = &loaded.notes[idx];
                        if note.length.is_some() {
                            let kind = LnKind::from(loaded.ln_mode);
                            let mut st = LnState::default();
                            start_ln(&mut st, idx, kind, Judgement::Pg, 0.0);
                            ln_processing.insert(
                                lane_key_usize(*key),
                                (st, note.position.0.as_f64(), note.length.unwrap().as_f64()),
                            );
                        } else {
                            consumed[idx] = true;
                        }
                    }
                    judge_state.record(Judgement::Pg, JudgeDir::Neutral);
                }
            }
            // hold_update（复刻）：LN 尾部到达 → 判尾
            let now_y = player.playback_state().progressed_y.0.as_f64();
            let mut done: Vec<usize> = Vec::new();
            for (key, (st, head_y, len_y)) in ln_processing.iter_mut() {
                match update_ln(st, *head_y, *len_y, now_y) {
                    None => {}
                    Some(tail) => {
                        if let Some(j) = tail {
                            judge_state.record(j, JudgeDir::Neutral);
                        }
                        let _ = key;
                        done.push(*key);
                    }
                }
            }
            for k in done {
                ln_processing.remove(&k);
            }
            // miss_detection：auto 模式应完全跳过（复刻 368 行 return）
            // （不执行任何 Pr 判定）
            t += step;
        }
        eprintln!(
            "Auto 模拟: total={} pg={} gr={} gd={} bd={} pr={} combo={} max={}",
            loaded.note_count(),
            judge_state.pg,
            judge_state.gr,
            judge_state.gd,
            judge_state.bd,
            judge_state.pr,
            judge_state.combo,
            judge_state.max_combo
        );
        assert_eq!(judge_state.pr, 0, "Auto 模式不应产生 POOR");
        assert!(
            judge_state.pg >= loaded.note_count() as u32,
            "Auto 应判定全部音符为 Pg（{}）",
            judge_state.pg
        );
    }

    /// 复刻 `manual_input_judge` 的 lane key → lane_idx 映射（scratch=0，键号）。
    fn lane_key_usize(key: bms_rs::chart::prelude::Key) -> usize {
        match key {
            bms_rs::chart::prelude::Key::Scratch(_) => 0,
            bms_rs::chart::prelude::Key::Key(n) => usize::from(n),
            _ => usize::MAX,
        }
    }

    /// LN 时长换算验证：`measure_seconds`（BPM 分段累积）应与播放头从
    /// LN head 推进到 tail 的真实时间一致（旧实现 `len_y/100` 偏差百倍，
    /// 导致松手误判 Pr → combo 清零）。
    #[test]
    fn ln_duration_matches_player_progress() {
        use gametime::{TimeSpan, TimeStamp};
        use std::time::Duration;

        let home = std::env::var("HOME").unwrap_or_default();
        let path =
            PathBuf::from(home).join(".local/share/lr2oraja/songs/rainbow_ogg/rainbowA.bms");
        if !path.exists() {
            eprintln!("跳过：找不到真实铺面 {}", path.display());
            return;
        }
        let loaded = LoadedChart::load(&path).expect("加载真实铺面");

        // 提取 BPM 变化点
        let mut bpm_changes: Vec<(f64, f64)> = Vec::new();
        for (y, flows) in loaded.chart.flow_events() {
            for f in flows {
                if let bms_rs::chart::prelude::FlowEvent::Bpm(b) = f {
                    bpm_changes.push((y.as_f64(), b.as_f64()));
                }
            }
        }
        bpm_changes.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let init_bpm = loaded.chart.init_bpm().as_f64();

        // 统计偏差
        let mut max_err = 0.0f64;
        let mut bad = 0u32;
        let mut checked = 0u32;
        let lns: Vec<_> = loaded
            .notes
            .iter()
            .filter(|n| n.length.is_some())
            .collect();
        if lns.is_empty() {
            eprintln!("跳过：rainbow 无长音");
            return;
        }
        for note in lns.iter().take(30) {
            let len_y = note.length.unwrap().as_f64();
            let head_y = note.position.0.as_f64();
            let calc = crate::gameplay::measure_seconds(head_y, len_y, &bpm_changes, init_bpm);
            // 旧实现（bug）的参考值：len_y/100
            let old_bug = len_y / 100.0;
            // 播放头实测：从 head 时刻推进到 tail_y
            let start = TimeStamp::from_elapsed(0).unwrap();
            let reaction = TimeSpan::from_duration(Duration::from_secs_f64(0.5));
            let range = VisibleRangePerBpm::new(loaded.chart.init_bpm(), reaction);
            let mut player = ChartPlayer::start(loaded.chart, range, start);
            let head_nanos = (note.activate_time * 1e9) as u64;
            let tail_y = head_y + len_y;
            player.update(TimeStamp::from_elapsed(head_nanos).unwrap());
            let mut t = head_nanos;
            let mut measured = 0.0f64;
            while t < head_nanos + 120_000_000_000 {
                let now = TimeStamp::from_elapsed(t).unwrap();
                player.update(now);
                if player.playback_state().progressed_y.0.as_f64() >= tail_y {
                    measured = (t - head_nanos) as f64 / 1e9;
                    break;
                }
                t += 1_000_000;
            }
            if measured == 0.0 {
                eprintln!(
                    "LN t={:.2}s len_y={:.2} head_y={:.2}: 播放头 600s 内未到达 tail_y（y 到 {:.2}）",
                    note.activate_time,
                    len_y,
                    head_y,
                    player.playback_state().progressed_y.0.as_f64()
                );
                continue;
            }
            checked += 1;
            let err = (measured - calc).abs();
            max_err = max_err.max(err);
            if err > 0.02 {
                bad += 1;
                eprintln!(
                    "LN t={:.2}s len_y={:.2}: 实测={:.3}s 新换算={:.3}s 旧bug={:.3}s",
                    note.activate_time, len_y, measured, calc, old_bug
                );
            }
        }
        assert!(
            checked > 0,
            "rainbow 应有长音可验证（checked={checked}）"
        );
        assert!(
            bad == 0,
            "{} 个 LN 的新换算偏差 > 20ms（最大 {:.3}s）——LN 松手会误判 Pr",
            bad, max_err
        );
    }

    /// measure → 播放头时间换算（beatoraja `delta_y × 240 / bpm`）：
    /// 恒 BPM 每小节 240/BPM 秒；BPM 变化按分段累积。
    #[test]
    fn measure_seconds_bpm_segments() {
        // 恒 BPM：1 measure = 240/BPM 秒（4/4 拍）
        assert_eq!(crate::gameplay::measure_seconds(0.0, 1.0, &[], 120.0), 2.0);
        assert_eq!(crate::gameplay::measure_seconds(0.0, 1.0, &[], 240.0), 1.0);
        assert_eq!(crate::gameplay::measure_seconds(2.0, 4.0, &[], 240.0), 4.0);
        // BPM 变化：0→2 小节 @120（4s）+ 2→4 小节 @240（2s）= 6s
        let changes = vec![(2.0, 240.0)];
        assert!(
            (crate::gameplay::measure_seconds(0.0, 4.0, &changes, 120.0) - 6.0).abs() < 1e-9
        );
        // head 在 BPM 变化点之后：变化点之前的段不参与
        let changes2 = vec![(1.0, 300.0)];
        assert!(
            (crate::gameplay::measure_seconds(2.0, 2.0, &changes2, 120.0) - 1.6).abs() < 1e-9,
            "2 measure @300 = 240/300×2 = 1.6s"
        );
        // 对比旧实现（len_y/100）：偏差一个数量级，确认修复必要性
        let old_bug = 1.0 / 100.0;
        assert!(
            (crate::gameplay::measure_seconds(0.0, 1.0, &[], 120.0) - old_bug).abs() > 1.0,
            "旧实现把 1 measure 算成 0.01s"
        );
    }

    /// 完美输入模拟：对每个音符在其 activate_time 时刻按键（delta≈0），
    /// 验证判定为 Pg 且 combo 持续累积、断连判定（Bd/Pr）才清零。
    #[test]
    fn perfect_input_keeps_combo() {
        let home = std::env::var("HOME").unwrap_or_default();
        let path =
            PathBuf::from(home).join(".local/share/lr2oraja/songs/rainbow_ogg/rainbowA.bms");
        if !path.exists() {
            eprintln!("跳过：找不到真实铺面 {}", path.display());
            return;
        }
        let loaded = LoadedChart::load(&path).expect("加载真实铺面");
        let mut judge_state = crate::gameplay::judge::JudgeState::default();
        let w = crate::gameplay::judge::JudgeWindows::for_level(loaded.rank);

        // 逐个音符在 activate_time 时刻模拟完美按键（复刻 manual_input_judge 的 delta 计算）
        let mut prev_combo: u32 = 0;
        for (i, note) in loaded.notes.iter().enumerate() {
            // 长音 head 命中后不 consumed，但完美输入下按住到尾，这里仅统计普通判定
            let delta = 0.0; // 完美时机
            let j = crate::gameplay::judge::judge(delta, &w).expect("窗口内必有判定");
            if matches!(
                note.kind,
                bms_rs::chart::prelude::NoteKind::Landmine
            ) {
                continue;
            }
            judge_state.record(j, crate::gameplay::judge::JudgeDir::Neutral);
            // 普通音符（非长音）后 combo 应连续 +1（delta=0 恒 Pg）
            if note.length.is_none() {
                assert!(
                    judge_state.combo == prev_combo + 1,
                    "音符 {i}（t={:.2}s）完美判定后 combo 应 +1：{} vs {}",
                    note.activate_time,
                    judge_state.combo,
                    prev_combo + 1
                );
                prev_combo += 1;
            }
            let _ = i;
        }
        assert!(
            judge_state.pg > 100,
            "完美输入应全部 Pg（{} 个）",
            judge_state.pg
        );
    }

    /// 端到端判定模拟：逐帧（5ms）复刻 gameplay 系统链
    /// （miss_detection → 按键判定），完美输入（音符到达帧按键）下
    /// combo 不应被误判清零、全部音符应被判定。
    #[test]
    fn end_to_end_judgement_perfect_input() {
        use crate::gameplay::judge::{JudgeDir, JudgeState, JudgeWindows, Judgement, judge};

        let home = std::env::var("HOME").unwrap_or_default();
        let path =
            PathBuf::from(home).join(".local/share/lr2oraja/songs/rainbow_ogg/rainbowA.bms");
        if !path.exists() {
            eprintln!("跳过：找不到真实铺面 {}", path.display());
            return;
        }
        let loaded = LoadedChart::load(&path).expect("加载真实铺面");
        let w = JudgeWindows::for_level(loaded.rank);
        let mut j = JudgeState::default();
        // 普通音符/LN head 判定后即 consumed（简化：不模拟 LN 按住）
        let mut consumed = vec![false; loaded.notes.len()];
        let step = 0.005; // 5ms 帧
        let total = loaded.total_sec + 2.0;
        let mut expected_combo: u32 = 0;
        let mut input_i = 0usize;
        let mut t = 0.0;
        while t < total {
            // miss_detection：过 bd 窗口 + 50ms 未判定 → POOR（断连）
            for (i, n) in loaded.notes.iter().enumerate() {
                if !consumed[i] && t - n.activate_time > w.bd_ms / 1000.0 + 0.05 {
                    consumed[i] = true;
                    j.record(Judgement::Pr, JudgeDir::Neutral);
                }
            }
            // 按键：每个音符在其 activate_time 所在帧按键一次（完美时机）
            while input_i < loaded.notes.len() && t + step >= loaded.notes[input_i].activate_time {
                let n = &loaded.notes[input_i];
                if !consumed[input_i] {
                    let delta = t - n.activate_time; // ∈ [0, step)
                    if let Some(jj) = judge(delta, &w) {
                        consumed[input_i] = true;
                        j.record(jj, JudgeDir::Neutral);
                        // 完美输入恒 Pg：每次判定后 combo 必须单调 +1（不得被误判清零）
                        expected_combo += 1;
                        assert_eq!(
                            j.combo, expected_combo,
                            "完美输入下 combo 应连续累积（音符 {} t={:.2}s）",
                            input_i, n.activate_time
                        );
                    }
                }
                input_i += 1;
            }
            t += step;
        }

        let judged = j.judged();
        eprintln!(
            "端到端: total={} judged={} pg={} gr={} gd={} bd={} pr={} max_combo={}",
            loaded.note_count(),
            judged,
            j.pg,
            j.gr,
            j.gd,
            j.bd,
            j.pr,
            j.max_combo
        );
        assert!(
            judged as usize >= loaded.note_count(),
            "完美输入应判定全部音符：{} / {}",
            judged,
            loaded.note_count()
        );
        assert!(
            j.pg as usize + j.gr as usize >= loaded.note_count() - 10,
            "完美输入应几乎全 PG/GR：pg={} gr={} total={}",
            j.pg,
            j.gr,
            loaded.note_count()
        );
    }
}






